//! CLI: command → action → Out. Pages come from the cache, local files or the fetch router (fetch.rs);
//! `FATS_OFFLINE=1` keeps every command off the network.

use crate::cache::{Asset, Cache, Cursor, Detached, Done, Entry, HopRow, Meta, NoDoc, Snap, State, jload, now};
use crate::clean::{Doc, host_of};
use crate::fetch::{self, Hop, Plan};
use crate::lang::{At, CARD, Cmd, Flag, Id, Match, Out, WORDS, bang, bin, emit, fix, ksize, mib, parse, short};
use crate::outline::{self, Page, Shown};
use crate::provider::{self, Ask, Hit, Tier};
use crate::{search, settings, stats};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Input cap per page, raised per command with `max_bytes:` up to the ceiling. Cleaning costs ~15× the input in
/// memory (a 16 MB page peaked at 239 MB, a 53 MB one at 743 MB), so the default keeps a page near 250 MB
/// (`fats config` shows both).
pub const MAX_BYTES: u64 = 16 << 20;
pub const MAX_BYTES_CEIL: u64 = 64 << 20;

/// The command's input cap (`max_bytes:`, validated by the parser).
fn cap_of(cmd: &Cmd) -> u64 {
    cmd.get("max_bytes").and_then(|v| v.parse().ok()).unwrap_or(MAX_BYTES)
}

/// Directories local files may come from. Unset in the shell (it reads what its user can); `fats mcp` sets
/// the working directory + the cache, because there the model picks the path.
pub static FENCE: OnceLock<Vec<PathBuf>> = OnceLock::new();

pub fn fence(dirs: Vec<PathBuf>) {
    FENCE.set(dirs).ok(); // the first fence holds
}

/// Why the fence refuses this local read, if it does. A missing file falls through to `not_found`.
fn fenced(f: &str, path: &Path) -> Option<&'static str> {
    let dirs = FENCE.get()?;
    if f == "-" {
        return Some("stdin carries the MCP protocol");
    }
    let p = path.canonicalize().ok()?; // symlinks resolved: ./link → /etc/hosts is outside
    let inside = dirs.iter().filter_map(|d| d.canonicalize().ok()).any(|d| p.starts_with(d));
    (!inside).then_some("outside the working directory and the cache")
}

/// Run one invocation: (stdout text, exit code, the picture it asked for: `fats mcp` attaches it). Never panics
/// on user input.
pub fn run(argv: &[String]) -> (String, i32, Option<String>) {
    let cmd = parse(argv);
    let set = settings::get();
    let json = cmd.has("json") || cmd.flag("json") || set.json.value;
    let mut c = Cache::open();
    let mut out = if let Some(q) = &cmd.ask {
        Out::ask(q.clone())
    } else if cmd.flag("dry") {
        plan(&cmd, &c)
    } else {
        let t0 = Instant::now();
        let mut o = dispatch(&cmd, &mut c);
        o.rec.entry("ms").or_insert((t0.elapsed().as_millis() as u64).into());
        if let Err(e) = c.save() {
            o = cache_write(e);
        }
        o
    };
    if let Some(m) = cmd.count("max") {
        out.cap(m, &cmd.with_mod("max", Some(&(m * 2).to_string())));
    }
    if let Some(i) = cmd.ids.first() {
        out.rec.entry("id").or_insert(i.page().into());
    }
    if let Some(u) = cmd.urls.first().or(cmd.files.first()) {
        out.rec.entry("url").or_insert(u.as_str().into());
    }
    let mut rows = set.rejects.clone();
    rows.extend(ignored(&cmd));
    out.warn(rows);
    let image = out.rec.get("image").and_then(|v| v.as_str()).map(String::from);
    let tail = footer(&c, &out.lines, !json && FENCE.get().is_none());
    (emit(&out, json) + &tail, out.rc, image)
}

/// The CLI's background line (`show`: never MCP, never `json`): `~ u7 done · u9 pending 12s`, this session's
/// detached runs not named on the answer's `=` lines (a batch's every input); a finished one once
/// (`Detached::told`: shown, or named).
fn footer(c: &Cache, lines: &[String], show: bool) -> String {
    let named = |n| lines.iter().any(|h| h.starts_with('=') && h.split([' ', '.']).any(|w| w == format!("u{n}")));
    let (mut over, mut on) = (Vec::new(), Vec::new());
    for (n, r, alive) in c.runs().into_iter().filter(|r| !r.1.told) {
        let id = if is_prefetch(&r) { format!("prefetch u{n}") } else { format!("u{n}") };
        if alive && r.done.is_none() {
            on.extend((!named(n)).then_some((id, n, r)));
            continue;
        }
        // a failed run says why (its code) and what it cost: its own bill (`provider::charged`), the calls it cut
        // from its stored copy (`upto`), as `stats` prints them
        let m: Meta = c.st.u.get(n - 1).and_then(|e| jload(&c.dir(e)?.join("meta.json"))).unwrap_or_default();
        let bill =
            |cost: f64| if cost + m.upto() > 0.0 { provider::bill(cost, m.upto()) } else { "not charged".into() };
        let fail = |d: Done| format!("failed · {} · {}", d.code, bill(d.cost));
        let end = r.done.map_or("! abandoned".into(), |d| if d.ok { "done".into() } else { fail(d) });
        over.extend((!named(n)).then(|| format!("{id} {end}")));
        if show || named(n) {
            c.run_tell(n);
        }
    }
    // `prefetch u1, u11 pending 31s · next: …`: one word for a search's runs, after the others; the first run's age
    // and next move (`ready`): a bare read of a pending id answers at once, `wait:` waits
    on.sort_by_key(|o| o.0.starts_with("prefetch"));
    let ids = on.iter().map(|o| o.0.as_str()).collect::<Vec<_>>().join(", ").replace(", prefetch u", ", u");
    over.extend(
        on.first().map(|(_, n, r)| format!("{ids} pending {:.0}s · next: {}", now() - r.t, ready(c, *n, r, &[]))),
    );
    if over.is_empty() || !show { String::new() } else { format!("~ {}\n", over.join(" · ")) }
}

/// Every accepted mod is honoured or named here: a `!` row, never silently dropped. Honoured: match, dry, json,
/// fresh, limit, max, cost, provider, js, country, timeout, wait; site and since by a web search; from by a list.
/// `more` beside a list (the old hint `u1 links more`) is named too: lists page by `from:K`.
fn ignored(cmd: &Cmd) -> Vec<String> {
    let search = is_search(cmd);
    let lists = WORDS.lists.iter().copied().chain(["media", "comments"]);
    let (list, at) = (lists.into_iter().find(|w| cmd.has(w)), cmd.ids.first().map(|i| i.page() + " "));
    let fix = |w| format!("! ignored more · lists page by from:K → {} {}{w} from:K", bin(), at.unwrap_or_default());
    let more = list.filter(|_| cmd.has("more")).map(fix);
    cmd.mods
        .iter()
        .filter_map(|(k, v)| {
            let (code, why) = match k.as_str() {
                "verbose" if v == "true" => ("not_in_v1", "no diagnostics in this build"),
                "fetch" if !search => ("ignored", "only a web search prefetches"),
                "site" | "since" if !search || is_gh(cmd) => ("ignored", "only a web search uses it"),
                "country" if is_gh(cmd) => ("ignored", "only a web search or a page fetch uses it"),
                "zone" if !cmd.has("setup") => ("ignored", "only setup uses it"),
                "from" if list.is_none() => ("ignored", "only a links, cards, media or comments list pages"),
                _ => return None,
            };
            // the arrow only to a command that does something without the mod (`fats u1`, never a bare `fats open`)
            let rest = Some(cmd.with_mod(k, None)).filter(|r| !parse(std::slice::from_ref(r)).idle);
            Some(format!("! {code} {k}:{v} · {why}{}", rest.map_or(String::new(), |r| format!(" → {} {r}", bin()))))
        })
        .chain(more)
        .collect()
}

/// Free words (or `site:` alone) without a page id, a local file or a page URL: a web search (`fats <words>`,
/// `fats <host> <words>`, `fats site:x.com/news since:1d`).
fn is_search(cmd: &Cmd) -> bool {
    let any = !cmd.words.is_empty() || cmd.get("site").is_some();
    any && cmd.ids.is_empty() && cmd.files.is_empty() && page_url(cmd).is_none()
}

/// `fats gh <words>`: a GitHub repository search (`github`), not a web search.
pub fn is_gh(cmd: &Cmd) -> bool {
    cmd.words.len() > 1 && cmd.words[0] == "gh" && cmd.urls.is_empty()
}

/// The one URL of the command when it names a page (a path past the host, or a query); a bare host is a site.
fn page_url(cmd: &Cmd) -> Option<&str> {
    let page = |u: &&String| url::Url::parse(u).is_ok_and(|p| p.path() != "/" || p.query().is_some());
    cmd.urls.first().filter(page).filter(|_| cmd.urls.len() == 1).map(String::as_str)
}

/// One search as the command asks for it.
struct Search {
    /// The query sent: `site:` first, the `since:` hint last.
    q: String,
    /// The host of `site:`: results off it are dropped (the engine may ignore `site:` on common words).
    site: Option<String>,
    limit: usize,
    country: Option<String>,
    /// `fats gh <words>`: GitHub, not the web (`q` = the words after `gh`, up to `GH_MAX` results).
    gh: bool,
    /// The router's plan: oxy, bd beside it after the hedge; `provider:` `cost:` `timeout:` `hedge:`.
    plan: Plan,
}

/// The command → a search, or the `?` line. `site:` (or the host of `fats <host> <words>`) goes in front of the
/// words; `since:` becomes `after:YYYY-MM-DD` at the end: neither provider has a date parameter (Oxylabs
/// `/v1/search` takes query, max_results, location only), so it is a hint to the engine, not a filter.
fn search_of(cmd: &Cmd) -> Result<Search, Out> {
    let b = bin();
    if let Some(l) = cmd.links.first() {
        return Err(Out::ask(format!("? l{l} needs a page: {b} u1 l{l} · search: {b} {}", cmd.words.join(" "))));
    }
    if cmd.urls.len() > 1 {
        let (site, words) = (short(&cmd.urls[0], 60), cmd.words.join(" "));
        return Err(Out::ask(format!("? one site per search: {b} {site} {words}")));
    }
    let gh = is_gh(cmd);
    let site =
        cmd.get("site").map(String::from).or(cmd.urls.first().map(|u| short(u, 2048).trim_end_matches('/').into()));
    let site = site.filter(|_| !gh);
    let after = cmd.get("since").filter(|_| !gh).and_then(|v| crate::lang::since_date(v, (now() / 86_400.0) as i64));
    let words = cmd.words[usize::from(gh)..].iter().cloned();
    let q = site.iter().map(|s| format!("site:{s}")).chain(words).chain(after.map(|d| format!("after:{d}")));
    let q = q.collect::<Vec<_>>().join(" ");
    // the redactor masks what is printed; a key in the words would still be sent to the provider
    if crate::lang::redact(&q, &settings::secrets()) != q {
        let what = "search · the words hold a configured key · not sent · $0";
        return Err(refused("key_in_query", "", what, &At::default()));
    }
    if q.chars().count() > 2048 {
        return Err(Out::ask(format!("? query is {} chars · a search takes 2048 at most", q.chars().count())));
    }
    if let Some(p) = cmd.get("provider").filter(|p| !gh && !matches!(*p, "oxy" | "bd" | "auto")) {
        return Err(Out::ask(format!("? provider:{p} cannot search · provider:oxy|bd|auto")));
    }
    let max = if gh { settings::limit("gh_max") } else { provider::SEARCH_MAX };
    let (p, asked) = (plan_of(cmd, None, 0.0), wait_of(cmd));
    // a lead slower than `wait:` runs detached (`searching`): no `wait:`, one call (`one_call`), so `dry` routes that
    let wait = Some(p.search_wait(asked)).filter(|w| *w <= asked);
    Ok(Search {
        q,
        site: site.map(|s| s.split('/').next().unwrap_or_default().to_ascii_lowercase()),
        limit: cmd.count("limit").unwrap_or(10).min(max),
        country: cmd.get("country").filter(|_| !gh).map(str::to_uppercase),
        gh,
        plan: Plan { wait, ..p },
    })
}

/// `fats <words>`: one web search. Each result becomes an id holding its URL and no body; `fats u7` fetches it
/// through the tiers into the same id. Oxylabs first, Bright Data when Oxylabs has no key or fails. The search
/// record (provider, ms, cost, hits) is stored as `j<N>` for `stats`, health and `more`.
fn web(cmd: &Cmd, c: &mut Cache) -> Result<Out, Out> {
    let s = search_of(cmd)?;
    let what = format!("search \"{}\"", s.q);
    if settings::get().offline.value {
        return Ok(offline(&what));
    }
    let t0 = Instant::now();
    if s.gh {
        return github(cmd, c, &s, t0);
    }
    // a search past `wait:` detaches; the detached run (`resume` named its job) is that one call, run to its end
    if s.plan.wait.is_none() && c.job.is_none() {
        return searching(c, cmd);
    }
    let mut run = fetch::run_search(&s.plan, &s.q, s.limit, s.country.as_deref());
    let mut found = run.iter_mut().find(|h| h.ok).map(|h| (h.tier, std::mem::take(&mut h.hits)));
    let mut rows = match (&mut found, &s.site) {
        (None, _) => fetch::skipped(&s.plan, &run, "", &cmd.canon(), ""),
        (Some((_, hits)), Some(host)) => on_site(hits, host),
        _ => Vec::new(),
    };
    // no arrow: the rows shown are all a search returns; `limit:20` would only buy them again
    if s.limit < cmd.count("limit").unwrap_or(0) {
        rows.push(format!("! ignored limit · a search returns {} at most", s.limit));
    }
    if run.is_empty() {
        return Ok(nothing_searched(&s, &what, rows));
    }
    let job = c.job_of(&cmd.raw).map_err(cache_write)?;
    let served = found.as_ref().map_or("", |f| f.0.spec().name);
    let meta = search_meta(&s, run.iter().map(Hop::row).collect(), served, t0);
    let o = match found {
        Some((_, hits)) => listed(c, job, meta, &hits)?,
        // every provider failed: the record is kept (`j<N>`, the error answers in `error.txt`), then its row
        None => c
            .serp_put(job, &meta, fetch::error_text(&run).as_deref())
            .map_err(cache_write)
            .and_then(|()| search_out(c, cmd, job, &meta))?,
    };
    let mut o = prefetched(c, cmd, job, o);
    o.warn(rows);
    Ok(o)
}

/// A detached search's patience (`resume`): twice the search timeout (one call at the retry's time).
const PATIENCE: Duration = provider::SEARCH_TIMEOUT.saturating_mul(2);

/// A search whose lead provider answers slower than `wait:` (`Plan::search_wait`, bd) runs detached like a fetch
/// : `fats --continue j<J>` searches with that patience and keeps the record `j<J>`; this call answers by
/// `wait:` (from the command's start, `Cache::born`, as `wait_for`) with the list, the failure, or `pending` →
/// `fats j<J>` (the list once stored, $0 more).
fn searching(c: &mut Cache, cmd: &Cmd) -> Result<Out, Out> {
    let job = c.job_of(&cmd.raw).map_err(cache_write)?;
    detach(format!("j{job}"), &c.root.join(&c.session).join(format!("run/j{job}.log")), None).map_err(cache_write)?;
    poll_until(c.born, wait_of(cmd), || c.serp(job).is_some());
    match c.serp(job) {
        Some(m) if m.ok => Ok(serp_list(c, job, 0, true)),
        Some(m) => search_out(c, cmd, job, &m),
        None => Ok(search_pending(c, job).unwrap_or_default()),
    }
}

/// A search job not stored yet and younger than its detached patience (`resume`): `pending` → `fats j<J> wait:30`.
fn search_pending(c: &Cache, job: usize) -> Option<Out> {
    let j = c.job(job)?;
    let q = Some(parse(std::slice::from_ref(&j.cmd))).filter(is_search).and_then(|cmd| search_of(&cmd).ok())?.q;
    let age = (now() - j.t).max(0.0);
    let row = format!("= j{job} pending {age:.0}s search \"{q}\" → {}", poll_next(&format!("j{job}"), 0));
    (c.serp(job).is_none() && age < PATIENCE.as_secs_f64()).then(|| waiting(row, format!("j{job}")))
}

/// `fats gh <words>`: GitHub repositories by stars (`provider::gh_repos`), listed and opened like web results
/// (each through the ladder), prefetched the same way. Free; the record `j<N>` holds a `gh` hop a call.
fn github(cmd: &Cmd, c: &mut Cache, s: &Search, t0: Instant) -> Result<Out, Out> {
    let wait = wait_of(cmd);
    let got = provider::all_within(vec![provider::gh_repos(s.q.clone(), s.limit)], wait).pop().flatten();
    let got = got.unwrap_or_else(|| Err(provider::late(wait)));
    let job = c.job_of(&cmd.raw).map_err(cache_write)?;
    // one hop a call: the search, and the account call beside it, so `stats` counts both
    let calls = got.as_ref().map_or_else(|f| vec![(f.code, f.why.clone())], |(_, calls)| calls.clone());
    let gh = HopRow { provider: "gh".into(), ms: t0.elapsed().as_millis() as u64, ..Default::default() };
    let hops = calls.into_iter().map(|(code, why)| HopRow { code: code.into(), why, ..gh.clone() });
    let meta = search_meta(s, hops.collect(), if got.is_ok() { "github" } else { "" }, t0);
    let fail = match got {
        Ok((hits, _)) => return listed(c, job, meta, &hits).map(|o| prefetched(c, cmd, job, o)),
        Err(fail) => fail,
    };
    c.serp_put(job, &meta, Some(&fail.body)).map_err(cache_write)?;
    search_out(c, cmd, job, &meta).and_then(Err) // one answer, now and on `fats jN`
}

/// `fetch:N` (default 1): search `job`'s top N results not fetched yet start fetching detached into their own ids,
/// so `fats u1` a moment later is usually a cache hit. Free: plain only (`Plan::prefetch`), so `cost:` never
/// stops one. A `prefetching u1 u2 free, plain only` row above the list's last line names the ids started.
fn prefetched(c: &mut Cache, cmd: &Cmd, job: usize, mut o: Out) -> Out {
    let open =
        |(i, e): (usize, &Entry)| (e.job == Some(job) && e.serp && e.dir.is_empty()).then(|| (i + 1, e.url.clone()));
    let top: Vec<(usize, String)> = c.st.u.iter().enumerate().filter_map(open).take(prefetch_n(cmd)).collect();
    let mut queued = Vec::new();
    for (n, url) in &top {
        // a stored page opens on demand (no alias id); a running one, a private host, offline: `at_once` says no
        let want = Want { url, slot: Some(*n), ..Want::default() };
        let free = c.lookup(url).is_none() && at_once(c, want, cmd).is_none();
        queued.extend(free.then(|| reserve(c, Entry::default(), want, cmd).ok()).flatten().map(|n| (n, want, 0.0)));
    }
    // THE placement a batch's runs get: `fetch:20` starts 4 processes, not 20
    let ids: Vec<String> = queued.iter().map(|q| format!("u{}", q.0)).collect();
    if !ids.is_empty() && launch(c, &queued, cmd).is_ok() {
        let row =
            format!("prefetching {} free, plain only (detached; fetch:0 = none) · status: {}", ids.join(" "), bin());
        o.lines.insert(o.lines.len() - 1, row);
        o.set("prefetch", ids);
    }
    o
}

/// How many results a search fetches detached (`fetch:N`).
pub const PREFETCH: usize = 1;

fn prefetch_n(cmd: &Cmd) -> usize {
    cmd.get("fetch").and_then(|v| v.parse().ok()).unwrap_or(PREFETCH)
}

/// Drop the hits off `host` (not it, not under it): Google may ignore `site:` when the words are common (5 of 10
/// off-site, live 2026-10-13). The `! dropped` row, or none.
fn on_site(hits: &mut Vec<Hit>, host: &str) -> Vec<String> {
    let n = hits.len();
    // `.docs.ex.com` ends with `.ex.com`: the host or a subdomain of it
    hits.retain(|h| format!(".{}", host_of(&h.url)).ends_with(&format!(".{host}")));
    let gone = n - hits.len();
    (gone > 0).then(|| format!("! dropped {gone} off-site · not on {host}")).into_iter().collect()
}

/// A detached run a search's `fetch:N` started (its command is the search), not a page command.
fn is_prefetch(r: &Detached) -> bool {
    is_search(&parse(std::slice::from_ref(&r.cmd)))
}

/// No search provider could run (no key, or the cost cap): the first skipped row is the cause; none (cannot
/// happen today) = the code's own fix.
fn nothing_searched(s: &Search, what: &str, mut rows: Vec<String>) -> Out {
    let code = if s.plan.keys.is_empty() { "no_key" } else { "budget_hit" };
    let eq = format!("= ! {code} {what} · no search provider can run · 0ms spent $0");
    let first = if rows.is_empty() { bang("", code, "", &At::default()) } else { rows.remove(0) };
    let mut o = Out::fail(code, eq, first);
    o.warn(rows);
    o
}

/// A search's record (`j<N>/meta.json`): who served it (`served`, "" = every provider failed), its hops, their
/// cost. A web search's hops are its tiers' (`Hop::row`); `fats gh` has a `gh` hop a call (search, account).
fn search_meta(s: &Search, hops: Vec<HopRow>, served: &str, t0: Instant) -> Meta {
    Meta {
        url: format!("search:{}", s.q),
        provider: served.into(),
        ms: t0.elapsed().as_millis() as u64,
        cost: provider::spend(hops.iter().map(|h| h.cost)),
        ok: !served.is_empty(),
        code: hops.last().map(|h| h.code.clone()).unwrap_or_default(),
        t: now(),
        kind: "serp".into(),
        hops,
        country: s.country.clone().unwrap_or_default(),
        ..Default::default()
    }
}

/// A failed search's answer from its record.
fn search_out(c: &Cache, cmd: &Cmd, job: usize, m: &Meta) -> Result<Out, Out> {
    let q = m.url.strip_prefix("search:").unwrap_or(&m.url);
    let detail = format!("· tried {} · {}ms spent {}", m.tried(), m.ms, provider::bill(m.cost, m.upto()));
    // a slow provider gets more time (cut at `wait:`, or past its own timeout), never a time its call already had
    // (a detached bd search ran 30 s, `one_call`); `fresh:true` changes nothing
    let last = m.hops.last();
    let arg = match m.code.as_str() {
        "provider_timeout" if last.is_some_and(|h| h.why.starts_with("wait:")) => "wait:30",
        "provider_timeout" if last.is_some_and(|h| h.ms < 30_000) => "timeout:30",
        _ => "",
    };
    let (id, done) = (cmd.canon(), fetch::spent(m, &[], &fetch::keyed()));
    let at = At { id: &id, arg, done: &done, ..At::default() };
    let (who, owners) = fetch::fail_detail(m, "", &[]);
    // The same failure on this query again = a count, never the call that failed twice
    let n = fetch::fail_count(m, c.serps().iter().filter(|s| s.0 != job).map(|s| &s.1), Some(arg));
    let next = match n {
        Some(_) => String::new(),
        None if is_gh(cmd) && m.code == "bad_key" => "gh auth login, or unset GH_TOKEN".into(), // `setup` keeps no GH key
        None => fix(&m.code, &at),
    };
    let what = format!("{who}{}", n.unwrap_or_default()).trim_start().into();
    let row = Flag { code: m.code.clone(), what, next, row: true }.bang("") + &provider::pages(&m.code, owners);
    Ok(Out::fail(&m.code, format!("= ! {} j{job} search \"{q}\" {detail}", m.code), row))
}

/// A search's hits → one unfetched id each (url, no body) + the record `j<job>` → the first page of the list.
fn listed(c: &mut Cache, job: usize, mut m: Meta, hits: &[Hit]) -> Result<Out, Out> {
    let es: Vec<Entry> =
        hits.iter().map(|h| Entry { url: h.url.clone(), job: Some(job), serp: true, ..Entry::default() }).collect();
    let first = c.alloc(es, None).map_err(cache_write)?;
    m.extra.insert("first".into(), first.into());
    m.extra.insert("hits".into(), serde_json::to_value(hits).unwrap_or_default());
    c.serp_put(job, &m, None).map_err(cache_write)?;
    Ok(serp_list(c, job, 0, true))
}

/// A fresh search's part of its `=` line: provider, country, the tiers tried before it, its time and bill.
fn searched(rec: &Meta) -> String {
    let cc = if rec.country.is_empty() { String::new() } else { format!(" {}", rec.country) };
    // gh's two calls run side by side, never one `after` the other: the account call's note (`github`), and
    // GitHub's limit without a token, said here, never in the record's `provider`
    let gh = rec.provider == "github";
    let tried = if rec.hops.len() > 1 && !gh { format!(" after {}", rec.tried()) } else { String::new() };
    let mut note: String = rec.hops.iter().filter(|h| gh && h.code != "ok").map(|h| h.why.as_str()).collect();
    note += if gh && settings::gh_token().is_empty() { " · no token: 10 searches/min" } else { "" };
    let usd = provider::bill(rec.cost, rec.upto());
    format!("{}{cc}{tried}{note} · {:.1}s · spent {usd}", rec.provider, rec.ms as f64 / 1e3)
}

/// Search `job`'s results from hit `off`, up to the list cap; the cursor remembers where `more` goes on. `new` =
/// this command ran the search (its cost and time on the `=` line), else the record's age.
fn serp_list(c: &mut Cache, job: usize, off: usize, new: bool) -> Out {
    let b = bin();
    let Some(rec) = c.serp(job) else { return Out::ask(format!("? j{job} has no search record · {b} <words>")) };
    let hits: Vec<Hit> = rec.extra.get("hits").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    let first = rec.extra.get("first").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    let query = rec.url.strip_prefix("search:").unwrap_or(&rec.url);
    let (mut parts, mut rows) = (Vec::new(), Vec::new());
    for (i, hit) in hits.iter().enumerate().skip(off) {
        let n = first + i;
        let snip = if hit.snippet.is_empty() { String::new() } else { format!(" — \"{}\"", short(&hit.snippet, 90)) };
        let row = match rec.provider.as_str() {
            // `★12.4k owner/repo · description · pushed 2026-10-01`: the title names the repo, the URL adds nothing
            "github" => format!("u{n}  {} · {}", hit.title, hit.snippet),
            _ => format!("u{n}  {}  {}{snip}", short(&hit.url, 40), short(&hit.title, 60)),
        };
        // a URL already in the cache opens from it (`cached`); a failed plain-only prefetch says nothing of the paid
        // tiers, so its row names no render (`fats uN` walks the whole ladder, `dispatch_one`)
        let row = if c.lookup(&hit.url).is_some() { format!("{row} · cached $0") } else { row };
        let billed = new && i == off;
        let mut part = Out::new(vec![row.clone()]);
        part.set("i", i).set("id", format!("u{n}")).set("url", hit.url.clone()).set("cache", !new);
        part.set("title", hit.title.clone()).set("snippet", hit.snippet.clone()).set("position", i + 1);
        part.set("query", query).set("job", format!("j{job}")).set("provider", rec.provider.clone());
        part.set("cost", if billed { rec.cost } else { 0.0 }).set("ms", if billed { rec.ms } else { 0 });
        parts.push(part);
        rows.push(row);
    }
    // a search's own `limit:N` = at most N rows a page (20 at most, `more` goes on) within a read's token cap; else
    // what fits the list cap. A cut by tokens says so: `14 shown u1-u14 of limit:20 (1000 tok cap)`
    let n = c.job(job).and_then(|j| parse(&[j.cmd]).count("limit")).map(|n| n.min(provider::SEARCH_MAX));
    let (mut lines, cap) = (Vec::new(), n.map_or(outline::LIST_CAP, |_| outline::READ_CAP));
    let next = off + outline::cap_rows(&mut lines, &rows, 0, (cap, 1, n));
    let cut = n.filter(|&n| lines.len() < n && next < hits.len());
    let cut = cut.map_or(String::new(), |n| format!(" of limit:{n} ({cap} tok cap)"));
    let total = hits.len();
    let how = if new { searched(&rec) } else { format!("cache {}", outline::fmt_age((now() - rec.t).max(0.0))) };
    let shown = match (off, lines.len()) {
        (0, count) if count == total => String::new(),
        (_, 0) => " · all shown".into(),
        // the rows this answer printed, counted: `5 results` above 4 rows read as one lost (docs-3)
        (_, count) => format!(" · {count} shown u{}-u{}{cut}", first + off, first + off + count - 1),
    };
    let mut head = vec![format!("= j{job} search \"{query}\" · {total} results{shown} · {how}")];
    head.append(&mut lines);
    let at = first + off.min(total.saturating_sub(1));
    head.push(match (total, next < total) {
        (0, _) if query.starts_with("site:") => format!("no on-site results · try without site: → {b} <words>"),
        (0, _) => format!("no results · fewer words: {b} <words>"),
        (_, true) => format!("open one: {b} u{at} · filter: {b} u{at} <words> · more: {b} more"),
        (_, false) => format!("open one: {b} u{at} · filter: {b} u{at} <words>"),
    });
    if total > 0 {
        c.st.cursor = Some(Cursor { u: at, serp: Some((job, next)), ..Cursor::default() });
    }
    let mut o = Out::new(head);
    o.set("id", format!("j{job}")).set("provider", rec.provider.clone()).set("query", query).set("results", total);
    o.set("cost", if new { rec.cost } else { 0.0 }).set("ms", if new { rec.ms } else { 0 }).set("cache", !new);
    o.parts = parts;
    o
}

/// `dry:true` of a search: the providers in order, the ids it would add, the most it can cost.
fn search_dry(cmd: &Cmd, c: &Cache) -> Result<Out, Out> {
    let search = search_of(cmd)?;
    let (runs, max) = fetch::route(&search.plan);
    let cc = search.country.as_ref().map(|cc| format!(" · country:{cc}")).unwrap_or_default();
    let off = settings::get().offline.value;
    let q = &search.q;
    let new = format!("new u{}-u{}", c.st.u.len() + 1, c.st.u.len() + search.limit);
    let line = match (runs.is_empty(), off) {
        (_, true) => format!("search \"{q}\" · $0 · ! offline → {}", fix("offline", &At::default())),
        _ if search.gh => format!("search github \"{q}\" by stars · limit:{} → {new} · $0", search.limit),
        (true, _) => format!("search \"{q}\" · nothing can search · $0{}", nokey_row(&search.plan)),
        _ => format!(
            "search \"{q}\" via {} · limit:{}{cc} · wait:{}s → {new} · up to ${max:.4}{}",
            path_of(&runs),
            search.limit,
            wait_of(cmd).as_secs(),
            nokey_row(&search.plan)
        ),
    };
    let n = prefetch_n(cmd).min(search.limit);
    let pre = format!("then fetch:{n} prefetches the top {n} detached · free, plain only");
    let mut o = Out::new(vec![
        format!("= dry {} · spends nothing now", cmd.canon()),
        line,
        fetch::hedges(cmd.get("hedge")),
        format!("run it: {} {}", bin(), cmd.with_mod("dry", None)),
    ]);
    if n > 0 && !runs.is_empty() && !off {
        o.lines.insert(2, pre);
    }
    o.set("cost", 0.0).set("dry", true);
    Ok(o)
}

/// ` · ! no_key oxy · would try … → fats setup · oxy <sign-up>` for the plan's tiers without a key (`fetch::no_key`).
fn nokey_row(p: &Plan) -> String {
    fetch::no_key(p, &p.tiers(), "", false).iter().map(|r| format!(" · {r}")).collect()
}

/// The router plan for one URL (None = the command's web search) from the command's mods; `spent` = what earlier
/// URLs of this command cost.
fn plan_of(cmd: &Cmd, url: Option<&str>, spent: f64) -> Plan {
    let mut p = Plan::of(cmd, url, spent);
    p.prefetch = url.is_some() && is_search(cmd);
    if settings::get().offline.value {
        p.keys.clear();
    }
    p
}

/// A route as `dry:true` prints it: `plain → bd ∥ oxyjs after 5s`, a tier's second rung `bd retry`. A race
/// (two providers' renders) keeps the no-JS rung after plain only after its captcha: `→ oxy (after a captcha)`
/// (`fetch::route` simulates that wall, so the `up to` holds it).
pub fn path_of(route: &[(Tier, Option<Duration>)]) -> String {
    let gate = route.iter().any(|r| r.1.is_some()) && route.first().is_some_and(|r| r.0 == Tier::Plain);
    let step = |(i, (t, hedge)): (usize, &(Tier, Option<Duration>))| match (i, hedge) {
        (0, _) => t.spec().short.to_string(),
        (_, Some(d)) => format!(" ∥ {} after {}s", t.spec().short, d.as_secs_f64()),
        (_, None) if route[..i].iter().any(|r| r.0 == *t) => format!(" → {} retry", t.spec().short),
        (_, None) => format!(" → {}{}", t.spec().short, if i == 1 && gate { " (after a captcha)" } else { "" }),
    };
    route.iter().enumerate().map(step).collect()
}

/// `dry:true`: what the command would read, add and spend; writes and spends nothing.
pub fn plan(cmd: &Cmd, c: &Cache) -> Out {
    if cmd.has("account") {
        return stats::account_dry(c, cmd.flag("fresh"));
    }
    if cmd.has("setup") {
        return stats::setup(cmd.get("zone"), true, wait_of(cmd));
    }
    if is_search(cmd) {
        return merged(search_dry(cmd, c));
    }
    let mut dry = Dry { cmd, c, next: c.st.u.len(), refused: 0 };
    let mut lines = vec![format!("= dry {} · spends nothing now", cmd.canon())];
    for f in &cmd.files {
        let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        lines.push(format!("{f} {}B local → clean → outline → {} · $0", ksize(size as usize), dry.to(None)));
    }
    for (i, u) in cmd.urls.iter().enumerate() {
        // a URL named twice: one run, as `fetch_all` runs it
        let twice = cmd.urls[..i].contains(u).then(|| format!("{} (same as above) · $0", short(u, 60)));
        lines.push(twice.unwrap_or_else(|| dry.would(&short(u, 60), u, None)));
    }
    let cur = c.st.cursor.clone().unwrap_or_default();
    let ids = match (&cmd.ids[..], cur.u) {
        ([], u) if u > 0 && cmd.urls.is_empty() && cmd.files.is_empty() => vec![Id { u, ..Id::default() }],
        (ids, _) => ids.to_vec(),
    };
    for id in &ids {
        lines.push(dry.id_row(id));
    }
    // every row refused: there is nothing to run, and the plan is a failure
    let none = dry.refused > 0 && dry.refused == lines.len() - 1;
    if !none {
        lines.push(fetch::hedges(cmd.get("hedge")));
        lines.push(format!("run it: {} {}", bin(), cmd.with_mod("dry", None)));
    }
    let mut o = Out::new(lines);
    o.rc = i32::from(none);
    o.set("cost", 0.0).set("dry", true);
    o
}

/// One `dry:true` run: the rows it prints come from here.
struct Dry<'a> {
    cmd: &'a Cmd,
    c: &'a Cache,
    /// The u number the last new id took.
    next: usize,
    /// Rows the fence refuses: nothing would run for them.
    refused: usize,
}

impl Dry<'_> {
    /// The id a fetch would store into: the unfetched search result `slot`, else a new one.
    fn to(&mut self, slot: Option<usize>) -> String {
        self.next += usize::from(slot.is_none());
        slot.map_or_else(|| format!("new u{}", self.next), |n| format!("fills u{n}"))
    }

    /// What a fetch of `u` would do: a cache hit, a refusal, or the route it would take and its worst cost.
    fn would(&mut self, what: &str, u: &str, slot: Option<usize>) -> String {
        if let Some((_, age)) = self.c.lookup(u).filter(|_| !self.cmd.fresh()) {
            return format!("{what} cache hit, cache {} → {} · $0", outline::fmt_age(age), self.to(slot));
        }
        if provider::private_host(u) {
            self.refused += 1;
            return format!("{what} ! private_address → {}", fix("private_address", &At { url: u, ..At::default() }));
        }
        let mut p = plan_of(self.cmd, Some(u), 0.0);
        fetch::route_of(self.c, u, &mut p);
        let (tiers, cost) = fetch::route(&p);
        if settings::get().offline.value {
            return format!("{what} not cached → ! offline (FATS_OFFLINE) · $0");
        }
        // the one lookup a plan makes (free): a host no resolver knows would end the run before any tier
        if fetch::host_hop(u).is_some_and(|h| h.code == "no_host") {
            self.refused += 1;
            let at = At { url: u, ..At::default() };
            // the run stores its failure in an id all the same
            let (host, to, id) = (host_of(u), fix("no_host", &at), self.to(slot));
            return format!("{what} ! no_host {host} does not resolve (2 resolvers) · {id} · $0 → {to}");
        }
        let nokey = nokey_row(&p);
        if tiers.is_empty() {
            // js:true or provider: left only tiers without a key
            return format!("{what} not cached · nothing can fetch it · {} · $0{nokey}", self.to(slot));
        }
        let to = self.to(slot);
        let mods = dry_mods(self.cmd, u, &tiers);
        // a paid ask runs one provider at a time (`Plan::hedged`)
        let off = if p.hedged() { "" } else { " · hedge:off, one provider at a time" };
        let quote = if p.js { format!(" · {}", fetch::JS_QUOTE) } else { String::new() };
        format!("{what} fetch {} → {to} · up to ${cost:.4}{quote}{off}{mods}{nokey}", path_of(&tiers))
    }

    /// What reading id `i` would do: follow a link, fetch a search result, else read the snapshot.
    fn id_row(&mut self, i: &Id) -> String {
        let (cmd, c, page) = (self.cmd, self.c, i.page());
        if let Some(&li) = cmd.links.first() {
            let link = load(c, i).and_then(|l| l.doc.ok()).and_then(|d| d.links.get(li.wrapping_sub(1)).cloned());
            return match link {
                Some(u) => self.would(&format!("{page}{} l{li} {}", i.sec_tail(), short(&u, 60)), &u, None),
                None => format!("{page}{} l{li} unknown · $0", i.sec_tail()),
            };
        }
        // a refetch (`fresh:true`, `js:true`) of a fetched page is a new id (`fetch_again`), a search result fills its own
        let again = cmd.fresh().then(|| load(c, i)).flatten().map(|l| l.meta.url).filter(|u| u.starts_with("http"));
        // an id whose run has not ended reads it (`pending`, `! abandoned`), the way `dispatch_one` does
        let ran = pending(c, i.u, &[]).is_some_and(|o| o.rc == 0 || !cmd.fresh());
        match c.st.result(i.u).filter(|e| i.job.is_none() && !ran && (e.dir.is_empty() || cmd.fresh())) {
            Some(e) => self.would(&format!("{page} {} (search result)", short(&e.url, 60)), &e.url, Some(i.u)),
            None if let Some(u) = again => self.would(&format!("{page} {}", short(&u, 60)), &u, None),
            None => format!("{page} local read · $0"),
        }
    }
}

/// The `country:` and `device:` a fetch would send, as `dry:true` prints them (`provider::sent`).
fn dry_mods(cmd: &Cmd, u: &str, tiers: &[(Tier, Option<Duration>)]) -> String {
    let ask = Ask { url: u.into(), country: cmd.get("country").map(String::from), ..Ask::default() };
    let country = match (provider::sent(&ask, Tier::Bd), cmd.get("country")) {
        (Some(cc), from) if tiers.iter().any(|t| t.0 != Tier::Plain) => {
            format!(" · country:{cc}{}", if from.is_none() { " (from the domain)" } else { "" })
        }
        _ => String::new(),
    };
    let device = match cmd.get("device") {
        Some("mobile") => " · device:mobile (oxy only; bd serves desktop)",
        _ => "",
    };
    country + device
}

/// `fats j<N>`: a search's list, its failure (its `!` row and tried path, as when it ran) or `pending`; a read
/// naming `wait:` waits for a search still running, as for a page (`wait_for`), then reads the ids its run stored.
fn job_read(cmd: &Cmd, c: &mut Cache, j: usize) -> Out {
    poll_until(c.born, wait_of(cmd), || cmd.get("wait").is_none() || search_pending(c, j).is_none());
    c.reload();
    let failed = c.serp(j).filter(|m| !m.ok).zip(c.job(j)).map(|(m, x)| search_out(c, &parse(&[x.cmd]), j, &m));
    let mut o = failed.map(merged).or_else(|| search_pending(c, j)).unwrap_or_else(|| job_view(c, j));
    if !cmd.words.is_empty() {
        let q = cmd.words.join(" ");
        o.warn(vec![format!("j{j} ! not_in_v1 multi-page filter → {} j{j}.u1 {q}", bin())]);
    }
    o
}

/// No page, no URL, no words: what to type; a list word alone names the page it needs (the current one, else u1).
fn nothing_to_do(cmd: &Cmd, u: usize) -> Out {
    let (b, list) = (bin(), WORDS.lists.iter().chain(&["media", "comments"]).find(|w| cmd.has(w)));
    let page = list.map(|w| format!("? {w} needs a page: {b} u{} {w}", u.max(1)));
    Out::ask(page.unwrap_or(format!("? nothing to do · {b} <url> · {b} u1#2 · {b} u1 <words>")))
}

fn dispatch(cmd: &Cmd, c: &mut Cache) -> Out {
    let b = bin();
    if let Some(o) = command(cmd, c) {
        return o;
    }
    if let Some(&j) = cmd.jobs.first()
        && cmd.ids.is_empty()
        && cmd.urls.is_empty()
        && cmd.files.is_empty()
    {
        return job_read(cmd, c, j);
    }
    let one = match (cmd.files.as_slice(), page_url(cmd), cmd.words.is_empty()) {
        ([f], None, false) if cmd.urls.is_empty() => Some(local(c, f, cmd)),
        ([], Some(u), false) => Some(fetch(c, Want { url: u, ..Want::default() }, cmd)),
        _ => None,
    };
    if let Some(o) = one {
        // a document + words: read it, then filter it like `fats u3 <words>`; still running → its `pending`
        let o = merged(o);
        return match (o.rc, c.st.cursor.clone(), o.rec.contains_key("pending")) {
            (0, Some(cur), false) => dispatch_one(cmd, c, &cur, Id { u: cur.u, ..Id::default() }, true),
            _ => o,
        };
    }
    if is_search(cmd) {
        // free words alone = the omnibox search, not a filter of the last page; a URL + words = within that site
        return merged(web(cmd, c));
    }
    if !cmd.files.is_empty() || !cmd.urls.is_empty() {
        return fetch_all(cmd, c);
    }
    let cur = c.st.cursor.clone().unwrap_or_default();
    let mut ids = cmd.ids.clone();
    if ids.is_empty() && (!cmd.links.is_empty() || cmd.has("more") || cmd.has("up")) {
        if cur.u == 0 {
            return Out::ask(format!("? no current page · {b} <url> first"));
        }
        // a bare `more` goes on with a search list, else with the current page (`read`)
        if let Some((j, off)) = cur.serp.filter(|_| cmd.links.is_empty() && cmd.has("more")) {
            return serp_list(c, j, off, false);
        }
        ids.push(Id { u: cur.u, ..Id::default() });
    }
    let Some(id) = ids.first().cloned() else { return nothing_to_do(cmd, cur.u) };
    // one page per command in v1: name the others instead of dropping them (the cross-page filter comes later)
    let rest: Vec<String> = ids[1..]
        .iter()
        .map(|i| {
            let (p, q) = (i.page(), cmd.words.join(" "));
            let what = if q.is_empty() { "read" } else { "filter" };
            format!("{p} ! not_in_v1 multi-page {what} → {}", format!("{b} {p} {q}").trim_end())
        })
        .collect();
    let mut out = dispatch_one(cmd, c, &cur, id, false);
    out.warn(rest);
    out
}

/// The commands that read no page: status (also `fats` alone), version, stats, config, account, setup, guide;
/// `routes` (gone) a `?`, `install` a `!` with the install page.
fn command(cmd: &Cmd, c: &mut Cache) -> Option<Out> {
    let b = bin();
    Some(match () {
        _ if cmd.idle || cmd.has("status") => stats::status(c),
        _ if cmd.has("version") => {
            Out::new(vec![format!("= {b} {} · status: {b} · guide: {b} guide", env!("CARGO_PKG_VERSION"))])
        }
        _ if cmd.has("stats") => stats::stats(c),
        _ if cmd.has("config") => stats::config(),
        _ if cmd.has("account") => stats::account(c, cmd.flag("fresh"), wait_of(cmd)),
        _ if cmd.has("setup") => stats::setup(cmd.get("zone"), settings::get().offline.value, wait_of(cmd)),
        // gone with the learned start: a `?`, never a paid web search
        _ if cmd.has("routes") => {
            Out::ask(format!("? routes: gone (no learned starts) · a page's route: {b} <url> dry:true · {b} config"))
        }
        _ if cmd.has("guide") => Out::new(CARD.lines().map(String::from).collect()),
        _ => return None,
    })
}

/// One page id + mods. `new` = this command just stored the page (no `· cache` age on it).
fn dispatch_one(cmd: &Cmd, c: &mut Cache, cur: &Cursor, id: Id, new: bool) -> Out {
    let mine = id.job.is_none_or(|j| c.job(j).is_some_and(|x| x.session == c.session));
    let refetch = cmd.fresh() && cmd.links.is_empty() && cmd.media.is_empty() && !new;
    // a fetch still running answers at once, or as it ends within a `wait:` the read names (`pending`'s arrow:
    // `poll_wait`: one turn per 30 s, not per 5); `fresh:true` starts only an abandoned one again
    if mine && cmd.get("wait").is_some() && wait_for(c, id.u, wait_of(cmd)).is_some() {
        c.reload();
    }
    if let Some(o) = pending(c, id.u, &cmd.words).filter(|o| mine && (o.rc == 0 || !refetch)) {
        return o;
    }
    // a failed prefetch (plain only) goes up the whole ladder only when named (`fats u3`), never on a bare read word
    // (`more`, `up`)
    let missed = || c.run(id.u).is_some_and(|(r, _)| r.done.as_ref().is_some_and(|d| !d.ok) && is_prefetch(&r));
    let again = refetch || (!cmd.ids.is_empty() && missed());
    if let Some(url) = c.st.result(id.u).filter(|e| mine && (e.dir.is_empty() || again)).map(|e| e.url.clone()) {
        return open_result(cmd, c, cur, id, &url);
    }
    if refetch {
        return fetch_again(cmd, c, &id);
    }
    if let Some(&li) = cmd.links.first() {
        return open_link(cmd, c, &id, li);
    }
    if let Some(&k) = cmd.media.first() {
        return asset(c, &id, k, cmd.fresh());
    }
    read(cmd, c, cur, &id, new)
}

/// Pictures fetched on demand (`asset`): plain only, never a paid tier; an `image/*` answer of at most
/// `ASSET_MAX` bytes within `ASSET_WAIT` (`fats config`).
pub const ASSET_MAX: u64 = 10_000_000;
pub const ASSET_WAIT: Duration = Duration::from_secs(15);

/// `fats u3 m1`: picture m1 of the page (`fats u3 media` order), fetched plain into `<page>/assets/` once; a
/// re-run reads it there. The `=` line says what viewing it costs (`outline::viewed`) before any pixel is sent:
/// the shell's own reader opens the path; `fats mcp` attaches the pixels (`image` in the record). `fresh` (`fats u3
/// m1 fresh:true`) downloads it again, never the page. A file too big for this path: the `max_bytes:` retry is no
/// use (`At::done`), it opens in a browser.
fn asset(c: &Cache, id: &Id, k: usize, fresh: bool) -> Out {
    let Some(page) = load(c, id) else { return unknown(c, id) };
    let media = page.doc.as_ref().map_or(&[][..], |d| &d.media[..]);
    let (pic, rec) = (media.get(k.wrapping_sub(1)), page.dir.join("assets").join(format!("m{k}.json")));
    let src = pic.map_or("", |p| p.src.as_str());
    let ok = |a: &Asset| a.src == src && a.ext.bytes().all(|b| b.is_ascii_alphanumeric()); // the record names no path
    let kept = jload::<Asset>(&rec).filter(|a| !fresh && ok(a) && rec.with_extension(&a.ext).is_file());
    let (new, got) = match (pic, kept) {
        (None, _) => (true, Err(("no_media".into(), format!("· the page lists {}", media.len())))),
        (_, Some(a)) => (false, Ok(a)),
        _ => (true, download(&rec, src)),
    };
    let subject = format!("{} m{k}", page.eq);
    let a = match got {
        Ok(a) => a,
        Err((code, what)) => {
            let at = At { id: &page.pid, url: src, arg: &format!("m{k}"), done: &["max_bytes:64m"] };
            let row = bang(&format!("{} m{k}", page.pid), &code, &what, &at);
            return Out::fail(&code, format!("= {subject} ! {code} {what}"), row);
        }
    };
    let (kind, path) = (if pic.is_some_and(|p| p.video) { "poster" } else { "photo" }, rec.with_extension(&a.ext));
    let (ext, bytes, path) = (&a.ext, ksize(a.bytes as usize), path.display());
    // what viewing it costs: low = the standard tier `fats mcp` sends, high = unscaled on Claude 4.7+ (`~1.5–4.7k`)
    let [lo, hi] = [outline::STD, outline::HI_RES].map(|t| ksize(outline::viewed(a.w, a.h, t).1 as usize));
    let lo = if lo == hi { String::new() } else { lo.replace('k', "") + "–" };
    let view = if a.w == 0 { "size unread, view cost unknown".into() } else { format!("~{lo}{hi} tok if viewed") };
    let how = if new { format!("plain {}ms $0", a.ms) } else { format!("cache {}", outline::fmt_age(now() - a.t)) };
    let mut o =
        Out::new(vec![format!("= {subject} {kind} {}x{} {ext} {bytes}B · saved {path} · {view} · {how}", a.w, a.h)]);
    o.set("id", page.eq).set("image", path.to_string()).set("cost", 0.0).set("cache", !new);
    o
}

/// Picture `src` fetched plain: the file, then its record `rec`. Err = (code, what). A 403/410 is a signed link of
/// the cached page that has expired (the page fetched again carries a fresh one) or a host that refuses
/// downloads from elsewhere (hotlink protection): which one is not known.
fn download(rec: &Path, src: &str) -> Result<Asset, (String, String)> {
    let err = |code: &str, what: String| (code.to_string(), what);
    if settings::get().offline.value {
        return Err(err("offline", "FATS_OFFLINE is on".into()));
    }
    let t0 = Instant::now();
    let ask = Ask { url: src.into(), timeout: Some(ASSET_WAIT), max_bytes: ASSET_MAX, html: true, ..Ask::default() };
    let got = provider::block(provider::get(Tier::Plain, &ask, "")).map_or(Err("net_error".into()), |g| g.0);
    let raw = got.map_err(|f| match f.code {
        "too_big" => err(f.code, format!("{} (>{}B)", f.why, ksize(ASSET_MAX as usize)).trim_start().into()),
        code => err(code, f.why),
    })?;
    let ctype = raw.ctype.split(';').next().unwrap_or_default().trim().to_string();
    match raw.status {
        200..300 if ctype.starts_with("image/") => {}
        200..300 => return Err(err("not_image", if ctype.is_empty() { "no content-type".into() } else { ctype })),
        s @ (403 | 410) => return Err(err(&format!("http_{s}"), "· signed link expired or hotlink-blocked".into())),
        s => return Err(err(&format!("http_{s}"), String::new())),
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(&raw.body)).with_guessed_format().ok();
    let (w, h) = reader.and_then(|r| r.into_dimensions().ok()).unwrap_or_default();
    let ext = image::ImageFormat::from_mime_type(&ctype).map_or("img", |f| f.extensions_str()[0]);
    let file = rec.with_extension(ext);
    let write = |e: std::io::Error| err("cache_write", format!("{}: {e}", file.display()));
    std::fs::create_dir_all(rec.parent().unwrap_or(rec)).map_err(write)?;
    std::fs::write(&file, &raw.body).map_err(write)?;
    let (bytes, ms) = (raw.body.len() as u64, t0.elapsed().as_millis() as u64);
    let a = Asset { src: src.into(), ext: ext.into(), bytes, ms, t: now(), w, h };
    crate::cache::jsave(rec, &a).map_err(|e| err("cache_write", e))?;
    Ok(a)
}

fn open_result(cmd: &Cmd, c: &mut Cache, cur: &Cursor, id: Id, url: &str) -> Out {
    let o = merged(fetch(c, Want { url, slot: Some(id.u), ..Want::default() }, cmd));
    let open = cmd.words.is_empty() && cmd.links.is_empty() && cmd.media.is_empty() && cmd.reserved.is_empty();
    let open = open && id.sec.is_none();
    if o.rc != 0 || open || c.st.result(id.u).is_some_and(|e| e.dir.is_empty()) {
        return o;
    }
    let mut out = dispatch_one(cmd, c, cur, id, true);
    let warn = o.rec.get("warnings").and_then(|w| w.as_array()).into_iter().flatten();
    out.warn(warn.filter_map(|w| w.as_str().map(String::from)).collect());
    out
}

/// A new id of `url` that a stored page points at (`u3 fresh`, `u3 l4`): its label says where it came from.
fn refetch(c: &mut Cache, cmd: &Cmd, url: &str, from: &str) -> Out {
    merged(fetch(c, Want { url, prov: Some(from), slot: None }, cmd))
}

/// `fresh:true` on a stored page: a refetch is a new id; the old snapshot stays readable.
fn fetch_again(cmd: &Cmd, c: &mut Cache, id: &Id) -> Out {
    let Some(l) = load(c, id) else { return unknown(c, id) };
    match l.meta.url.strip_prefix("file://") {
        Some(f) => merged(local(c, f, cmd)),
        None if l.meta.url == "stdin:" => {
            Out::ask(format!("? {} came from stdin · pipe it again: … | {} -", l.pid, bin()))
        }
        None if l.meta.url.is_empty() => no_doc(&l.pid, &l.eq, &l.meta, &[], &NoDoc::Missing, None), // forgotten
        None => refetch(c, cmd, &l.meta.url, &format!("{} fresh", l.pid)),
    }
}

/// `u3 l4`: fetch the page's link 4 as a new id.
fn open_link(cmd: &Cmd, c: &mut Cache, id: &Id, li: usize) -> Out {
    let Some(l) = load(c, id) else { return unknown(c, id) };
    let doc = match &l.doc {
        Ok(d) => d,
        Err(why) => return no_doc(&l.pid, &l.eq, &l.meta, &[], why, None),
    };
    let sec = id.sec_tail();
    match li.checked_sub(1).and_then(|i| doc.links.get(i)) {
        Some(u) => refetch(c, cmd, u, &format!("{}{sec} l{li}", l.pid)),
        None => {
            let n = doc.links.len();
            let have = if n == 0 { "no links".into() } else { format!("have l1-l{n}") };
            Out::ask(format!("? l{li} unknown on {} · {have} · {} {} links", l.pid, bin(), l.pid))
        }
    }
}

/// Read a stored page: links, a filter, `all`, `full`, a section (part), `more`, else the outline.
fn read(cmd: &Cmd, c: &mut Cache, cur: &Cursor, id: &Id, new: bool) -> Out {
    let named = |m: &Match| cmd.get("match") == Some(m.name());
    let rung = [Match::Exact, Match::Near, Match::Topic].into_iter().find(named).unwrap_or(Match::Words);
    let pinned = cmd.get("match").is_some();
    let more = cmd.has("more") && cur.u == id.u && id.job.is_none();
    let (from, limit) = (cmd.count("from").map_or(0, |n| n.saturating_sub(1)), cmd.count("limit"));
    page_cmd(c, id, new, |p| {
        Ok(if let Some(kind) = WORDS.lists.iter().find(|w| cmd.has(w)) {
            outline::list(p, from, limit, &cmd.words, kind)
        } else if !cmd.words.is_empty() {
            search::grep(p, &cmd.words, (rung, pinned), id.sec, limit)?
        } else if cmd.has("all") {
            outline::outline_all(p)
        } else if cmd.has("media") {
            outline::media(p, from, limit)
        } else if cmd.has("comments") {
            outline::comments(p, id.sec, from, limit)
        } else if cmd.has("full") && (1..=p.doc.sections.len()).contains(&id.sec.unwrap_or(1)) || more && cur.full {
            outline::read_full(p, id.sec.or(cur.k.filter(|_| more)), if more && cur.full { cur.part + 1 } else { 0 })
        } else if let Some(k) = id.sec.or(cur.k.filter(|_| more)) {
            outline::read_sec(p, k, if more && cur.k == Some(k) { cur.part + 1 } else { id.part })?
        } else if cmd.has("more") {
            outline::outline_all(p)
        } else {
            outline::outline(p, None)
        })
    })
    .unwrap_or_else(|| unknown(c, id))
}

/// A whole-command refusal: the `= ! {code} {what}` line and the `!` row (`{subject} ! {code} → fix`), with no
/// provider page (the codes here are never about a provider's key or money).
fn refused(code: &str, subject: &str, what: &str, at: &At) -> Out {
    Out::fail(code, format!("= ! {code} {what}"), bang(subject, code, "", at))
}

/// The cache dir refused a write: say which file and where the dir is set.
fn cache_write(e: impl std::fmt::Display) -> Out {
    refused("cache_write", "", &e.to_string(), &At::default())
}

/// Both arms are an `Out`: a failure (a bad cache write, a fenced read) is itself the answer, carried by `?`.
fn merged(r: Result<Out, Out>) -> Out {
    r.unwrap_or_else(|o| o)
}

/// A URL whose own host is loopback, private or link-local: refused before any tier sees it.
fn private(url: &str) -> Out {
    let what = format!("{} · loopback, private and link-local hosts are not fetched · 0ms spent $0", short(url, 60));
    refused("private_address", &short(url, 60), &what, &At { url, ..At::default() })
}

fn offline(what: &str) -> Out {
    let detail = format!("{what} · not cached · FATS_OFFLINE is on: cache and local files only · 0ms spent $0");
    refused("offline", what, &detail, &At::default())
}

/// `fats j3`: the job's command, when it ran, and its pages (each as a typed-back id).
fn job_view(c: &Cache, j: usize) -> Out {
    let b = bin();
    let (Some(job), Some((st, _))) = (c.job(j), ids_of(c, Some(j))) else {
        return unknown(c, &Id { job: Some(j), u: 1, ..Id::default() });
    };
    if job.cmd.is_empty() {
        return refused("expired", "", &format!("j{j} · its command and pages went after an hour"), &At::default());
    }
    let mut rows = Vec::new();
    for (idx, entry) in st.u.iter().enumerate().filter(|(_, e)| e.job == Some(j)) {
        let u = idx + 1;
        if entry.dir.is_empty() {
            // a search result whose URL is in the cache opens from it (as the list and `stats` say); THE liveness
            // rule (`Cache::run`, the lock), as `stats` reads it
            let cached = if c.lookup(&entry.url).is_some() { "cached $0" } else { "not fetched" };
            let run = if c.run(u).is_some_and(|r| !r.1 && r.0.done.is_none()) { "! abandoned" } else { "pending" };
            let what = if entry.serp { format!("search result, {cached}") } else { run.into() };
            rows.push(format!("j{j}.u{u} {what} {} → {b} j{j}.u{u}", short(&entry.url, 60)));
            continue;
        }
        let (meta, doc) = c.page(entry);
        let size = |doc: Doc| format!("{} · {}~tok", short(&doc.title, 50), ksize(doc.tokens.clean));
        let what = doc.map_or_else(|_| format!("{} · no doc", meta.kind), size);
        let prov = if entry.alias { "cache" } else { meta.provider.as_str() };
        rows.push(format!("j{j}.u{u} {prov} {}ms ${:.4} {} | {what}", meta.ms, meta.cost, short(&entry.url, 60)));
    }
    let age = outline::fmt_age((now() - job.t).max(0.0));
    // a search's rows are results, fetched or not (`9 pages` for 9 unfetched results)
    let what = if st.u.iter().any(|e| e.job == Some(j) && e.serp) { "results" } else { "pages" };
    let mut lines = vec![format!("= j{j} {} {what} · {age} ago · s:{} · {b} {}", rows.len(), job.session, job.cmd)];
    let next = match rows.len() {
        0 => format!("no pages stored · rerun: {b} {}", job.cmd),
        _ => format!("open: {b} j{j}.u{}", st.u.iter().position(|e| e.job == Some(j)).map_or(1, |p| p + 1)),
    };
    lines.extend(rows);
    lines.push(next);
    Out::new(lines).with("id", format!("j{j}"))
}

fn unknown(c: &Cache, id: &Id) -> Out {
    let b = bin();
    let last = Some(c.job_count()).filter(|&n| n > 0).map(|n| format!("last job j{n}"));
    let Some((st, _)) = ids_of(c, id.job) else {
        let last = last.unwrap_or("no jobs yet".into());
        return Out::ask(format!("? j{} unknown · {last} · {b} <url>", id.job.unwrap_or_default()));
    };
    let have = if st.u.is_empty() { "none".into() } else { format!("u1-u{}", st.u.len()) };
    Out::ask(format!("? {} unknown · have {have} · {}", id.page(), last.unwrap_or(format!("{b} <url>"))))
}

struct Loaded {
    pid: String,
    eq: String,
    meta: Meta,
    doc: Result<Doc, NoDoc>,
    dir: std::path::PathBuf,
    other: bool,
}

/// The ids job `j` sees (None = this session's): (its session's state, true when that is another session).
/// None = an unknown job.
fn ids_of(c: &Cache, j: Option<usize>) -> Option<(State, bool)> {
    let Some(j) = j else { return Some((c.st.clone(), false)) };
    let s = c.job(j)?.session;
    // another session's state, read-only, from inside the cache (`Cache::dir`)
    let other = || c.dir(&Entry { dir: s.clone(), ..Entry::default() }).and_then(|d| jload(&d.join("state.json")));
    Some(if s == c.session { (c.st.clone(), false) } else { (other().unwrap_or_default(), true) })
}

/// Resolve an id (`u3`, `j8.u3`) to its snapshot. None = unknown id.
fn load(c: &Cache, id: &Id) -> Option<Loaded> {
    let (st, other) = ids_of(c, id.job)?;
    let e = st.u.get(id.u.checked_sub(1)?)?.clone();
    let (meta, doc) = c.page(&e);
    let (pid, eq) = match (other, e.job) {
        (true, _) => (id.page(), id.page()),
        (false, Some(j)) => (format!("u{}", id.u), format!("j{j}.u{}", id.u)),
        (false, None) => (format!("u{}", id.u), format!("u{}", id.u)),
    };
    Some(Loaded { pid, eq, meta, doc, dir: c.dir(&e).unwrap_or_default(), other })
}

/// The other stored copies of page `n`'s URL in this session: the newest id a paid ask (`js:true`, `provider:`:
/// its meta says `hedge:off`) served with other text than `doc`'s (`u2 (rendered)`), else, when `doc` shows none of
/// the page's own words, a copy's part that does (`u1#3fy (page content)`), else, when it holds no record of
/// its own either, a copy's unread page record (`u1#2fv (page record) · 995 tok · $0`, never the error page
/// as "the best copy" beside it); and whether one of two copies with the same served text was a paid ask (the paid
/// hint bought that copy back already); and the first newer copy asked another way (country, provider: a geo
/// fix that ran), whether the run that fetched this one named `country:`.
type Copies = (Option<String>, bool, (Option<String>, bool));
fn copies(c: &Cache, n: usize, (meta, doc): (&Meta, &Doc), log: &[String]) -> Copies {
    let text = |d: &Doc| d.sections[..d.served()].iter().flat_map(|s| s.lines.clone()).collect::<Vec<_>>();
    // a paid ask, or the longer time (the 150 s call that answered is the page)
    let (me, fix) = (&c.st.u[n - 1], settings::limit::<u64>("render_fix_s"));
    let paid = |m: &Meta| m.hedge == "off" || m.hops.iter().any(|h| h.timeout_s >= fix);
    let owner = |m: &Meta| Tier::parse(&m.provider).map(Tier::owner);
    let runs = c.st.u.iter().enumerate().filter(|(_, e)| e.dir == me.dir).filter_map(|(i, _)| c.run(i + 1));
    let asked = runs.into_iter().any(|(r, _)| parse(std::slice::from_ref(&r.cmd)).get("country").is_some());
    let mut found = (None, false, (None, asked));
    // A copy with neither the page's own words nor a record of its own (a 536 B error page)
    let bare = outline::shown(doc, meta).is_none() && outline::record_in(doc, meta, &[]).is_none();
    for (i, o) in c.st.u.iter().enumerate().filter(|(_, o)| o.url == me.url && !o.dir.is_empty() && o.dir != me.dir) {
        let (m, Ok(d)) = c.page(o) else { continue };
        let other = m.ok && (m.country != meta.country || owner(&m) != owner(meta));
        found.2.0 = found.2.0.or((i >= n && other).then(|| format!("u{}", i + 1)));
        let same = text(&d) == text(doc);
        found.1 |= same && (paid(&m) || paid(meta));
        // a paid render with other text, else (this copy without the page's own words) a copy's part holding them
        let held = outline::shown(&d, &m).filter(|_| outline::shown(doc, meta).is_none());
        let rec = || outline::record_in(&d, &m, log).filter(|_| bare).map(|r| format!("u{}{r}", i + 1));
        found.0 = match held {
            _ if !same && m.ok && paid(&m) => Some(format!("u{} (rendered)", i + 1)),
            Some(tail) => Some(format!("u{}{tail} (page content)", i + 1)),
            None => found.0.or_else(rec),
        };
    }
    found
}

/// Run `f` on a page. None = unknown id. Moves the cursor only in this session. `new` = this command made the
/// snapshot; every other read names its age (`· cache 15h`) so a stale page is never shown as current.
fn page_cmd(c: &mut Cache, id: &Id, new: bool, f: impl FnOnce(&Page) -> Result<Shown, String>) -> Option<Out> {
    let Loaded { pid, eq, meta, doc, dir, other } = load(c, id)?;
    let (before, others) = c.st.u.get(id.u - 1).filter(|_| !other).map(|me| seen(c, me)).unwrap_or_default();
    // A copy with no doc gets the URL's other copies too (its newer served copy is its move)
    let blank = Doc::default();
    let (render, again, fixed) = if other {
        (None, false, (None, false))
    } else {
        copies(c, id.u, (&meta, doc.as_ref().unwrap_or(&blank)), &before)
    };
    let doc = match doc {
        Ok(d) => d,
        Err(why) => return Some(no_doc(&pid, &eq, &meta, &others, &why, render)),
    };
    let age = (!new).then(|| (now() - meta.t).max(0.0));
    // The part read now is read before the next move is computed (never `→` this part)
    let key = id.sec.and_then(|k| outline::part_key(&doc, k, id.part));
    let read = std::iter::once(id.sec_tail()).chain(key.clone()).chain(before).collect();
    let p = Page { id: pid, eq, doc: &doc, meta: &meta, age, at: (read, render, again, fixed, others) };
    let (mut lines, cursor) = match f(&p) {
        Ok(s) => s,
        Err(q) => return Some(Out::ask(q)),
    };
    // every read's `=` line: a stored page's age, then the page's flags as the last group (`outline::flags`)
    if let Some(l) = lines.first_mut().filter(|l| l.starts_with("= ")) {
        if let Some(a) = age {
            // once: a head line names it already (`· cache 2m`, `· og cache 2m`)
            let age = format!("cache {}", outline::fmt_age(a));
            let age = if l.contains(&format!(" {age}")) { String::new() } else { format!(" · {age}") };
            l.push_str(&(age + &stale(c, id)));
        }
        l.push_str(&outline::flags(&p));
    }
    let mut o = Out::new(lines);
    record(&mut o, &p, &dir.to_string_lossy(), new);
    if id.sec.is_some() {
        o.set("id", format!("{}{}", p.eq, id.sec_tail())); // the section read, as named
    }
    if id.sec.is_some()
        && !other
        && let Err(e) = c.read_mark(id.u, &[id.sec_tail()].into_iter().chain(key).collect::<Vec<_>>())
    {
        o = cache_write(e);
    }
    if let Some(mut cur) = cursor
        && !other
    {
        cur.u = id.u;
        c.st.cursor = Some(cur);
    }
    Some(o)
}

/// What the session read of entry `me`'s page: its parts under any id of its folder (a cache hit is a new id of the
/// same folder) and the text of parts read under any copy of its URL (`outline::part_key`); and
/// the URL's other copies (their cuts bound the timeout fix: `fetch::fix_arg`; their failures count).
fn seen(c: &Cache, me: &Entry) -> (Vec<String>, Vec<Meta>) {
    let copies = c.st.u.iter().filter(|e| !e.dir.is_empty() && (e.url == me.url || e.dir == me.dir));
    let read = copies.clone().flat_map(|e| e.read.iter().filter(|r| e.dir == me.dir || r.starts_with('=')).cloned());
    let others = copies.filter(|e| e.dir != me.dir).filter_map(|e| c.meta(&e.dir));
    (read.collect(), others.collect())
}

/// ` (3 jobs ago; newest j8)` after a bare `uN` whose page came from an older job than the session's newest
/// ids (a fetch or a search's results): one session per user means `u1` may be another task's page, or not the
/// first result of the search just run. `j5.u1` names its job: no tail.
fn stale(c: &Cache, id: &Id) -> String {
    let Some(j) = c.st.u.get(id.u.wrapping_sub(1)).and_then(|e| e.job).filter(|_| id.job.is_none()) else {
        return String::new();
    };
    let newer: std::collections::BTreeSet<usize> = c.st.u.iter().filter_map(|e| e.job).filter(|&k| k > j).collect();
    let s = if newer.len() == 1 { "" } else { "s" };
    newer.last().map_or(String::new(), |n| format!(" ({} job{s} ago; newest j{n})", newer.len()))
}

fn record(o: &mut Out, p: &Page, path: &str, new: bool) {
    let m = p.meta;
    o.set("id", p.eq.clone())
        .set("url", p.doc.final_url.clone())
        .set("provider", m.provider.clone())
        .set("cost", if new { m.cost } else { 0.0 })
        .set("bytes_in", if new { m.bytes } else { 0 })
        .set("cache", !new)
        .set("age", (now() - m.t).max(0.0) as u64)
        .set("path", path.to_string())
        .set("mode", p.doc.mode.clone());
    if let Some(l) = o.lines.first()
        && let Some(i) = l.find("match:")
    {
        o.set("match", l[i + 6..].to_string());
    }
}

/// The move that works today for a zip (docx, xlsx) this build cannot read: pipe its XML through stdin.
fn text_of_zip(path: &str) -> String {
    format!("unzip -p {path} '*.xml' | {} -", bin())
}

/// A snapshot without a doc: a body this build cannot read, a failed fetch, or a converter crash. One shape:
/// `= {eq} ! {code} {what}`, then the `!` row with the table fix (`lang::bang`) and, for a key code, the provider's
/// page.
pub(crate) fn no_doc(id: &str, eq: &str, m: &Meta, others: &[Meta], why: &NoDoc, served: Option<String>) -> Out {
    let url = short(&m.url, 60);
    let mut at = At { id, url: &m.url, ..At::default() };
    let (mut owners, mut next) = (Vec::new(), None);
    let zip = if let NoDoc::Zip(path) = why { text_of_zip(&path.to_string_lossy()) } else { String::new() };
    let (code, what, detail) = match why {
        NoDoc::Zip(_) => {
            at.arg = &zip;
            let what =
                format!("zip (docx/xlsx) · {url} · {}B cached, no zip text in this build", ksize(m.bytes as usize));
            ("not_in_v1", what, format!("zip {url}"))
        }
        NoDoc::Parse(e) => ("parse_failed", format!("· {url} · {e}"), url.clone()),
        NoDoc::Missing if !m.ok => {
            let code = if m.code.is_empty() || m.code == "ok" { "fetch_failed" } else { &m.code };
            let what = format!("· {} · tried {} · {}ms spent ${:.4}", host_of(&m.url), m.tried(), m.ms, m.cost);
            // A newer copy that served is the move (`copies`), no count beside it
            let (detail, again, fix, named) = fetch::fail_row(m, others, &url, id, served.is_some());
            (owners, next) = (named, Some(served.map(|s| format!("{} {s}", bin())).or(fix).unwrap_or_default()));
            (code, what, detail + &again)
        }
        NoDoc::Missing if m.url.is_empty() => ("expired", "· its page and URL went after an hour".into(), "".into()),
        NoDoc::Missing => ("snapshot_missing", format!("· {url} · the cached body is gone"), url.clone()),
    };
    // a failed fetch's move (`fail_row`: none = a COUNT), else the table's fix
    let next = next.unwrap_or_else(|| fix(code, &at));
    let row = Flag { code: code.into(), what: detail, next, row: true }.bang(id) + &provider::pages(code, owners);
    let mut o = Out::fail(code, format!("= {eq} ! {code} {what}"), row);
    if let NoDoc::Zip(path) = why {
        o.set("kind", "zip").set("path", path.to_string_lossy().into_owned());
    }
    o
}

/// Local files, then URLs (cache or network); every URL's id is reserved in input order (`dry:true` names the same
/// ones), then every run is placed before any is waited for (`launch`: at most `batch_parallel` processes), all
/// under the one `wait:`; they share the `cost:` cap. A URL named twice is one run, its second row `(same as
/// above)`. Several inputs = one job: a `=` summary line, then each input's block; `json` = one record per input,
/// carrying `i`. Exit 0 when any input succeeded (the failures are `!` rows), else the worst code.
fn fetch_all(cmd: &Cmd, c: &mut Cache) -> Out {
    let (t0, mut spent) = (Instant::now(), 0.0);
    let want = |url| Want { url, ..Want::default() };
    let mut started: Vec<_> = cmd.files.iter().map(|f| Some(local(c, f, cmd).map(Err))).collect();
    let mut queued = Vec::new();
    for (i, u) in cmd.urls.iter().enumerate() {
        if cmd.urls[..i].contains(u) {
            started.push(None);
            continue;
        }
        let now = at_once(c, want(u), cmd).map(Err);
        let id = now.unwrap_or_else(|| reserve(c, Entry::default(), want(u), cmd).map(Ok).map_err(cache_write));
        if let Ok(Ok(n)) = id {
            queued.push((n, want(u), spent));
            spent += fetch::route(&plan_of(cmd, Some(u), spent)).1;
        }
        started.push(Some(id));
    }
    // a cache that takes no run file fails every input alike
    if let Err(e) = launch(c, &queued, cmd) {
        return cache_write(e);
    }
    let mut parts: Vec<Out> = Vec::new();
    for (i, (f, s)) in cmd.files.iter().chain(&cmd.urls).zip(started).enumerate() {
        // a URL named before: its first row's (`same`)
        let first = cmd.urls.iter().position(|u| u == f).and_then(|j| parts.get(cmd.files.len() + j));
        let mut o = match s {
            Some(s) => merged(s.and_then(|s| s.map_or_else(Ok, |n| finish(c, n, want(f), cmd)))),
            None => first.map_or_else(Out::default, same),
        };
        o.rec.entry("url").or_insert(f.as_str().into());
        o.rec.entry("ms").or_insert((t0.elapsed().as_millis() as u64).into());
        o.set("i", i);
        parts.push(o);
    }
    if parts.len() == 1 {
        return parts.pop().unwrap_or_default();
    }
    // the counts are the rows below: a `pending` row is no page in context yet (not `3 ok` over 2 pending)
    let pending = parts.iter().filter(|p| p.rec.contains_key("pending")).count();
    let ok = parts.iter().filter(|p| p.rc == 0).count() - pending;
    let mut fails = indexmap::IndexMap::<&str, usize>::new(); // first-seen order
    let errs =
        parts.iter().filter(|p| p.rc != 0).map(|p| p.rec.get("error").and_then(|v| v.as_str()).unwrap_or("failed"));
    errs.for_each(|e| *fails.entry(e).or_default() += 1);
    let pend = if pending > 0 { format!(" · {pending} pending") } else { String::new() };
    let fails: String = pend + &fails.iter().map(|(e, n)| format!(" · {n} ! {e}")).collect::<String>();
    let cost = parts.iter().filter_map(|p| p.rec.get("cost").and_then(|v| v.as_f64())).fold(0.0, |a, b| a + b);
    let (job, ms) = (c.job.map(|j| format!("j{j} ")).unwrap_or_default(), t0.elapsed().as_millis());
    let mut all = Out::new(vec![format!("= {job}{} inputs · {ok} ok{fails} · {ms}ms spent ${cost:.4}", parts.len())]);
    all.rc = if ok + pending > 0 { 0 } else { parts.iter().map(|p| p.rc).max().unwrap_or(1) };
    all.lines.extend(parts.iter().flat_map(|p| p.lines.iter().cloned()));
    all.set("id", c.job.map(|j| format!("j{j}")));
    all.parts = parts;
    all
}

/// A URL the batch named before: the first row's record and code under one line, never its cost again.
fn same(first: &Out) -> Out {
    let eq = first.lines.first().and_then(|l| l.split(' ').nth(1)).unwrap_or_default();
    let rec = Box::new(first.rec.iter().filter(|(k, _)| *k != "cost").map(|(k, v)| (k.clone(), v.clone())).collect());
    Out { lines: vec![format!("= {eq} (same as above)")], rc: first.rc, rec, parts: vec![] }
}

/// What a fetch asks for: the URL, the label of where it came from (`u3 fresh`, `u3 l4`), and the unfetched search
/// result id it fills (`slot`).
#[derive(Clone, Copy, Default)]
struct Want<'a> {
    url: &'a str,
    prov: Option<&'a str>,
    slot: Option<usize>,
}

/// A URL → a new id (or the unfetched search result `slot`): the cached snapshot (unless `fresh:true`), else the
/// router in a detached run (`resume`) that this call waits for up to `wait:`. A run still going
/// then answers `pending` and fills the id later; a failed fetch is stored too (meta and the best copy), so its id
/// explains what was tried.
fn fetch(c: &mut Cache, want: Want, cmd: &Cmd) -> Result<Out, Out> {
    let new = |c: &mut Cache| {
        let n = reserve(c, Entry::default(), want, cmd).map_err(cache_write)?;
        launch(c, &[(n, want, 0.0)], cmd).map_err(cache_write).and_then(|()| finish(c, n, want, cmd))
    };
    at_once(c, want, cmd).map_or_else(|| new(c), Ok)
}

/// Reserve `want`'s id now (a search result's own `slot`) as `entry` (its url and job set here), so it prints
/// while the run goes on. Its job: the search's for a result it fills, else this command's.
fn reserve(c: &mut Cache, entry: Entry, want: Want, cmd: &Cmd) -> Result<usize, String> {
    let job = want.slot.and_then(|n| c.st.result(n)).and_then(|e| e.job).map_or_else(|| c.job_of(&cmd.raw), Ok)?;
    c.alloc(vec![Entry { url: want.url.into(), job: Some(job), ..entry }], want.slot)
}

/// A URL's answer that needs no network: the cache, its run already going, a private host, offline.
fn at_once(c: &mut Cache, want: Want, cmd: &Cmd) -> Option<Out> {
    let url = want.url;
    let out = cached(c, url, want, cmd).or_else(|| running(c, url, &cmd.words));
    let out = out.or_else(|| provider::private_host(url).then(|| private(url)));
    out.or_else(|| settings::get().offline.value.then(|| offline(&short(url, 60))))
}

/// Run `n`'s answer: its page once it ended within `wait:`, else `pending`.
fn finish(c: &mut Cache, n: usize, want: Want, cmd: &Cmd) -> Result<Out, Out> {
    let Some(done) = wait_for(c, n, wait_of(cmd)) else {
        return Ok(pending(c, n, &cmd.words).unwrap_or_default());
    };
    if let Some(e) = done.fail {
        return Err(cache_write(e));
    }
    c.reload();
    let mut out = outlined(c, n, true, want.prov);
    out.warn(done.warn);
    if !done.ok && out.rc == 0 {
        // the best copy is shown, but the fetch failed: scripts and agents see it in rc and `error`
        out.rc = 1;
        out.set("error", done.code);
    }
    Ok(out)
}

/// How long a command waits for the network before it answers (`wait:` 0–300): a fetch then says `pending`,
/// a search, `gh`, `account`, `setup` give up the calls still running. 30 s: agents read `pending` as a failure
/// (polled 1-2×, then quit); 14/27 walled runs ended within 30 s, 24/27 within it + one `wait:30` poll.
pub const WAIT: u64 = 30;

pub fn wait_of(cmd: &Cmd) -> Duration {
    Duration::from_secs(cmd.get("wait").and_then(|v| v.parse().ok()).unwrap_or(WAIT))
}
/// How often the waiting call reads the run file.
pub const POLL: Duration = Duration::from_millis(20);

/// THE placement, a batch's and a search's prefetch alike: the reserved runs, in input order, dealt round-robin onto
/// at most `batch_parallel` lanes (50 URLs ran 50 processes at once, 608 MB), ONE process a lane, `fats --continue
/// j<J>.u<N>` on its first run (`detach`), which runs the others queued behind it after (`Detached::by`,
/// `resume`). No host rule: renders leave through the providers' own exits, and one run a host made 3 walled
/// pages of one site take 107 s, not 50.
/// A run = its reserved id, what it asks, what the command's earlier URLs may cost.
fn launch(c: &Cache, runs: &[(usize, Want, f64)], cmd: &Cmd) -> Result<(), String> {
    let cap = settings::limit::<usize>("batch_parallel").max(1);
    for (head, &(n, ..)) in runs.iter().enumerate().take(cap) {
        let job = c.st.u.get(n - 1).and_then(|e| e.job).unwrap_or_default();
        // the lane's lock before its run files: no reader sees one without it (`Cache::run`)
        let lock = c.run_claim(n).ok_or_else(|| format!("{}: not locked", c.run_file(n, "lock").display()))?;
        for (k, &(id, w, spent)) in runs.iter().skip(head).step_by(cap).enumerate() {
            let (by, status) = if k == 0 { (0, String::new()) } else { (n, format!("queued after u{n}")) };
            let (url, line, prov) = (w.url.into(), cmd.raw.clone(), w.prov.map(String::from));
            c.run_put(id, &Detached { url, cmd: line, prov, job, spent, by, status, t: now(), ..<_>::default() })?;
        }
        detach(format!("j{job}.u{n}"), &c.run_file(n, "log"), Some(lock))?;
    }
    Ok(())
}

/// `fats --continue <id>` in its own process (own process group, stdio to `log`, `lock` its stdin: held while it
/// lives); in-process on a thread holding `lock` when a test's stand-in transport answers.
fn detach(id: String, log: &std::path::Path, lock: Option<std::fs::File>) -> Result<(), String> {
    // and its folder: a search may be the session's first command (a fetch's `run_put` made it)
    let made = log.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::File::create(log));
    let log = made.map_err(|e| e.to_string())?;
    if provider::in_process() {
        std::thread::spawn(move || (resume(&id), lock));
        return Ok(());
    }
    let mut p = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    p.args(["--continue", &id]).stdin(lock.map_or_else(std::process::Stdio::null, std::process::Stdio::from));
    p.stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut p, 0);
    let mut child = p.spawn().map_err(|e| format!("fats --continue {id}: {e}"))?;
    // reaped when it ends while this process still runs (`fats mcp`); else the system reaps the orphan
    std::thread::spawn(move || child.wait().ok());
    Ok(())
}

/// Run `n` as it ended, waiting while it lives (its lock held: a run that has not started yet answers `starting`)
/// until `wait` after the COMMAND started (`Cache::born`), never `wait` per input: three walled URLs waited
/// 3 × 30 s, past an MCP host's 60 s call limit. None = no such run, still running, or abandoned.
fn wait_for(c: &Cache, n: usize, wait: Duration) -> Option<Done> {
    poll_until(c.born, wait, || c.run(n).is_none_or(|(r, alive)| r.done.is_some() || !alive));
    c.run(n)?.0.done
}

/// Sleep until `done` says so or `wait` after the command started (`born`): every wait of a command is one deadline.
fn poll_until(born: Instant, wait: Duration, done: impl Fn() -> bool) {
    while !done() && born.elapsed() < wait {
        std::thread::sleep(POLL);
    }
}

/// A pending run's next move, `fats u3 wait:30 (free, usually ready in ~11s)`: the median time of this session's
/// calls on its route (its host, the tiers it waits for: `bd running`) from the newest one's start, never past the
/// running calls' own deadline (`Plan::timeout_of` as they started: `timeout:1s`), less the time now, rounded
/// once; from < 3 such calls or past it no number, `(free; check again in 30 s)`: a wrong number is worse than none
/// (one constant promised 42 s for a 0.6 s plain prefetch; any host's bd calls promised 28 s, it took 60.5 s).
fn ready(c: &Cache, n: usize, run: &Detached, words: &[String]) -> String {
    let words = words.iter().map(|w| format!(" {w}")).collect::<String>();
    // a tier the run waits for (`bd running · oxyjs running`): `oxy running` is no part of `oxyjs running`
    let on = |h: &HopRow| run.status.contains(&format!("{} running", provider::short(&h.provider)));
    let metas = c.st.u.iter().filter(|e| !e.alias && host_of(&e.url) == host_of(&run.url));
    let mut ms: Vec<u64> = metas.filter_map(|e| c.meta(&e.dir)).flat_map(|m| m.hops).filter(on).map(|h| h.ms).collect();
    ms.sort_unstable();
    let end = ms.get(ms.len() / 2).filter(|_| ms.len() >= 3).map(|p| (run.since + *p as f64 / 1e3).min(run.until));
    let left = end.map_or(0.0, |e| (e - now()).round().max(0.0)) as u64;
    poll_next(&format!("u{n}{words}"), left)
}

/// THE pending move, a page's and a search's alike: `fats u3 wait:30 (free, usually ready in ~11s)`, or
/// `(free; check again in 30 s)` with no time to promise (`left` 0).
fn poll_next(id: &str, left: u64) -> String {
    let poll = settings::limit::<u64>("poll_wait");
    let eta = if left == 0 { format!("; check again in {poll} s") } else { format!(", usually ready in ~{left}s") };
    format!("{} {id} wait:{poll} (free{eta})", bin())
}

/// An id whose detached run has not ended, as the answer: `= j5.u3 pending 31s ex.com · bd running → fats u3 <words>
/// wait:30 (free, usually ready in ~11s)`, or, when its process died, its failed copy (`Detached::cut`, `! abandoned`,
/// the read the sweep's stored copy gives). None = no such run, or it ended (the page is stored).
fn pending(c: &Cache, n: usize, words: &[String]) -> Option<Out> {
    let (run, alive) = c.run(n).filter(|x| x.0.done.is_none())?;
    let (id, age, host) = (format!("u{n}"), (now() - run.t).max(0.0).round() as u64, host_of(&run.url));
    let eq = format!("j{}.u{n}", run.job);
    if alive {
        let status = if run.status.is_empty() { "starting" } else { &run.status };
        return Some(waiting(format!("= {eq} pending {age}s {host} · {status} → {}", ready(c, n, &run, words)), id));
    }
    Some(no_doc(&id, &eq, &run.cut(), &[], &NoDoc::Missing, None))
}

/// THE pending answer, a page's and a search's: its one row, its id, `pending` in the record.
fn waiting(row: String, id: String) -> Out {
    Out::new(vec![row]).with("id", id).with("pending", true)
}

/// The live run already fetching `url` in this session, as its `pending` answer: one fetch per URL in flight.
fn running(c: &Cache, url: &str, words: &[String]) -> Option<Out> {
    c.runs().into_iter().filter(|r| r.1.url == url).find_map(|r| pending(c, r.0, words).filter(|o| o.rc == 0))
}

/// `fats --continue j5.u3`: the detached half of a fetch. Runs the router for the run `fetch` reserved and stores
/// the page in that id, whether or not the call that started it still waits. Exit code for `main`.
pub fn resume(id: &str) -> i32 {
    let (mut c, cmd) = (Cache::open(), parse(&[id.to_string()]));
    // a detached search (`searching`): its own command, past every lead's `search_s` (`wait:0`): no `wait:`, so
    // `search_of` plans the one call at the retry's time (`one_call`), never cut
    if let (Some(&j), None) = (cmd.jobs.first(), cmd.ids.first()) {
        let Some(job) = c.job(j) else { return 2 };
        c.job = Some(j);
        let rc = web(&parse(&[format!("{} wait:0", job.cmd)]), &mut c).map_or(1, |o| o.rc);
        return rc.max(i32::from(c.save().is_err()));
    }
    let Some(n) = cmd.ids.first().map(|i| i.u) else { return 2 };
    // its lane: this run, then the ones queued behind it (`launch`), one after another; the worst exit code
    c.runs().into_iter().filter(|r| r.0 == n || r.1.by == n).map(|r| run_one(&mut c, r.0)).max().unwrap_or(1)
}

/// Run `n` of a lane, its result in its run file. Exit code.
fn run_one(c: &mut Cache, n: usize) -> i32 {
    // one run per id: its launcher's lock is this process's stdin; a run another process claimed (pid) stops
    let Some((mut r, _)) = c.run(n).filter(|x| x.0.done.is_none() && x.0.pid == 0) else { return 1 };
    r.pid = std::process::id();
    if c.run_put(n, &r).is_err() {
        return 1;
    }
    let w = Cache::at(c.root.clone(), c.session.clone());
    let seen = |(status, since, until)| {
        w.run_put(n, &Detached { status, since, until, ..r.clone() }).ok(); // progress only: the run goes on
    };
    let done = routed(c, &r, n, &seen);
    r.done = Some(done);
    i32::from(w.run_put(n, &r).is_err())
}

/// The router for run `n`, its page stored in `u<n>`: how it ended.
fn routed(c: &mut Cache, run: &Detached, n: usize, seen: fetch::Seen) -> Done {
    let (cmd, url) = (parse(std::slice::from_ref(&run.cmd)), run.url.as_str());
    let mut p = plan_of(&cmd, Some(url), run.spent);
    fetch::route_of(c, url, &mut p);
    p.prior = c.lookup(url).and_then(|(dir, _)| c.dir(&Entry { dir, ..Entry::default() }));
    let t0 = Instant::now();
    let ask = Ask {
        url: url.into(),
        country: cmd.get("country").map(String::from),
        mobile: cmd.get("device") == Some("mobile"),
        max_bytes: cap_of(&cmd),
        ..Ask::default()
    };
    let mut hops = fetch::run(&p, &ask, seen);
    let mut got = fetch::outcome(url, &mut hops, t0.elapsed().as_millis() as u64);
    let served = Tier::parse(&got.meta.provider);
    got.meta.country = served.and_then(|t| provider::sent(&ask, t)).unwrap_or_default();
    // a paid ask ran one provider at a time: `stats` and the `=` line say `hedge:off` (`Meta::hedge_off`)
    got.meta.hedge = if p.hedged() { "" } else { "off" }.into();
    let (ok, code, cost, id) = (got.meta.ok, got.meta.code.clone(), got.meta.cost, format!("u{n}"));
    got.meta.extra.insert("skipped".into(), fetch::skipped(&p, &hops, "", &id, url).into());
    let file = got.file.as_ref().map(|(name, body)| (name.as_str(), body.as_slice()));
    let (index, error, doc) = (ok.then_some(url), got.error.as_deref(), got.doc.as_ref());
    let snap = Snap { file, error, doc, meta: got.meta, index, slot: Some(n) };
    if let Err(e) = c.ingest(snap, run.job, t0) {
        return Done { ok, code, cost, fail: Some(e), ..Done::default() };
    }
    Done { ok, code, cost, warn: fetch::ignored(&ask, &hops, &id).into_iter().collect(), fail: None }
}
/// The snapshot indexed under `key` (unless `fresh:true`) as a new id of `url` that points at it: no second
/// copy, no cost. None = not cached.
fn cached(c: &mut Cache, key: &str, want: Want, cmd: &Cmd) -> Option<Out> {
    let (dir, _) = c.lookup(key).filter(|_| !cmd.fresh())?;
    let n = reserve(c, Entry { dir, alias: true, ..Entry::default() }, want, cmd);
    Some(n.map_or_else(cache_write, |n| outlined(c, n, false, want.prov)))
}

/// A local file (or stdin, `-`) → a new id: the stored snapshot when the same bytes came from the same place
/// before, else converted and stored.
fn local(c: &mut Cache, f: &str, cmd: &Cmd) -> Result<Out, Out> {
    let t0 = Instant::now();
    let path = if let Some(r) = f.strip_prefix("~/") { dirs::home_dir().unwrap_or_default().join(r) } else { f.into() };
    let bytes = read_local(f, &path, cap_of(cmd))?;
    let url = if f == "-" {
        "stdin:".to_string()
    } else {
        let abs = std::fs::canonicalize(&path).unwrap_or(path);
        format!("file://{}", abs.to_string_lossy())
    };
    // the same bytes from the same place again: the stored snapshot (read without a second parse)
    let key = blake3::Hasher::new().update(url.as_bytes()).update(&[0]).update(&bytes).finalize();
    let key = format!("blake3:{}", key.to_hex());
    if let Some(out) = cached(c, &key, Want { url: &url, ..Want::default() }, cmd) {
        return Ok(out);
    }
    let (bytes, charset) = provider::decode_body(bytes, None);
    let kind = crate::clean::kind_of("", &bytes);
    if kind == "zip" {
        let next = text_of_zip(f);
        let mut o =
            Out::new(vec![format!("= ! not_in_v1 zip text: {f} · not in this build (pages, files, cache) → {next}")]);
        o.rc = 1;
        o.set("error", "not_in_v1").set("hint", next).set("what", "zip");
        return Ok(o);
    }
    // provider `local`, one free hop, never a render
    let meta = Meta {
        url,
        provider: "local".into(),
        ok: true,
        code: "ok".into(),
        t: now(),
        bytes: bytes.len() as u64,
        kind: kind.into(),
        render: Some(false),
        charset: charset.into(),
        hops: vec![HopRow { provider: "local".into(), code: "ok".into(), ..Default::default() }],
        ..Meta::default()
    };
    // a converter crash is the answer here (nothing stored, no job spent)
    let doc = crate::cache::build(&bytes, &meta.url, &meta.url, meta.render == Some(true), kind)
        .map_err(|e| no_doc(f, f, &meta, &[], &NoDoc::Parse(e), None))?;
    let job = c.job_of(&cmd.raw).map_err(cache_write)?;
    let name = crate::cache::raw_name(kind);
    let snap = Snap { file: Some((&name, &bytes)), error: None, doc: Some(&doc), meta, index: Some(&key), slot: None };
    let n = c.ingest(snap, job, t0).map_err(cache_write)?;
    Ok(outlined(c, n, true, None))
}

/// A page just stored as `u<n>`, as its outline (`new` = fetched by this command, else a cache hit).
fn outlined(c: &mut Cache, n: usize, new: bool, prov: Option<&str>) -> Out {
    page_cmd(c, &Id { u: n, ..Id::default() }, new, |p| Ok(outline::outline(p, prov))).unwrap_or_default()
}

/// A local file's (or stdin's) bytes, up to `cap`. Err = the answer: fenced, unreadable, or over the cap.
fn read_local(f: &str, path: &Path, cap: u64) -> Result<Vec<u8>, Out> {
    if let Some(why) = fenced(f, path) {
        return Err(refused("not_allowed", f, &format!("{f} · {why}"), &At::default()));
    }
    let mut v = Vec::new();
    let read = match f {
        "-" => std::io::stdin().take(cap + 1).read_to_end(&mut v),
        _ => std::fs::File::open(path).and_then(|h| h.take(cap + 1).read_to_end(&mut v)),
    };
    if let Err(e) = read {
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let at = At { arg: &dir.to_string_lossy(), ..At::default() };
        return Err(refused("no_file", f, &format!("{f} · {e}"), &at));
    }
    if v.len() as u64 > cap {
        let what = format!("over the {} input cap (ceiling {}) · nothing stored", mib(cap), mib(MAX_BYTES_CEIL));
        let at = At { url: f, done: &["max_bytes:64m"][..usize::from(cap >= MAX_BYTES_CEIL)], ..At::default() };
        return Err(refused("too_big", f, &format!("{f} · {what}"), &at));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(s: &str) -> Cmd {
        parse(&s.split(' ').map(String::from).collect::<Vec<_>>())
    }

    /// `more` beside a list (the old hint syntax) is named, never a silent page 1 (review 2026-10-14 #18).
    #[test]
    fn more_beside_a_list_is_named() {
        let b = bin();
        let row = |w: &str| format!("! ignored more · lists page by from:K → {b} {w} from:K");
        assert_eq!(ignored(&cmd("u1 links more")), [row("u1 links")]);
        assert_eq!(ignored(&cmd("u2#3 media more")), [row("u2 media")]);
        assert_eq!(ignored(&cmd("links more")), [row("links")]);
        assert!(ignored(&cmd("u1 more")).is_empty() && ignored(&cmd("u1 links from:3")).is_empty());
    }

    /// A timed-out search's fix gives its call more time than it had, never the same paid cut again.
    #[test]
    fn a_search_timeout_fix_adds_time() {
        let row = |why: &str, ms: u64| {
            let hops = vec![HopRow {
                provider: "brightdata".into(),
                code: "provider_timeout".into(),
                ms,
                why: why.into(),
                ..Default::default()
            }];
            let m = Meta { url: "search:rust".into(), code: "provider_timeout".into(), hops, ..Meta::default() };
            let out = search_out(&Cache::at(PathBuf::new(), "t".into()), &parse(&["rust".into()]), 1, &m)
                .unwrap_or_else(|o| o);
            out.lines.join("\n")
        };
        let b = bin();
        let row15 = row("", 15_000);
        assert!(row15.ends_with(&format!("! provider_timeout → {b} rust fresh:true timeout:30 ≤$0.0025")), "{row15}");
        assert!(row("", 30_000).ends_with(&format!("! provider_timeout → {b} rust fresh:true ≤$0.0025")), "ran 30 s");
        let wait = row("wait:5s", 5_000);
        assert!(
            wait.ends_with(&format!("! provider_timeout bd wait:5s → {b} rust fresh:true wait:30 ≤$0.0025")),
            "{wait}"
        );
    }

    /// The four answers for a snapshot without a doc, each `= {eq} ! {code} …` + the `!` row with the table fix
    /// (and the provider's page for a key code): a zip body, a converter crash, a failed fetch with no body kept,
    /// a body that is gone.
    #[test]
    fn no_doc_answers() {
        let b = bin();
        let hop = |code: &str| HopRow { provider: "oxylabs".into(), code: code.into(), ..Default::default() };
        let m = Meta { url: "https://ex.com/a".into(), ok: true, bytes: 2048, ..Meta::default() };
        let lines = |why: &NoDoc, m: &Meta| no_doc("u3", "j2.u3", m, &[], why, None).lines.join("\n");
        let zip = lines(&NoDoc::Zip("/c/u3/raw.zip".into()), &m);
        assert_eq!(
            zip,
            format!(
                "= j2.u3 ! not_in_v1 zip (docx/xlsx) · ex.com/a · 2.0kB cached, no zip text in this build\n\
                 u3 ! not_in_v1 zip ex.com/a → unzip -p /c/u3/raw.zip '*.xml' | {b} -"
            )
        );
        let o = no_doc("u3", "u3", &m, &[], &NoDoc::Zip("/c/u3/raw.zip".into()), None);
        assert_eq!((o.rc, o.rec["kind"].as_str(), o.rec["path"].as_str()), (1, Some("zip"), Some("/c/u3/raw.zip")));
        assert_eq!(
            lines(&NoDoc::Parse("pdf: bad xref".into()), &m),
            format!(
                "= j2.u3 ! parse_failed · ex.com/a · pdf: bad xref\nu3 ! parse_failed ex.com/a → {b} config · report the URL"
            )
        );
        assert_eq!(
            lines(&NoDoc::Missing, &m),
            format!(
                "= j2.u3 ! snapshot_missing · ex.com/a · the cached body is gone\nu3 ! snapshot_missing ex.com/a → {}",
                crate::lang::priced(&format!("{b} https://ex.com/a fresh:true"), "https://ex.com/a")
            )
        );
        const K: &str = " · oxy https://dashboard.oxylabs.io (a Web API instance's key)";
        let failed =
            Meta { ok: false, code: "bad_key".into(), ms: 700, cost: 0.001, hops: vec![hop("bad_key")], ..m.clone() };
        assert_eq!(
            lines(&NoDoc::Missing, &failed),
            format!(
                "= j2.u3 ! bad_key · ex.com · tried oxy:bad_key✗ · 700ms spent $0.0010\nu3 ! bad_key oxy → {b} setup{K}"
            )
        );
        let timed = Meta { code: "provider_timeout".into(), hops: vec![hop("provider_timeout")], ..failed.clone() };
        let t = lines(&NoDoc::Missing, &timed);
        assert!(t.ends_with(&format!("\nu3 ! provider_timeout ex.com/a → {b} u3 fresh:true js:true ≤$0.0030")), "{t}");
        let gone = Meta { code: String::new(), hops: vec![], ..failed };
        assert!(lines(&NoDoc::Missing, &gone).starts_with("= j2.u3 ! fetch_failed · ex.com · tried - · "));
    }

    /// THE liveness rule (`Cache::run`): a run lives while its lock is held, the launcher's before the process
    /// starts, however long that takes (a 2 s window printed `! abandoned` under load); `pending`, status and
    /// `fats jN` read the same verdict.
    #[test]
    fn a_run_lives_while_its_lock_is_held() {
        let root = std::env::temp_dir().join(format!("fats-start-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let mut c = Cache::at(root.clone(), "t".into());
        let job = c.job_of("https://ex.com/a").expect("job");
        c.alloc(vec![Entry { url: "https://ex.com/a".into(), job: Some(job), ..Entry::default() }], None).expect("id");
        let lock = c.run_claim(1).expect("the launcher's lock");
        c.run_put(1, &Detached { url: "https://ex.com/a".into(), job: 1, t: now() - 60.0, ..Detached::default() })
            .expect("run file");
        let read = || (c.run(1).map(|r| r.1), pending(&c, 1, &[]).expect("a run").lines.join("\n"));
        let (alive, out) = read();
        assert!(alive == Some(true) && out.contains(" pending 60s ex.com · starting → "), "pid 0 at 60 s: {out}");
        let row = |c: &Cache| job_view(c, job).lines.get(1).cloned().unwrap_or_default();
        assert!(row(&c).starts_with("j1.u1 pending ex.com/a → "), "{}", row(&c));
        drop(lock);
        let (alive, out) = read();
        assert!(alive == Some(false) && out.contains(" ! abandoned ex.com"), "no lock: {out}");
        assert!(row(&c).starts_with("j1.u1 ! abandoned ex.com/a → "), "{}", row(&c));
        std::fs::remove_dir_all(&root).ok();
    }

    /// A session holding one plain id, then a saved Oxylabs answer listed as a search: (cache, its root, the hits,
    /// the search record, the listed output). No network: the provider call is the only part not run.
    fn listed_session(tag: &str) -> (Cache, PathBuf, Vec<Hit>, Meta, Out) {
        let root = std::env::temp_dir().join(format!("fats-serp-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let mut cache = Cache::at(root.clone(), "t".into());
        let old = Entry { url: "https://ex.com/".into(), dir: "t/u1".into(), ..Entry::default() };
        assert_eq!(cache.alloc(vec![old], None), Ok(1));
        let body = include_bytes!("../tests/fixtures/oxy-search.json");
        let hits = provider::hits(Tier::Oxy, body, 10).expect("fixture");
        let job = cache.job_of("heidelberg university").expect("job");
        let meta = Meta {
            url: "search:heidelberg university".into(),
            provider: "oxylabs".into(),
            ms: 1630,
            cost: 0.001,
            kind: "serp".into(),
            t: now(),
            ..Default::default()
        };
        let out = merged(listed(&mut cache, job, meta.clone(), &hits));
        (cache, root, hits, meta, out)
    }

    /// The list shape, one unfetched id per result after the ids already in the session, and `u<N> dry:true`
    /// resolving the result's URL.
    #[test]
    fn search_results_become_ids() {
        let (cache, root, _, _, out) = listed_session("ids");
        let b = bin();
        // the list cap is 300 real tokens (`clean::tok`): four of the five snippet rows, then `more`
        let first =
            "= j1 search \"heidelberg university\" · 5 results · 4 shown u2-u5 · oxylabs · 1.6s · spent $0.0010";
        assert_eq!(out.lines[0], first);
        assert_eq!(
            out.lines[1],
            "u2  uni-heidelberg.de/en  Heidelberg University — \"Heidelberg University – the oldest in Germany, recognized globally. \
             A leader in science, …\""
        );
        let wiki = "u4  en.wikipedia.org/wiki/Heidelberg_Univer…  Heidelberg University - Wikipedia";
        assert!(out.lines[3].starts_with(wiki));
        let last = format!("open one: {b} u2 · filter: {b} u2 <words> · more: {b} more");
        assert_eq!(out.lines.last().map(String::as_str), Some(&*last));
        assert_eq!((out.lines.len(), out.parts.len(), out.rc), (6, 5, 0));
        assert_eq!(out.parts[0].rec["url"], "https://www.uni-heidelberg.de/en");
        assert_eq!((out.parts[0].rec["cost"].as_f64(), out.parts[1].rec["cost"].as_f64()), (Some(0.001), Some(0.0)));
        let unfetched = |n| cache.st.result(n).filter(|e| e.dir.is_empty()).cloned();
        let entry = unfetched(2).expect("u2 is a search result");
        assert_eq!((entry.url.as_str(), entry.job), ("https://www.uni-heidelberg.de/en", cache.job));
        assert!(unfetched(1).is_none() && unfetched(7).is_none());
        let dry = plan(&cmd("u2 dry:true"), &cache).lines.join("\n");
        assert!(dry.contains("u2 uni-heidelberg.de/en (search result)") && !dry.contains("local read"), "{dry}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// 20 results: the list stops at the cap, `more` goes on from the cursor, `more` at the end says so.
    #[test]
    fn more_lists_past_the_cap() {
        let (mut cache, root, hits, meta, _) = listed_session("more");
        let b = bin();
        let many: Vec<Hit> = (0..4).flat_map(|_| hits.clone()).collect();
        let job = cache.job_of("again").expect("job");
        let out = merged(listed(&mut cache, job, meta, &many));
        let rows = out.lines.iter().filter(|l| l.starts_with('u')).count();
        assert!((3..20).contains(&rows), "{:?}", out.lines);
        assert!(out.lines.last().is_some_and(|l| l.ends_with(&format!("more: {b} more"))));
        assert_eq!(cache.st.cursor.as_ref().and_then(|k| k.serp), Some((job, rows)));
        let more = dispatch(&cmd("more"), &mut cache);
        assert!(more.lines[0].starts_with(&format!("= j{job} search ")), "{:?}", more.lines);
        assert!(more.lines[0].contains(&format!(" shown u{}-", 7 + rows)), "{:?}", more.lines);
        assert!(more.lines[1].starts_with(&format!("u{}  ", 7 + rows)), "{:?}", more.lines);
        std::fs::remove_dir_all(&root).ok();
    }
}
