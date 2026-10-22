//! Self-observability: `fats` status, `fats stats`, `fats config`, `fats account`.

use crate::cache::{Cache, Entry, MINE, Meta, Week, bump, code_of, jload, jsave, usage};
use crate::cli::{ASSET_MAX, ASSET_WAIT, FENCE, MAX_BYTES, MAX_BYTES_CEIL, copies, no_doc, path_of};
use crate::lang::{At, Cmd, Out, bin, ksize, mib, paint, screen, short};
use crate::outline::{Page, fmt_age, photos};
use crate::provider::{ACCOUNTS, Account, Api, KEYS, Tier, account_calls, bill, late, links, spend};
use crate::{fetch, settings};
use serde_json::{Map, Value, json};
use std::path::Path;
use std::time::Duration;

/// Provider health window: the last N calls per provider, whole cache.
const HEALTH: usize = 10;

/// A path's permission bits (`0o700`); None = missing, or not a unix system (Windows: ACLs, not mode bits).
fn mode(#[cfg_attr(not(unix), expect(unused_variables, reason = "no mode bits to read"))] p: &Path) -> Option<u32> {
    #[cfg(unix)]
    return std::fs::metadata(p).ok().map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o777);
    #[cfg(not(unix))]
    None
}

/// `auth.json` readable by others (mode not 0600): the `!` row with the fix. Never reads the keys.
fn key_mode() -> Option<String> {
    let (p, file) = (crate::settings::get().key_dir.value.join(settings::AUTH), settings::AUTH);
    let mode = mode(&p)?;
    (mode & 0o077 != 0)
        .then(|| format!("! key_mode {file} is {mode:o}, readable by others → chmod 600 {}", p.display()))
}

/// A key (`p` = its switch: `oxy`, `gh`) as `fats` and `config` name it: its tail (`…6XBR`), `none` or `off`; never
/// its value.
pub fn key(p: &str) -> String {
    if !settings::on(p) {
        return "off".into();
    }
    settings::key(p).map_or("none".into(), |k| tail(&k))
}

/// A key as a printed line names it: `…` and its last 4 chars (`…6XBR`), these only from a key of 16 chars or more
/// (4 of them then tell little); never more of it.
pub fn tail(k: &str) -> String {
    format!("…{}", k.get(k.len().saturating_sub(4)..).filter(|_| k.len() >= 16).unwrap_or_default())
}

/// A stored record's meta.json is a search's (in `j<N>/`), not a page's (in `u<N>/`).
pub fn is_serp(meta: &std::path::Path) -> bool {
    meta.parent().and_then(|d| d.file_name()).is_some_and(|n| n.to_string_lossy().starts_with('j'))
}

pub fn status(c: &Cache) -> Out {
    let b = bin();
    let (bytes, pics, metas) = usage(&c.root);
    let searches = metas.iter().filter(|m| is_serp(m)).count();
    let pages = metas.len() - searches;
    let mut lines = vec![match c.st.u.last() {
        None if crate::setup::no_keys() && settings::on("free") => {
            let off = format!("pages free, no key · web search off: needs one key → {}", fetch::search_key());
            format!("= s:{} no pages · {off}", c.session)
        }
        None => format!("= s:{} no pages · {b} <url> · {b} ./page.html", c.session),
        Some(e) => {
            let open = c.st.u.iter().filter(|e| e.serp && e.dir.is_empty() && c.lookup(&e.url).is_none()).count();
            let n = c.st.u.len();
            let open = if open > 0 { format!(" ({open} search results not fetched)") } else { String::new() };
            format!("= s:{} {n} ids{open} · last u{n} {}", c.session, short(&e.url, 60))
        }
    }];
    let m = mode(&c.root).map_or(String::new(), |m| format!(" (mode {m:o})"));
    let (root, size, jobs, pics) = (c.root.display(), ksize(bytes as usize), c.job_count(), ksize(pics as usize));
    let swept = if c.root.join(MINE).is_file() { "" } else { " · not swept (dir not created by fats)" };
    let sizes = format!("{size}B ({pics}B pictures) · {pages} pages · {searches} searches{swept}");
    lines.push(format!("cache {root}{m} · {sizes} · {jobs} jobs"));
    lines.push(crate::setup::keys_line());
    lines.extend(crate::setup::env_rows());
    lines.extend(key_mode());
    let run = |(n, r, alive): &(usize, crate::cache::Detached, bool)| {
        let age = (crate::cache::now() - r.t).max(0.0) as u64;
        let state = if *alive { format!("pid {}", r.pid) } else { "! abandoned".into() };
        // alive with no pid yet: it has not taken its lock (`cache::Cache::run`)
        format!("u{n} {} {age}s", if *alive && r.pid == 0 { "starting".into() } else { state })
    };
    let runs: Vec<String> = c.runs().iter().filter(|r| r.1.done.is_none()).map(run).collect();
    lines.extend((!runs.is_empty()).then(|| format!("running {} · read one: {b} u<N>", runs.join(" · "))));
    lines.extend(health(&metas));
    lines.push(account_line(c, crate::cache::now()));
    lines.push(crate::telemetry::status());
    lines.push(match FENCE.get() {
        Some(_) => "mcp: serving (this process) · local files from the working directory + the cache".into(),
        None => crate::mcp::hosts_row(b),
    });
    let at = c.st.cursor.as_ref().filter(|k| k.u > 0);
    let at = at.map(|k| format!("at u{}{} · ", k.u, k.k.map(|s| format!("#{s}")).unwrap_or_default()));
    let n = c.st.u.len();
    let next = if n > 0 { format!("outline: {b} u{n} · ") } else { String::new() };
    lines.push(format!("{}{next}stats: {b} stats · config: {b} config", at.unwrap_or_default()));
    lines.insert(1, crate::lang::to_guide()); // the agent's skill, right under the `=` line
    Out::new(lines)
}

/// Last HEALTH calls per provider over every snapshot in the cache: outcomes newest last (✓ ok, ✗ failed,
/// – cancelled after the other provider won), ok count, p50 ms of the calls that finished, and what the
/// cancelled ones were billed (list price: the provider may finish them).
fn health(paths: &[std::path::PathBuf]) -> Vec<String> {
    let mut metas: Vec<Meta> = paths.iter().filter_map(|p| jload(p)).collect();
    metas.sort_by(|a, b| a.t.total_cmp(&b.t));
    let mut rows =
        vec![format!("health (last {HEALTH} calls, whole cache): provider ok/n p50ms outcomes (– cancelled)")];
    for t in Tier::ALL {
        let hops: Vec<_> = metas.iter().flat_map(|m| m.hops.iter()).filter(|h| h.provider == t.spec().name).collect();
        let last = &hops[hops.len().saturating_sub(HEALTH)..];
        if last.is_empty() {
            // the state the `keys` line names: off (its key kept or not), no key, or bd's key without its zone
            let none = match (fetch::keyed().contains(&t), key(t.switch()).as_str()) {
                (true, _) => "- no calls yet",
                (_, "off") if settings::key(t.switch()).is_some() => "off (key kept)",
                (_, "off") => "off",
                (_, "none") => "no key",
                _ => "no zone",
            };
            rows.push(format!("{} {none}", t.spec().short));
            continue;
        }
        let ok = last.iter().filter(|h| h.code == "ok").count();
        let gone: Vec<_> = last.iter().filter(|h| h.code == "abandoned").collect();
        let mut ms: Vec<u64> = last.iter().filter(|h| h.code != "abandoned").map(|h| h.ms).collect();
        ms.sort_unstable();
        let mark = |h: &&crate::cache::HopRow| match h.code.as_str() {
            "ok" => '✓',
            "abandoned" => '–',
            _ => '✗',
        };
        let marks: String = last.iter().map(mark).collect();
        let p50 = ms.get(ms.len() / 2).map_or("-".into(), |m| format!("{m}ms"));
        let (cost, upto) = (spend(gone.iter().map(|h| h.cost)), spend(gone.iter().map(|h| h.upto)));
        let billed = format!(" · {} cancelled, billed {}", gone.len(), crate::provider::bill(cost, upto));
        let gone = if gone.is_empty() { String::new() } else { billed };
        rows.push(format!("{} {ok}/{} {p50} {marks}{gone}", t.spec().short, last.len()));
    }
    rows.extend(limits(&metas).map(|l| format!("oxy limits {l}")));
    rows
}

/// `250 scrape · 10 search`: the newest request budget Oxylabs sent per endpoint (hop `limit`). The window is
/// undocumented, so no unit is printed.
fn limits(metas: &[Meta]) -> Option<String> {
    // the newest answer's budget (max_by keeps the last of equal times: a run's last oxylabs hop)
    let newest = |serp: bool| {
        let hops = metas.iter().filter(|m| (m.kind == "serp") == serp).flat_map(|m| m.hops.iter().map(|h| (m.t, h)));
        let oxy = hops.filter(|(_, h)| h.provider.starts_with("oxylabs"));
        oxy.filter_map(|(t, h)| Some((t, h.limit?))).max_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, l)| l)
    };
    let part = |(serp, k): (bool, &str)| newest(serp).map(|l| format!("{l} {k}"));
    let parts: Vec<String> = [(false, "scrape"), (true, "search")].into_iter().filter_map(part).collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// ` · provider answers <path>` when a failed run kept them (`cache::ERROR_FILE`); its content is never printed.
fn kept(dir: Option<std::path::PathBuf>) -> String {
    let p = dir.map(|d| d.join(crate::cache::ERROR_FILE)).filter(|p| p.is_file());
    p.map_or(String::new(), |p| format!(" · provider answers {}", p.display()))
}

/// The session index: one row per search (query, results, provider, ms, cost) and per fetch (host · title ·
/// provider, ms, cost, tok; a failed one its `!` row), the last `list_rows` (`limit:N`, `all`), the rows whose
/// host or title holds the words after `stats`. `json` = every row whole: raw bytes → clean ~tok → outline ~tok,
/// loss, tried path. A search result shows as `search→u7`, not fetched yet or fetched.
pub fn stats(c: &Cache, cmd: &crate::lang::Cmd, json: bool) -> Out {
    let (n, s, keys, file) = (c.serps().len(), &c.session, &settings::get().key_dir.value, crate::cache::SPEND_FILE);
    // a ledger fats cannot write is a `!` row
    let rows = ledger();
    let (listed, tot_c, lost) = index(c, &rows);
    let tot_ms = listed.iter().map(|p| p.ms).sum::<u64>();
    let mut lines = vec![format!("= stats s:{s} · {n} searches · {} fetches · {tot_ms}ms spent {tot_c}", fetched(c))];
    // the index: the rows whose host or title holds the words (`search::holds`), the last `list_rows` of them
    // (`limit:N`; `all`; `full`, `json` = every one, each whole), then a line naming the rest, or that none held
    // the words
    let (b, at, json) = (bin(), cmd.words.iter().map(|w| format!(" {w}")).collect::<String>(), json || cmd.has("full"));
    let keep: Vec<&StatRow> = listed.iter().filter(|r| crate::search::holds(&r.name, &cmd.words)).collect();
    let n = cmd.count("limit").unwrap_or_else(|| settings::limit("list_rows"));
    let more = if json || cmd.has("all") { 0 } else { keep.len().saturating_sub(n) };
    lines.extend(keep[more..].iter().map(|r| if json { &r.full } else { &r.lean }.clone()));
    let none = (keep.is_empty() && !at.is_empty())
        .then(|| format!("0 of {} rows hold \"{}\" · all: {b} stats all", listed.len(), at.trim_start()));
    lines.extend((more > 0).then(|| format!("… {more} more · {b} stats{at} all")).or(none));
    // every session's spend: pages and searches are swept within the hour, what they cost stays
    let since = |t: f64| usd_of(&rows.iter().filter(|r| r.0 >= t).map(|r| r.1).collect::<Vec<_>>());
    let (day, all, n, at) = (since(crate::cache::now() - 86_400.0), since(0.0), rows.len(), keys.join(file));
    lines.push(format!("spend all sessions: 24h {day} · all {all} · {n} paid records · ledger {}", at.display()));
    lines.extend(lost.map(|why| format!("! cache_write ledger not written: {why} → {b} config · FATS_KEY_DIR")));
    lines.push(match c.st.u.len() {
        0 => format!("next: {b} <url> · {b} ./page.html"),
        n => format!("last page: {b} u{n} · settings: {b} config"),
    });
    Out::new(lines)
}

/// The session's pages fetched (a search result not fetched, or forgotten, is none).
fn fetched(c: &Cache) -> usize {
    c.st.u.iter().filter(|e| !matches!(e.dir.as_str(), "" | "-")).count()
}

/// This session's pages fetched and what it spent, as `fats stats` heads it (`fats setup` → Settings → Stats).
pub fn session(c: &Cache) -> (usize, String) {
    (fetched(c), index(c, &ledger()).1)
}

/// The session's index rows (newest last), its money as printed, and why the ledger cannot be written.
fn index(c: &Cache, rows: &[(f64, [f64; 2], std::path::PathBuf)]) -> (Vec<StatRow>, String, Option<std::io::Error>) {
    let (serps, s, keys, file) = (c.serps(), &c.session, &settings::get().key_dir.value, crate::cache::SPEND_FILE);
    // a probe: one a session with nothing in it never writes
    let lost = (serps.len() + c.st.u.len() > 0).then(|| settings::save_in(keys, file, b"", true).err()).flatten();
    // the session's money = its ledger rows written since it began (`State::t`; a wiped root or session restarts
    // the ids, and the rows of the records wiped with it name the same dirs; unknown = none) + a dead run's calls
    // in flight while its run file lives (the sweep writes their row when it deletes `run/`)
    let mine = rows.iter().filter(|r| r.2.starts_with(c.root.join(s)) && c.st.t.is_some_and(|b| r.0 >= b));
    let ids = c.st.u.iter().enumerate().filter(|(_, e)| e.dir != "-"); // a result forgotten unfetched: no row
    let mut listed: Vec<StatRow> =
        serps.iter().map(|(j, m)| search_row(c, *j, m)).chain(ids.map(|(i, e)| page_row(c, i + 1, e, ""))).collect();
    listed.sort_by_key(|r| r.at); // newest last: by job (one counter for searches and fetches), a search first
    let tot_c = usd_of(&mine.map(|r| r.1).chain(listed.iter().map(|p| [0.0, p.upto])).collect::<Vec<_>>());
    let tot_c = if lost.is_some() { "? (ledger not written)".into() } else { tot_c }; // its rows are missing
    (listed, tot_c, lost)
}

/// The spend ledger's rows: (unix s, [cost, unverified], the record's dir).
fn ledger() -> Vec<(f64, [f64; 2], std::path::PathBuf)> {
    let at = settings::get().key_dir.value.join(crate::cache::SPEND_FILE);
    let row = |l: &str| serde_json::from_str(l).ok();
    std::fs::read_to_string(at).unwrap_or_default().lines().filter_map(row).collect()
}

/// `$0.0225 + ≤$0.0060 unverified`: what ledger amounts total (`provider::bill`).
fn usd_of(p: &[[f64; 2]]) -> String {
    bill(spend(p.iter().map(|x| x[0])), spend(p.iter().map(|x| x[1])))
}

/// `fats` on a terminal (`home` screen): what fats is, READY? (`ready`), TRY, YOUR STUFF (`yours`: this
/// session's pages, the last one, every session's spend this calendar month, UTC), MORE. `status` = the agent's.
pub fn home(c: &Cache) -> String {
    let this = month(crate::cache::now()).0;
    let spent: Vec<_> = ledger().into_iter().filter(|r| month(r.0).0 == this).map(|r| r.1).collect();
    let pages: Vec<_> = c.st.u.iter().enumerate().filter(|(_, e)| !matches!(e.dir.as_str(), "" | "-")).collect();
    let yours = pages.last().map_or(String::new(), |(n, e)| {
        let (id, last, spent) = (format!("u{}", n + 1), short(&e.url, 50), usd_of(&spent).replace("≤$", "up to $"));
        paint(screen("yours"), &[("pages", pages.len().to_string()), ("id", id), ("last", last), ("spent", spent)])
    });
    let home = paint(screen("home"), &[("ready", ready()), ("yours", yours)]);
    crate::lang::redact(&home, &settings::secrets()) // a URL can carry a key
}

/// The READY? rows (`ready` screen: the ✓ row, the ○ row, then `name|label|on|off|no key` per provider; an
/// empty no-key text = it needs none): on and keyed = ✓, else ○ and why (`settings::on`, `settings::key`).
fn ready() -> String {
    let mut rows = screen("ready").lines();
    let marks = [rows.next().unwrap_or_default(), rows.next().unwrap_or_default()];
    let row = |l: &str| {
        let [name, label, on, off, none] = l.split('|').collect::<Vec<_>>()[..] else { return String::new() };
        let (from, texts) = (settings::key(name).map(|k| tail(&k)), [on, off, none]);
        let at = if !settings::on(name) { 1 } else { usize::from(from.is_none() && !none.is_empty()) * 2 };
        let text = crate::lang::draw(texts[at], &[("from", from.as_deref().unwrap_or_default())]);
        paint(marks[at.min(1)], &[("label", &format!("{label:<20}")), ("text", &text)])
    };
    rows.map(row).collect::<Vec<_>>().join("\n")
}

/// One `stats` row: `lean` (the index line), `full` (every field: `json`), `name` (what a word filters), its ms,
/// and the cost no ledger row holds: a dead run's calls in flight.
pub struct StatRow {
    pub lean: String,
    full: String,
    name: String,
    ms: u64,
    upto: f64,
    /// Its place: (job, u), a search's = (job, 0).
    at: (usize, usize),
}

impl StatRow {
    /// `id host · title · what`: one line, the title cut (`outline::title_cut`, never under 30 chars) to what the
    /// rest leaves of `row_chars` (limits.json); `full` = the same with every field and the whole title (no
    /// `lean`: `full` is the line). No title, or the host's own name: `id host · what`.
    fn new(id: &str, host: &str, title: &str, (lean, full): (String, String)) -> StatRow {
        let lean = if lean.is_empty() { full.clone() } else { lean };
        let room = settings::limit::<usize>("row_chars")
            .saturating_sub(id.chars().count() + host.chars().count() + lean.chars().count() + 7);
        let named = |t: String| if title.is_empty() || title == host { String::new() } else { t + " · " };
        let (cut, whole) = (named(crate::outline::title_cut(title, room.max(30))), named(title.into()));
        let (lean, full) = (format!("{id} {host} · {cut}{lean}"), format!("{id} {host} · {whole}{full}"));
        StatRow { lean, full, name: format!("{host} {title}"), ms: 0, upto: 0.0, at: (0, 0) }
    }
}

/// `stats` row of search `j<job>`: `j4 search "rust http client" · 10 results · oxylabs 1800ms $0.0010`; its tried
/// path in `full`, and on a failed one.
fn search_row(c: &Cache, job: usize, m: &Meta) -> StatRow {
    let n = m.extra.get("hits").and_then(|h| h.as_array()).map_or(0, |h| h.len());
    let n = if m.ok { format!("{n} results") } else { format!("! {}", m.code) };
    let cc = if m.country.is_empty() { String::new() } else { format!("@{}", m.country) };
    let q = m.url.strip_prefix("search:").unwrap_or(&m.url);
    let prov = if m.provider.is_empty() { "-" } else { &m.provider };
    let (ms, usd, tried) = (m.ms, crate::provider::bill(m.cost, m.upto()), m.path(true));
    let kept = kept(Some(c.root.join(&c.session).join(format!("j{job}"))));
    let lean = format!("j{job} search \"{}\" · {n} · {prov}{cc} {ms}ms {usd}", short(q, 60));
    let full = format!("{lean} · {tried}{kept}");
    StatRow { lean: if m.ok { lean } else { full.clone() }, full, name: q.into(), ms, upto: 0.0, at: (job, 0) }
}

/// `stats` row of page `<job>u<u>` (`j3.u2` in a job's list; a cache hit costs nothing; a swept page's cost is in
/// the total only).
pub fn page_row(c: &Cache, u: usize, e: &Entry, job: &str) -> StatRow {
    let (pid, empty, at) = (format!("{job}u{u}"), e.dir.is_empty(), (e.job.unwrap_or_default(), u));
    let id = if e.serp { format!("search→{pid}") } else { pid.clone() };
    // a detached fetch adds its cost here once it ends; THE liveness rule (`Cache::run`, the lock); a dead one = its
    // failed copy (`Detached::cut`), its calls in flight in the row and the total until the sweep stores it
    let run = empty.then(|| c.run(u)).flatten().filter(|r| r.0.done.is_none());
    let dead = run.as_ref().filter(|r| r.0.dead(r.1, Some(e))).map(|r| r.0.cut());
    if empty && dead.is_none() {
        let idle = if c.lookup(&e.url).is_some() { "cached" } else { "not fetched" };
        let what = format!("{} → {} {pid}", if run.is_some() { "pending" } else { idle }, bin());
        return StatRow { at, ..StatRow::new(&id, &short(&e.url, 60), "", (String::new(), what)) };
    }
    let (meta, doc) = dead.map_or_else(|| c.page(e), |m| (m, Err(crate::cache::NoDoc::Missing)));
    let errs = kept(c.dir(e));
    let (cost, ms) = if e.alias { (0.0, 0) } else { (meta.cost, meta.ms) };
    let cc = if meta.country.is_empty() { String::new() } else { format!("@{}", meta.country) };
    let name = if meta.provider.is_empty() { "swept" } else { meta.provider.as_str() }; // no meta: its dir went
    let prov = if e.alias { "cache".into() } else { format!("{name}{cc}") };
    let head = format!("{prov} {ms}ms ${cost:.4}{}", meta.how());
    let host = crate::outline::place(&e.url, meta.final_url.as_deref().unwrap_or(&e.url));
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let pics = c.dir(e).map_or_else(Vec::new, |d| crate::cache::assets(&d));
    let (pic_bytes, pic_ms) = pics.iter().fold((0, 0), |(b, t), a| (b + a.bytes as usize, t + a.ms));
    let line = format!(" · pictures {} plain {}B {pic_ms}ms $0", pics.len(), ksize(pic_bytes));
    let pics = if pics.is_empty() { String::new() } else { line };
    let (tail, raw) = (format!("{}{pics}{errs}", meta.path(true)), ksize(meta.bytes as usize));
    let (title, lean, full) = match doc {
        Ok(doc) => {
            // a failed copy (a captcha page kept) keeps its `!` code on the row
            let head = if meta.ok { head } else { format!("{head} ! {}", meta.code) };
            let charset = if meta.charset.is_empty() { String::new() } else { format!(" {}", meta.charset) };
            // the `=` line's id and figure, as a read of this copy prints them (`fats u2`, `fats j3.u2`)
            let eq = if job.is_empty() { e.eq(u) } else { pid.clone() };
            let page = Page { id: pid.clone(), eq, doc: &doc, meta: &meta, age: None, at: Default::default() };
            let shown = crate::outline::outline(&page, None).0;
            let (outline, size) =
                (shown.iter().map(|l| crate::clean::tok(l)).sum::<usize>(), crate::outline::size(&page, &shown));
            let (clean, shots) = (ksize(doc.tokens.clean), photos(&doc, &meta));
            let (mode, l) = (&doc.mode, &doc.loss);
            let sizes = format!("{head} raw {raw}B{charset} → clean {clean}~tok ({mode}) → outline {outline}~tok");
            let pruned = format!("{} of {} numbers pruned", l.vs_light_lost, l.light_facts);
            let signal = doc.info.get("signal").map(|v| format!(" · signal {v}")).unwrap_or_default();
            let full = format!("{sizes}{shots} · {pruned} · price_nums_pruned {}{signal} · {tail}", l.price_pruned);
            (doc.title, format!("{head} · {}tok", ksize(size)), full)
        }
        // a failed copy = the `!` row its read prints (`cli::no_doc`), what it ran before the move; a starved one's
        // newer served copy (`cli::copies`) is its move
        Err(why) if !meta.ok => {
            let starved =
                fetch::budget_row(&meta, "", &[]).and_then(|_| copies(c, u, (&meta, &Default::default()), &[]).0);
            let row = no_doc(&pid, "", &meta, &[], &why, starved).lines.split_off(1).join("\n");
            let (row, mid) = (row.strip_prefix(&format!("{pid} ")).unwrap_or(&row), format!("{head} · {tail}"));
            let row = row.split_once(" → ").map_or(format!("{row} · {mid}"), |(b, n)| format!("{b} · {mid} → {n}"));
            (String::new(), String::new(), row)
        }
        Err(_) => (String::new(), String::new(), format!("{head} {} raw {raw}B · {tail}", meta.kind)),
    };
    StatRow { ms, upto: if empty { meta.upto() } else { 0.0 }, at, ..StatRow::new(&id, host, &title, (lean, full)) }
}

/// Every setting with its source (flag · env · default · build); refused inputs show next to the value used.
pub fn config() -> Out {
    let s = crate::settings::get();
    let mut rows = store_rows(s);
    let src = |k: &str, at| {
        let lanes = match k {
            "batch_parallel" => " · lanes (RAM guard, not a provider limit)",
            "plain_host_pace_ms" => " · plain waits its turn a host (home-IP brake), never a paid call",
            _ => "",
        };
        let at = match at {
            "build" => "build · limits.json".into(),
            "env" => format!("env FATS_{}", k.to_uppercase()),
            at => at.to_string(),
        };
        at + lanes
    };
    let limits = settings::LIMITS.keys().map(|k| (*k, settings::limit_of(k)));
    rows.extend(limits.map(|(k, (v, at))| (k, v.to_string(), src(k, at))));
    rows.extend(fetch_rows(s));
    rows.push(instant_row());
    for t in Tier::ALL {
        let sp = t.spec();
        let price = format!("${:.2}/1k", sp.price * 1e3);
        // a provider without a key: its `no_key` row, the one text every command prints
        let k = match key(t.switch()).as_str() {
            "off" => " · off (settings.json)".into(),
            _ if sp.env.is_empty() => String::new(),
            "none" if t == t.owner() => format!(" · key none {}", no_key_rows(sp.short, false).concat()),
            "none" => " · key none".into(),
            k => format!(" · key {k} {}", settings::AUTH),
        };
        rows.push((sp.short, format!("{}s {price}", sp.timeout.as_secs()), format!("build{k}")));
    }
    let b = bin();
    let mut lines = vec![format!("= config · {} settings · source: flag · env · default · build", rows.len())];
    lines.extend(rows.into_iter().map(|(k, v, s)| format!("{k} {v} · {s}")));
    lines.push(format!(
        "env: FATS_SESSION FATS_CACHE FATS_KEY_DIR FATS_JSON FATS_NOCACHE FATS_OFFLINE FATS_ALLOW_PRIVATE FATS_CHUNK FATS_ACCOUNT_TTL FATS_ACCEPT_MD FATS_TELEMETRY FATS_TELEMETRY_DEBUG DO_NOT_TRACK · status: {b}"
    ));
    let mut out = Out::new(lines);
    out.warn(settings::unknown(s));
    out
}

/// A `config` row: (name, value in effect, where it came from).
type Row = (&'static str, String, String);

/// A setting's source, and the value refused for env `k` when one was: `env FATS_JSON`, `default (refused x)`.
fn source(src: &str, k: &str) -> String {
    let n = k.trim_start_matches("FATS_").to_lowercase(); // a `settings.json` value's row names it so
    let at = |r: &&String| r.contains(&format!(" {k}=")) || r.contains(&format!(" {n}="));
    let refused = crate::settings::get().rejects.iter().find(at).map(|r| {
        format!(" (refused {})", r.split(" · ").next().unwrap_or_default().trim_start_matches("! bad_setting "))
    });
    format!("{src}{}", refused.unwrap_or_default())
}

/// `config` rows of the session, the cache and the reader.
fn store_rows(s: &settings::Settings) -> Vec<Row> {
    let p = |v: &std::path::Path| v.display().to_string();
    let swept = if s.cache.value.join(MINE).is_file() { "swept" } else { "not swept (dir not created by fats)" };
    let bak = if crate::setup::bak().is_some() { " · bak" } else { "" }; // an old key file beside the keys
    vec![
        ("session", s.session.value.clone(), source(s.session.src, "FATS_SESSION")),
        ("cache", p(&s.cache.value), format!("{} · {swept}", s.cache.src)),
        ("key_dir", p(&s.key_dir.value), format!("{}{bak}", s.key_dir.src)),
        ("json", s.json.value.to_string(), source(s.json.src, "FATS_JSON")),
        ("nocache", s.nocache.value.to_string(), source(s.nocache.src, "FATS_NOCACHE")),
        ("chunk", format!("{}tok", s.chunk.value), source(s.chunk.src, "FATS_CHUNK")),
        ("match", "words (exact first when unpinned)".into(), "default".into()),
        ("max_bytes", format!("{} (ceiling {})", mib(MAX_BYTES), mib(MAX_BYTES_CEIL)), "default · max_bytes:".into()),
        tables(),
        ("network", if s.offline.value { "off" } else { "on" }.into(), s.offline.src.into()),
        ("allow_private", s.allow_private.value.to_string(), source(s.allow_private.src, "FATS_ALLOW_PRIVATE")),
        ("telemetry", s.telemetry.value.name().into(), source(s.telemetry.src, "FATS_TELEMETRY")),
        ("telemetry_debug", s.telemetry_debug.value.to_string(), source(s.telemetry_debug.src, "FATS_TELEMETRY_DEBUG")),
    ]
}

/// The `tables` row: the built-in JSON tables (`settings::TABLES`), their rows in all and per file.
fn tables() -> Row {
    let rows = |json: &str| serde_json::from_str(json).map_or(0, |v| settings::table_rows(&v));
    let each: Vec<(&str, usize)> = settings::TABLES.iter().map(|(name, json)| (*name, rows(json))).collect();
    let total: usize = each.iter().map(|e| e.1).sum();
    let each: Vec<String> = each.iter().map(|(name, n)| format!("{name} {n}")).collect();
    ("tables", format!("{} files · {total} rows", each.len()), format!("built-in · {}", each.join(", ")))
}

/// `config` rows of the router, search, documents, money and MCP.
fn fetch_rows(s: &settings::Settings) -> Vec<Row> {
    let b = bin();
    use crate::provider::{SEARCH_MAX, SEARCH_TIMEOUT};
    let (keys, secs, cc) = (fetch::keyed(), SEARCH_TIMEOUT.as_secs(), crate::provider::home());
    // the search order the keys present run (an oxy-only config named bd); no key: both, as a price list
    let none = !keys.iter().any(|t| t.can("search"));
    let by = |t: Tier, what: &str| (none || keys.contains(&t)).then(|| format!("{what}{:.4}", t.spec().price));
    let order = [by(Tier::Oxy, "oxy $"), by(Tier::Bd, "bd (Google via the zone) $")];
    let order = order.into_iter().flatten().collect::<Vec<_>>().join(", then ");
    let search = format!("{order} · 10 results, at most {SEARCH_MAX} · {secs}s per provider · country: else {cc}");
    // `provider::sent`: a US default is what an Oxylabs call sends; Bright Data then picks its best (`bd_body`)
    let bd = if cc == "US" { " on oxy (bd: its best for the site)" } else { "" };
    let country = format!("country:, else the domain's ccTLD (24 European, .uk → GB), else {cc}{bd}");
    let oxy = fetch::PDF_OXY.as_secs();
    let pdf = format!(
        "plain → oxy (no JS, {oxy}s) → bd (raw bytes), never oxyjs · a .pdf suffix or an answer that says PDF \
         · a /pdf/ path only gives oxy {oxy}s and bd raw bytes"
    );
    let lines = crate::text::LINES;
    let text = format!(
        "text, json, xml bodies (by content type or start) served as they are, a line a row (no id), {lines} \
         lines per section · never a provider, never a route"
    );
    let (on, accept) = (if s.accept_md.value { "on" } else { "off" }, crate::provider::MD_ACCEPT);
    let md = format!(
        "{on} · plain sends Accept: {accept} · a text/markdown answer is cleaned as a page (md) unless thin or flat vs \
         the stored copy · a JS shell or 4xx with the page's own og card is served as the card (og)"
    );
    let (max, wait) = (ksize(ASSET_MAX as usize), ASSET_WAIT.as_secs());
    let pictures =
        format!("{b} uN mK: plain only, image/* at most {max}B, {wait}s, $0 · <page>/assets/ · mcp attaches it");
    let ttl = format!("{}m · {b} account re-reads after it; status reads {ACCOUNT_FILE} only", s.account_ttl.value);
    let ladder = path_of(&fetch::route(&fetch::Plan::new("", fetch::Mode::Auto, false, fetch::COST_CAP, None)).0);
    let auto = format!("auto: {ladder} · oxy no-JS also: files");
    vec![
        // the page ladder the keys present run, a one-key retry included (`fetch::route`, as `dry:true` prints it)
        ("provider", auto, "default · provider:plain|bd|oxy|decodo".into()),
        ("country", country, source(s.country.src, "FATS_COUNTRY") + " · country:"),
        ("search", search, "default · provider:oxy|bd limit: country: site: since: timeout: cost:".into()),
        ("device", "desktop".into(), "default · device:mobile (oxy, decodo)".into()),
        ("hedge", fetch::hedges(None) + " (both billed when both run)", "hedge:N sets both · hedge:0 races".into()),
        ("pdf", pdf, "build · timeout: sets oxy's too".into()),
        ("text", text, "build".into()),
        ("markdown", md, source(s.accept_md.src, "FATS_ACCEPT_MD")),
        ("timeout", "per tier below".into(), "build · timeout:1..300 caps plain, then one wall for paid calls".into()),
        ("cost_cap", format!("${}", fetch::COST_CAP), "default · cost:".into()),
        ("gov_labels", fetch::SITES.gov.join(" "), "build (skip bd): last label, or 2nd under a cc".into()),
        (
            "bd_zone",
            settings::zone().map_or_else(|| format!("none → {b} setup"), String::from),
            s.bd_zone.as_ref().map_or("unset", |z| z.src).into(),
        ),
        ("low_balance", format!("${LOW_BALANCE:.2} · a bd balance under it is a ! low_balance row"), "build".into()),
        ("account_ttl", ttl, source(s.account_ttl.src, "FATS_ACCOUNT_TTL") + " · fresh:true reads now"),
        ("mcp", format!("{b} mcp · stdio · one tool fats(cmd) · files: cwd + cache"), "build".into()),
        ("pictures", pictures, "build".into()),
    ]
}

/// The `instant` row (every command answers by `wait:`): how long a call waits and polls, the prefetch, and each
/// wait outside a provider, the second resolver named as the third party it is.
fn instant_row() -> Row {
    use crate::cli::{POLL, PREFETCH, WAIT};
    let (poll, dns) = (POLL.as_millis(), settings::limit::<u64>("dns_wait"));
    let (doh, doh_wait) = (Api::Doh.row().url, crate::provider::DOH_WAIT.as_secs_f32());
    let pre = if Tier::Plain.key().is_some() { format!("fetch:{PREFETCH}") } else { "off (free fetch off)".into() };
    let instant = format!(
        "wait:{WAIT}s then a fetch or a search slower than wait: (bd) is pending, a gh/account/setup call is cut (provider_timeout wait:) · poll {poll}ms · a run lives while it holds its lock, else abandoned · prefetch {pre} \
         · dns dns_wait:{dns}s, else dns_timeout $0 (an error in < 1 s: dns_failed), on no address a third party: {doh} (A then AAAA, {doh_wait}s each, the host name only)"
    );
    ("instant", instant, "default · wait:0..300 fetch:N".into())
}

/// `fats setup <name>` (its key form, `setup::agent`); `fats setup <name> on|off`, `fats config set <name> <value>`,
/// `fats config unset <name>`: one `settings.json` change, read from the next command on; `dry:true` names it and
/// writes nothing. Refused = `! refused · <why>`.
pub fn change(cmd: &crate::lang::Cmd) -> Out {
    let s: Vec<&str> = cmd.sub.iter().map(String::as_str).collect();
    let r = match s[..] {
        [n] if cmd.has("setup") => return crate::setup::agent(n, cmd.flag("dry")),
        _ if cmd.flag("dry") => Ok(()),
        ["telemetry", v] if cmd.has("setup") => settings::set("telemetry", v),
        [n, v] if cmd.has("setup") => settings::set_on(n, v == "on"),
        ["set", n, v] => settings::set(n, v),
        ["unset", n] => settings::unset(n),
        _ => Err(format!("{} config set <name> <value> · names: {} config", bin(), bin())),
    };
    let head = if cmd.has("setup") { "setup" } else { "config" };
    let what = if cmd.flag("dry") { "would save" } else { "saved" };
    match r {
        Ok(()) => {
            let row = format!("= {head} {} · {what} {} · from the next command", s.join(" "), settings::PREFS);
            // a key switched off may have an old copy beside it: where it is, how to delete it
            let off = matches!(s[..], [_, "off"]) && cmd.has("setup");
            Out::new([row].into_iter().chain(crate::setup::bak_row().filter(|_| off)).collect())
        }
        Err(e) => Out { rc: 1, ..Out::new(vec![format!("! refused · {e}")]).with("error", "refused") },
    }
}

// ---------------------------------------------------------------- `fats account`

/// `<cache root>/account.json`: the last `fats account` read, rows only (no provider answer is kept).
pub const ACCOUNT_FILE: &str = "account.json";
/// A Bright Data balance under this many USD gets a `! low_balance` row.
pub const LOW_BALANCE: f64 = 1.0;

/// Low = under `LOW_BALANCE` as printed (cents), so a row never says `$1.00 left < $1.00`.
fn low(balance: Option<f64>) -> Option<f64> {
    balance.filter(|x| (x * 100.0).round() < LOW_BALANCE * 100.0)
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct AccountRead {
    pub t: f64,
    pub rows: Vec<Account>,
}

/// What `account` and `account dry:true` start from: the account providers with a key, in row order (Bright
/// Data's account API needs no zone), their names, and the last read (`ACCOUNT_FILE`).
fn account_state(c: &Cache) -> (Vec<(Tier, String)>, Vec<&'static str>, Option<AccountRead>) {
    let keys: Vec<(Tier, String)> = ACCOUNTS.iter().filter_map(|&t| Some((t, t.saved_key()?))).collect();
    let names = keys.iter().map(|k| k.0.spec().short).collect();
    (keys, names, jload(&c.root.join(ACCOUNT_FILE)))
}

/// THE cache rule: a read stands in for a new one while it is younger than the TTL, covers the same providers,
/// and every row is ok (a failed row is read again, so a fixed key shows at once).
pub fn account_hit(r: &AccountRead, now: f64, ttl_min: u64, keyed: &[&str]) -> bool {
    let same = r.rows.iter().map(|a| a.provider.as_str()).eq(keyed.iter().copied());
    now - r.t < ttl_min as f64 * 60.0 && same && r.rows.iter().all(|a| a.code == "ok")
}

/// This calendar month in UTC: (first day, next month's first day), `YYYY-MM-DD`.
fn month(now: f64) -> (String, String) {
    let (y, m, _) = crate::lang::civil((now / 86_400.0).floor() as i64);
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    (format!("{y:04}-{m:02}-01"), format!("{ny:04}-{nm:02}-01"))
}

/// `1,571`.
fn group(n: u64) -> String {
    let s = n.to_string();
    let parts: Vec<&str> = s.as_bytes().rchunks(3).rev().filter_map(|c| std::str::from_utf8(c).ok()).collect();
    parts.join(",")
}

/// `$1,571.88` (`d` = 2 decimals), `$0.0090` (4).
fn usd(x: f64, d: usize) -> String {
    let (c, p) = ((x.abs() * 10f64.powi(d as i32)).round() as u64, 10u64.pow(d as u32));
    format!("{}${}.{:0d$}", if x < 0.0 { "-" } else { "" }, group(c / p), c % p)
}

/// The account reads a key may be scoped without, by the word their row prints (`Account::denied`).
const READS: [(Api, &str); 4] =
    [(Api::BdBalance, "balance"), (Api::BdZones, "zones"), (Api::BdCosts, "spend"), (Api::BdZoneCost, "requests")];

/// One provider's rows. A `!` row is built like every other (`lang::bang`, the one `FIX` table), then the
/// provider's own page when its docs give one (`provider::pages`).
pub fn account_rows(a: &Account, limits: Option<&str>) -> Vec<String> {
    let p = a.provider.as_str();
    // a read cut at `wait:` (`provider::late`) gets more time
    let arg = if a.why.starts_with("wait:") { "wait:30" } else { "" };
    let at = At { id: "account", arg, ..Default::default() };
    let bang = |code: &str, what: &str| crate::lang::bang(p, code, what, &at) + &crate::provider::pages(code, [p]);
    let n = |v: Option<u64>| v.map_or("n/a".into(), group);
    match a.code.as_str() {
        "ok" if p == "oxy" => {
            let scrapers = a.scrapers.map_or(String::new(), |s| format!(" · {s} scrapers"));
            let lim = limits.map_or(String::new(), |l| format!(" · limits {l}"));
            vec![format!("oxy  key ok{scrapers} · no balance api{lim}")]
        }
        "ok" => {
            // a read the key is scoped without says so in its own place (`Account::denied`)
            let denied: Vec<&str> = READS.iter().filter(|r| a.denied.contains(&r.0)).map(|r| r.1).collect();
            let or = |w: &str, v: String| if denied.contains(&w) { format!("{w} no permission") } else { v };
            let money = |v: Option<f64>, d| v.map_or("n/a".into(), |x| usd(x, d));
            // the zone's own month: the number `stats` spend is checked against, in its 4 decimals
            let (zone, un, serp) = (money(a.zone_cost, 4), n(a.reqs_unblocker), n(a.reqs_serp));
            let reqs = match settings::zone() {
                Some(_) => format!("zone {zone} mtd · {un} unblocker + {serp} serp reqs this month"),
                None => "reqs n/a (no BRIGHTDATA_UNLOCKER_ZONE)".into(),
            };
            let (left, spent) =
                (or("balance", money(a.balance, 2) + " left"), or("spend", money(a.spend, 2) + " spent this month"));
            let zones = or("zones", if a.zones.is_empty() { "none".into() } else { a.zones.join(", ") });
            let scoped = denied.join(" api, no ");
            let key = if scoped.is_empty() { String::new() } else { format!("key ok (scoped: no {scoped} api) · ") };
            let head = format!("{p}  {key}{left} · {spent} · {} · zones: {zones}", or("requests", reqs));
            let low = low(a.balance).map(|x| bang("low_balance", &format!("{} left < ${LOW_BALANCE:.2}", usd(x, 2))));
            [Some(head), low].into_iter().flatten().collect()
        }
        "no_key" => no_key_rows(p, false),
        "bad_key" => vec![bang("bad_key", "replace the saved key")],
        // Fetch's row, byte for byte (`fetch::fail_detail` names the account after the code)
        "bad_zone" => vec![crate::lang::bang("", "bad_zone", &format!("{p} {}", a.why), &at)],
        code => vec![bang(code, &a.why)],
    }
}

/// `fats account`: one row per provider (balance, spend, usage, zones; Oxylabs offers a key check only), from
/// `account.json` while `account_hit`, else read now (`fresh:true` always reads). Free, read-only calls.
pub fn account(c: &Cache, fresh: bool, wait: Duration) -> Out {
    let (set, b, now) = (settings::get(), bin(), crate::cache::now());
    let (keys, names, old) = account_state(c);
    let ttl = set.account_ttl.value;
    let (read, cached) = match old {
        Some(old) if set.offline.value || (!fresh && account_hit(&old, now, ttl, &names)) => (old, true),
        _ if set.offline.value => {
            let hint = format!("unset FATS_OFFLINE · then {b} account");
            return Out::fail("offline", "= ! offline account · not read yet · FATS_OFFLINE is on".into(), hint);
        }
        _ => {
            let read = AccountRead { t: now, rows: read_accounts(&keys, now, wait) };
            if let Err(e) = jsave(&c.root.join(ACCOUNT_FILE), &read) {
                return Out::fail("cache_write", format!("= ! cache_write {e}"), format!("{b} config · FATS_CACHE"));
            }
            (read, false)
        }
    };
    let metas: Vec<Meta> = usage(&c.root).2.iter().filter_map(|p| jload(p)).collect();
    let lim = limits(&metas);
    let age = (now - read.t).max(0.0);
    let none = ACCOUNTS.iter().filter(|t| !names.contains(&t.spec().short)).map(|t| no_key(*t));
    let part = |acct: Account| account_part(&acct, lim.as_deref(), cached, age);
    let parts: Vec<Out> = read.rows.iter().cloned().chain(none).map(part).collect();
    let mut lines: Vec<String> = parts.iter().flat_map(|p| p.lines.iter().cloned()).collect();
    let count = read.rows.len();
    let calls: usize = read.rows.iter().map(|a| a.calls).sum();
    let ms: u64 = read.rows.iter().map(|a| a.ms).max().unwrap_or(0); // read side by side
    let when = match cached {
        true => format!("{} ago (cached, ttl {ttl}m)", fmt_age(age)),
        false => format!("read now · {calls} calls {ms}ms"),
    };
    // no provider read: the command failed (rc 1), named by the first row's code
    let errors: Vec<Option<&str>> = parts.iter().map(|p| p.rec.get("error").and_then(|e| e.as_str())).collect();
    let code = errors.iter().copied().collect::<Option<Vec<_>>>().and_then(|c| c.first().map(|c| c.to_string()));
    let bang = code.as_ref().map_or(String::new(), |code| format!(" ! {code}"));
    lines.insert(0, format!("={bang} account · {count} provider{} · {when} · $0", if count == 1 { "" } else { "s" }));
    // usage pages of the keyed providers only: a hint never names a provider without a key
    let usage_pages: Vec<String> = names.iter().filter_map(|&p| Some(format!("{p} {}", links(p)?.usage?))).collect();
    let pages = if usage_pages.is_empty() { String::new() } else { format!(" · usage: {}", usage_pages.join(" · ")) };
    // A key code's row is its next move (a bad zone: `fats setup`, no retry), never `again:` beside it
    let key = errors.iter().flatten().any(|e| crate::fetch::KEY_CODES.contains(e));
    let again = if key { String::new() } else { format!("again: {b} account fresh:true · ") };
    lines.push(format!("{again}calls: {b} account dry:true{pages}"));
    let mut out = Out::new(lines);
    out.parts = parts;
    out.set("cost", 0.0).set("cache", cached);
    if let Some(code) = code {
        out.rc = 1;
        out.set("error", code);
    }
    out
}

// ---------------------------------------------------------------- `fats setup`

/// `fats setup`: each provider's key (`auth.json`: `settings::key`; with `import`, `fats setup import-env`, the
/// environment's where none is saved), checked with its free read (`provider::check_key`: Oxylabs the scraper list,
/// Bright Data the zone list, Decodo a scrape of a host that does not exist). The check writes nothing it was not
/// asked to (`keep`). Bright Data's Web Unlocker zone: `zone:`, else the configured one, else the account's only
/// one. Never prints a key (its tail only); `dry:true` makes no call. The checks run side by side, each given up at
/// `wait`.
pub fn setup(zone: Option<&str>, import: bool, dry: bool, wait: Duration) -> Out {
    let b = bin();
    let (mut heads, mut lines, mut ok) = (Vec::new(), Vec::new(), true);
    // the free fetch switched off heads the summary, its move back the first row
    let off = (!settings::on("free")).then(crate::setup::free_off);
    heads.extend(off.as_ref().map(|o| o.0.to_string()));
    lines.extend(off.map(|o| o.1));
    let env = |t: Tier| settings::env_key(t.spec().short).filter(|_| import).map(|(var, k)| (k, var));
    let found: Vec<_> = KEYS.iter().map(|t| t.saved_key().map(|k| (k, settings::AUTH)).or_else(|| env(*t))).collect();
    let keyed = KEYS.iter().zip(&found).filter_map(|(t, k)| Some((*t, k.as_ref()?.0.clone())));
    let calls = if dry { Vec::new() } else { keyed.map(|(t, key)| crate::provider::check_key(t, key)).collect() };
    let mut reads = crate::provider::all_within(calls, wait).into_iter();
    for (t, found) in KEYS.into_iter().zip(found) {
        let (s, keyed) = (t.spec(), found.is_some());
        let (head, mut rows) = match found {
            None => ("none".into(), no_key_rows(s.short, true)),
            Some((k, at)) if dry => {
                let row = crate::provider::check_call(t).0.row();
                let what = format!("{} key {} {at} · would read {} {} · $0", s.short, tail(&k), row.method, row.url);
                ("not checked".into(), vec![what])
            }
            Some((k, at)) => match reads.next().flatten().unwrap_or_else(|| Err(late(wait))) {
                Ok(j) => keep(t, (&k, at), j, zone),
                Err(f) => (format!("! {}", f.code), account_rows(&failed(t, f), None)),
            },
        };
        // after the check (an imported key is saved by then): the env's row, which for a missing key IS its
        // `no_key` row, one row a missing key
        if let Some(env) = crate::setup::env_row(s.short) {
            if !keyed {
                rows.retain(|r| !r.starts_with("! no_key "));
            }
            rows.push(env);
        }
        // a key form still waiting or dead: its row, so the agent knows which link the user holds
        rows.extend(crate::setup::form_row(s.short).filter(|_| !keyed));
        ok &= !head.starts_with('!');
        heads.push(format!("{} {head}", s.short));
        lines.extend(rows);
    }
    let dir = settings::get().key_dir.value.display();
    let mut end = if dry { " · spends nothing now".to_string() } else { format!(" · key dir {dir}") };
    // no key and the free fetch on: what works now and the one key search needs, not a list of `none`
    if heads.iter().all(|h| h.ends_with(" none")) {
        heads = vec![format!("no key · pages free · web search off until one key is saved → {}", fetch::search_key())];
        end = String::new();
    }
    lines.insert(0, format!("={} setup {}{end} · $0", if dry { " dry" } else { "" }, heads.join(" · ")));
    lines.extend(crate::telemetry::row());
    lines.push(format!("status: {b} · balance: {b} account · {}", crate::lang::to_guide()));
    let mut out = Out { rc: i32::from(!ok), ..Out::new(lines) }; // a failed check = rc 1
    out.set("cost", 0.0).set("dry", dry);
    out
}

/// A key that passed its check: the head and row, with the zone for Bright Data. Saved only when asked: a key from
/// the environment (`at` = its variable: `fats setup import-env`) with the zone in use, a new zone `zone:` names; a
/// zone picked without it is named, unsaved. A check of the saved keys leaves the key dir as it was.
fn keep(t: Tier, (key, at): (&str, &str), got: (String, Vec<String>), arg: Option<&str>) -> (String, Vec<String>) {
    let (p, (what, zones)) = (t.spec().short, got);
    let mut zone = None;
    if t == Tier::Bd {
        match crate::setup::pick_zone(&zones, arg) {
            Ok(z) => zone = Some(z),
            Err(row) => return ("! no_zone".into(), vec![row]),
        }
    }
    let what = zone.as_ref().map_or(what, |z| format!("zone {z}"));
    let saved = at == settings::AUTH;
    let end = match zone.as_deref().filter(|z| settings::zone() != Some(*z)) {
        None if saved => String::new(),
        Some(z) if saved && arg.is_none() => format!(" · zone not saved → {} setup bd zone:{z}", bin()),
        _ => match settings::save_key(p, key, zone.as_deref()) {
            Ok(()) if zone.is_some() => format!(" · saved {} settings.json (600)", settings::AUTH),
            Ok(()) => format!(" · saved {} (600)", settings::AUTH),
            Err(e) => {
                let row = format!("{p} ! cache_write {e} → {} config · FATS_KEY_DIR", bin());
                return ("! cache_write".into(), vec![row]);
            }
        },
    };
    (format!("ok {what}"), vec![format!("{p} ok {what} · key {} {at}{end}", tail(key))])
}

/// A provider's `no_key` row outside a fetch, the fetch row's own text (`fetch::no_key`): what every tier of it
/// would add (the caller found no key for `p`); `setup` = inside `fats setup`.
fn no_key_rows(p: &str, setup: bool) -> Vec<String> {
    let owner = Tier::parse(p).map(Tier::owner);
    let (mine, keys) = Tier::ALL.into_iter().partition::<Vec<_>, _>(|t| Some(t.owner()) == owner);
    let plan = fetch::Plan { keys, ..fetch::Plan::new("", fetch::Mode::Auto, false, 1.0, None) };
    fetch::no_key(&plan, &mine, "", setup)
}

/// The account row of a provider without a key.
fn no_key(t: Tier) -> Account {
    Account { provider: t.spec().short.into(), code: "no_key".into(), ..Account::default() }
}

/// Read every keyed provider's account now, side by side; one still reading at `wait` is a `late` row.
fn read_accounts(keys: &[(Tier, String)], now: f64, wait: Duration) -> Vec<Account> {
    let read = |(t, key): &(Tier, String)| crate::provider::account(*t, key.clone(), month(now), settings::zone());
    let (got, ms) = (crate::provider::all_within(keys.iter().map(read).collect(), wait), wait.as_millis() as u64);
    got.into_iter().zip(keys).map(|(a, (t, _))| a.unwrap_or_else(|| Account { ms, ..failed(*t, late(wait)) })).collect()
}

/// A provider's account row for a failed call.
fn failed(t: Tier, f: crate::provider::Fail) -> Account {
    Account { code: f.code.into(), why: f.why, ..no_key(t) }
}

/// One provider's rows and its JSON record (the parsed account, never an answer).
fn account_part(acct: &Account, lim: Option<&str>, cached: bool, age: f64) -> Out {
    let mut part = Out::new(account_rows(acct, lim));
    if let Ok(serde_json::Value::Object(m)) = serde_json::to_value(acct) {
        part.rec = Box::new(m);
    }
    let name = Tier::parse(&acct.provider).map_or("", |t| t.spec().name);
    part.set("provider", name).set("cost", 0.0).set("cache", cached).set("age_s", age as u64);
    if acct.code != "ok" {
        part.set("error", acct.code.clone());
    }
    part
}

/// What a request would read: `account dry:true` lists the calls (all reads, no listed price) or the cache hit.
pub fn account_dry(c: &Cache, fresh: bool) -> Out {
    let (s, b, now) = (settings::get(), bin(), crate::cache::now());
    let (keys, names, old) = account_state(c);
    let ttl = s.account_ttl.value;
    let hit = old.as_ref().filter(|r| !fresh && account_hit(r, now, ttl, &names));
    let (from, to) = month(now);
    let call = |&(t, _): &(Tier, String)| {
        account_calls(t, (&from, &to), settings::zone()).into_iter().map(move |(a, r)| (t, a, r))
    };
    let calls: Vec<_> = if hit.is_some() { Vec::new() } else { keys.iter().flat_map(call).collect() };
    let mut lines =
        vec![format!("= dry account · {} calls · read-only, no listed price · $0 · spends nothing now", calls.len())];
    if let Some(r) = hit {
        let age = fmt_age((now - r.t).max(0.0));
        lines.push(format!("cache account.json {age} old (ttl {ttl}m) → no calls · fresh:true reads again"));
    }
    for (t, api, req) in &calls {
        let what = crate::lang::draw(api.row().what, &[("from", &from), ("to", &to)]);
        lines.push(format!("{} {} {} · {what}", t.spec().short, api.row().method, api.url(req)));
    }
    for t in ACCOUNTS.iter().filter(|t| !names.contains(&t.spec().short)) {
        lines.extend(account_rows(&no_key(*t), None));
    }
    lines.push(format!("run it: {b} account{}", if fresh { " fresh:true" } else { "" }));
    Out::new(lines).with("cost", 0.0).with("dry", true)
}

/// The status line, from `account.json` only (never a network call): `account: 2 providers · 12m ago · fats
/// account`, with any `!` code (`bd ! bad_key`, `bd ! low_balance`).
fn account_line(c: &Cache, now: f64) -> String {
    let b = bin();
    let Some(r) = jload::<AccountRead>(&c.root.join(ACCOUNT_FILE)) else {
        return format!("account: not read yet · {b} account");
    };
    let flag = |a: &Account| match a.code.as_str() {
        "ok" if low(a.balance).is_some() => Some(format!("{} ! low_balance", a.provider)),
        "ok" => None,
        code => Some(format!("{} ! {code}", a.provider)),
    };
    let bang: Vec<String> = r.rows.iter().filter_map(flag).collect();
    let bang = if bang.is_empty() { String::new() } else { format!(" · {}", bang.join(" · ")) };
    let n = r.rows.len();
    let age = fmt_age((now - r.t).max(0.0));
    format!("account: {n} provider{}{bang} · {age} ago · {b} account", if n == 1 { "" } else { "s" })
}

/// The summary's wire contract, also held by the receiving end: a count's ceiling, entries per map (crash spots
/// apart), providers in `keys`.
const MAX_COUNT: u64 = 1_000_000;
const MAX_ENTRIES: usize = 64;
const MAX_CRASHES: usize = 32;
const MAX_KEYS: usize = 16;

/// The classes a command's answer is a page read in: their answer's tokens are a `read` sample.
const PAGE_READS: [&str; 5] = ["read", "filter", "outline", "cards", "links"];

/// A command's class, a fixed word, never its words, ids or URL: a reserved word as itself (`stats`, `cards`),
/// else what it does. Reading words (`more`, `full`) are a read.
fn class(cmd: &Cmd) -> &'static str {
    let word = cmd.reserved.iter().find(|w| !["more", "up", "all", "full", "json"].contains(w));
    let page = !cmd.ids.is_empty() || !cmd.urls.is_empty() || !cmd.files.is_empty() || !cmd.jobs.is_empty();
    match () {
        _ if cmd.ask.is_some() => "usage",
        _ if cmd.flag("dry") => "dry",
        _ if cmd.idle => "status",
        _ if crate::cli::is_gh(cmd) => "gh",
        _ if !cmd.sub.is_empty() => "set",
        _ if let Some(w) = word => w,
        _ if !cmd.media.is_empty() => "media",
        _ if crate::cli::is_search(cmd) => "search",
        _ if !cmd.words.is_empty() => "filter",
        _ if cmd.has("more") || cmd.has("up") || cmd.ids.iter().any(|i| i.sec.is_some()) => "read",
        _ if page => "outline",
        _ => "other",
    }
}

/// Who runs fats: `mcp` through `fats mcp`, else the first host whose env marker is set (words.json `hosts`,
/// each read in that host's own code), else `shell` on a terminal, else `unknown`.
pub fn host_of(mcp: bool, set: impl Fn(&str) -> bool, tty: bool) -> &'static str {
    let marked = crate::lang::WORDS.hosts.iter().find(|h| set(h.0)).map(|h| h.1);
    if mcp { "mcp" } else { marked.unwrap_or(if tty { "shell" } else { "unknown" }) }
}

/// Count a command (`cli::run`): who ran it, its class, a read's answer tokens, and the code of a failed one
/// that stores no record (a fetch's or search's counts with its calls, `cache::tally`).
pub fn tally_run(root: &Path, cmd: &Cmd, out: &Out) {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    let tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let (class, host) = (class(cmd), host_of(FENCE.get().is_some(), set, tty));
    let stores = PAGE_READS.contains(&class) || ["search", "gh", "media"].contains(&class);
    let code = out.rec.get("error").and_then(|v| v.as_str()).filter(|_| !stores).map(code_of);
    // a read's tokens, counted before the lock is taken
    let tok = (PAGE_READS.contains(&class) && out.rc == 0).then(|| crate::clean::tok(&out.lines.join("\n")) as u64);
    bump(root, std::fs::File::lock, |w| {
        w.add(&["host", host], true);
        w.add(&["cmd", class], true);
        tok.into_iter().for_each(|n| w.sample("tok\tread", n));
        w.add(&["codes", code.as_deref().unwrap_or_default()], code.is_some());
    });
}

/// `file.rs:N kind` of a panic in OUR code (`outline.rs:412 index out of bounds`): its file under this crate's
/// `src/` and the message's leading lowercase words, at most `cap` chars, cut at the first other char (a digit,
/// a value, a quote, a path); None = a panic elsewhere (std, a dependency). Either separator names the same file
/// (Windows writes `src\outline.rs`), the key always `/`.
pub fn crash_key(file: &str, line: u32, msg: &str, cap: usize) -> Option<String> {
    let (file, ours) = (file.replace('\\', "/"), file!().replace('\\', "/"));
    let file = file.strip_prefix(ours.strip_suffix("stats.rs")?)?;
    let kind: String = msg.chars().take_while(|&c| c.is_ascii_lowercase() || c == ' ').take(cap).collect();
    Some(format!("{file}:{line} {}", kind.trim()).trim_end().into())
}

/// Count a panic in `root`'s tally: `crash_key`, nothing else of it; dropped while the tally's lock is held (by the
/// panicking thread, maybe: a wait would never end).
pub fn crash(root: &Path, file: &str, line: u32, msg: &str, cap: usize) {
    let Some(k) = crash_key(file, line, msg, cap) else { return };
    bump(root, |f| f.try_lock().map_err(Into::into), |w| w.add(&["crash", &k], true));
}

/// The binary's first step: its start time (`telemetry::STARTED`) and its panic hook: `crash`, then the default hook
/// prints it. Root and cap are read now, never inside a panic.
pub fn hook() {
    std::sync::LazyLock::force(&crate::telemetry::STARTED);
    let (root, cap, shown) =
        (settings::get().cache.value.clone(), settings::limit("crash_chars"), std::panic::take_hook());
    std::panic::set_hook(Box::new(move |p| {
        if let Some(l) = p.location() {
            crash(&root, l.file(), l.line(), p.payload_as_str().unwrap_or_default(), cap);
        }
        shown(p);
    }));
}

/// `week`'s usage summary from `root`'s tally: counts, codes and fixed names only (`keys` = the providers with a
/// key, never one); paid providers and hosts listed at 0 too, tiers once seen; the wire contract's caps, the
/// biggest counts kept.
pub fn weekly_of(root: &Path, week: &str) -> Value {
    let w: Week = jload(&root.join("telemetry/week").join(format!("{week}.json"))).unwrap_or_default();
    let switch = |ts: Vec<Tier>| {
        let mut v: Vec<&str> = ts.into_iter().filter(|t| *t != Tier::Plain).map(Tier::switch).collect();
        v.dedup();
        v
    };
    let (paid, mut keys) = (switch(Tier::ALL.to_vec()), switch(fetch::keyed()));
    keys.truncate(MAX_KEYS);
    let hosts = crate::lang::WORDS.hosts.iter().map(|h| h.1).chain(["mcp", "shell", "unknown"]);
    let host: Map<String, Value> = hosts.map(|h| (h.into(), 0.into())).collect();
    let provider: Map<String, Value> =
        paid.iter().map(|p| (p.to_string(), json!({"calls": 0, "served": 0, "search": 0}))).collect();
    let pct = |k: &str, q: usize| {
        let mut s = w.samples.get(k).cloned().unwrap_or_default();
        s.sort_unstable();
        let n = s.get(s.len() * q / 100).copied().unwrap_or_default();
        let unit = 10u64.pow(n.checked_ilog10().unwrap_or(0).saturating_sub(1)); // 2 significant digits: 1234 → 1200
        n.saturating_add(unit / 2) / unit * unit
    };
    let tok = |k: &str| json!({"p50": pct(k, 50), "p90": pct(k, 90)});
    let mut v = json!({"s": 1, "v": env!("CARGO_PKG_VERSION"), "os": std::env::consts::OS,
        "arch": crate::telemetry::arch(), "week": week, "keys": keys,
        "provider": provider, "host": host, "cmd": {}, "tier": {}, "codes": {},
        "tok": {"page": tok("tok\tpage"), "read": tok("tok\tread")},
        "ms": {"plain_p50": pct("ms\tplain", 50), "paid_p50": pct("ms\tpaid", 50)}, "crash": {}});
    let mut rows: Vec<(&String, &u64)> = w.n.iter().collect();
    rows.sort_by_key(|r| std::cmp::Reverse(*r.1));
    for (path, n) in rows {
        let p: Vec<&str> = path.split('\t').collect();
        let cap = if p[0] == "crash" { MAX_CRASHES } else { MAX_ENTRIES };
        let Some(map) = v.get_mut(p[0]) else { continue }; // a key the summary has no place for is dropped
        if p.len() > 1 && map.as_object().is_some_and(|m| m.len() < cap || m.contains_key(p[1])) {
            *p[1..].iter().fold(map, |at, k| &mut at[*k]) = (*n).min(MAX_COUNT).into();
        }
    }
    let tiers = v["tier"].as_object_mut().into_iter().flat_map(|t| t.values_mut()).filter_map(Value::as_object_mut);
    for t in tiers {
        ["ok", "fail", "billed", "upto"].into_iter().for_each(|k| _ = t.entry(k).or_insert(0.into()));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{now, week_of};
    use std::collections::BTreeMap;

    /// The request budget shown is the newest answer's, whatever order the cache walk found them in.
    #[test]
    fn limits_are_the_newest() {
        let m = |t: f64, l: u64| -> Meta {
            serde_json::from_value(serde_json::json!({"t": t, "hops": [{"provider": "oxylabs", "limit": l}]}))
                .unwrap_or_default()
        };
        assert_eq!(limits(&[m(9.0, 250), m(1.0, 100)]).as_deref(), Some("250 scrape"));
    }

    /// A call cancelled after the other provider won is still a call, its bill unverified: health shows it.
    #[test]
    fn health_counts_cancelled_calls() {
        let dir = crate::scratch::Scratch::new("health");
        let meta = serde_json::json!({"t": 1.0, "hops": [
            {"provider": "plain", "code": "js_shell", "ms": 300, "cost": 0.0},
            {"provider": "oxylabs", "code": "provider_error", "ms": 2000, "cost": 0.0},
            {"provider": "oxylabs-js", "code": "ok", "ms": 9000, "cost": 0.0015},
            {"provider": "brightdata", "code": "abandoned", "ms": 9000, "cost": 0.0, "upto": 0.0015},
        ]});
        let p = dir.join("meta.json");
        std::fs::write(&p, meta.to_string()).expect("write meta");
        let rows = health(&[p]);
        let want = [
            "plain 0/1 300ms ✗",
            "bd 0/1 - – · 1 cancelled, billed $0.0000 + ≤$0.0015 unverified",
            "oxy 0/1 2000ms ✗",
            "oxyjs 1/1 9000ms ✓",
        ];
        assert_eq!(rows[1..5], want);
    }

    /// A run left at `wait:3`, then `kill -9`: `stats` reads THE liveness rule (`Cache::run`) as `fats u1`
    /// does: `pending` while the run's lock is held, `! abandoned` once it is gone, never `pending` for a dead run.
    #[test]
    fn stats_reads_the_run_lock() {
        let root = crate::scratch::Scratch::new("stats-lock");
        let c = Cache::at(root.to_path_buf(), "t".into());
        let lock = c.run_claim(1).expect("the launcher's lock");
        let status = "plain:js_shell✗ · bd running".into();
        let run = crate::cache::Detached { url: "https://ex.com/a".into(), job: 1, status, ..Default::default() };
        c.run_put(1, &run).expect("run file");
        let e = Entry { url: run.url.clone(), ..Default::default() };
        let row = || page_row(&c, 1, &e, "");
        assert!(row().lean.starts_with("u1 ex.com/a · pending → "), "{}", row().lean);
        drop(lock);
        assert!(row().lean.starts_with("u1 ex.com · ! abandoned "), "{}", row().lean);
        // Its call in flight is a call we cut, in the row and the total; plain's is free
        let r = row();
        assert!(r.lean.contains(" · bd:abandoned✗≤$0.0015 (unverified) → ") && r.upto == 0.0015, "{}", r.lean);
    }

    /// The index rows: a long title cut at a word to fit `row_chars` (`…`), whole in `full`; a host's own name
    /// is no title; a search row; a failed search keeps its tried path.
    #[test]
    fn stats_rows_fit_one_line() {
        let title = "The Rust Programming Language and every book, guide and reference the project keeps online";
        let r = StatRow::new("u2", "rust-lang.org", title, ("plain 229ms $0 · 445tok".into(), "x".into()));
        assert!(r.lean.starts_with("u2 rust-lang.org · The Rust Programming") && r.lean.contains("… · plain 229ms"));
        assert!(
            r.lean.chars().count() <= settings::limit::<usize>("row_chars")
                && r.full == format!("u2 rust-lang.org · {title} · x"),
            "{}",
            r.lean
        );
        assert_eq!(StatRow::new("u3", "a.io", "a.io", (String::new(), "! x".into())).lean, "u3 a.io · ! x");
        let root = crate::scratch::Scratch::new("stats-rows");
        let c = Cache::at(root.to_path_buf(), "t".into());
        let hits = serde_json::json!([{"url": "https://a.io/"}, {"url": "https://b.io/"}]);
        let mut m = Meta {
            url: "search:rust http".into(),
            provider: "oxylabs".into(),
            ok: true,
            ms: 1800,
            cost: 0.001,
            ..Meta::default()
        };
        m.extra.insert("hits".into(), hits);
        assert_eq!(search_row(&c, 4, &m).lean, "j4 search \"rust http\" · 2 results · oxylabs 1800ms $0.0010");
        let hop = crate::cache::HopRow {
            provider: "brightdata".into(),
            code: "provider_timeout".into(),
            ..Default::default()
        };
        let m = Meta {
            ok: false,
            code: "provider_timeout".into(),
            provider: String::new(),
            cost: 0.0,
            hops: vec![hop],
            ..m
        };
        let r = search_row(&c, 5, &m);
        assert!(
            r.lean.starts_with("j5 search \"rust http\" · ! provider_timeout · - 1800ms ") && r.lean.contains(" · bd"),
            "{}",
            r.lean
        );
    }

    /// The cache rule: young, same providers, every row ok; anything else reads again.
    #[test]
    fn account_cache_rule() {
        let row = |p: &str, code: &str| Account { provider: p.into(), code: code.into(), ..Account::default() };
        let r = AccountRead { t: 1000.0, rows: vec![row("oxy", "ok"), row("bd", "ok")] };
        assert!(account_hit(&r, 1000.0 + 14.0 * 60.0, 15, &["oxy", "bd"]), "14m old, ttl 15m");
        assert!(!account_hit(&r, 1000.0 + 15.0 * 60.0, 15, &["oxy", "bd"]), "expired");
        assert!(!account_hit(&r, 1001.0, 0, &["oxy", "bd"]), "ttl 0 = always read");
        assert!(!account_hit(&r, 1001.0, 15, &["oxy"]), "a key was removed");
        let bad = AccountRead { t: 1000.0, rows: vec![row("oxy", "ok"), row("bd", "bad_key")] };
        assert!(!account_hit(&bad, 1001.0, 15, &["oxy", "bd"]), "a failed row is read again");
        assert_eq!(
            (usd(1234.5, 2), usd(0.42, 2), usd(-3.0, 2), group(1_234_567)),
            ("$1,234.50".into(), "$0.42".into(), "-$3.00".into(), "1,234,567".into())
        );
        assert_eq!(usd(0.009, 4), "$0.0090", "the zone figure in fats' own 4 decimals");
        assert_eq!(month(1_791_763_200.0), ("2026-10-01".into(), "2026-11-01".into()), "2026-10-12");
        assert_eq!(month(1_798_761_540.0), ("2026-12-01".into(), "2027-01-01".into()), "2026-12-31 23:59");
    }

    /// A check of a saved Bright Data key names the zone it picked (the account's only one) and saves nothing;
    /// `zone:` saves the zone it names, with the key's file rewritten as it was.
    #[test]
    fn a_check_names_the_zone_it_picked() {
        let dir = crate::scratch::Scratch::new("keep-zone");
        settings::tests::KEY_DIR.set(Some(dir.to_path_buf()));
        let saved = ("bd-key-0123456789abcdef", settings::AUTH);
        crate::scratch::auth(&dir, &[("bd", saved.0)]);
        let before = std::fs::read(dir.join(settings::AUTH)).expect("auth.json");
        let zones = || ("key ok".to_string(), vec!["z_only".to_string()]);
        let (head, rows) = keep(Tier::Bd, saved, zones(), None);
        let row = format!("bd ok zone z_only · key …cdef auth.json · zone not saved → {} setup bd zone:z_only", bin());
        assert_eq!((head.as_str(), rows), ("ok zone z_only", vec![row]));
        assert_eq!(std::fs::read(dir.join(settings::AUTH)).expect("auth.json"), before, "a check writes nothing");
        assert!(!dir.join(settings::PREFS).exists(), "no zone saved");
        let (_, rows) = keep(Tier::Bd, saved, zones(), Some("z_only"));
        assert!(rows[0].ends_with(" · saved auth.json settings.json (600)"), "{rows:?}");
        assert_eq!(settings::saved(settings::PREFS).bd_zone.as_deref(), Some("z_only"));
        settings::tests::KEY_DIR.set(None);
    }

    /// A key prints as its last 4 chars, only from a key of 16 or more: a short one shows none of itself.
    #[test]
    fn a_key_prints_as_its_tail() {
        assert_eq!((tail("oxyK3y-7f9QzR2mWb5Lp8TnVc"), tail("0123456789abcdef")), ("…TnVc".into(), "…cdef".into()));
        assert_eq!((tail("sunflower"), tail("0123456789abcde"), tail("")), ("…".into(), "…".into(), "…".into()));
    }

    /// `!` rows: built like every other (`lang::bang` + `FIX`), then the provider's own page when its docs give one.
    #[test]
    fn account_bang_rows_link() {
        let b = bin();
        let row = |p: &str, code: &str| Account { provider: p.into(), code: code.into(), ..Account::default() };
        // The fetch row's text (buys, price, every variable) and its key form; `setup` never pointing at itself
        let oxy = "! no_key oxy · would try no-JS fetch + JS render ≤$0.0025";
        assert_eq!(
            account_rows(&row("oxy", "no_key"), None),
            [format!("{oxy} → run {b} setup oxy, give the user its 3 lines")]
        );
        let bd = "! no_key bd · would try unblock+JS ≤$0.0015";
        assert_eq!(
            account_rows(&row("bd", "no_key"), None),
            [format!("{bd} → run {b} setup bd, give the user its 3 lines")]
        );
        // inside `fats setup`: what the key adds, no price, and its form; Decodo cannot search
        let bd_adds = "! no_key bd · search + JS pages · fats setup bd, 3 steps for the user";
        assert_eq!(no_key_rows("bd", true), [bd_adds.replace("fats", b)]);
        let decodo = "! no_key decodo · JS pages, no web search · fats setup decodo";
        assert_eq!(no_key_rows("decodo", true), [decodo.replace("fats", b)]);
        assert_eq!(
            account_rows(&row("oxy", "bad_key"), None),
            [format!(
                "oxy ! bad_key replace the saved key → {b} setup · oxy https://dashboard.oxylabs.io (a Web API instance's key)"
            )],
            "the product the key must come from"
        );
        let limited = Account { why: "retry in 5s".into(), ..row("bd", "rate_limited") };
        assert_eq!(account_rows(&limited, None), [format!("bd ! rate_limited retry in 5s → {b} account fresh:true")]);
        let oxy = Account { scrapers: Some(351), ..row("oxy", "ok") };
        assert_eq!(
            account_rows(&oxy, Some("250 scrape · 10 search")),
            ["oxy  key ok · 351 scrapers · no balance api · limits 250 scrape · 10 search"]
        );
        let low = Account { balance: Some(0.994), ..row("bd", "ok") };
        assert_eq!(
            account_rows(&low, None)[1],
            format!(
                "bd ! low_balance $0.99 left < $1.00 → top up, then {b} account fresh:true · bd https://brightdata.com/cp/billing"
            )
        );
        let rows = |x: f64| account_rows(&Account { balance: Some(x), ..row("bd", "ok") }, None).len();
        assert_eq!((rows(0.999), rows(1.0)), (1, 1), "prints as $1.00: not low");
    }

    /// A scoped key's missing reads are named by endpoint, never by the wording of its `what`: every Bright Data read
    /// has its word, and each word lands in its own place.
    #[test]
    fn scoped_reads_by_endpoint() {
        let calls = account_calls(Tier::Bd, ("a", "b"), Some("z"));
        assert!(calls.iter().all(|(api, _)| READS.iter().any(|r| r.0 == *api)), "a read without a word");
        let a = Account {
            provider: "bd".into(),
            code: "ok".into(),
            denied: vec![Api::BdZoneCost, Api::BdZones],
            ..Account::default()
        };
        let row = account_rows(&a, None).concat();
        assert!(row.starts_with("bd  key ok (scoped: no zones api, no requests api) · n/a left · "), "{row}");
        assert!(row.contains(" · requests no permission · zones: zones no permission"), "{row}");
    }

    /// The ISO week, its year included: 2027-01-01 is a Friday of 2026's week 53.
    #[test]
    fn weeks_are_iso() {
        assert_eq!((week_of(1_792_497_600.0), week_of(1_798_804_800.0)), ("2026-W43".into(), "2026-W53".into()));
    }

    /// A panic's count key: our file under `src/`, its line and the message's leading lowercase words, cut before
    /// any value; std's and a dependency's panics are not ours.
    #[test]
    fn crash_keys_hold_no_values() {
        let k = |f: &str, m: &str| crash_key(f, 412, m, 48);
        let oob = k("src/outline.rs", "index out of bounds: the len is 3 but the index is 5");
        assert_eq!(oob.as_deref(), Some("outline.rs:412 index out of bounds"));
        assert_eq!(k("src\\outline.rs", "index out of bounds").as_deref(), Some("outline.rs:412 index out of bounds"));
        assert_eq!(k("src/cli.rs", "https://secret.example/q?x=1 broke").as_deref(), Some("cli.rs:412 https"));
        assert_eq!(
            k("src/cli.rs", "byte index 9 is not a char boundary; it is inside 'é'").as_deref(),
            Some("cli.rs:412 byte index")
        );
        assert_eq!(k("src/cli.rs", "Hidden ").as_deref(), Some("cli.rs:412"));
        assert_eq!(k("src/cli.rs", "attempt 2 add with overflow").as_deref(), Some("cli.rs:412 attempt"));
        assert_eq!(crash_key("src/cli.rs", 1, &"a".repeat(80), 5).as_deref(), Some("cli.rs:1 aaaaa"));
        assert_eq!(k("/rustc/abc/library/core/src/slice/index.rs", "index out of bounds"), None);
        assert_eq!(k("/Users/x/.cargo/registry/src/serde-1/src/de.rs", "oops"), None);
    }

    /// The host by its env marker, each one alone, the first in words.json order when several are set; `mcp`
    /// first; no marker = `shell` on a terminal, else `unknown`.
    #[test]
    fn host_by_marker() {
        let only = |k: &'static str| move |v: &str| v == k;
        let got = ["CLAUDECODE", "CODEX_THREAD_ID", "PI_CODING_AGENT", "CURSOR_TRACE_ID"]
            .map(|k| host_of(false, only(k), false));
        assert_eq!(got, ["claude_code", "codex", "pi", "unknown"]);
        assert_eq!(host_of(false, |k| k != "CLAUDECODE", true), "codex");
        assert_eq!((host_of(true, |_| true, true), host_of(false, |_| false, true)), ("mcp", "shell"));
    }

    /// The summary from a seeded tally: calls per tier and provider (a cut call neither ok nor failed), codes from
    /// our table only (anything else = `other`), the record's own code once, p50/p90, a PDF, a crash, every paid
    /// provider and host listed; never a URL, query or path.
    #[test]
    fn weekly_counts_codes_only() {
        let root = crate::scratch::Scratch::new("weekly");
        let hop = |p: &str, code: &str, ms: u64, cost: f64, upto: f64| json!({"provider": p, "code": code, "ms": ms, "cost": cost, "upto": upto});
        let page: Meta = serde_json::from_value(json!({"url": "https://secret.example/a", "provider": "oxylabs-js",
            "ok": true, "kind": "pdf", "hops": [hop("plain", "js_shell", 300, 0.0, 0.0),
            hop("oxylabs-js", "ok", 9000, 0.0015, 0.0), hop("brightdata", "abandoned", 9000, 0.0, 0.0015)]}))
        .unwrap_or_default();
        let failed: Meta = serde_json::from_value(json!({"url": "https://secret.example/b", "provider": "",
            "code": "no_key", "hops": [hop("plain", "https://secret.example/b", 120, 0.0, 0.0)]}))
        .unwrap_or_default();
        let serp: Meta = serde_json::from_value(json!({"url": "search:secret words", "provider": "oxylabs",
            "ok": true, "kind": "serp", "hops": [hop("oxylabs", "ok", 900, 0.001, 0.0)]}))
        .unwrap_or_default();
        crate::cache::tally(&root, &page, Some(2049)); // rounded to 2 significant digits
        for (n, m) in [(100, &failed), (400, &serp)] {
            crate::cache::tally(&root, m, Some(n));
        }
        crash(&root, "src/outline.rs", 7, "attempt to subtract with overflow", 48);
        let w = weekly_of(&root, &week_of(now()));
        assert_eq!(w["tier"]["plain"], json!({"ok": 0, "fail": 2, "billed": 0, "upto": 0}));
        assert_eq!(w["tier"]["oxyjs"], json!({"ok": 1, "fail": 0, "billed": 1, "upto": 0}));
        assert_eq!(w["tier"]["bd"], json!({"ok": 0, "fail": 0, "billed": 0, "upto": 1}));
        assert_eq!(w["tier"]["oxy_search"]["billed"], 1);
        assert_eq!(w["codes"], json!({"js_shell": 1, "no_key": 1, "other": 1}));
        assert_eq!(
            w["provider"],
            json!({"bd": {"calls": 1, "served": 0, "search": 0},
            "oxy": {"calls": 1, "served": 1, "search": 1}, "decodo": {"calls": 0, "served": 0, "search": 0}})
        );
        assert_eq!((w["tok"]["page"].clone(), w["cmd"]["pdf"].clone()), (json!({"p50": 400, "p90": 2000}), json!(1)));
        assert_eq!(w["ms"], json!({"plain_p50": 0, "paid_p50": 9000}));
        assert_eq!(w["crash"], json!({"outline.rs:7 attempt to subtract with overflow": 1}));
        assert_eq!(w["host"].as_object().map(|h| h.len()), Some(6));
        let text = w.to_string();
        for leak in ["secret", "http", "search:", root.to_string_lossy().as_ref()] {
            assert!(!text.contains(leak), "{leak} in {text}");
        }
    }

    /// The wire contract's caps, on the client too: a count at most 1,000,000, at most 32 crash spots and 64
    /// entries a map, the biggest counts kept.
    #[test]
    fn weekly_caps_its_maps() {
        let root = crate::scratch::Scratch::new("weekly-caps");
        let rows =
            (0..40).map(|i| (format!("crash\tcli.rs:{i} oops"), i)).chain((0..70).map(|i| (format!("codes\tc{i}"), i)));
        let mut n: BTreeMap<String, u64> = rows.collect();
        n.insert("cmd\tread".into(), 5_000_000);
        n.insert("bogus\tx".into(), 1); // a key the summary has no place for: dropped
        crate::cache::jsave(&root.join("telemetry/week/2026-W40.json"), &Week { n, ..Week::default() }).expect("tally");
        let w = weekly_of(&root, "2026-W40");
        let len = |k: &str| w[k].as_object().map(|m| m.len());
        assert_eq!((len("crash"), len("codes"), w["cmd"]["read"].as_u64()), (Some(32), Some(64), Some(1_000_000)));
        assert!(w["crash"].get("cli.rs:39 oops").is_some() && w["crash"].get("cli.rs:0 oops").is_none());
        assert!(w.get("bogus").is_none(), "{w}");
    }

    /// A panic while the tally's lock is held (by the panicking thread, maybe) drops its count, never waits.
    #[test]
    fn a_crash_never_waits_for_the_tally() {
        let root = crate::scratch::Scratch::new("crash-lock");
        crash(&root, "src/cli.rs", 1, "first", 48);
        let week = root.join("telemetry/week").join(format!("{}.json", week_of(now())));
        let lock = std::fs::File::create(week.with_extension("lock")).expect("lock file");
        lock.lock().expect("held");
        crash(&root, "src/cli.rs", 2, "second", 48);
        drop(lock);
        let w: Week = jload(&week).unwrap_or_default();
        assert_eq!(w.n.keys().collect::<Vec<_>>(), ["crash\tcli.rs:1 first"]);
    }
}
