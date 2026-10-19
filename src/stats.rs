//! Self-observability: `fats` status, `fats stats`, `fats config`, `fats account`.

use crate::cache::{Cache, Entry, MINE, Meta, jload, jsave, usage};
use crate::cli::{ASSET_MAX, ASSET_WAIT, FENCE, MAX_BYTES, MAX_BYTES_CEIL, no_doc, path_of};
use crate::lang::{At, Out, bin, ksize, mib, short};
use crate::outline::{GREP_CAP, LIST_CAP, Page, READ_CAP, fmt_age, photos};
use crate::provider::{ACCOUNTS, Account, Api, Tier, account_calls, bill, late, links, spend};
use crate::{fetch, settings};
use std::time::Duration;

/// Provider health window: the last N calls per provider, whole cache.
const HEALTH: usize = 10;

/// A path's permission bits (`0o700`); None = missing, or not a unix system (Windows: ACLs, not mode bits).
fn mode(p: &std::path::Path) -> Option<u32> {
    #[cfg(unix)]
    return std::fs::metadata(p).ok().map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o777);
    #[cfg(not(unix))]
    None
}

/// A key file others can read (mode not 0600): the `!` row with the fix. Never reads the key itself.
fn key_mode(file: &str) -> Option<String> {
    let p = crate::settings::get().key_dir.value.join(file);
    let mode = mode(&p)?;
    (mode & 0o077 != 0)
        .then(|| format!("! key_mode {file} is {mode:o}, readable by others → chmod 600 {}", p.display()))
}

/// Where a provider's key comes from (`env`, `file`, `none`); never its value.
fn key(t: Tier) -> &'static str {
    crate::settings::key(t.spec().env, t.spec().file).map_or("none", |k| k.1)
}

fn zone() -> String {
    settings::zone().map_or_else(|| format!("none → {} setup", bin()), String::from)
}

pub fn status(c: &Cache) -> Out {
    let b = bin();
    let (bytes, pics, metas) = usage(&c.root);
    // a search's record lives in `j<N>/`, a page's in `u<N>/`
    let is_serp = |p: &&std::path::PathBuf| {
        p.parent().and_then(|d| d.file_name()).is_some_and(|n| n.to_string_lossy().starts_with('j'))
    };
    let searches = metas.iter().filter(is_serp).count();
    let pages = metas.len() - searches;
    let mut lines = vec![match c.st.u.last() {
        None => format!("= s:{} no pages · {b} <url> · {b} ./page.html · {b} guide", c.session),
        Some(e) => {
            let open = c.st.u.iter().filter(|e| e.serp && e.dir.is_empty() && c.lookup(&e.url).is_none()).count();
            let n = c.st.u.len();
            let open = if open > 0 { format!(" ({open} search results not fetched)") } else { String::new() };
            format!("= s:{} {n} ids{open} · last u{n} {}", c.session, short(&e.url, 60))
        }
    }];
    let m = mode(&c.root).map_or(String::new(), |m| format!(" (mode {m:o})"));
    let (root, size, jobs, pics) = (c.root.display(), ksize(bytes as usize), c.job_count(), ksize(pics as usize));
    let sizes = format!("{size}B ({pics}B pictures) · {pages} pages · {searches} searches");
    let sizes = if c.root.join(MINE).is_file() { sizes } else { "not swept (dir not created by fats)".into() };
    lines.push(format!("cache {root}{m} · {sizes} · {jobs} jobs"));
    let net = if crate::settings::get().offline.value { "off (FATS_OFFLINE)" } else { "on" };
    lines.push(match (key(Tier::Oxy), key(Tier::Bd)) {
        ("none", "none") => format!("keys none → {b} setup · network {net}"),
        (oxy, bd) => format!("keys oxy:{oxy} bd:{bd} · bd zone {} · network {net}", zone()),
    });
    lines.extend(["oxylabs.key", "brightdata.key"].into_iter().filter_map(key_mode));
    let run = |(n, r, alive): &(usize, crate::cache::Detached, bool)| {
        let age = (crate::cache::now() - r.t).max(0.0) as u64;
        let state = if *alive { format!("pid {}", r.pid) } else { "! abandoned".into() };
        // alive with no pid yet: it has not taken its lock (`cache::Cache::run`)
        format!("u{n} {} {age}s", if *alive && r.pid == 0 { "starting".into() } else { state })
    };
    let runs: Vec<String> = c.runs().iter().filter(|r| r.1.done.is_none()).map(run).collect();
    if !runs.is_empty() {
        lines.push(format!("running {} · read one: {b} u<N>", runs.join(" · ")));
    }
    lines.extend(health(&metas));
    lines.push(account_line(c, crate::cache::now()));
    lines.push(match FENCE.get() {
        Some(_) => "mcp: serving (this process) · local files from the working directory + the cache".into(),
        None => format!("mcp: ready · {b} mcp = stdio server, one tool fats(cmd)"),
    });
    let at = c.st.cursor.as_ref().filter(|k| k.u > 0);
    let at = at.map(|k| format!("at u{}{} · ", k.u, k.k.map(|s| format!("#{s}")).unwrap_or_default()));
    let n = c.st.u.len();
    let next = if n > 0 { format!("outline: {b} u{n} · ") } else { String::new() };
    lines.push(format!("{}{next}stats: {b} stats · config: {b} config", at.unwrap_or_default()));
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
            let none = if fetch::keyed().contains(&t) { "- no calls yet" } else { "no key" };
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

/// One row per search (provider, ms, cost, results) and per fetch: provider, ms, cost, raw bytes → clean ~tok →
/// outline ~tok, loss, cache hit. A search result shows as `search→u7`, not fetched yet or fetched.
pub fn stats(c: &Cache) -> Out {
    let (serps, s, keys, file) = (c.serps(), &c.session, &settings::get().key_dir.value, crate::cache::SPEND_FILE);
    // the ledger's rows: (unix s, [cost, unverified], the record's dir); a ledger fats cannot write is a `!` row
    let row = |l: &str| serde_json::from_str::<(f64, [f64; 2], std::path::PathBuf)>(l).ok();
    let rows: Vec<_> = std::fs::read_to_string(keys.join(file)).unwrap_or_default().lines().filter_map(row).collect();
    // a probe: one a session with nothing in it never writes
    let lost = (serps.len() + c.st.u.len() > 0).then(|| settings::save_in(keys, file, b"", true).err()).flatten();
    // the session's money = its ledger rows written since it began (`State::t`; a wiped root or session restarts
    // the ids, and the rows of the records wiped with it name the same dirs; unknown = none) + a dead run's calls
    // in flight while its run file lives (the sweep writes their row when it deletes `run/`)
    let mine = rows.iter().filter(|r| r.2.starts_with(c.root.join(s)) && c.st.t.is_some_and(|b| r.0 >= b));
    let ids = c.st.u.iter().enumerate().filter(|(_, e)| e.dir != "-"); // a result forgotten unfetched: no row
    let pages: Vec<_> = ids.map(|(i, e)| page_row(c, i + 1, e)).collect();
    let usd = |p: Vec<[f64; 2]>| bill(spend(p.iter().map(|x| x[0])), spend(p.iter().map(|x| x[1])));
    let tot_c = usd(mine.map(|r| r.1).chain(pages.iter().map(|p| p.1)).collect());
    let tot_c = if lost.is_some() { "? (ledger not written)".into() } else { tot_c }; // its rows are missing
    let tot_ms = serps.iter().map(|s| s.1.ms).sum::<u64>() + pages.iter().map(|p| p.2).sum::<u64>();
    let (fetched, n) = (c.st.u.iter().filter(|e| !matches!(e.dir.as_str(), "" | "-")).count(), serps.len());
    let mut lines = vec![format!("= stats s:{s} · {n} searches · {fetched} fetches · {tot_ms}ms spent {tot_c}")];
    lines.extend(serps.iter().map(|(j, m)| search_row(c, *j, m)).chain(pages.into_iter().map(|p| p.0)));
    // every session's spend: pages and searches are swept within the hour, what they cost stays
    let since = |t: f64| usd(rows.iter().filter(|r| r.0 >= t).map(|r| r.1).collect());
    let (day, all, n, at, b) = (since(crate::cache::now() - 86_400.0), since(0.0), rows.len(), keys.join(file), bin());
    lines.push(format!("spend all sessions: 24h {day} · all {all} · {n} paid records · ledger {}", at.display()));
    lines.extend(lost.map(|why| format!("! cache_write ledger not written: {why} → {b} config (FATS_KEY_DIR)")));
    lines.push(match c.st.u.len() {
        0 => format!("next: {b} <url> · {b} ./page.html"),
        n => format!("last page: {b} u{n} · settings: {b} config"),
    });
    Out::new(lines)
}

/// `stats` row of search `j<job>`.
fn search_row(c: &Cache, job: usize, m: &Meta) -> String {
    let n = m.extra.get("hits").and_then(|h| h.as_array()).map_or(0, |h| h.len());
    let n = if m.ok { format!("{n} results") } else { format!("! {}", m.code) };
    let cc = if m.country.is_empty() { String::new() } else { format!("@{}", m.country) };
    let q = m.url.strip_prefix("search:").unwrap_or(&m.url);
    let prov = if m.provider.is_empty() { "-" } else { &m.provider };
    let (ms, usd, q, tried) = (m.ms, crate::provider::bill(m.cost, m.upto()), short(q, 60), m.path(true));
    let kept = kept(Some(c.root.join(&c.session).join(format!("j{job}"))));
    format!("j{job} search {prov}{cc} {ms}ms {usd} {n} \"{q}\" · {tried}{kept}")
}

/// `stats` row of page `u<u>` (a cache hit costs nothing; a swept page's cost is in the total only), and the cost
/// no ledger row holds: a dead run's calls in flight.
fn page_row(c: &Cache, u: usize, e: &Entry) -> (String, [f64; 2], u64) {
    let (pid, empty) = (format!("u{u}"), e.dir.is_empty());
    let id = if e.serp { format!("search→{pid}") } else { pid.clone() };
    // a detached fetch adds its cost here once it ends; THE liveness rule (`Cache::run`, the lock); a dead one = its
    // failed copy (`Detached::cut`), its calls in flight in the row and the total until the sweep stores it
    let run = empty.then(|| c.run(u)).flatten().filter(|r| r.0.done.is_none());
    let dead = run.as_ref().filter(|r| r.0.dead(r.1, Some(e))).map(|r| r.0.cut());
    if empty && dead.is_none() {
        let idle = if c.lookup(&e.url).is_some() { "cached" } else { "not fetched" };
        let what = if run.is_some() { "pending" } else { idle };
        return (format!("{id} {what} {} → {} u{u}", short(&e.url, 60), bin()), [0.0; 2], 0);
    }
    let (meta, doc) = dead.map_or_else(|| c.page(e), |m| (m, Err(crate::cache::NoDoc::Missing)));
    let errs = kept(c.dir(e));
    let (cost, ms) = if e.alias { (0.0, 0) } else { (meta.cost, meta.ms) };
    let cc = if meta.country.is_empty() { String::new() } else { format!("@{}", meta.country) };
    let name = if meta.provider.is_empty() { "swept" } else { meta.provider.as_str() }; // no meta: its dir went
    let prov = if e.alias { "cache".into() } else { format!("{name}{cc}") };
    let head = format!("{id} {prov} {ms}ms ${cost:.4}");
    let pics = c.dir(e).map_or_else(Vec::new, |d| crate::cache::assets(&d));
    let (pic_bytes, pic_ms) = pics.iter().fold((0, 0), |(b, t), a| (b + a.bytes as usize, t + a.ms));
    let line = format!(" · pictures {} plain {}B {pic_ms}ms $0", pics.len(), ksize(pic_bytes));
    let pics = if pics.is_empty() { String::new() } else { line };
    let tail = format!("{}{}{pics}{errs}", meta.path(true), meta.hedge_off());
    let row = match doc {
        Ok(doc) => {
            let charset = if meta.charset.is_empty() { String::new() } else { format!(" {}", meta.charset) };
            let page = Page { id: pid.clone(), eq: pid, doc: &doc, meta: &meta, age: None, at: Default::default() };
            let outline: usize = crate::outline::outline(&page, None).0.iter().map(|l| crate::clean::tok(l)).sum();
            let (raw, clean, shots) = (ksize(meta.bytes as usize), ksize(doc.tokens.clean), photos(&doc, &meta));
            let (mode, l) = (&doc.mode, &doc.loss);
            let sizes = format!("{head} raw {raw}B{charset} → clean {clean}~tok ({mode}) → outline {outline}~tok");
            let pruned = format!("{} of {} numbers pruned", l.vs_light_lost, l.light_facts);
            let signal = doc.info.get("signal").map(|v| format!(" · signal {v}")).unwrap_or_default();
            format!("{sizes}{shots} · {pruned} · price_nums_pruned {}{signal} · {tail}", l.price_pruned)
        }
        // a failed copy = the `!` row its read prints (`cli::no_doc`), what it ran before the move
        Err(why) if !meta.ok => {
            let row = no_doc(&pid, "", &meta, &[], &why, None).lines.split_off(1).join("\n");
            let mid = format!("{} · {tail}", &head[id.len() + 1..]);
            row.split_once(" → ").map_or(format!("{row} · {mid}"), |(b, n)| format!("{b} · {mid} → {n}"))
        }
        Err(_) => format!("{head} {} raw {}B · {tail}", meta.kind, ksize(meta.bytes as usize)),
    };
    (row, [0.0, if empty { meta.upto() } else { 0.0 }], ms)
}

/// Every setting with its source (flag · env · default · build); refused inputs show next to the value used.
pub fn config() -> Out {
    let s = crate::settings::get();
    let mut rows = store_rows(s);
    rows.extend(settings::LIMITS.iter().map(|(k, v)| (*k, v.to_string(), "build · limits.json".into())));
    rows.extend(fetch_rows(s));
    rows.push(instant_row());
    for t in Tier::ALL {
        let sp = t.spec();
        let price = format!("${:.2}/1k", sp.price * 1e3);
        // a provider without a key: its `no_key` row, the one text every command prints
        let k = match key(t) {
            _ if sp.env.is_empty() => String::new(),
            "none" if t == t.owner() => format!(" · key none {}", no_key_rows(sp.short, false).concat()),
            "none" => " · key none".into(),
            k => format!(" · key {k} ({} or {})", sp.env, sp.file),
        };
        rows.push((sp.short, format!("{}s {price}", sp.timeout.as_secs()), format!("build{k}")));
    }
    let b = bin();
    let mut lines = vec![format!("= config · {} settings · source: flag · env · default · build", rows.len())];
    lines.extend(rows.into_iter().map(|(k, v, s)| format!("{k} {v} · {s}")));
    lines.push(format!(
        "env: FATS_SESSION FATS_CACHE FATS_KEY_DIR FATS_JSON FATS_NOCACHE FATS_OFFLINE FATS_ALLOW_PRIVATE FATS_CHUNK FATS_ACCOUNT_TTL FATS_ACCEPT_MD · status: {b}"
    ));
    Out::new(lines)
}

/// A `config` row: (name, value in effect, where it came from).
type Row = (&'static str, String, String);

/// A setting's source, and the value refused for env `k` when one was: `env FATS_JSON`, `default (refused x)`.
fn source(src: &str, k: &str) -> String {
    let refused = crate::settings::get().rejects.iter().find(|r| r.contains(&format!(" {k}="))).map(|r| {
        format!(" (refused {})", r.split(" · ").next().unwrap_or_default().trim_start_matches("! bad_setting "))
    });
    format!("{src}{}", refused.unwrap_or_default())
}

/// `config` rows of the session, the cache and the reader.
fn store_rows(s: &settings::Settings) -> Vec<Row> {
    let p = |v: &std::path::Path| v.display().to_string();
    let swept = if s.cache.value.join(MINE).is_file() { "swept" } else { "not swept (dir not created by fats)" };
    vec![
        ("session", s.session.value.clone(), source(s.session.src, "FATS_SESSION")),
        ("cache", p(&s.cache.value), format!("{} · {swept}", s.cache.src)),
        ("key_dir", p(&s.key_dir.value), s.key_dir.src.into()),
        ("json", s.json.value.to_string(), source(s.json.src, "FATS_JSON")),
        ("nocache", s.nocache.value.to_string(), source(s.nocache.src, "FATS_NOCACHE")),
        ("chunk", format!("{}tok", s.chunk.value), source(s.chunk.src, "FATS_CHUNK")),
        ("match", "words (exact first when unpinned)".into(), "default".into()),
        ("list_cap", format!("{LIST_CAP}tok"), "default".into()),
        ("grep_cap", format!("{GREP_CAP}tok"), "default".into()),
        ("read_cap", format!("{READ_CAP}tok"), "default".into()),
        ("max_bytes", format!("{} (ceiling {})", mib(MAX_BYTES), mib(MAX_BYTES_CEIL)), "default · max_bytes:".into()),
        tables(),
        ("network", if s.offline.value { "off" } else { "on" }.into(), s.offline.src.into()),
        ("allow_private", s.allow_private.value.to_string(), source(s.allow_private.src, "FATS_ALLOW_PRIVATE")),
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
        "text, json, xml bodies (by content type or start) served as they are, l<N> per line, {lines} lines per \
         section · never a provider, never a route"
    );
    let md = format!(
        "{} · plain sends Accept: {} · a text/markdown answer is cleaned as a page (md) unless thin or flat vs the \
         stored copy · a JS shell or 4xx with the page's own og card is served as the card (og)",
        if s.accept_md.value { "on" } else { "off" },
        crate::provider::MD_ACCEPT
    );
    let (max, wait) = (ksize(ASSET_MAX as usize), ASSET_WAIT.as_secs());
    let pictures =
        format!("{b} uN mK: plain only, image/* at most {max}B, {wait}s, $0 · <page>/assets/ · mcp attaches it");
    let ttl = format!("{}m · {b} account re-reads after it; status reads {ACCOUNT_FILE} only", s.account_ttl.value);
    let ladder = path_of(&fetch::route(&fetch::Plan::new("", fetch::Mode::Auto, false, fetch::COST_CAP, None)).0);
    vec![
        // the page ladder the keys present run, a one-key retry included (`fetch::route`, as `dry:true` prints it)
        ("provider", format!("auto: {} · oxy no-JS also: files", ladder), "default · provider:plain|bd|oxy".into()),
        ("country", country, source(s.country.src, "FATS_COUNTRY") + " · country:"),
        ("search", search, "default · provider:oxy|bd limit: country: site: since: timeout: cost:".into()),
        ("device", "desktop".into(), "default · device:mobile (oxy only)".into()),
        ("hedge", fetch::hedges(None) + " (both billed when both run)", "hedge:N sets both · hedge:0 races".into()),
        ("pdf", pdf, "build · timeout: sets oxy's too".into()),
        ("text", text, "build".into()),
        ("markdown", md, source(s.accept_md.src, "FATS_ACCEPT_MD")),
        ("timeout", "per tier below".into(), "build · timeout:1..300 caps plain, then one wall for paid calls".into()),
        ("cost_cap", format!("${}", fetch::COST_CAP), "default · cost:".into()),
        ("gov_labels", fetch::SITES.gov.join(" "), "build (skip bd): last label, or 2nd under a cc".into()),
        ("bd_zone", zone(), s.bd_zone.as_ref().map_or("unset", |z| z.src).into()),
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
    let (poll, gh) = (POLL.as_millis(), settings::GH_WAIT.as_secs());
    let (dns, doh, doh_wait) =
        (fetch::DNS_WAIT.as_secs_f32(), Api::Doh.row().url, crate::provider::DOH_WAIT.as_secs_f32());
    let instant = format!(
        "wait:{WAIT}s then a fetch or a search slower than wait: (bd) is pending, a gh/account/setup call is cut (provider_timeout wait:) · poll {poll}ms · a run lives while it holds its lock, else abandoned · prefetch fetch:{PREFETCH} \
         · gh auth token {gh}s · dns {dns}s, on no address a third party: {doh} ({doh_wait}s, the host name only)"
    );
    ("instant", instant, "default · wait:0..300 fetch:N".into())
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
    let key = |t: Tier| settings::key(t.spec().env, t.spec().file).map(|k| (t, k.0));
    let keys: Vec<(Tier, String)> = ACCOUNTS.iter().filter_map(|&t| key(t)).collect();
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
    let env = Tier::parse(p).map_or("", |t| t.spec().env);
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
        "bad_key" => vec![bang("bad_key", &format!("replace {env}"))],
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
                return Out::fail("cache_write", format!("= ! cache_write {e}"), format!("{b} config (FATS_CACHE)"));
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

/// `fats setup`: each provider's key from the first place that has it (`settings::find`), checked with its
/// cheapest read (Oxylabs: the scraper list; Bright Data: the zone list, names and types only, never an endpoint
/// that answers with a zone password), saved in the key dir (0600) unless it came from there. Bright Data's Web
/// Unlocker zone: `zone:`, else the configured one, else the account's only one. Never prints a key; `dry:true`
/// makes no call and writes nothing. The checks run side by side, each given up at `wait`.
pub fn setup(zone: Option<&str>, dry: bool, wait: Duration) -> Out {
    let b = bin();
    let (mut heads, mut lines, mut ok) = (Vec::new(), Vec::new(), true);
    let api = |t: Tier| if t == Tier::Bd { Api::BdZones } else { Api::OxyScrapers };
    let found: Vec<_> = ACCOUNTS.iter().map(|t| settings::find(t.spec().env, t.spec().file, &[])).collect();
    let keyed = ACCOUNTS.iter().zip(&found).filter_map(|(t, k)| Some((api(*t), k.as_ref()?.0.clone())));
    let calls = if dry { Vec::new() } else { keyed.map(|(to, key)| crate::provider::read_json(to, key)).collect() };
    let mut reads = crate::provider::all_within(calls, wait).into_iter();
    for (t, found) in ACCOUNTS.into_iter().zip(found) {
        let s = t.spec();
        let (head, rows) = match found {
            None => ("none".into(), no_key_rows(s.short, true)),
            Some((_, at)) if dry => {
                let row = api(t).row();
                let what = format!("{} key from {at} · would read {} {} · $0", s.short, row.method, row.url);
                ("not checked".into(), vec![what])
            }
            Some((key, at)) => match reads.next().flatten().unwrap_or_else(|| Err(late(wait))) {
                Ok(j) => keep(t, &key, at, &j, zone),
                Err(f) => (format!("! {}", f.code), account_rows(&failed(t, f), None)),
            },
        };
        ok &= !head.starts_with('!');
        heads.push(format!("{} {head}", s.short));
        lines.extend(rows);
    }
    let dir = settings::get().key_dir.value.display().to_string();
    let tail = if dry { " · spends nothing now".to_string() } else { format!(" · key dir {dir}") };
    lines.insert(0, format!("={} setup {}{tail} · $0", if dry { " dry" } else { "" }, heads.join(" · ")));
    lines.push(format!("status: {b} · balance: {b} account"));
    let mut out = Out { rc: i32::from(!ok), ..Out::new(lines) }; // a failed check = rc 1
    out.set("cost", 0.0).set("dry", dry);
    out
}

/// A key that passed its check: the head and row, with the zone for Bright Data; saved unless it came from the key dir.
fn keep(t: Tier, key: &str, at: &str, j: &serde_json::Value, zone: Option<&str>) -> (String, Vec<String>) {
    let s = t.spec();
    let mut files = vec![(s.file, key.to_string())];
    let what = match t {
        Tier::Bd => match pick_zone(j, zone) {
            Ok(z) => {
                files.push((settings::ZONE_FILE, z.clone()));
                format!("zone {z}")
            }
            Err(row) => return ("! no_zone".into(), vec![row]),
        },
        _ => format!("{} scrapers", j["scrapers"].as_array().map_or(0, Vec::len)),
    };
    files.retain(|(f, _)| at != "file" || *f != s.file);
    let dir = &settings::get().key_dir.value;
    if let Some(e) = files.iter().find_map(|(f, v)| settings::save_in(dir, f, v.as_bytes(), false).err()) {
        let fix = format!("{} config (FATS_KEY_DIR)", bin());
        return ("! cache_write".into(), vec![format!("{} ! cache_write {e} → {fix}", s.short)]);
    }
    let saved: Vec<&str> = files.iter().map(|f| f.0).collect();
    let saved = if saved.is_empty() { "kept".into() } else { format!("saved {} (600)", saved.join(" ")) };
    (format!("ok {what}"), vec![format!("{} ok {what} · key from {at} · {saved}", s.short)])
}

/// Bright Data's Web Unlocker zone from its zone list: `zone:`, else the configured one, else the only one.
/// Err = the `! no_zone` row naming the account's unblocker zones.
fn pick_zone(j: &serde_json::Value, arg: Option<&str>) -> Result<String, String> {
    let zones = j.as_array().into_iter().flatten().filter(|z| z["type"] == "unblocker");
    let names: Vec<&str> = zones.filter_map(|z| z["name"].as_str()).collect();
    let saved = || settings::find("BRIGHTDATA_UNLOCKER_ZONE", settings::ZONE_FILE, &[]).map(|z| z.0);
    let want = arg.map(String::from).or_else(saved);
    match (want, names.as_slice()) {
        (Some(w), _) if names.contains(&w.as_str()) => Ok(w),
        (None, [one]) => Ok(one.to_string()),
        (want, _) => {
            let what = want.map_or("pick one".into(), |w| format!("{w} is not a Web Unlocker zone here"));
            let list = if names.is_empty() { "none (add one in the control panel)".into() } else { names.join(", ") };
            let at = At { id: "setup", arg: names.first().copied().unwrap_or("<name>"), ..Default::default() };
            Err(crate::lang::bang("bd", "no_zone", &format!("{what} · unblocker zones: {list}"), &at))
        }
    }
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
        let what = api.row().what.replace("{from}", &from).replace("{to}", &to);
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let dir = std::env::temp_dir().join(format!("fats-health-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let meta = serde_json::json!({"t": 1.0, "hops": [
            {"provider": "plain", "code": "js_shell", "ms": 300, "cost": 0.0},
            {"provider": "oxylabs", "code": "provider_error", "ms": 2000, "cost": 0.0},
            {"provider": "oxylabs-js", "code": "ok", "ms": 9000, "cost": 0.0015},
            {"provider": "brightdata", "code": "abandoned", "ms": 9000, "cost": 0.0, "upto": 0.0015},
        ]});
        let p = dir.join("meta.json");
        std::fs::write(&p, meta.to_string()).expect("write meta");
        let rows = health(&[p]);
        std::fs::remove_dir_all(&dir).ok();
        let want = [
            "plain 0/1 300ms ✗",
            "bd 0/1 - – · 1 cancelled, billed $0.0000 + ≤$0.0015 unverified",
            "oxy 0/1 2000ms ✗",
            "oxyjs 1/1 9000ms ✓",
        ];
        assert_eq!(rows[1..], want);
    }

    /// A run left at `wait:3`, then `kill -9`: `stats` reads THE liveness rule (`Cache::run`) as `fats u1`
    /// does: `pending` while the run's lock is held, `! abandoned` once it is gone, never `pending` for a dead run.
    #[test]
    fn stats_reads_the_run_lock() {
        let root = std::env::temp_dir().join(format!("fats-stats-lock-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let c = Cache::at(root.clone(), "t".into());
        let lock = c.run_claim(1).expect("the launcher's lock");
        let status = "plain:js_shell✗ · bd running".into();
        let run = crate::cache::Detached { url: "https://ex.com/a".into(), job: 1, status, ..Default::default() };
        c.run_put(1, &run).expect("run file");
        let e = Entry { url: run.url.clone(), ..Default::default() };
        let row = || page_row(&c, 1, &e);
        assert!(row().0.starts_with("u1 pending "), "{}", row().0);
        drop(lock);
        assert!(row().0.starts_with("u1 ! abandoned "), "{}", row().0);
        // Its call in flight is a call we cut, in the row and the total; plain's is free
        let (row, cost, _) = row();
        assert!(row.contains(" · bd:abandoned✗≤$0.0015 (unverified) → ") && cost == [0.0, 0.0015], "{row}");
        std::fs::remove_dir_all(&root).ok();
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

    /// `!` rows: built like every other (`lang::bang` + `FIX`), then the provider's own page when its docs give one.
    #[test]
    fn account_bang_rows_link() {
        let b = bin();
        let row = |p: &str, code: &str| Account { provider: p.into(), code: code.into(), ..Account::default() };
        // The fetch row's text (buys, price, every variable), `setup` never pointing at itself
        assert_eq!(
            account_rows(&row("oxy", "no_key"), None),
            [format!(
                "! no_key oxy · would try no-JS fetch + JS render ≤$0.0025 · set OXYLABS_WEB_API_KEY → {b} setup · oxy https://dashboard.oxylabs.io/en/registration"
            )]
        );
        let bd = "! no_key bd · would try unblock+JS ≤$0.0015 · set BRIGHTDATA_API_KEY + BRIGHTDATA_UNLOCKER_ZONE";
        let page = " · bd https://brightdata.com/cp/start";
        assert_eq!(account_rows(&row("bd", "no_key"), None), [format!("{bd} → {b} setup{page}")]);
        assert_eq!(no_key_rows("bd", true), [format!("{bd}{page}")]);
        assert_eq!(
            account_rows(&row("oxy", "bad_key"), None),
            [format!(
                "oxy ! bad_key replace OXYLABS_WEB_API_KEY → {b} setup · oxy https://dashboard.oxylabs.io (a Web API instance's key)"
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

    /// A scoped key's missing reads are named by endpoint, never by the wording of its `what` (review 2026-10-14
    /// #15): every Bright Data read has its word, and each word lands in its own place.
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
}
