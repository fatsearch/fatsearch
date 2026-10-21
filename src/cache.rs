//! CACHE: root, session, ids, cursor, URL index, global jobs, page snapshots.
//! ```text
//!   <root>/index.json                url → {dir, t}          (shared, flock)
//!   <root>/jobs.json                 [{session, cmd, t}]     j<N> = N-th row, global monotonic (flock)
//!   <root>/account.json              last `fats account` read: {t, rows: [Account]} (parsed numbers, no answers)
//!   <root>/swept                     its mtime = the last sweep of every session (`sweep`)
//!   <root>/pace/<host>               the home-IP brake: the last plain start to that host (`provider::pace`)
//!   <root>/telemetry/                `ping/` day and week stamps · `days/` day marks · `week/` the weekly tally
//!                                    (`bump`) · `latest.json` · `sent.jsonl` (`telemetry::at`); not in `usage`
//!   <root>/<session>/state.json      {u: [{url, dir, alias?, job, serp?}], cursor}
//!   <root>/<session>/u<N>/           raw.html · meta.json · doc.json (rebuilt from raw.html when stale)
//!                                    · error.txt (0600): the providers' error answers, masked, never printed
//!   <root>/<session>/u<N>/assets/    m<K>.<ext> + m<K>.json: picture K of the page, fetched on demand (`Asset`)
//!   <root>/<session>/j<N>/           meta.json of web search j<N> (hits inside) · error.txt; its results are `serp`
//!                                    entries with no dir until a command fetches them into the same id
//!   <root>/<session>/run/u<N>.*      a fetch that outlives its call (`Detached`): .json the run, .lock held by
//!                                    its process, .log that process's stdout + stderr
//!   <root>/<session>/run/setup-<p>.* the key form for provider p (`setup::run_file`): .json token, port, deadline;
//!                                    .log its output
//! ```

use crate::clean::{DOC_V, Doc, build_doc};
use crate::settings::limit;
use chrono::Datelike;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn jload<T: DeserializeOwned>(p: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(p).ok()?).ok()
}

/// A failed run's provider answers (`fetch::error_text`): `stats` points at the file, nothing prints it.
pub const ERROR_FILE: &str = "error.txt";

/// THE spend ledger, in the key dir (never swept, so `stats` still sums what was spent once the pages are gone):
/// one JSON line `[unix s, [cost, unverified], "<dir>"]` per stored fetch or search record that cost or may have,
/// and per dead run's calls in flight once its `run/` is swept; `dir` = the record's path (`<root>/default/u3`).
/// No URL, no query. A row it cannot write is never a failed fetch: `stats` probes the file and says why.
pub const SPEND_FILE: &str = "spend.log";

fn spend_log(paid: [f64; 2], dir: &Path) {
    let row = serde_json::json!([now(), paid, dir.to_string_lossy()]).to_string() + "\n";
    let keys = &crate::settings::get().key_dir.value;
    (paid[0] + paid[1] > 0.0).then(|| crate::settings::save_in(keys, SPEND_FILE, row.as_bytes(), true).ok());
}

/// Write `ERROR_FILE` in `dir`, readable by its owner only.
fn keep_error(dir: &Path, body: &[u8]) -> Result<(), String> {
    crate::settings::save_in(dir, ERROR_FILE, body, false).map_err(|e| format!("{}/{ERROR_FILE}: {e}", dir.display()))
}

/// The numbers in `dir`'s entries named `<prefix><N><suffix>` (`u3.json`, `j4`), smallest first; none when `dir`
/// cannot be read.
fn numbered(dir: &Path, prefix: char, suffix: &str) -> Vec<usize> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let name = |e: fs::DirEntry| e.file_name().to_str()?.strip_prefix(prefix)?.strip_suffix(suffix)?.parse().ok();
    let mut v: Vec<usize> = rd.flatten().filter_map(name).collect();
    v.sort_unstable();
    v
}

/// Atomic write: tmp + rename.
pub fn jsave<T: Serialize>(p: &Path, v: &T) -> Result<(), String> {
    let dir = p.parent().ok_or("no parent dir")?;
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // pid + a per-process counter: two threads (MCP) never share a temp file
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = p.with_extension(format!("{}.{n}.tmp", std::process::id()));
    let body = serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?;
    fs::write(&tmp, body).and_then(|_| fs::rename(&tmp, p)).map_err(|e| format!("{}: {e}", p.display()))
}

/// Read-modify-write a shared root file under an exclusive lock.
pub fn edit<T: DeserializeOwned + Serialize + Default, R>(p: &Path, f: impl FnOnce(&mut T) -> R) -> Result<R, String> {
    edit_when(p, File::lock, f)
}

/// `edit` once `lock` takes the lock: `File::lock` waits for it, a `try_lock` gives up while another holds it.
fn edit_when<T: DeserializeOwned + Serialize + Default, R>(
    p: &Path,
    lock: impl Fn(&File) -> std::io::Result<()>,
    f: impl FnOnce(&mut T) -> R,
) -> Result<R, String> {
    fs::create_dir_all(p.parent().ok_or("no parent dir")?).map_err(|e| format!("{}: {e}", p.display()))?;
    let file = File::create(p.with_extension("lock")).map_err(|e| e.to_string())?;
    lock(&file).map_err(|e| e.to_string())?;
    let mut v: T = jload(p).unwrap_or_default();
    let r = f(&mut v);
    jsave(p, &v)?;
    Ok(r)
}

#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Cursor {
    pub u: usize,
    pub k: Option<usize>,
    pub part: usize,
    pub full: bool,
    /// `more` continues the result list of this search: (job, next hit, 0-based).
    pub serp: Option<(usize, usize)>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Entry {
    pub url: String,
    pub dir: String,
    pub alias: bool,
    pub job: Option<usize>,
    /// A web search result; an empty `dir` = not fetched yet.
    pub serp: bool,
    /// The parts of it read in this session (`#5`, `#5b`) and their text's keys (`=1f…`, read under any copy of the
    /// URL): a next move never points back at one.
    pub read: Vec<String>,
}

impl Entry {
    /// Page `u`'s `=` line id read in its own session: `j3.u2` (the job that fetched it), else `u2`.
    pub fn eq(&self, u: usize) -> String {
        self.job.map_or_else(|| format!("u{u}"), |j| format!("j{j}.u{u}"))
    }
}

/// Put `e` in slot `n` when it is a reserved id (`State::result`; keeping its job and `serp`), else append it:
/// its 1-based u number.
fn place(st: &mut State, mut e: Entry, slot: Option<usize>) -> usize {
    match slot.filter(|&n| st.result(n).is_some()).and_then(|n| Some((n, st.u.get_mut(n - 1)?))) {
        Some((n, old)) => {
            (e.job, e.serp) = (old.job, old.serp);
            *old = e;
            n
        }
        None => {
            st.u.push(e);
            st.u.len()
        }
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct State {
    pub u: Vec<Entry>,
    pub cursor: Option<Cursor>,
    /// When this session began (unix s, its first state write, `with_state`, or search record, `serp_put`): `stats`
    /// counts the ledger rows since; none (never written) counts none.
    pub t: Option<f64>,
}

impl State {
    /// THE slot check: the entry of `u<n>` when a fetch may fill it: a search result, fetched or not, or an id
    /// reserved for a detached fetch (an empty `dir` = not yet). Its first fetch fills it, `fresh:true` refills a
    /// search result.
    pub fn result(&self, n: usize) -> Option<&Entry> {
        self.u.get(n.checked_sub(1)?).filter(|e| e.serp || e.dir.is_empty())
    }
}

/// A fetch that outlives its call (past `wait:`), `run/u<N>.json`: written by the call that reserved `u<N>`, then
/// by the process that runs it (`fats --continue j<J>.u<N>`): its pid once it holds `u<N>.lock`, `status` as tiers
/// start and land, `done` at the end. The lock is held for that process's life: no `done` + a free lock = it died.
/// Any other writer writes under that lock, read again there (`run_tell`).
#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)] // a run file an older binary wrote reads too
pub struct Detached {
    pub url: String,
    /// The command line it came from (its mods: `country:`, `js:`, `cost:` …).
    pub cmd: String,
    /// The `← u3 l4` label of the page it fills.
    pub prov: Option<String>,
    pub job: usize,
    /// Its batch's `cost:` pool (`fetch::pooled`); None = a run alone.
    pub pool: Option<PathBuf>,
    /// Queued behind run `by` of its lane: that run's process takes it after its own (`cli::launch`); 0 = none.
    pub by: usize,
    pub t: f64,
    pub pid: u32,
    /// The tiers so far: `plain:blocked_403✗ · oxy running`.
    pub status: String,
    /// When the newest running call started and when the last one is cut (epoch s; 0 = none yet): `cli::ready`.
    pub since: f64,
    pub until: f64,
    pub done: Option<Done>,
    /// Its end was shown (`cli::footer`).
    pub told: bool,
}

impl Detached {
    /// THE dead-run rule (`stats`, `sweep`): no lock held (`alive`, `Cache::run`), never ended, and its id holds
    /// no page (a store that died before its `done` already wrote its ledger row).
    pub fn dead(&self, alive: bool, id: Option<&Entry>) -> bool {
        !alive && self.done.is_none() && id.is_some_and(|e| e.dir.is_empty())
    }

    /// A dead run as its id's failed copy: code `abandoned`, its calls in flight (`bd running` in its status) each a
    /// call we cut (`bd:abandoned✗≤$0.0015 (unverified)`: no status came back). Its read and `stats` row before the
    /// sweep, stored in the id by it.
    pub fn cut(&self) -> Meta {
        let hop = |t: &crate::provider::Spec| HopRow { provider: t.name.into(), upto: t.price, ..HopRow::default() };
        let tier = |s: &str| crate::provider::Tier::parse(s.strip_suffix(" running")?).map(|t| hop(t.spec()));
        let hops = self.status.split(" · ").filter_map(tier).map(|h| HopRow { code: "abandoned".into(), ..h });
        let hops: Vec<_> = hops.collect();
        let provider = hops.last().map(|h| h.provider.clone()).unwrap_or_default();
        Meta { url: self.url.clone(), code: "abandoned".into(), t: self.t, provider, hops, ..Meta::default() }
    }
}

/// How a detached fetch ended: what the call that waits for it adds to the stored page's outline.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Done {
    pub ok: bool,
    pub code: String,
    pub cost: f64,
    /// Its `!` rows (tiers skipped, mods ignored, a route not kept).
    pub warn: Vec<String>,
    /// The page could not be stored: the cache write error.
    pub fail: Option<String>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
pub struct Meta {
    pub url: String,
    pub final_url: Option<String>,
    /// Where a bd ask landed by the page's same-site canonical link (`fetch::canonical`): `outline::lost_page`
    /// judges it; the links' base stays `final_url`.
    pub landed: Option<String>,
    /// The page whose meta refresh plain followed (`provider::refresh`): the outline's `uN#0 info | refreshed
    /// from: <url>` row, never judged; `lost_page` judges the target (`final_url`) as any landing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refreshed_from: Option<String>,
    pub provider: String,
    pub ms: u64,
    pub cost: f64,
    pub ok: bool,
    pub code: String,
    pub t: f64,
    pub bytes: u64,
    pub kind: String,
    pub hops: Vec<HopRow>,
    /// The body is a browser render (its `<noscript>` was never shown, so the cleaner drops it). Set by
    /// whoever stored the body.
    pub render: Option<bool>,
    /// The exit country the serving provider was asked for (`FR`); empty = none.
    pub country: String,
    /// The encoding the body was decoded from before it was stored as UTF-8 (`windows-1250`); empty = UTF-8.
    pub charset: String,
    /// `off` = a paid ask, one provider at a time (`fetch::Plan::hedged`), said on every read; empty = hedged.
    pub hedge: String,
    /// Keys this build does not know survive a rewrite.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One provider call in a fetch's or search's `hops`, as meta.json keeps it: `fetch::Hop::row` writes it, every
/// view reads its fields. `provider` is the stored name (`brightdata`, `oxylabs-js`); only the fields a view
/// surfaces are kept: provider, code, cost, ms, why (`tried`/`health`/`fail_detail`), and the request budget
/// `limit`/`remaining` (`limits`, kept as the two numbers Oxylabs sends).
#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
pub struct HopRow {
    pub provider: String,
    pub code: String,
    pub cost: f64,
    pub ms: u64,
    pub why: String,
    /// A call WE cut (our timeout, `wait:`, abandoned): its list price, unverified (`fetch::billed`); not in `cost`.
    pub upto: f64,
    /// The request shape: the exit country sent (empty = none) and the call's timeout; the tier says `run_js`.
    pub country: String,
    pub timeout_s: u64,
    pub limit: Option<u32>,
    pub remaining: Option<u32>,
    /// Ms plain waited for its turn (the home-IP brake), apart from `ms`; none = no wait.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waited_ms: Option<u64>,
}

impl Meta {
    /// The tier path (`plain:js_shell✗ oxy✓ bd:abandoned✗$0.0015`, a code only on failures, the bill of a
    /// cancelled call, why a served call was asked again: `plain✓md:thin`, `fetch::md_weak`), the one format every
    /// view prints.
    /// `·fr` beside the serving tier when its exit country is not the default one (`oxyjs·fr`), else empty.
    pub fn geo(&self) -> String {
        let c = self.country.to_uppercase();
        if c.is_empty() || c == crate::provider::home() { String::new() } else { format!("·{}", c.to_lowercase()) }
    }

    pub fn tried(&self) -> String {
        self.path(false)
    }

    /// `tried` with each paid call's request shape (`stats`): `oxyjs@US/60s✓`, `bd/60s` = no country sent.
    pub fn path(&self, shape: bool) -> String {
        // the failures a paid provider does not bill, said so (a refused key is no call: no `$`); a paid tier's
        // site block with no cost is one bd relayed (`provider::bd_refused`), a billed one shows its `$` above
        let unbilled = |c: &str| crate::provider::REFUSED.contains(&c) || matches!(c, "provider_timeout" | "abandoned");
        let geo = if shape { String::new() } else { self.geo() };
        let hop = |h: &HopRow| {
            let (p, s) = (crate::provider::short(&h.provider), h.timeout_s);
            let cc = if h.country.is_empty() { String::new() } else { format!("@{}", h.country) };
            let p = if shape && s > 0 && p != "plain" { format!("{p}{cc}/{s}s") } else { p.to_string() };
            match h.code.as_str() {
                // the exit country beside the tier only when it is not the default one (`oxyjs·fr✓`)
                "ok" => format!("{p}{geo}✓{}", h.why),
                // a site's 5xx Oxylabs relayed: billed by assumption (`provider::charged`, none reconciled)
                c if h.cost > 0.0 && c.starts_with("http_5") => format!("{p}:{c}✗${:.4} (assumed)", h.cost),
                // every billed hop shows its bill; a paid tier's unbilled failure says it cost nothing
                _ if h.cost > 0.0 => format!("{p}:{}✗${:.4}", h.code, h.cost),
                // a call we cut: the provider may finish and bill it (bd billed 15 s search cuts)
                _ if h.upto > 0.0 => format!("{p}:{}✗≤${:.4} (unverified)", h.code, h.upto),
                // `stats` adds the provider's own reason (`provider::reason`, ≤ 50 chars, no body text)
                c if p != "plain" && (unbilled(c) || c.starts_with("http_5") || c.starts_with("blocked_")) => {
                    let why = if shape && c == "provider_error" && !h.why.is_empty() {
                        format!(", {}", h.why)
                    } else {
                        String::new()
                    };
                    format!("{p}:{c}✗$0 (not charged{why})")
                }
                _ => format!("{p}:{}✗", h.code),
            }
        };
        let hops: Vec<String> = self.hops.iter().map(hop).collect();
        if hops.is_empty() { "-".into() } else { hops.join(" ") }
    }

    /// What the calls we cut may have cost on top of `cost` (`HopRow::upto`).
    pub fn upto(&self) -> f64 {
        crate::provider::spend(self.hops.iter().map(|h| h.upto))
    }

    /// How the run went besides its tiers, the one format `=` and `stats` print: ` · hedge:off` when a paid ask
    /// ran one provider at a time (`fetch::Plan::hedged`), ` · waited 7.6s` when plain waited for its turn.
    pub fn how(&self) -> String {
        let waited = self.hops.iter().filter_map(|h| h.waited_ms).sum::<u64>() as f64 / 1e3;
        let waited = if waited > 0.0 { format!(" · waited {waited:.1}s") } else { String::new() };
        if self.hedge == "off" { " · hedge:off" } else { "" }.to_string() + &waited
    }
}

/// Why a snapshot has no doc.
pub enum NoDoc {
    /// A zip body (docx, xlsx) this build cannot read, and where it is cached.
    Zip(PathBuf),
    /// No body stored: the fetch failed, or the snapshot is incomplete.
    Missing,
    /// The converter crashed on this body.
    Parse(String),
}

#[derive(Serialize, Deserialize, Default)]
pub struct Job {
    pub session: String,
    pub cmd: String,
    pub t: f64,
}

/// Picture K of a page, fetched on demand (`cli::asset`): `<page>/assets/m<K>.<ext>` (`ext` from its content
/// type), then this record as `m<K>.json` (written last: no record = not stored). `w`, `h` = the size read from
/// the bytes (0 = unread).
#[derive(Serialize, Deserialize, Default)]
pub struct Asset {
    pub src: String,
    pub ext: String,
    pub bytes: u64,
    pub ms: u64,
    pub t: f64,
    pub w: u32,
    pub h: u32,
}

/// The pictures stored under page dir `dir` (`Asset`).
pub fn assets(dir: &Path) -> Vec<Asset> {
    let d = dir.join("assets");
    numbered(&d, 'm', ".json").into_iter().filter_map(|k| jload(&d.join(format!("m{k}.json")))).collect()
}

/// What `Cache::ingest` stores.
pub struct Snap<'a> {
    /// (`raw.html|raw.pdf|raw.zip|…`, body); None = a failed fetch with no body: meta.json alone.
    pub file: Option<(&'a str, &'a [u8])>,
    /// The providers' error answers (`ERROR_FILE`).
    pub error: Option<&'a [u8]>,
    pub doc: Option<&'a Doc>,
    pub meta: Meta,
    /// The key later reads find it by (the URL; a local file's content hash); None = not reused.
    pub index: Option<&'a str>,
    /// The unfetched search result this fills.
    pub slot: Option<usize>,
}

/// `st` is a snapshot; every change goes through `alloc` / `save`, which re-read state.json under the session
/// lock, so parallel agents in one session never overwrite each other's ids.
pub struct Cache {
    pub root: PathBuf,
    pub session: String,
    pub st: State,
    cursor0: Option<Cursor>,
    /// This invocation's job, created by the first page it stores: every page of one command shares it.
    pub job: Option<usize>,
    /// When this command opened the cache (`cli::run`, one MCP call): its `wait:` deadline (`cli::wait_for`).
    pub born: Instant,
    /// Every snapshot's meta.json this command read (`meta`): read once, however many answers ask.
    metas: std::cell::RefCell<std::collections::HashMap<String, Option<Meta>>>,
}

/// A path read from a cache file stays inside the cache: relative, no `..`, no root (`"dir": "/etc"` is refused).
/// Rebuilt from its parts: the stored `t/u1` joins with the OS separator (`…\cache\t\u1` on Windows). The
/// rebuilt path is judged, not the stored one: on Windows a part with a drive (`t/C:x`) replaces what came before.
fn inside(rel: &str) -> Option<PathBuf> {
    let p: PathBuf = Path::new(rel).components().collect();
    (!rel.is_empty() && p.components().all(|c| matches!(c, std::path::Component::Normal(_)))).then_some(p)
}

/// fats made this root (`private_root` writes it): the sweep deletes only under a root holding it.
pub const MINE: &str = ".fatsearch-cache";

/// The cache root is its owner's only (0700 on unix): jobs.json keeps every command (queries too), account.json
/// balances. A missing root is created so, with `MINE` in it, and so is the default root (fats's own name in the
/// OS cache dir, made by builds before the sweep); one `FATS_CACHE` names is left as it is (`fats` shows its mode,
/// `config` whether it is swept). An error here is reported by the first write (`cache_write`).
pub fn private_root(root: &Path, default: bool) -> bool {
    let mut b = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    if !root.exists() && b.recursive(true).create(root).is_ok() || default && !root.join(MINE).exists() {
        fs::write(root.join(MINE), b"").ok();
    }
    root.is_dir()
}

/// What the sweep may delete in a session: a page `u<N>` (`u<N>-<k>`) or a search `j<N>` holding its meta.json,
/// or `run/` (detached runs); nothing else, ever.
fn ours(d: &Path) -> bool {
    let n = d.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    n == "run" || matches!(n.as_bytes(), [b'u' | b'j', b'0'..=b'9', ..]) && d.join("meta.json").is_file()
}

/// Seconds since `p` was modified (a clock set back reads 0); a missing file is endlessly old.
fn age(p: &Path) -> u64 {
    fs::metadata(p).and_then(|m| m.modified()).map_or(u64::MAX, |t| t.elapsed().map_or(0, |d| d.as_secs()))
}

/// THE lifetime rule: a stored dir (a page `u<N>`, a search `j<N>`, `run/`) is gone once unread for `cache_idle_s`
/// (the dir's mtime: `Cache::page` touches it) or `cache_ttl_s` after its fetch (its meta.json's mtime).
fn expired(dir: &Path) -> bool {
    let meta = dir.join("meta.json");
    age(dir) >= limit("cache_idle_s") || meta.exists() && age(&meta) >= limit("cache_ttl_s")
}

fn dirs_in(p: &Path) -> Vec<PathBuf> {
    let real = |e: &fs::DirEntry| e.file_type().is_ok_and(|t| t.is_dir()); // a symlink is never followed out
    fs::read_dir(p).map_or_else(|_| Vec::new(), |rd| rd.flatten().filter(real).map(|e| e.path()).collect())
}

/// THE sweep, run after the answer is out (`main`, `mcp`): `expired` dirs of this session every run, of every
/// session at most once per `sweep_every_s` (the `swept` file's mtime), only under a root fats made (`MINE`), only
/// `ours` dirs of a session dir (a real dir holding a state.json; a symlink is never followed). A session dir and
/// its state.json stay (ids never restart, so `u1` never names another page); the full sweep also forgets what
/// the hour is over for: an index row with its page, a job's command once `cache_ttl_s` old, and a
/// history row's URL (a search's results too) once its page is gone and its job forgotten: its id reads `! expired`.
pub fn sweep() {
    let s = crate::settings::get();
    let (root, marker, mine) = (&s.cache.value, s.cache.value.join("swept"), s.cache.value.join(MINE).is_file());
    let all = mine && age(&marker) >= limit("sweep_every_s") && fs::write(&marker, b"").is_ok();
    let gone = |d: &str| inside(d).is_none_or(|d| expired(&root.join(d)));
    let jobs = all.then(|| {
        let idx = |i: &mut Map<String, Value>| i.retain(|_, r| !gone(r["dir"].as_str().unwrap_or_default()));
        edit(&root.join("index.json"), idx).ok();
        edit(&root.join("jobs.json"), |js: &mut Vec<Job>| {
            js.iter_mut().filter(|j| now() - j.t >= limit::<f64>("cache_ttl_s")).for_each(|j| j.cmd.clear());
            js.iter().map(|j| j.cmd.is_empty()).collect::<Vec<_>>()
        })
    });
    // a fetched page keeps its (gone) dir name, never its URL; a result never fetched gets `-` (`{:-<1}` pads an
    // empty name with one `-`): `stats` gives it no row
    let forget = |e: &mut Entry| *e = Entry { dir: format!("{:-<1}", e.dir), job: e.job, ..Entry::default() };
    let sessions = dirs_in(root).into_iter().filter(|d| all || d.ends_with(&s.session.value));
    for session in sessions.filter(|d| mine && d.join("state.json").is_file()) {
        let c = Cache::at(root.clone(), session.file_name().unwrap_or_default().to_string_lossy().into());
        for d in dirs_in(&session).iter().filter(|d| ours(d) && expired(d)) {
            let runs = if d.ends_with("run") { c.runs() } else { Vec::new() };
            // a dead run's calls in flight (`Detached::cut`) are money no store wrote down: the one sweep whose rename
            // claims its run file stores them as the id's failed copy (`! abandoned`, its ledger row); a live run
            // keeps `run/`
            let claim = |n: usize| fs::rename(c.run_file(n, "json"), c.run_file(n, "swept")).is_ok();
            for (n, r, _) in runs.iter().filter(|r| r.1.dead(r.2, c.st.u.get(r.0.wrapping_sub(1))) && claim(r.0)) {
                let snap = Snap { file: None, error: None, doc: None, meta: r.cut(), index: None, slot: Some(*n) };
                Cache::at(root.clone(), c.session.clone()).ingest(snap, r.job, Instant::now()).ok();
            }
            runs.iter().all(|r| !r.2).then(|| fs::remove_dir_all(d).unwrap_or(()));
        }
        if let Some(Ok(js)) = &jobs {
            let old = |e: &&mut Entry| e.job.and_then(|j| js.get(j.checked_sub(1)?)) != Some(&false) && gone(&e.dir);
            edit(&session.join("state.json"), |st: &mut State| st.u.iter_mut().filter(old).for_each(forget)).ok();
        }
    }
}

pub fn url_key(url: &str) -> &str {
    let u = crate::html::py_strip(url);
    u.split('#').next().unwrap_or(u)
}

/// The detached run in file `json` (`Cache::run_file`) and whether it lives, THE liveness rule (a pending id,
/// status, a batch pool's floor): its lock is held. The launcher takes it before the run file exists and hands it
/// to the run's process (`Cache::run_claim`), so a run not scheduled yet lives too (a 2 s pid-0 window read
/// `! abandoned` under load). The lock is probed BEFORE the file is read: a process that wrote `done` and let go
/// reads as ended with it. A queued run lives while its lane's process does (the lock of the run it is queued
/// behind, `by`).
pub fn run_at(json: &Path) -> Option<(Detached, bool)> {
    let held = |at: PathBuf| matches!(File::open(at).map(|f| f.try_lock()), Ok(Err(fs::TryLockError::WouldBlock)));
    let own = held(json.with_extension("lock"));
    let r: Detached = jload(json)?;
    let alive = own || (r.by > 0 && r.done.is_none() && held(json.with_file_name(format!("u{}.lock", r.by))));
    Some((r, alive))
}

impl Cache {
    pub fn open() -> Cache {
        let s = crate::settings::get();
        private_root(&s.cache.value, s.cache.src == "default");
        Cache::at(s.cache.value.clone(), s.session.value.clone())
    }

    pub fn at(root: PathBuf, session: String) -> Cache {
        let st: State = jload(&root.join(&session).join("state.json")).unwrap_or_default();
        Cache { root, session, cursor0: st.cursor.clone(), st, job: None, born: Instant::now(), metas: <_>::default() }
    }

    /// The meta.json of the snapshot in `dir` (an entry's, a `lookup`'s), read once a command: a snapshot's never
    /// changes once stored.
    pub fn meta(&self, dir: &str) -> Option<Meta> {
        let load = || jload(&self.under(dir)?.join("meta.json"));
        self.metas.borrow_mut().entry(dir.into()).or_insert_with(load).clone()
    }

    /// Change the freshest state.json under the session lock; `st` becomes that state.
    /// A cursor this command moved is kept (a search's list, then its prefetch's `alloc`), else the file's.
    fn with_state<R>(&mut self, f: impl FnOnce(&mut State) -> R) -> Result<R, String> {
        let moved = (self.st.cursor != self.cursor0).then(|| self.st.cursor.clone());
        let (r, st) = edit(&self.root.join(&self.session).join("state.json"), |st: &mut State| {
            st.cursor = moved.unwrap_or_else(|| st.cursor.take());
            st.t = st.t.or(Some(now())); // the session's start: its first write, before anything it spends is stored
            let r = f(st);
            (r, st.clone())
        })?;
        self.st = st;
        Ok(r)
    }

    /// Re-read state.json: a detached fetch filled an id.
    pub fn reload(&mut self) {
        self.st = jload(&self.root.join(&self.session).join("state.json")).unwrap_or_default();
    }

    pub fn run_file(&self, n: usize, ext: &str) -> PathBuf {
        self.root.join(&self.session).join("run").join(format!("u{n}.{ext}"))
    }

    /// Detached run `n` and whether it lives (`run_at`).
    pub fn run(&self, n: usize) -> Option<(Detached, bool)> {
        run_at(&self.run_file(n, "json"))
    }

    pub fn run_put(&self, n: usize, r: &Detached) -> Result<(), String> {
        jsave(&self.run_file(n, "json"), r)
    }

    /// Run `n`'s end was shown (`cli::footer`): written under its lock, read again there; a busy lock = it runs
    /// (or a reader probes): told next time.
    pub fn run_tell(&self, n: usize) {
        let Some(_lock) = File::create(self.run_file(n, "lock")).ok().filter(|f| f.try_lock().is_ok()) else { return };
        if let Some(r) = jload::<Detached>(&self.run_file(n, "json")) {
            self.run_put(n, &Detached { told: true, ..r }).ok(); // a footer line, shown again at worst
        }
    }

    /// This session's runs: (n, run, alive).
    pub fn runs(&self) -> Vec<(usize, Detached, bool)> {
        let found = numbered(&self.root.join(&self.session).join("run"), 'u', ".json");
        found.into_iter().filter_map(|n| self.run(n).map(|(r, a)| (n, r, a))).collect()
    }

    /// Run `n`'s lock, taken by its launcher (`cli::launch`) and handed to the run (`cli::detach`: the process's stdin,
    /// so held until it ends; on Windows the launcher keeps it and the run takes it again; a reader's probe only delays
    /// it). Its log (`run_file(n, "log")`) takes the process's stdout + stderr.
    pub fn run_claim(&self, n: usize) -> Option<File> {
        let p = self.run_file(n, "lock");
        fs::create_dir_all(p.parent()?).ok()?;
        let f = File::create(p).ok()?;
        f.lock().ok()?;
        Some(f)
    }

    /// Persist the cursor if this command moved it (read-only commands write nothing).
    pub fn save(&mut self) -> Result<(), String> {
        if self.st.cursor == self.cursor0 { Ok(()) } else { self.with_state(|_| ()) }
    }

    /// Log a part of `u<n>` as read (`Entry::read`): its id tail (`#2ft`) and its text's key (`outline::part_key`).
    pub fn read_mark(&mut self, n: usize, read: &[String]) -> Result<(), String> {
        let new = |e: &Entry| read.iter().filter(|t| !e.read.contains(t)).cloned().collect::<Vec<_>>();
        self.with_state(|st| st.u.get_mut(n - 1).map(|e| e.read.extend(new(e)))).map(|_| ())
    }

    /// Append entries (or fill `slot`, see `place`): the first one's 1-based u number, unique even with parallel
    /// writers.
    pub fn alloc(&mut self, es: Vec<Entry>, slot: Option<usize>) -> Result<usize, String> {
        self.with_state(|st| {
            let n = st.u.len() + 1;
            es.into_iter().map(|e| place(st, e, slot)).min().unwrap_or(n)
        })
    }

    /// Store search `job`'s record (hits inside) and its providers' error answers; health and stats read it
    /// like a fetch's.
    pub fn serp_put(&self, job: usize, meta: &Meta, error: Option<&[u8]>) -> Result<(), String> {
        let dir = self.root.join(&self.session).join(format!("j{job}"));
        jsave(&dir.join("meta.json"), meta)?; // a failed search stores no ids: it starts the session (`State::t`)
        edit(&self.root.join(&self.session).join("state.json"), |st: &mut State| st.t = st.t.or(Some(now())))?;
        spend_log([meta.cost, meta.upto()], &dir);
        tally(&self.root, meta, None);
        error.map_or(Ok(()), |e| keep_error(&dir, e))
    }

    /// Search `job`'s record in this session.
    pub fn serp(&self, job: usize) -> Option<Meta> {
        jload(&self.dir(&Entry { dir: format!("{}/j{job}", self.session), ..Entry::default() })?.join("meta.json"))
    }

    /// This session's searches, oldest first: (job, record).
    pub fn serps(&self) -> Vec<(usize, Meta)> {
        numbered(&self.root.join(&self.session), 'j', "").into_iter().filter_map(|j| Some((j, self.serp(j)?))).collect()
    }

    /// A cache-relative dir from a cache file, still inside the root once symlinks are resolved (a link in the
    /// cache to a dir outside it is refused); None when it leaves the root or does not exist.
    fn under(&self, rel: &str) -> Option<PathBuf> {
        let p = self.root.join(inside(rel)?);
        p.canonicalize().ok()?.starts_with(self.root.canonicalize().ok()?).then_some(p)
    }

    /// Indexed snapshot of a URL (or a local file's content key, `blake3:`): (dir, age in s), while it lives
    /// (`expired`) and is the page asked for: its meta.json names that URL, never another page in a reused dir.
    pub fn lookup(&self, url: &str) -> Option<(String, f64)> {
        let on = !crate::settings::get().nocache.value;
        let idx: Map<String, Value> = on.then(|| jload(&self.root.join("index.json"))).flatten()?;
        let row = idx.get(url_key(url))?; // `{dir, t}`, written by `ingest`
        let (dir, t) = (row["dir"].as_str()?.to_string(), row["t"].as_f64()?);
        let m: Meta = jload(&self.dir(&Entry { dir: dir.clone(), ..Entry::default() })?.join("meta.json"))?;
        (url_key(&m.url) == url_key(url) || url.starts_with("blake3:")).then(|| (dir, now() - t))
    }

    /// New global job id for a fetch-type command.
    /// The invocation's job number (`j8`), registered on first use.
    pub fn job_of(&mut self, cmd: &str) -> Result<usize, String> {
        if let Some(j) = self.job {
            return Ok(j);
        }
        let (session, cmd) = (self.session.clone(), cmd.to_string());
        let j = edit(&self.root.join("jobs.json"), |jobs: &mut Vec<Job>| {
            jobs.push(Job { session, cmd, t: now() });
            jobs.len()
        })?;
        self.job = Some(j);
        Ok(j)
    }

    /// Job `j` (1-based); None when unknown or its session name would leave the cache root.
    pub fn job(&self, j: usize) -> Option<Job> {
        let jobs: Vec<Job> = jload(&self.root.join("jobs.json"))?;
        jobs.into_iter().nth(j.checked_sub(1)?).filter(|x| inside(&x.session).is_some())
    }

    pub fn job_count(&self) -> usize {
        jload::<Vec<Job>>(&self.root.join("jobs.json")).map_or(0, |j| j.len())
    }

    /// THE read of a stored dir (`page`, `lookup`, `serp`, `cli::seen`, `stats`): an entry's snapshot dir, under the
    /// root (None when state.json, or a symlink in the cache, points outside it) and not `expired`.
    pub fn dir(&self, e: &Entry) -> Option<PathBuf> {
        self.under(&e.dir).filter(|d| !expired(d))
    }

    /// meta + doc of an entry; doc.json is rebuilt from the body (`raw_name` of its kind) when missing or of another
    /// schema version.
    pub fn page(&self, e: &Entry) -> (Meta, Result<Doc, NoDoc>) {
        let Some(d) = self.dir(e) else {
            return (Meta { url: e.url.clone(), ok: true, ..Meta::default() }, Err(NoDoc::Missing));
        };
        // a read keeps it (`expired`); by path: Windows sets a folder's time only through a handle `File::open` lacks
        fs::set_times(&d, fs::FileTimes::new().set_modified(SystemTime::now())).ok();
        let meta: Meta = jload(&d.join("meta.json")).unwrap_or_default();
        let current = |x: &Doc| x.v == DOC_V && x.chunk == crate::settings::get().chunk.value;
        if let Some(doc) = jload::<Doc>(&d.join("doc.json")).filter(current) {
            return (meta, Ok(doc));
        }
        let f = raw_name(&meta.kind);
        let Ok(raw) = fs::read(d.join(&f)) else { return (meta, Err(NoDoc::Missing)) };
        if crate::clean::sniff(&raw) == Some("zip") {
            return (meta, Err(NoDoc::Zip(d.join(f))));
        }
        let url = if meta.url.is_empty() { e.url.clone() } else { meta.url.clone() };
        let fin = meta.final_url.clone().unwrap_or_else(|| url.clone());
        let kind = crate::clean::sniff(&raw).unwrap_or(&meta.kind);
        // a read-only cache still serves the page
        let doc = build(&raw, &url, &fin, meta.render == Some(true), kind).map_err(NoDoc::Parse);
        (meta, doc.inspect(|x| drop(jsave(&d.join("doc.json"), x))))
    }

    /// Store a fetched/local body as the next u of this session (or in `Snap::slot`). The snapshot is written to a
    /// private temp dir first and renamed to `u<N>` under the session lock; an existing dir is never touched
    /// (`u<N>-2` instead). `t0` = when the command started: a hop without `ms` gets the elapsed time. Storage
    /// only: the caller builds the doc first, so every error from here is a cache write error.
    pub fn ingest(&mut self, snap: Snap, job: usize, t0: Instant) -> Result<usize, String> {
        let Snap { file, error, doc, mut meta, index, slot } = snap;
        let sdir = self.root.join(&self.session);
        fs::create_dir_all(&sdir).map_err(|e| format!("{}: {e}", sdir.display()))?;
        if self.under(&self.session).is_none() {
            return Err(format!("{}: links out of the cache", sdir.display()));
        }
        // one per ingest: detached runs of one job may share a process (threads under the test transport)
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = sdir.join(format!(".tmp-{}-{job}-{seq}", std::process::id()));
        fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        let staged = (|| {
            file.map_or(Ok(()), |(name, raw)| fs::write(tmp.join(name), raw).map_err(|e| e.to_string()))?;
            error.map_or(Ok(()), |e| keep_error(&tmp, e))?;
            doc.map_or(Ok(()), |doc| jsave(&tmp.join("doc.json"), doc))?;
            // a time no one measured (a hop's, the fetch's) = the time since the command started
            let ms = t0.elapsed().as_millis() as u64;
            let times = meta.hops.iter_mut().map(|h| &mut h.ms).chain([&mut meta.ms]);
            times.filter(|m| **m == 0).for_each(|m| *m = ms);
            jsave(&tmp.join("meta.json"), &meta)
        })();
        staged.inspect_err(|_| drop(fs::remove_dir_all(&tmp)))?; // our own temp dir, never a snapshot
        let (session, url, root) = (self.session.clone(), meta.url.clone(), self.root.clone());
        let placed = self.with_state(|st| -> Result<(usize, String), String> {
            let free = slot.filter(|&n| st.result(n).is_some());
            let n = free.unwrap_or(st.u.len() + 1);
            let name = |k| if k == 1 { format!("{session}/u{n}") } else { format!("{session}/u{n}-{k}") };
            let rel = (1..).map(name).find(|r| !root.join(r).exists()).unwrap_or_default();
            fs::rename(&tmp, root.join(&rel)).map_err(|e| e.to_string())?;
            place(st, Entry { url, dir: rel.clone(), job: Some(job), ..Entry::default() }, free);
            Ok((n, rel))
        })?;
        let (n, rel) = placed.inspect_err(|_| drop(fs::remove_dir_all(&tmp)))?;
        spend_log([meta.cost, meta.upto()], &self.root.join(&rel));
        tally(&self.root, &meta, doc.map(|d| d.tokens.clean));
        if let Some(key) = index {
            edit(&self.root.join("index.json"), |idx: &mut Map<String, Value>| {
                idx.insert(url_key(key).into(), serde_json::json!({"dir": rel, "t": now()}));
            })?;
        }
        Ok(n)
    }
}

/// Body → Doc by its kind (`clean::kind_of`, kept in meta.json): a PDF, a server's Markdown, a text document, else
/// a page; `og` = a JS shell served as its own og card (`fetch::og_card`), made again from the same body.
pub fn build(raw: &[u8], url: &str, fin: &str, js: bool, kind: &str) -> Result<Doc, String> {
    guarded(|| match kind {
        "pdf" => crate::pdf::build_pdf(raw, url, fin, limit("pdf_budget_s")),
        "md" => Ok(crate::clean::markdown(raw, url, fin)),
        k if crate::clean::LISTS.text.contains(&k) => Ok(crate::text::build(raw, k, url, fin)),
        k => Ok(build_doc(&crate::html::parse(&String::from_utf8_lossy(raw)), url, fin, js))
            .map(|d| if k == "og" { crate::clean::card(d) } else { d }),
    })
}

/// The stored body's file name for a kind: `raw.html` (also an og card's), `raw.pdf`, `raw.zip`,
/// `raw.txt`, `raw.json`, `raw.xml`.
pub fn raw_name(kind: &str) -> String {
    match kind {
        "" | "og" => "raw.html".into(),
        "text" => "raw.txt".into(),
        k => format!("raw.{k}"),
    }
}

/// A converter panic is an error, never a crash.
pub fn guarded(run: impl FnOnce() -> Result<Doc, String>) -> Result<Doc, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
        .unwrap_or_else(|_| Err("parse_failed: converter crashed".to_string()))
}

/// Bytes under the root, the pictures among them (`assets/`), every live snapshot's meta.json path (`expired`:
/// the sweep's rule, so a dead copy counts in no health row), all sessions (status: size, pages, provider health).
/// Telemetry's `telemetry/` (its stamps, day marks and the weekly tally `Week`) is no cached page: a command's own
/// count never changes the next's status.
pub fn usage(root: &Path) -> (u64, u64, Vec<PathBuf>) {
    fn walk(p: &Path, acc: &mut (u64, u64, Vec<PathBuf>), top: bool) {
        let Ok(rd) = fs::read_dir(p) else { return };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                (!top || e.file_name() != "telemetry").then(|| walk(&e.path(), acc, false));
            } else if let Ok(m) = e.metadata() {
                acc.0 += m.len();
                acc.1 += if p.ends_with("assets") { m.len() } else { 0 };
                if e.file_name() == "meta.json" && !expired(p) {
                    acc.2.push(e.path());
                }
            }
        }
    }
    let mut acc = (0, 0, Vec::new());
    root.join(MINE).is_file().then(|| walk(root, &mut acc, true)); // never a dir fats did not make (`/`, a home)
    acc
}

/// The weekly tally the usage summary is built from (`stats::weekly`): counts, codes and fixed names only, one file a
/// week (`<cache>/telemetry/week/2026-W43.json`, this week's and last week's kept). Sessions are swept within the hour,
/// so it counts as things happen: a stored fetch or search (`tally`), a command (`stats::tally_run`), a panic
/// (`stats::crash`).
/// `n` = each counter by its path in the summary (`tier\toxyjs\tok`), `samples` = the last `week_samples` of each
/// `tok\tpage` (a stored page's clean tokens), `tok\tread` (a read's answer), `ms\tplain`, `ms\tpaid` (a page call
/// that served).
#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct Week {
    pub n: BTreeMap<String, u64>,
    pub samples: BTreeMap<String, Vec<u64>>,
}

impl Week {
    pub fn add(&mut self, path: &[&str], by: bool) {
        by.then(|| *self.n.entry(path.join("\t")).or_default() += 1);
    }

    pub fn sample(&mut self, path: &str, x: u64) {
        let v = self.samples.entry(path.into()).or_default();
        v.push(x);
        v.drain(..v.len().saturating_sub(crate::settings::limit("week_samples")));
    }
}

pub const WEEK_S: f64 = 604_800.0;
/// `2026-W43`: the ISO week of unix time `t`.
pub fn week_of(t: f64) -> String {
    let w = chrono::DateTime::from_timestamp(t as i64, 0).unwrap_or_default().date_naive().iso_week();
    format!("{}-W{:02}", w.year(), w.week())
}

/// Add to this week's tally (`<root>/telemetry/week/`), at level `full` only (a count made under an opt-out would go
/// once it is lifted), under its lock as `lock` takes it (`edit_when`); a week's first write drops every older file
/// but last week's. A count fats cannot write is lost, never a failed command.
pub fn bump(root: &Path, lock: impl Fn(&File) -> std::io::Result<()>, f: impl FnOnce(&mut Week)) {
    let full = crate::telemetry::level() == crate::telemetry::Level::Full;
    let (week, last, dir) = (week_of(now()), week_of(now() - WEEK_S), root.join("telemetry").join("week"));
    let p = dir.join(format!("{week}.json"));
    let keep = |e: &std::fs::DirEntry| [&week, &last].iter().any(|w| e.file_name().to_string_lossy().starts_with(*w));
    let old = std::fs::read_dir(&dir).into_iter().flatten().flatten().filter(|e| full && !p.is_file() && !keep(e));
    old.for_each(|e| drop(std::fs::remove_file(e.path())));
    full.then(|| edit_when(&p, lock, f).ok());
}

/// A code as counted: one errors.json names (`js_shell`; a `*` row's prefix + digits: `http_404`), else `other`.
pub fn code_of(c: &str) -> String {
    let digits = |d: &str| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit());
    let hit = |k: &&str| k.strip_suffix('*').map_or(*k == c, |p| c.strip_prefix(p).is_some_and(digits));
    crate::lang::FIX.iter().map(|r| r.0).find(hit).map_or("other".into(), |_| c.into())
}

/// Count a stored fetch or search record (`cache::ingest`, `cache::serp_put`): its calls per tier (a call cut
/// after the other provider won is neither ok nor failed) and paid provider, each failed call's code and the
/// record's own when no call ended with it (`no_key`), the serving page call's ms, the page's clean tokens
/// (`tok`), a PDF. No URL, no query: every name comes from our tables.
pub fn tally(root: &Path, m: &Meta, tok: Option<usize>) {
    let serp = m.kind == "serp";
    bump(root, File::lock, |w| {
        for h in &m.hops {
            let t = crate::provider::Tier::parse(&h.provider);
            let Some(short) = t.map(|t| t.spec().short).or((h.provider == "gh").then_some("gh")) else { continue };
            let (ok, failed) = (h.code == "ok", !matches!(h.code.as_str(), "ok" | "abandoned"));
            let tier = if serp { format!("{short}_search") } else { short.into() };
            for (k, by) in [("ok", ok), ("fail", failed), ("billed", h.cost > 0.0), ("upto", h.upto > 0.0)] {
                w.add(&["tier", &tier, k], by);
            }
            w.add(&["codes", &code_of(&h.code)], failed);
            let paid = t.filter(|t| *t != crate::provider::Tier::Plain).map(crate::provider::Tier::switch);
            w.add(&["provider", paid.unwrap_or_default(), if serp { "search" } else { "calls" }], paid.is_some());
            let ms = if t == Some(crate::provider::Tier::Plain) { "ms\tplain" } else { "ms\tpaid" };
            (ok && !serp).then(|| w.sample(ms, h.ms));
        }
        w.add(&["codes", &code_of(&m.code)], !m.ok && !m.code.is_empty() && !m.hops.iter().any(|h| h.code == m.code));
        let served =
            crate::provider::Tier::parse(&m.provider).filter(|t| m.ok && !serp && *t != crate::provider::Tier::Plain);
        w.add(&["provider", served.map(crate::provider::Tier::switch).unwrap_or_default(), "served"], served.is_some());
        tok.into_iter().for_each(|n| w.sample("tok\tpage", n as u64));
        w.add(&["cmd", "pdf"], m.kind == "pdf");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stored dir is a path under the cache root or nothing; on Windows `push("C:x")` replaces the path.
    #[test]
    fn a_stored_dir_stays_inside() {
        assert_eq!(inside("t/u1"), Some(Path::new("t").join("u1")));
        for bad in ["", "/etc", "../x", "t/../x", "./t"] {
            assert_eq!(inside(bad), None, "{bad}");
        }
        assert_eq!(inside("t/C:x").is_none(), cfg!(windows), "a drive on Windows, a plain name elsewhere");
    }

    /// The footer read a run, the run's process wrote `done` and let go, the footer wrote its stale copy
    /// back without `done` (`! abandoned` from then on). `told` is now written under the run's lock, read again.
    #[test]
    fn a_run_is_told_under_its_lock() {
        let root = crate::scratch::Scratch::new("tell");
        let c = Cache::at(root.to_path_buf(), "t".into());
        let ended = Detached { url: "https://ex.com/".into(), done: Some(Done::default()), ..Detached::default() };
        c.run_put(1, &ended).expect("run file");
        let lock = c.run_claim(1).expect("the run's lock");
        assert!(c.run(1).is_some_and(|r| r.1), "a held lock = alive");
        c.run_tell(1);
        assert!(c.run(1).is_some_and(|r| !r.0.told), "busy: nothing written");
        drop(lock);
        c.run_tell(1);
        assert!(c.run(1).is_some_and(|r| r.0.told && r.0.done.is_some() && !r.1), "free: told, its done kept");
    }

    /// A run queued behind another (`by`) lives while that one's process holds its lock, and ends with it.
    #[test]
    fn a_queued_run_lives_with_its_lane() {
        let root = crate::scratch::Scratch::new("lane");
        let c = Cache::at(root.to_path_buf(), "t".into());
        let lock = c.run_claim(1).expect("the lane's lock");
        c.run_put(2, &Detached { url: "https://ex.com/b".into(), by: 1, ..Detached::default() }).expect("run file");
        assert!(c.run(2).is_some_and(|r| r.1), "queued: alive");
        drop(lock);
        assert!(c.run(2).is_some_and(|r| !r.1), "its lane died: abandoned");
    }
}
