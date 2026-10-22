//! FETCH: the router. `auto` = the ladder (`Plan::ladder`): plain first, then the keyed tiers cheapest first, Oxylabs
//! and Decodo without JavaScript, then the renders (Bright Data, Oxylabs, Decodo) racing one hedge apart (`hedge:`,
//! 5 s) while the content check fails. Oxylabs without JS is a page rung: after plain's captcha with both keys, first
//! with the free fetch off, on every page with its key alone, never after a shell (`Plan::ladder`; Decodo's the same);
//! it also serves a search, a PDF's bytes and `provider:oxy`.
//! The first good body wins and the other call is cancelled, its bill unverified (`billed`). Government hosts skip
//! Bright Data (it refuses them). A PDF (`pdf_run`) goes plain → Oxylabs without JS → Bright Data for its bytes, never
//! JavaScript; a `/pdf/` path (`Plan::hint`) only gets the file timeout and raw bytes, page rules otherwise; a zip
//! document (docx, xlsx) never goes to a provider, and a text document plain served (`clean::Lists::text`) stops it.
//! `next` is the whole decision table, pure, so it is tested without a network.
//! Every page fetch walks the same ladder; `js:true` walks the table's render order (`JS_QUOTE`).

use crate::cache::{Cache, HopRow, Meta, guarded, raw_name};
use crate::clean::{self, Doc};
use crate::html::{Node, Tree, py_space};
use crate::lang::{At, Cmd, Flag, bang, bin, fix, priced, uniq};
use crate::outline::{fmt_age, is_thin, lost_page};
use crate::provider::{
    Ask, Fail, Hit, Limit, PROVIDERS, REFUSED, Raw, SEARCH_TIMEOUT, Tier, search_country, sent, spend,
};
use crate::settings::limit;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// Default `cost:` cap per command, USD.
pub const COST_CAP: f64 = 0.05;
/// The URL tables (`src/data/sites.json`).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, bound(deserialize = "'de: 'static"))]
pub struct Sites {
    /// Government labels (`gov()`), a TLD convention, no site: Bright Data refuses those hosts, so `auto` sends
    /// them straight to Oxylabs. The first two are TLDs; the rest are public suffixes under a country code.
    pub gov: Vec<&'static str>,
    /// URL suffixes that name a document, and its kind (what `clean::kind_of` finds in the answer; `html` = a
    /// page). A text document's suffix only keeps a `/pdf/` path from being a PDF hint (`Plan::hint`): the answer
    /// says what it is; `secret` = a key or credential (`cli::secret`: never read from disk). THE list: a token
    /// with one of them is a path too (`lang::is_file`, `doc_kind`).
    doc_ext: Vec<(&'static str, &'static str)>,
    /// Path segments that may name a PDF without the suffix (`arxiv.org/pdf/2405.15793`, `…/doi/pdf/10.1145/…`);
    /// a hint only, since `…/docs/pdf/merge.html` is a page (`Plan::hint`).
    pdf_path: Vec<&'static str>,
    /// Second-level labels under a two-letter country code that are public suffixes themselves (`co.uk`,
    /// `gov.uk`, `com.au`). A built-in approximation of the Public Suffix List's ICANN section.
    sld: Vec<&'static str>,
    /// Hosts where every subdomain is someone else's site (`alice.github.io`): each is its own route. The few the
    /// Public Suffix List's private section has that agents meet most (a registry row like `sld`, no site).
    shared: Vec<&'static str>,
    /// The fence's blocks (`provider::private_ip`, CIDR): this host (0/8), private, CGNAT, loopback, link-local
    /// (169.254.169.254 = cloud metadata), IETF, documentation, benchmarking, multicast, reserved and broadcast;
    /// IPv6 ULA, link-local, site-local, multicast, documentation (`::` and `::1` carry IPv4 0.0.0.0 and 0.0.0.1).
    pub fence: Vec<&'static str>,
}

pub static SITES: LazyLock<Sites> = LazyLock::new(|| crate::settings::table("sites"));
/// Oxylabs without JS in a PDF run: the whole file comes back as escaped JSON (a 4.9 MB PDF took 9.5 s,
/// a 352 kB one 2.1 s; measured 2026-10-12).
pub const PDF_OXY: Duration = Duration::from_secs(30);
/// How long the first provider runs alone before the second starts beside it (`hedge:0` = both at once).
/// Both are billed when both run, so the hedge only buys time on slow answers.
pub const HEDGE: Duration = Duration::from_secs(5);
/// The hedge on a web search (Oxylabs, then Bright Data): a hedge starts at the first provider's p50, and a
/// search's p50 is about 2 s (Web API `/v1/search` measured 0.9-1.5 s), so Bright Data still has time to answer
/// inside a short `wait:` when Oxylabs is slow.
pub const SEARCH_HEDGE: Duration = Duration::from_millis(2500);

/// The hedges in effect, as `dry:true` and `fats config` show them: `hedge search 2.5 s · fetch 5 s · default`,
/// or the one `hedge:` mod value that sets both.
pub fn hedges(flag: Option<&str>) -> String {
    let default =
        || format!("hedge search {} s · fetch {} s · default", SEARCH_HEDGE.as_secs_f64(), HEDGE.as_secs_f64());
    flag.map_or_else(default, |v| format!("hedge {v} s · flag hedge:{v}"))
}
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Auto,
    Only(Tier),
}

#[derive(Default, Clone)]
pub struct Plan {
    pub mode: Mode,
    /// `js:true`: skip plain (and Oxylabs without JS, where it runs); Bright Data, then Oxylabs rendering JavaScript.
    pub js: bool,
    pub cap: f64,
    pub gov: bool,
    /// The URL names a document by its suffix: `pdf`, `zip`, a text document (`Sites::doc_ext`).
    pub doc: Option<&'static str>,
    /// The path may name a file (`Sites::pdf_path`, no `.htm(l)` suffix): Oxylabs gets `PDF_OXY` and Bright Data sends
    /// raw bytes, but a page that passes the content check is served as a page; only an answer that says PDF makes it
    /// a PDF run.
    pub hint: bool,
    /// Tiers with a key configured.
    pub keys: Vec<Tier>,
    /// `timeout:` for the JavaScript tiers (bd, oxyjs); plain and oxy without JS keep their own.
    pub timeout: Option<Duration>,
    pub hedge: Duration,
    /// A web search, not a page: no gate tiers (an answer has no content check), search prices and timeout.
    pub search: bool,
    /// A search's prefetch (`fetch:N`): speculative work is free, so plain only, whatever `provider:` the search
    /// named; a page that needs a paid tier is fetched when opened (`fats u<N>` runs the whole ladder).
    pub prefetch: bool,
    /// The render that sent the page's stored copy: `js:true` never asks it again (`rendered`).
    pub avoid: Option<Tier>,
    /// A search's `wait:` (every command answers by it): no tier starts after it, a call still running is cut
    /// (`provider::late`).
    /// None = a detached fetch, which runs to its end.
    pub wait: Option<Duration>,
    /// The folder of the copy stored before, the Markdown guard's yardstick (`md_weak`); None = none.
    pub prior: Option<std::path::PathBuf>,
    /// A batch's `cost:` pool (`pooled`) and this run's id in it; None = the run has `cap` to itself.
    pub pool: Option<(std::path::PathBuf, usize)>,
}

impl Plan {
    pub fn new(url: &str, mode: Mode, js: bool, cap: f64, timeout: Option<Duration>) -> Plan {
        let gov = gov(url);
        let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
        let kind = doc_kind(&path);
        let (page, doc) = (kind == Some("html"), kind.filter(|k| *k != "html"));
        let hint = doc.is_none() && !page && SITES.pdf_path.iter().any(|p| path.contains(p));
        Plan { mode, js, cap, gov, doc, hint, keys: keyed(), timeout, hedge: HEDGE, ..Plan::default() }
    }

    /// A web search: the search rows (Oxylabs, then Bright Data beside it after the hedge); `provider:` = that one
    /// only.
    pub fn search(mode: Mode, cap: f64, timeout: Option<Duration>) -> Plan {
        let mut p = Plan { search: true, hedge: SEARCH_HEDGE, ..Plan::new("", mode, false, cap, timeout) };
        p.keys.retain(|t| t.can("search"));
        p
    }

    /// THE plan of a command's mods (`provider:`, `js:`, `cost:`, `timeout:`, `hedge:`), a page's for `url`, else a
    /// search's: what a fetch runs, `dry:true` routes and an arrow quotes (`worst`).
    pub fn of(cmd: &Cmd, url: Option<&str>) -> Plan {
        let mode = cmd.get("provider").and_then(Tier::parse).map_or(Mode::Auto, Mode::Only);
        let cap = cmd.get("cost").and_then(|v| v.parse::<f64>().ok()).unwrap_or(COST_CAP).max(0.0);
        let timeout = cmd.get("timeout").and_then(|v| v.parse().ok()).map(Duration::from_secs);
        let mut p =
            url.map_or_else(|| Plan::search(mode, cap, timeout), |u| Plan::new(u, mode, cmd.flag("js"), cap, timeout));
        p.hedge = cmd.get("hedge").and_then(|v| v.parse().ok()).map_or(p.hedge, Duration::from_secs);
        p
    }

    /// THE document rule: this run fetches a PDF when its URL names one (`.pdf`) or an answer said
    /// so (magic, or the content type on a failure). A PDF is never rendered: no oxyjs, and `js:true` does not
    /// skip plain.
    pub fn pdf_run(&self, done: &[Hop]) -> bool {
        !self.search && (self.doc == Some("pdf") || done.iter().any(|h| h.kind == "pdf"))
    }

    /// THE ladder rule: the tiers in the order they may run, given the finished hops (`pdf_run`; none = before any
    /// answer). `auto` = the capability rows (`provider::Spec::can`) that do this job, cheapest first (table order on a
    /// tie: bd before oxyjs); a paid no-JS tier leaves a page's ladder when another provider's render has a key (it
    /// served 3 of 20 pages plain failed on), so with one provider's key every rung of that provider runs, unless
    /// plain's wall needs a render (`Check::render_first`: after a JS challenge or shell oxy no-JS served 0/33, after a
    /// 403 or captcha 11/26). With both keys it stays, before the renders, after plain's captcha only
    /// (`Check::no_js_first`: Reddit 25/34 in ~4.4 s for $0.0010, a render $0.0015), or first when plain is off (no
    /// free verdict to wait for: a page both renders cut at 60 s served without JS in 6.4 s); one try, no site log.
    /// Never after a shell, the page needs JS (0/3 served, each +$0.0010, +4–10 s), nor after a paid no-JS tier OUR cut
    /// stopped: the next is the same slow page without JS (oxy 10 s, then decodo 15 s).
    /// `provider:oxy` = every tier of that key, without JS first (oxy, oxyjs).
    pub fn ladder(&self, done: &[Hop]) -> Vec<Tier> {
        let pdf = self.pdf_run(done);
        let job = match () {
            _ if self.search => "search",
            _ if pdf => "pdf",
            _ if self.js => "js",
            _ => "page",
        };
        let mut rows: Vec<Tier> = Tier::ALL.into_iter().filter(|t| t.can(job) && (!self.gov || t.can("gov"))).collect();
        rows.sort_by(|a, b| a.spec().price.total_cmp(&b.spec().price));
        let render = |t: Tier| rows.iter().any(|&r| r.can("js") && r.owner() != t.owner() && self.keys.contains(&r));
        let page = job == "page" && !self.hint;
        let plain = |codes: &[&str]| done.iter().any(|h| h.tier == Tier::Plain && codes.contains(&h.code.as_str()));
        // a paid no-JS tier yields to another provider's render but after plain's `no_js_first` wall, or when plain
        // is off (no free verdict to wait for: the cheap fast rung leads), to its own render after a `render_first` one
        let first = !self.keys.contains(&Tier::Plain) || plain(&CHECK.no_js_first);
        let cut = done.iter().any(|h| !h.tier.can("js") && h.tier.spec().price > 0.0 && h.code == "provider_timeout");
        let yields = |t: Tier| cut || if render(t) { !first } else { plain(&CHECK.render_first) };
        let drops = |t: &Tier| page && !t.can("js") && t.spec().price > 0.0 && yields(*t);
        let all = match self.mode {
            _ if self.prefetch => vec![Tier::Plain],
            Mode::Only(o) => Tier::ALL.into_iter().filter(|&t| t == o || t.owner() == o).collect(),
            Mode::Auto => rows.iter().copied().filter(|t| !drops(t)).collect(),
        };
        // behind a `one_render` gate, plain's or a render's, ONE render: it answers every render alike (bd and oxyjs
        // both billed for one shell); a render the provider refused or erred on rendered nothing, so the next may try
        let gate = done.iter().any(|h| CHECK.one_render.contains(&h.code.as_str()));
        let refused = |t: &Tier| done.iter().any(|h| h.tier == *t && REFUSED.contains(&h.code.as_str()));
        let one = all.iter().copied().find(|t| t.can("js") && self.keys.contains(t) && !refused(t)).filter(|_| gate);
        let keep = |t: Tier| t.can(job) && self.avoid != Some(t);
        all.into_iter().filter(|&t| keep(t) && one.is_none_or(|o| o == t || !t.can("js"))).collect()
    }

    /// A tier's timeout, given the finished hops. `timeout:` stretches only the slow tiers: the 10 s plain and oxy
    /// no-JS gates exist to hand a hard page on quickly, so they keep their time, but never past what the agent's wall
    /// left (`agent_left`; plain runs first, so its cap is `timeout:` itself: `timeout:2` on a 12 s page ran 12 s;
    /// `provider:oxy timeout:1` ran its no-JS call 20 s, 21 s in all). In a PDF run, or on a `/pdf/` path (`hint`),
    /// Oxylabs gets `PDF_OXY`. The one-key retry of a timed-out call (`retries`) gets twice OUR time, never twice the
    /// agent's `timeout:` (a cap it asked for), and never past the one-key wall (`wall_left`). A named no-JS tier
    /// (`provider:oxy`, the cut tier's offer) gates nothing to hand on quickly: twice its time, as a cut one's retry.
    fn timeout_of(&self, t: Tier, done: &[Hop]) -> Duration {
        let cut = done.iter().find(|h| h.tier == t && h.code == "provider_timeout");
        let k = 1 + u32::from(one_call(self) || cut.is_some() || (self.mode == Mode::Only(t) && !t.can("js")));
        let wall = cut.and_then(|h| wall_left(done, h)).unwrap_or(Duration::MAX);
        let left = self.agent_left(done);
        let ours = |d: Duration| left.unwrap_or((d * k).min(wall));
        let paid_file = t.spec().price > 0.0 && t.can("pdf") && (self.pdf_run(done) || self.hint);
        match () {
            _ if self.search => ours(SEARCH_TIMEOUT),
            _ if t.can("js") => ours(t.spec().timeout),
            _ if paid_file => ours(PDF_OXY),
            _ => (t.spec().timeout * k).min(wall).min(left.unwrap_or(Duration::MAX)),
        }
    }

    /// Plain and oxy without JS (the no-JS rows, `!t.can("js")`) are gates of a fetch: what follows waits for their
    /// verdict, and only some failures are worth it. In a paid ask (`js:true`, `provider:`) every tier is a gate: ONE
    /// provider runs, the next only on its failure (`hedge:off`; a hedge bills both). Bd leads a render: it won 14/14
    /// of the races it answered, and fails fast (js_shell ~2.3 s) where oxyjs fails slow (68-71 s timeouts over
    /// 192 runs).
    fn gate(&self, t: Tier) -> bool {
        !self.search && (!t.can("js") || !self.hedged() || self.timeout.is_some())
    }

    /// With `timeout:8` and both keys, bd was cut at 8 s and oxyjs, hedged at 5 s, cut 8 s later: 13.3 s. So
    /// the agent's `timeout:` is ONE wall for the provider calls of a page run, one at a time (`gate`): what the
    /// finished ones left of it (plain, free, capped by it but not counted, and the host check before it); the run
    /// stops with < 1 s left (`next`). None = no `timeout:`.
    fn agent_left(&self, done: &[Hop]) -> Option<Duration> {
        let used = done.iter().filter(|h| !self.search && h.tier != Tier::Plain).map(|h| h.ms).sum();
        self.timeout.map(|t| t.saturating_sub(Duration::from_millis(used)))
    }

    /// The router picks and races the tiers (`auto`), or the one tier asked for is free (`provider:plain`: no
    /// `hedge:off` to print); false = the agent asked for a paid tier (`js:true`, `provider:bd`): `hedge:off`.
    pub fn hedged(&self) -> bool {
        self.search || self.mode == Mode::Only(Tier::Plain) || (self.mode == Mode::Auto && !self.js)
    }

    /// THE search patience: `wait:`, or longer, the leading keyed search provider's own
    /// answer time (`Spec::search_s`: bd answers in 4.5-25 s, bd-only got 0/3 searches at 5 s).
    /// Longer than `wait:` = the search runs detached (`cli::searching`): the caller still answers at `wait:`.
    pub fn search_wait(&self, wait: Duration) -> Duration {
        let lead = self.ladder(&[]).into_iter().find(|t| self.keys.contains(t)).map_or(0, |t| t.spec().search_s);
        wait.max(Duration::from_secs(lead))
    }
}

// ---------------------------------------------------------------- site

/// The site of a URL (`canonical`: a same-site landing): its registrable domain (`m.youtube.com` → `youtube.com`,
/// `www.bbc.co.uk` → `bbc.co.uk`), on a shared host the subdomain (`alice.github.io`); an IP address or a
/// one-label host stays whole.
pub fn site(url: &str) -> String {
    let host = clean::host_of(url);
    let host = host.trim_end_matches('.');
    if host.parse::<std::net::IpAddr>().is_ok() {
        return host.into();
    }
    let host = host.strip_prefix("www.").unwrap_or(host);
    let labels: Vec<&str> = host.split('.').collect();
    let shared = SITES.shared.iter().find(|s| host.ends_with(&format!(".{s}")));
    let keep = match labels.as_slice() {
        _ if let Some(s) = shared => s.split('.').count() + 1,
        // `co.uk` alone keeps its 2 labels either way
        [.., sld, tld] if tld.len() == 2 && SITES.sld.contains(sld) => 3,
        _ => 2,
    };
    labels[labels.len().saturating_sub(keep)..].join(".")
}

/// What `js:true` takes: a render's time (oxyjs renders measured 14-32 s, 2026-10); its price = `worst`.
/// `≈`, never `~`: an arrow pasted into zsh unquoted stops at `~25s` (a user's home dir it looks up).
pub const JS_QUOTE: &str = "≈25s";

/// One call's outcome: the hop row in meta.json, plus the body and doc for the store.
#[derive(Default)]
pub struct Hop {
    pub tier: Tier,
    pub ok: bool,
    pub code: String,
    pub status: u16,
    pub ms: u64,
    pub cost: f64,
    /// `cache::HopRow::upto`: a call we cut, its price unverified (`billed`).
    pub upto: f64,
    /// The request shape (`cache::HopRow`): the country sent (`run`, `run_search`), the timeout (`drive`).
    pub country: String,
    pub timeout_s: u64,
    /// `clean::kind_of`: `html`, `pdf`, `zip`, `text`, `json`, `xml`; empty = no body.
    pub kind: &'static str,
    pub body: Vec<u8>,
    /// Where the answer came from: the base of its links and pictures.
    pub final_url: String,
    /// Where a bd ask landed by its same-site canonical link (`canonical`): `Meta::landed`, never a base.
    pub landed: Option<String>,
    /// The page whose meta refresh plain followed to this one (`provider::refresh`): `Meta::refreshed_from`.
    pub refreshed_from: Option<String>,
    /// Visible characters (picks the best copy when every hop fails).
    pub vis: usize,
    pub doc: Option<Doc>,
    /// A failed HTML answer's parse, kept so the best failed copy is cleaned without a second parse.
    pub tree: Option<Tree>,
    pub charset: &'static str,
    /// A search's answer, best first.
    pub hits: Vec<Hit>,
    /// The provider's own error answer, masked: kept as `error.txt`, never printed.
    pub err: Vec<u8>,
    /// `Fail::why`: the token the `!` row adds to the code.
    pub why: String,
    /// The provider's request budget from this answer (`provider::Limit`): stored as two numbers in the hop.
    pub limit: Option<Limit>,
    /// When the call landed (`billed`): a thin answer waits the hedge window from here (`next`).
    pub end: Option<Instant>,
    /// The HTML answer's `<html lang>` primary subtag, lower case (`it`); empty = none declared (`langs`).
    pub lang: String,
    /// Ms plain waited for its turn before it started (`provider::pace`), not in `ms`.
    pub waited: u64,
}

impl Hop {
    /// A call the provider failed: its code, status, reason token, budget and (masked) answer.
    fn failed(tier: Tier, f: Fail) -> Hop {
        Hop { tier, code: f.code.into(), status: f.status, err: f.body, why: f.why, limit: f.limit, ..Hop::default() }
    }

    pub fn row(&self) -> HopRow {
        HopRow {
            provider: self.tier.spec().name.into(),
            code: self.code.clone(),
            cost: self.cost,
            ms: self.ms,
            why: self.why.clone(),
            upto: self.upto,
            country: self.country.clone(),
            timeout_s: self.timeout_s,
            limit: self.limit.map(|l| l.limit),
            remaining: self.limit.map(|l| l.remaining),
            waited_ms: Some(self.waited).filter(|&w| w > 0),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Step {
    Start(Tier),
    Wait(Duration),
    Stop,
}

/// THE final answers: about the target itself, so every tier gets the same one. The first ends the job and cancels
/// every call in flight: a missing page, an oversized body, a private address (a provider must not be sent one
/// either), a host that does not exist or got no verdict on its address (`host_hop`). Not final: a provider's
/// refusal or unlock failure, such as Bright Data's 502 / `x-brd-error-code` (`reject_block`, `resolve_failed_*`:
/// "each attempt uses a different peer, and the same request has been observed to succeed on a later attempt",
/// docs.brightdata.com products/web-unlocker/error-codes) or its policy refusals (another provider may serve that
/// host).
const FINAL: &[&str] =
    &["not_found", "too_big", "private_address", "no_host", "dns_timeout", "dns_failed", "tls_failed"];

/// A failed gate hop (plain, or oxy without JS) worth the next tier: not a zip document, not a final answer
/// (`FINAL`).
fn escalates(h: &Hop) -> bool {
    !h.ok && h.kind != "zip" && !FINAL.contains(&h.code.as_str())
}

/// Codes that are about the key, not the page: a refused key or zone, or its request budget spent. The same key on
/// another tier gets the same answer.
pub const KEY_CODES: &[&str] = &["bad_key", "bad_zone", "rate_limited"];

/// A provider refused `t`'s key, said its budget is spent, or refused this URL (`provider_refused`, bd's policy:
/// a government host the labels missed) on an earlier hop: no tier sends that key again in this run (oxy → oxyjs
/// share one), and it is no provider left to cover a failure (`retries`).
fn refused(done: &[Hop], t: Tier) -> bool {
    let no = |c: &str| KEY_CODES.contains(&c) || c == "provider_refused";
    done.iter().any(|h| no(&h.code) && h.tier.owner() == t.owner())
}

/// THE decision table: what happens next, given finished hops and running calls (tier, time running).
pub fn next(p: &Plan, done: &[Hop], running: &[(Tier, Duration)]) -> Step {
    // a call still running `grace_s` past its own timeout is given up on
    let over = running.iter().any(|&(t, e)| e > p.timeout_of(t, done) + Duration::from_secs(limit("grace_s")));
    // a thin first answer waits for the other provider's call already running, for the hedge
    // window after it landed (not the call's whole timeout; reddit served bd's 1.2k tok, oxyjs held 9.3k)
    let other = |h: &Hop| running.iter().any(|r| r.0.owner() != h.tier.owner());
    let window = |h: &Hop| p.hedge.saturating_sub(h.end.map_or(Duration::ZERO, |e| e.elapsed()));
    let thin_wait = done.iter().filter(|h| h.ok).map(window).min().filter(|w| !w.is_zero());
    if let Some(w) = thin_wait.filter(|_| !over && done.iter().all(|h| !h.ok || thin(h) && other(h))) {
        return Step::Wait(left(p, running, done).min(w));
    }
    let spent_wall = p.agent_left(done).is_some_and(|l| l < Duration::from_secs(1));
    if over || spent_wall || done.iter().any(|h| h.ok || FINAL.contains(&h.code.as_str())) {
        return Step::Stop;
    }
    let started = |t: Tier| done.iter().any(|h| h.tier == t) || running.iter().any(|r| r.0 == t);
    // a gate's verdict worth the next tier (a zip document goes on whatever plain said)
    let settled = |q: Tier| p.doc == Some("zip") || !done.iter().any(|h| h.tier == q && escalates(h));
    let spent = held(done, running);
    let can = |t: Tier| p.keys.contains(&t) && !refused(done, t) && spent + t.spec().price <= p.cap + 1e-9;
    // `prev` = the last tier that ran: one that could not start (no key, no budget) does not gate the next
    let mut prev: Option<Tier> = None;
    for t in p.ladder(done) {
        if started(t) {
            prev = Some(t);
            continue;
        }
        match prev.map(|q| (q, running.iter().find(|r| r.0 == q).map(|r| r.1))) {
            // plain and oxy without JS are gates: what follows waits for their verdict, and only some failures
            // are worth the next tier
            Some((q, Some(_))) if p.gate(q) => break,
            Some((q, None)) if p.gate(q) && settled(q) => break,
            // a provider still inside its hedge delay runs alone
            Some((_, Some(e))) if e < p.hedge && can(t) => {
                return Step::Wait((p.hedge - e).min(left(p, running, done)));
            }
            _ => {}
        }
        if can(t) {
            return Step::Start(t);
        }
    }
    match done.last() {
        _ if !running.is_empty() => Step::Wait(left(p, running, done)),
        Some(h) if retries(p, done, h) && can(h.tier) => Step::Start(h.tier),
        _ => Step::Stop,
    }
}

/// THE retry rule: with ONE paid provider on the keyed ladder (one key, or a gov
/// host or `provider:bd` with both, or the other one refused this run) there is no other provider to cover a transient
/// failure, so its failed call gets ONE retry after OUR cut (`provider_timeout`) or a network error, never after the
/// provider's own refusal (`provider_error`, a non-200 API answer, `provider::read`: a final refusal; retries served
/// 0/7 vs 3/9 after our cut), never on the site's own status (`Hop::status`: a 5xx page, a wall served as
/// 503), a wall or a shell (the same answer again), nor in a prefetch (its page fetch covers it), nor a detached
/// search (`one_call`: it got the retry's time at once, `timeout_of`), nor under the agent's `timeout:` (a hard budget;
/// a search cut there is not asked again either); inside `cost:` (`next`'s `can`); never with less time than the call
/// it repeats had (bd-only retries after a 60 s cut got 23.9-30 s under the wall, served 0/5): the run ends at the
/// first timeout, its row buys more time (`fix_arg`). Never after a page render's own cut (a `js` row at its 40 s; a
/// search is no render): of 131 served renders 8 took 40-60 s (1 of them read later), 0 longer, so a second render at
/// twice the time and price buys almost nothing and blocks the run 40-80 s more; that cut ends the run.
fn retries(p: &Plan, done: &[Hop], h: &Hop) -> bool {
    let keyed = p.ladder(&[]).into_iter().filter(|t| p.keys.contains(t) && t.spec().price > 0.0 && !refused(done, *t));
    let paid = uniq(keyed, |t| t.owner());
    let cut = h.code == "provider_timeout" && (p.search || !h.tier.can("js"));
    let transient = (cut || h.code == "net_error") && !one_call(p) && p.timeout.is_none();
    let first = done.iter().filter(|d| d.tier == h.tier).count() == 1;
    let time = wall_left(done, h).is_none_or(|w| w >= Duration::from_millis(h.ms));
    paid.len() == 1 && h.tier.spec().price > 0.0 && transient && first && time
}

/// THE one-key wall (bd-only took 154 s to fail, a 60 s cut, then a 120 s retry): a run with one paid provider
/// is final within `one_key_wall_s` of its first call, first visit included. What is left of it when `failed`
/// landed (the run's start = the earliest hop's landing less its time); None = no landing time kept (a test's hop).
/// The retry gets at most that (`timeout_of`), none when less than the failed call had is left (`retries`).
fn wall_left(done: &[Hop], failed: &Hop) -> Option<Duration> {
    let start = done.iter().filter_map(|h| h.end?.checked_sub(Duration::from_millis(h.ms))).min()?;
    let gone = failed.end?.saturating_duration_since(start);
    Some(Duration::from_secs(limit("one_key_wall_s")).saturating_sub(gone))
}

/// A search asked once: a detached one (no `wait:`: `cli::search_of` past `wait:`, `cli::resume`; one detached
/// search ran bd 15,004 + 14,999 ms) runs its call to the retry's time at once, never cut and asked again under
/// the same cut (bd-only `timeout:1` asked twice for one cut; `timeout:` = `retries`). An inline search keeps its
/// retry whatever its `wait:` (the 30 s default is no detach, S6).
fn one_call(p: &Plan) -> bool {
    p.search && p.wait.is_none()
}

/// A served answer the reader would flag `! thin` (`outline::is_thin`).
fn thin(h: &Hop) -> bool {
    h.ok && h.doc.as_ref().is_some_and(is_thin)
}

/// An app page (`no_copy`): a JS shell, and shells or timeouts from two different paid providers (one
/// provider's shell is that provider's failure, airbnb: bd shells, Oxylabs renders it; OUR short cut is no
/// provider's answer). The ladder runs on
/// until then, so this only names the outcome.
fn app_page(done: &[Hop]) -> bool {
    let ran = |h: &&Hop| h.tier.spec().price > 0.0 && !ours(&h.code, h.upto);
    let paid = |h: &&Hop| ran(h) && matches!(h.code.as_str(), "js_shell" | "provider_timeout");
    done.iter().any(|h| h.code == "js_shell") && uniq(done.iter().filter(paid), |h| h.tier.owner()).len() > 1
}

/// Time until the longest running call is given up on.
fn left(p: &Plan, running: &[(Tier, Duration)], done: &[Hop]) -> Duration {
    let grace = Duration::from_secs(limit("grace_s"));
    let d = running.iter().map(|&(t, e)| (p.timeout_of(t, done) + grace).saturating_sub(e)).max();
    d.unwrap_or(Duration::ZERO).max(Duration::from_millis(10))
}

pub type Call = Pin<Box<dyn Future<Output = Hop> + Send>>;

/// Run the table: start what `next` says, wait for the first answer, cancel what is left. `call` gets the tier
/// and the hops so far (a PDF run by then: `pdf_run`; its timeout: `timeout_of`).
/// A cancelled call is a hop too (`abandoned`, or cut at the plan's `wait`; its bill unverified, `billed`).
/// `done` = hops settled before the first call (`host_hop`); `seen` gets the tiers so far each time one starts or
/// lands.
pub fn drive(p: &Plan, call: &dyn Fn(Tier, &[Hop]) -> Call, mut done: Vec<Hop>, seen: Seen) -> Vec<Hop> {
    crate::provider::block(async {
        let mut set = tokio::task::JoinSet::new();
        let mut running = Vec::<Running>::new();
        let (end, mut cut) = (p.wait.map(|w| Instant::now() + w), None);
        loop {
            let now: Vec<(Tier, Duration)> = running.iter().map(|r| (r.0, r.1.elapsed())).collect();
            let step = pooled(p, &done, &now);
            let left = end.map(|e| e.saturating_duration_since(Instant::now()));
            if left == Some(Duration::ZERO) && !matches!(step, Step::Stop) {
                cut = p.wait;
                break;
            }
            match step {
                Step::Start(t) => {
                    let id = set.spawn(call(t, &done)).id();
                    running.push((t, Instant::now(), id, p.timeout_of(t, &done)));
                }
                Step::Wait(d) => {
                    let d = left.map_or(d, |l| d.min(l));
                    // a pool's turn has no call to land (`pooled`): a sleep, never a join on an empty set (a spin)
                    if set.is_empty() {
                        tokio::time::sleep(d).await;
                    } else if let Ok(Some(r)) = tokio::time::timeout(d, set.join_next_with_id()).await
                        && let Some(h) = landed(r, &mut running)
                    {
                        done.push(cut_why(p, h));
                    }
                }
                Step::Stop => break,
            }
            let mark = |h: &Hop| if h.ok { "✓".to_string() } else { format!(":{}✗", h.code) };
            let tried = done.iter().map(|h| format!("{}{}", h.tier.spec().short, mark(h)));
            let busy = running.iter().map(|r| format!("{} running", r.0.spec().short));
            // the newest running call's start, and the last running call's deadline (its timeout as it started)
            let at = |i: Instant| crate::cache::now() - i.elapsed().as_secs_f64();
            let since = running.iter().map(|r| at(r.1)).fold(0.0, f64::max);
            let until = running.iter().map(|r| at(r.1) + r.3.as_secs_f64()).fold(0.0, f64::max);
            seen((tried.chain(busy).collect::<Vec<_>>().join(" · "), since, until));
        }
        set.abort_all();
        let abandoned = |t: Tier| Hop { tier: t, code: "abandoned".into(), ..Hop::default() };
        let gone = |t: Tier| cut.map_or_else(|| abandoned(t), |wait| Hop::failed(t, crate::provider::late(wait)));
        for (t, t0, _, d) in running {
            done.push(cut_why(p, Hop { timeout_s: d.as_secs(), ..billed(t0, false, t.spec().price, gone(t)) }));
        }
        done
    })
    .unwrap_or_else(|| vec![Hop { code: "net_error".into(), ..Hop::default() }])
}

/// A batch's `cost:` pool file: each run's `[floor, hold]` (`seed`, `pooled`).
type Pool = BTreeMap<usize, [f64; 2]>;

/// THE batch's `cost:` pool (`Plan::pool`, one file a batch): under its lock this run's cap is what the other runs
/// leave (`claims`), a paid call holds its price before it starts, and every turn sets the run's hold to what it
/// `held`; its floor goes when it stops (plain served it: within its plain call): Σ billed ≤ `cost:`. The trade-offs
/// of a floor set at launch (`seed`): a queued page keeps its share until its turn; a page short of its rung stops
/// and gives its share back, so of two pages on even shares the one that meets the cap second gets both (which one
/// is a race, never input order; one of them always runs on). A run short only of the floors of runs already
/// started waits a turn (`POOL_TURN`, within its `timeout:`) with its own floor given back, never `budget_hit`
/// while those come back (2 pages at `cost:0.002`, $0.001 each: the one plain could not serve needs a $0.0015 render
/// as the other's plain serves); two such runs never wait on each other.
fn pooled(p: &Plan, done: &[Hop], running: &[(Tier, Duration)]) -> Step {
    let Some((at, me)) = &p.pool else { return next(p, done, running) };
    crate::cache::edit(at, |pool: &mut Pool| {
        let left = |soon| (p.cap - spend(claims(at, pool, *me, soon).into_iter().map(|o| o.1))).max(0.0);
        let step = next(&Plan { cap: left(false), ..p.clone() }, done, running);
        let hope = || matches!(next(&Plan { cap: left(true), ..p.clone() }, done, running), Step::Start(_));
        let turn = matches!(step, Step::Stop) && hope();
        let start = if let Step::Start(t) = step { t.spec().price } else { 0.0 };
        let floor = if matches!(step, Step::Stop) { 0.0 } else { pool.get(me).map_or(0.0, |h| h[0]) };
        pool.insert(*me, [floor, held(done, running) + start]);
        if turn { Step::Wait(p.agent_left(done).map_or(POOL_TURN, |l| l.min(POOL_TURN))) } else { step }
    })
    .unwrap_or(Step::Stop)
}

/// How often a run short of the pool looks again (`pooled`): a plain call it waits for takes ~0.1-10 s.
const POOL_TURN: Duration = Duration::from_millis(250);

/// The other runs' claims on the pool `at`, each the larger of its hold and the floor it still holds (` held`: not
/// spent yet); none = no row. A run that died (`cache::run_at`: its lock let go, killed before it gave its floor
/// back) holds no floor; its hold stays, a call it started may be billed. `soon` = only a queued run's floor
/// (`Detached::by`, not started: no pid) counts: a started one gives its floor back when it stops.
fn claims(at: &std::path::Path, pool: &Pool, me: usize, soon: bool) -> Vec<(usize, f64, &'static str)> {
    let run = |n: usize| crate::cache::run_at(&at.with_file_name(format!("u{n}.json")));
    let alive = |n| run(n).is_none_or(|(r, alive)| alive && !(soon && (r.by == 0 || r.pid != 0)));
    let claim = |(&n, &[f, h]): (&usize, &[f64; 2])| if f > h && alive(n) { (n, f, " held") } else { (n, h, "") };
    pool.iter().filter(|o| *o.0 != me).map(claim).filter(|o| o.1 > 0.0).collect()
}

/// What a run spent or holds, THE one rule of the cap: billed, maybe billed (a cut call's `upto`) and running.
fn held(done: &[Hop], running: &[(Tier, Duration)]) -> f64 {
    spend(done.iter().map(|h| h.cost + h.upto).chain(running.iter().map(|r| r.0.spec().price)))
}

/// A batch's pool at its launch: each run's floor = one paid try whatever plain says (its uncapped route's dearest
/// rung: the cheapest, oxy's $0.001, left a page plain sent to a render short), held until the run stops, so one
/// page's hedge never buys a queued page's first paid try. Floors over `cap` are cut to an even share each
/// (`cap` / pages; a first page's whole floor left the next none, which a $0.001 rung would have served): no page
/// is starved before it runs; one whose share is short stops and gives it back (`pooled`).
pub fn seed(at: &std::path::Path, runs: impl Iterator<Item = (usize, Plan)>, cap: f64) -> Result<(), String> {
    let dearest = |p: Plan| route(&Plan { cap: f64::INFINITY, ..p }).0.into_iter().map(|t| t.0.spec().price);
    let floors: Vec<(usize, f64)> = runs.map(|(n, p)| (n, dearest(p).fold(0.0, f64::max))).collect();
    let share = if spend(floors.iter().map(|f| f.1)) > cap + 1e-9 { cap / floors.len() as f64 } else { cap };
    let pool: Pool = floors.into_iter().map(|(n, f)| (n, [f.min(share), 0.0])).collect();
    crate::cache::edit(at, |old: &mut Pool| *old = pool)
}

/// Why WE cut a call (`Hop::why`, named by `fail_detail`): `timeout:8s` = the agent's `timeout:` (its fix is
/// the default time), else `took > 40 s here` (`fail_detail` reads it as `cut at 40 s (3m ago)`).
fn cut_why(p: &Plan, mut h: Hop) -> Hop {
    let s = h.timeout_s;
    if h.code == "provider_timeout" && h.status == 0 && h.why.is_empty() {
        // under `timeout:` a render gets what the agent's wall left (`agent_left`): its cut is named as the agent typed
        // it
        let user = p.timeout.map(|w| w.as_secs()).filter(|&w| (h.tier.can("js") && s <= w) || s == w);
        h.why = user.map_or_else(|| format!("took > {s} s here"), |w| format!("timeout:{w}s"));
    }
    h
}

/// THE host check's hop (`provider::host_check`), before any tier runs: a plain hop, final, $0.
fn host_hop(url: &str, wall: Option<Duration>) -> Option<Hop> {
    let t0 = Instant::now();
    let (code, why) = crate::provider::host_check(url, wall)?;
    Some(Hop { tier: Tier::Plain, code: code.into(), ms: t0.elapsed().as_millis() as u64, why, ..Hop::default() })
}

/// A running call, and its timeout (`Plan::timeout_of` when it started).
type Running = (Tier, Instant, tokio::task::Id, Duration);

/// Who watches a run (`drive`): its status, the newest running call's start and the last one's deadline (epoch
/// seconds); a detached fetch writes them to its run file; a call nobody watches passes `&drop`.
pub type Seen<'a> = &'a dyn Fn((String, f64, f64));

/// A finished call's hop, taken off `running` (a call that panicked is `parse_failed`); None = not one of ours.
fn landed(r: Result<(tokio::task::Id, Hop), tokio::task::JoinError>, running: &mut Vec<Running>) -> Option<Hop> {
    let id = r.as_ref().map_or_else(|e| e.id(), |(id, _)| *id);
    let at = running.iter().position(|x| x.2 == id)?;
    let (tier, t0, _, d) = running.remove(at);
    let failed = || Hop { tier, code: "parse_failed".into(), ms: t0.elapsed().as_millis() as u64, ..Hop::default() };
    Some(Hop { timeout_s: d.as_secs(), ..r.map(|(_, h)| h).unwrap_or_else(|_| failed()) })
}

/// THE billing rule's other half (both providers): a finished provider call → its hop: how long it took, the
/// tier's price when the provider bills its answer (`provider::charged`), else for a call WE cut (our timeout,
/// `timeout:`, `wait:`, abandoned: no status, the provider never answered) the price as `upto`, unverified: the
/// provider may finish it (bd billed ≥ 2 of 4 searches cut at 15 s; +0 for 1-s cuts); the
/// provider's own refusal, error envelope or 5xx stays $0. THE one place a hop's time and bill are set.
fn billed(t0: Instant, bill: bool, price: f64, mut h: Hop) -> Hop {
    let cut = h.status == 0 && matches!(h.code.as_str(), "provider_timeout" | "abandoned");
    (h.ms, h.end) = (t0.elapsed().as_millis() as u64, Some(Instant::now()));
    (h.cost, h.upto) = if bill { (price, 0.0) } else { (0.0, if cut { price } else { 0.0 }) };
    h
}

/// One provider call → its hop (`billed`): a failure as `Hop::failed`, an answer through `ok`.
async fn hop_of<T>(t: Tier, price: f64, call: impl Future<Output = Called<T>>, ok: impl FnOnce(T) -> Hop) -> Hop {
    let t0 = Instant::now();
    let (r, bill) = call.await;
    billed(t0, bill, price, r.map_or_else(|f| Hop::failed(t, f), ok))
}

/// A provider call's answer, and whether it may be billed (`provider::get`).
type Called<T> = (Result<T, Fail>, bool);

/// The network transport: one provider call, then the content check. A `/pdf/` path (`hint`) asks Bright Data for
/// raw bytes like a PDF run, but only a PDF run fails a page as `not_pdf`.
pub fn run(p: &Plan, ask: &Ask, seen: Seen) -> Vec<Hop> {
    let call = |t: Tier, done: &[Hop]| -> Call {
        let (mut ask, pdf) = (ask.clone(), p.pdf_run(done));
        (ask.js, ask.doc) = (t.renders(), pdf || p.hint);
        ask.timeout = Some(p.timeout_of(t, done));
        let (key, price, prior, card) = (t.key().unwrap_or_default(), t.spec().price, p.prior.clone(), !p.js && !pdf);
        let (langs, turn) = (langs(&ask, done), (t == Tier::Plain).then(|| crate::provider::pace(&ask.url)));
        Box::pin(async move {
            let ok = |raw: Raw| Hop { limit: raw.limit, ..judge(t, raw, &ask.url, ask.js, pdf) };
            let (key, ok) = (&key, &ok);
            let get = |a: Ask| async move { hop_of(t, price, crate::provider::get(t, &a, key), ok).await };
            let mut first = get(ask.clone()).await;
            if let Some(why) = first.doc.as_ref().and_then(|d| md_weak(d, prior.as_deref())) {
                // the HTML ask failing keeps the Markdown copy, its why says so (`md:thin+html:blocked_403`)
                let h = get(Ask { html: true, ..ask.clone() }).await;
                let (ms, failed) = (first.ms + h.ms, format!("{why}+html:{}", h.code));
                first = if h.ok { Hop { ms, why: why.into(), ..h } } else { Hop { ms, why: failed, ..first } };
            }
            first.waited = turn.map_or(0, |t| t.1);
            geo(if card && !ask.js { og_card(first, &ask.url) } else { first }, &langs)
        })
    };
    let hops = drive(p, &call, host_hop(&ask.url, p.agent_left(&[])).into_iter().collect(), seen).into_iter();
    hops.map(|h| Hop { country: sent(ask, h.tier).unwrap_or_default(), ..h }).collect()
}

/// THE language rule: the languages a provider's copy should be in: the `country:` mod's (`locale`),
/// else the page's own: its URL's locale words (the ccTLD, the first host label and the FIRST path segment shaped
/// `xx` / `xx-yy`: `.fr`, `de.`, `en-gb.`, `/es/es/`, `/pt-br/`; a later segment is the site's own word, Booking's
/// `/hotel/fr/` is the hotel's country: 10 of 16 false flags in a 196-copy replay) with
/// the `<html lang>` of plain's copy when plain held the page (`judge`: never a wall's). None of them = no
/// expectation, no flag: `js:true`, which skips plain, never flags a non-English `.com`. A served copy whose
/// primary tag is none of them is a provider's geo copy: its hop's `why` says `lang:it` (`oxyjs✓lang:it`),
/// `outline::page_flags` flags it on every read with the fix. Where none of them names one and no query value
/// does either (`hl=fr`, `lang=de`), a call that sends no country (`provider::sent`: Bright Data at home US) expects
/// `en`; the ask is kept for that (`geo`). Its own pick served Booking and Airbnb in Spanish (a
/// replay on 234 saved paid copies: 21 more foreign copies flagged, 0 English or own-language ones).
fn langs(ask: &Ask, done: &[Hop]) -> (Vec<String>, Option<Ask>) {
    let named = ask.country.as_deref().map(|c| locale(&c.to_lowercase())).unwrap_or_default();
    let plain = done.iter().filter(|h| h.tier == Tier::Plain && !h.lang.is_empty()).map(|h| h.lang.clone());
    let Ok(u) = url::Url::parse(&ask.url) else { return (named, None) };
    let host: Vec<&str> = u.host_str().unwrap_or_default().split('.').collect();
    let label = host.first().filter(|_| host.len() > 2).copied();
    let shaped = |s: &&str| s.len() == 2 || (s.len() == 5 && matches!(s.as_bytes()[2], b'-' | b'_'));
    let first = u.path_segments().into_iter().flatten().next();
    let words = label.into_iter().chain(first).filter(shaped).flat_map(|s| s.split(['-', '_']));
    let words = host.last().copied().into_iter().chain(words).flat_map(|w| locale(&w.to_lowercase()));
    let own = uniq(words.chain(plain), String::clone);
    let tagged = |v: &str| shaped(&v) && v.split(['-', '_']).any(|w| !locale(&w.to_lowercase()).is_empty());
    let query = u.query_pairs().any(|(_, v)| tagged(&v));
    let bare = (named.is_empty() && own.is_empty() && !query).then(|| ask.clone());
    (if named.is_empty() { own } else { named }, bare)
}

/// The languages a locale word names through `Providers::cctld`: a ccTLD (`uk`) or a country (`gb`) → its row's
/// (`ch` → de fr it), a row's language (`en`) → itself; any other word names none.
fn locale(word: &str) -> Vec<String> {
    let rows = &PROVIDERS.cctld;
    let langs = |r: &&str| r.split(' ').skip(1).map(String::from).collect::<Vec<_>>();
    let row = rows.get(word).or_else(|| rows.values().find(|r| r.split(' ').next() == Some(word)));
    row.map_or_else(|| rows.values().flat_map(langs).filter(|l| l == word).take(1).collect(), langs)
}

/// A provider's copy outside `langs` (none = no expectation; `en` when it names no locale and the call sent no
/// country) says so in its `why`, after a status it carries: `lang:it`, `403+lang:it`. A copy whose own canonical
/// link names the asked URL (`landed`) is the site's page in the site's language, so no `en` is expected of it: a
/// geo copy names its language's URL (`x.es.html`) or another site. The dropped expectation leaves a trace, never
/// a flag: `own:es` in its `why` (`bd✓own:es` in the tried path: `stats`, meta.json's hops).
fn geo(mut h: Hop, (langs, bare): &(Vec<String>, Option<Ask>)) -> Hop {
    let own = |a: &Ask| url::Url::parse(&a.url).is_ok_and(|u| h.landed.as_deref() == Some(u.as_str()));
    let unsent = bare.as_ref().filter(|a| sent(a, h.tier).is_none());
    let langs = if unsent.is_some() { &vec!["en".into()] } else { langs };
    if h.ok && h.tier != Tier::Plain && !h.lang.is_empty() && !langs.is_empty() && !langs.contains(&h.lang) {
        let tag = if unsent.is_some_and(own) { "own" } else { "lang" };
        h.why = format!("{}{}{tag}:{}", h.why, if h.why.is_empty() { "" } else { "+" }, h.lang);
    }
    h
}

/// THE Markdown guard: a server's own `text/markdown` answer (`clean::kind_of` `md`) is not served when it is thin
/// (`thin_chars` of prose) or under a fifth of the prose of the copy stored before, or flat (no heading) where
/// that copy had ≥ 3 sections; plain then asks again for HTML only (`Ask::html`), and the Markdown is served when
/// that ask fails. `prior` = the stored copy's folder: its doc.json is read only for a Markdown answer, never
/// rebuilt. The reason is the served hop's `why` (`plain✓md:thin` in `tried`).
fn md_weak(d: &Doc, prior: Option<&std::path::Path>) -> Option<&'static str> {
    if d.mode != "md" {
        return None;
    }
    let flat = d.sections.iter().enumerate().all(|(i, s)| s.title == format!("part {}", i + 1));
    let stored = prior.and_then(|p| crate::cache::jload::<Doc>(&p.join("doc.json")));
    let (n, tok) = stored.map_or((0, 0), |p| (p.sections.len(), p.tokens.clean));
    let thin = d.loss.pruned_chars < limit("thin_chars") || d.tokens.clean * 5 < tok;
    thin.then_some("md:thin").or((flat && n >= 3).then_some("md:flat"))
}

/// og_only, THE card rule (any tier but the render, oxyjs, and never in a `js:true` or PDF run): a JS shell, or a
/// 4xx that is not a missing page, whose head carries a page's own og card is served now as that card: `mode` og,
/// ok, the ladder stops (`outline` adds `! js_shell → … js:true` while a render is left). A page's own card = an
/// og:description of ≥ 40 chars, an og:title or og:image beside it, and an og:title that is not the og:site_name:
/// a wall or an app shell carrying the site-wide card (a maps shell, a login wall) is not the page. Stored as kind
/// `og` (`clean::card`), so a rebuild of the body makes the card again.
fn og_card(mut h: Hop, url: &str) -> Hop {
    let shell = !h.ok && (h.code == "js_shell" || (400..500).contains(&h.status) && !FINAL.contains(&h.code.as_str()));
    let own = |t: &Tree| {
        let (title, site) = (t.meta("og:title").map(clean::fold), t.meta("og:site_name").map(clean::fold));
        let desc = t.meta("og:description").map_or(0, |d| d.trim().chars().count());
        desc >= 40 && (title.is_some() || t.meta("og:image").is_some()) && (title.is_none() || title != site)
    };
    let Some(tree) = h.tree.as_ref().filter(|t| shell && own(t)) else { return h };
    let fin = if h.final_url.is_empty() { url } else { &h.final_url };
    if let Ok(card) = guarded(|| Ok(clean::card(clean::build_doc(tree, url, fin, false)))) {
        (h.ok, h.code, h.kind, h.doc, h.tree) = (true, "ok".into(), "og", Some(card), None);
    }
    h
}

/// The search transport: one search call per tier `next` starts; an answer that parses is `ok`, even empty.
pub fn run_search(p: &Plan, q: &str, limit: usize, country: Option<&str>) -> Vec<Hop> {
    let call = |t: Tier, done: &[Hop]| -> Call {
        let (q, country, wait, price) =
            (q.to_string(), country.map(String::from), p.timeout_of(t, done), t.spec().price);
        Box::pin(async move {
            let ok = |(hits, limit)| Hop { tier: t, ok: true, code: "ok".into(), hits, limit, ..Hop::default() };
            hop_of(t, price, crate::provider::search(t, &q, limit, country.as_deref(), wait), ok).await
        })
    };
    let cc = search_country(country);
    drive(p, &call, Vec::new(), &drop).into_iter().map(|h| Hop { country: cc.clone(), ..h }).collect()
}

/// `route`'s answer: each tier with its hedge delay (None = after the call before it), and the worst-case cost.
pub type Route = (Vec<(Tier, Option<Duration>)>, f64);

/// What `dry:true` prints: the tiers a fully failing run would try, in order, each with the delay after which it
/// starts beside the call before it (`next` hedges it) or None (it waits for that call's verdict), and the
/// worst-case cost.
pub fn route(p: &Plan) -> Route {
    // a worst case: every call runs out its time (a clock, so the one-key wall decides a retry); under the
    // agent's one wall every render fails at once, so each one left gets to start and bill; plain meets the
    // wall that keeps every rung after it (a captcha keeps oxy no-JS with both keys), and so does a page's no-JS
    // rung (cut, it would skip the next one: `ladder`)
    let page = !p.search && !p.hint && !p.pdf_run(&[]);
    let fail = |t: Tier, ms: Duration, end| {
        let ms = if p.timeout.is_some() { 0 } else { ms.as_millis() as u64 };
        let wall = t == Tier::Plain || page && !t.can("js");
        let code = if wall { CHECK.no_js_first[0] } else { "provider_timeout" }.into();
        Hop { tier: t, code, kind: "html", cost: t.spec().price, ms, end, ..Hop::default() }
    };
    let (mut done, mut path, mut clock) = (Vec::new(), Vec::new(), Instant::now());
    while let Step::Start(t) = next(p, &done, &[]) {
        // a tier hedged at or past a search's `wait:` never starts beside: it runs only after a fast failure
        let beside = match next(p, &done, &[(t, p.hedge)]) {
            Step::Start(u) if p.wait.is_none_or(|w| p.hedge < w) => Some((u, Some(p.hedge))),
            _ => None,
        };
        let start = clock;
        for (u, d) in std::iter::once((t, None)).chain(beside) {
            let (ms, at) = (p.timeout_of(u, &done), start + d.unwrap_or_default());
            path.push((u, d));
            clock = clock.max(at + ms);
            done.push(fail(u, ms, Some(at + ms)));
        }
    }
    (path, spend(done.iter().map(|h| h.cost)))
}

/// The `!` row for what the provider that served the page did not honour: `device:mobile` on Bright Data (the
/// page's language: a page flag, `outline::page_flags`).
pub fn ignored(ask: &Ask, hops: &[Hop], id: &str) -> Vec<String> {
    let bd = hops.iter().any(|h| h.ok && h.tier == Tier::Bd);
    let at = At { id, url: &ask.url, arg: "device:mobile", ..Default::default() };
    let what = "device bd · Bright Data has no device setting: desktop page";
    (ask.mobile && bd).then(|| bang(id, "ignored", what, &at)).into_iter().collect()
}

/// `!` rows for tiers a failed run never tried: one per missing key, one for the cap with what the rest would
/// cost. `subject` heads the rows (a snapshot's id, empty for a search); `id` = what the hint reruns, on `url` ("" =
/// a search; the cap's arrow priced for the URL's own plan, as its `dry:true`).
pub fn skipped(p: &Plan, hops: &[Hop], subject: &str, id: &str, url: &str) -> Vec<String> {
    let zip = p.doc == Some("zip") && p.ladder(&[])[0] == Tier::Plain;
    if hops.iter().any(|h| h.ok) || zip || hops.iter().any(|h| p.gate(h.tier) && !escalates(h)) {
        return Vec::new();
    }
    let left: Vec<Tier> = p.ladder(hops).into_iter().filter(|t| !hops.iter().any(|h| h.tier == *t)).collect();
    // two paid providers lost the page: the site refuses it, a third key is no move (so no `→ fats setup` either)
    let lost = uniq(hops.iter().filter(|h| !h.ok && h.tier.spec().price > 0.0), |h| h.tier.owner()).len();
    let mut rows = if lost >= 2 { Vec::new() } else { no_key(p, &left, subject, false) };
    let over: Vec<Tier> = left.into_iter().filter(|t| p.keys.contains(t) && !refused(hops, *t)).collect();
    // the same rule as `next`/`pooled`: a tier the cap stopped (one cut at a search's `wait:` was not); in a batch
    // the cap the agent typed, the other runs' claims read under the pool's lock (who spent it: `budget_row`)
    let others =
        |(at, me): &(std::path::PathBuf, usize)| crate::cache::edit(at, |o: &mut Pool| claims(at, o, *me, false));
    let by = p.pool.as_ref().and_then(|pool| others(pool).ok());
    let (spent, others) = (held(hops, &[]), spend(by.unwrap_or_default().iter().map(|o| o.1)));
    if over.iter().any(|t| spent + others + t.spec().price > p.cap + 1e-9) {
        let want = spent + spend(over.iter().map(|t| t.spec().price));
        let names: Vec<&str> = over.iter().map(|t| t.spec().short).collect();
        // up to the next $0.0001, past a sum's float noise: 3 × $0.0015 sums to 0.0045000000000000005, never $0.0046
        let (cap, want) = (p.cap, (want * 1e4 - 1e-6).ceil() / 1e4);
        let what = format!("{} not tried · ${want:.4} > cost:{cap}", names.join(" "));
        rows.push(bang(subject, "budget_hit", &what, &At { id, url, arg: &want.to_string(), ..At::default() }));
    }
    rows
}

/// The document kind a path's suffix names (`Sites::doc_ext`), lower case in: `html` = a page.
pub fn doc_kind(path: &str) -> Option<&'static str> {
    SITES.doc_ext.iter().find(|e| path.ends_with(e.0)).map(|e| e.1)
}

/// The tiers with a key configured (`Tier::key`). Unit tests run with the keys of the world they model
/// (`tests::KEYED`): the key dir of the machine that runs them never changes an answer (the binary's tests set
/// their own).
pub fn keyed() -> Vec<Tier> {
    #[cfg(test)]
    let has = |t: &Tier| tests::KEYED.contains(t);
    #[cfg(not(test))]
    let has = |t: &Tier| t.key().is_some();
    Tier::ALL.into_iter().filter(has).collect()
}

/// THE `no_key` row (fetch, search, dry, setup, account), one per provider of `left` without a key: what its key
/// would add to this run and the price, `! no_key bd · would try unblock+JS ≤$0.0015 → fats setup bd` (the key's
/// one way in: its form, which names the sign-up page; a key in the environment is never sent); `setup` = said
/// inside `fats setup`: what the key adds and its form, no arrow (`offer`).
pub fn no_key(p: &Plan, left: &[Tier], subject: &str, setup: bool) -> Vec<String> {
    let row = |o: &Tier| {
        let (s, mine) = (o.owner().spec(), left.iter().filter(|t| t.owner() == o.owner()));
        let buys = if p.search { vec!["web search"] } else { uniq(mine.clone().map(|t| t.spec().buys), |b| *b) };
        let (cost, sw) = (spend(mine.map(|t| t.spec().price)), o.switch());
        let kept = crate::settings::key(sw).map_or("", |_| " (key kept)");
        // a switch turned off, or bd's key without its zone, is no missing key: its row says which (`! off free`)
        let (code, what, arg) = match (crate::settings::on(sw), sw) {
            (false, "free") => ("off", "free fetch turned off".into(), sw),
            (false, _) => ("off", format!("turned off{kept}"), sw),
            (true, "bd") if !kept.is_empty() => ("no_zone", "key saved, no Web Unlocker zone yet".into(), "<name>"),
            _ => ("no_key", String::new(), ""),
        };
        if code != "no_key" {
            return bang(subject, code, &format!("{sw} · {what}"), &At { arg, ..At::default() });
        }
        if setup {
            return format!("! no_key {}", offer(o.owner()));
        }
        let what = format!("{} · would try {} ≤${cost:.4}", s.short, buys.join(" + "));
        let next = fix("no_key", &At { arg: s.short, ..At::default() });
        Flag { code: "no_key".into(), what, next, row: true }.bang(subject)
    };
    // the tiers without a key, one row a provider (both Oxylabs tiers share one), the cheapest first
    let mut owners = uniq(left.iter().filter(|t| !p.keys.contains(t)), |t| t.owner());
    let cost = |o: &&Tier| spend(left.iter().filter(|t| t.owner() == o.owner()).map(|t| t.spec().price));
    owners.sort_by(|a, b| cost(a).total_cmp(&cost(b)));
    owners.into_iter().map(row).collect()
}

/// What provider `o`'s key adds, said inside `fats setup` (and `keys`) with no price: an agent relays a bare `≤$X` on
/// a status row as the price of an answer. Then its form, for the user (`oxy · search + JS pages · fats setup oxy,
/// 3 steps for the user`); a provider that cannot search says so (`decodo · JS pages, no web search · …`).
fn offer(o: Tier) -> String {
    let (b, s) = (bin(), o.spec().short);
    if Tier::ALL.into_iter().any(|t| t.owner() == o && t.can("search")) {
        format!("{s} · search + JS pages · {b} setup {s}, 3 steps for the user")
    } else {
        format!("{s} · JS pages, no web search · {b} setup {s}")
    }
}

/// The move to the key a web search needs first, the cheapest searcher's (`→ fats setup oxy`, `lang::fix`).
pub fn search_key() -> String {
    fix("no_key", &At { arg: searchers().first().copied().unwrap_or_default(), ..At::default() })
}

/// The providers a web search runs on, cheapest first by the price a search there is billed at (a tier's one price
/// a request, page or search: `billed`): the first is the key to add (`oxy`).
pub fn searchers() -> Vec<&'static str> {
    let mut all: Vec<Tier> = Tier::ALL.into_iter().filter(|t| t.can("search")).collect();
    all.sort_by(|a, b| a.spec().price.total_cmp(&b.spec().price));
    all.into_iter().map(|t| t.spec().short).collect()
}

/// The heads of `no_key`'s rows: a provider's key missing, its switch off (`! off oxy · …`), bd's zone missing.
pub const KEY_ROWS: [&str; 3] = ["! no_key ", "! off ", "! no_zone "];

/// A failed run's stored rows for the tiers it never tried (`skipped`).
pub fn skips(m: &Meta) -> impl Iterator<Item = &str> {
    m.extra.get("skipped").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|r| r.as_str())
}

/// A failed run's stored `budget_hit` row (`skipped`: the cap stopped it) as read now: a batch page's says who spent
/// the batch's pool (`m`'s `pool`), every page with a claim, this one too, the parts summing to it: `(spent $0.0020:
/// u1 $0.0010, u3 $0.0010)` (the others alone read as $0.0010 left). Read at each print, never stored: a page stops
/// while another still holds an unbilled try (`u1 $0.0015 held`), which it bills after ($0.0020). Its arrow is the
/// reader's (`lang::restamp` for `id`), through the settle gate (`clear`, `others` = the session's other copies):
/// one that plans a settled tier is dropped, the row stays. Then, for a user who asked to pay, one provider's ask
/// by name (` · if the user said pay, fats u1 fresh:true provider:oxy ≤$0.0025`, `left_ask`): a provider that
/// fetches without JS first (oxy, then decodo: the table's order; its ask starts at that call and renders after),
/// then bd; past the same gate, priced as that provider's whole path (`dry:true`'s `up to`). Only while two paid
/// providers have a key: with one, the ask would repeat the row's own move.
pub fn budget_row(m: &Meta, id: &str, others: &[Meta]) -> Option<String> {
    let row = skips(m).find(|r| r.starts_with("! budget_hit "))?;
    let at = std::path::Path::new(m.extra.get("pool").and_then(|v| v.as_str()).unwrap_or_default());
    let all = crate::cache::jload(at).map(|pool: Pool| claims(at, &pool, 0, false)).filter(|a| !a.is_empty());
    let ids = all.iter().flatten().map(|o| format!("u{} ${:.4}{}", o.0, o.1, o.2)).collect::<Vec<_>>().join(", ");
    let by = all.map(|a| format!(" (spent ${:.4}: {ids}) → ", spend(a.iter().map(|o| o.1))));
    let row = crate::lang::restamp(&row.replacen(" → ", by.as_deref().unwrap_or(" → "), 1), id, &m.url);
    let row = row.split_once(" → ").filter(|r| !clear(r.1, m, others)).map_or(row.clone(), |r| r.0.into());
    let mut own: Vec<Tier> = keyed().into_iter().filter(|t| t.owner() == *t && t.spec().price > 0.0).collect();
    own.sort_by_key(|t| t.can("js"));
    let pay = left_ask(id, &m.url, &own, &[], |a| clear(a, m, others)).filter(|_| own.len() > 1);
    Some(row + &pay.map_or(String::new(), |a| format!(" · if the user said pay, {}", a.1)))
}

/// A failed run's stored `no_key` rows (`skipped`) whose provider still has no key (`Tier::switch` names it).
pub fn no_key_left(m: &Meta) -> Vec<&str> {
    let left = |n: &str| !keyed().iter().any(|t| t.switch() == n);
    skips(m).filter(|&r| KEY_ROWS.iter().find_map(|k| r.strip_prefix(k)?.split(' ').next()).is_some_and(left)).collect()
}

/// THE re-render rule: `js:true` on a page whose copy a render sent never asks that render again while another
/// provider's render has a key: it sends the same copy (bd's thin Reddit copy, bought twice).
pub fn rendered(m: &Meta, keys: &[Tier]) -> Option<Tier> {
    let t = Tier::parse(&m.provider).filter(|t| t.can("js"))?;
    keys.iter().any(|k| k.can("js") && k.owner() != t.owner()).then_some(t)
}

/// A government host by its labels (`Sites::gov`), never a site list: its last label is one (`sec.gov`,
/// `army.mil`), or the one before a two-letter country code under which it sits (`data.gov.uk`, `x.gouv.fr`,
/// not `go.pl`). Bright Data refuses such hosts; one it refuses outside the rule (`europa.eu`) costs one $0 hop.
fn gov(url: &str) -> bool {
    let host = clean::host_of(url);
    let l: Vec<&str> = host.trim_end_matches('.').rsplit('.').collect();
    SITES.gov.contains(&l[0]) || l.len() >= 3 && l[0].len() == 2 && SITES.gov.contains(&l[1])
}

/// The tiers a fix must not send again (`lang::At::done`): every tier this run called, every tier without a key
/// (`keys`: a hint never names a provider the user has no key for), bd on a gov host, and a no-JS
/// tier after a JS shell or challenge (`Check::render_first`: oxy no-JS served 0/33 there). Once no other paid
/// tier is left, a no-JS tier whose only signal was OUR cut (`ours`: it never said no) is not spent: it is the
/// move, at twice its time (`timeout_of`; oxy cut at 10 s served a thread 6 of 6 later). A render we cut is
/// spent: no longer call follows it (`fix_arg`).
/// `cuts` = the hops of the URL's other copies this session (cli `seen`): a tier another copy of the URL tried is no
/// fix either (a `provider:oxy` offer never shows again after the `provider:oxy` copy).
pub fn spent(m: &Meta, cuts: &[HopRow], keys: &[Tier]) -> Vec<&'static str> {
    let all_hops: Vec<&HopRow> = m.hops.iter().chain(cuts).collect();
    let js_wall = all_hops.iter().any(|h| CHECK.render_first.contains(&h.code.as_str()));
    let site_no =
        |t: &Tier| all_hops.iter().any(|h| h.code == "error_page" && Tier::parse(&h.provider) == Some(t.owner()));
    let barred = |t: &Tier| !keys.contains(t) || gov(&m.url) && !t.can("gov") || js_wall && !t.can("js") || site_no(t);
    // Nor, then, one whose only word was its own error (`provider_error`) unless another copy's too or it settled
    let error = |h: &HopRow| h.code == "provider_error" && !cuts.iter().any(|c| c.provider == h.provider);
    let sett = settled(m, cuts, None);
    let error = |h: &HopRow| error(h) && !sett.contains(&h.provider.as_str());
    let done = |all: bool| {
        let gate = |h: &HopRow| ours(&h.code, h.upto) && Tier::parse(&h.provider).is_some_and(|t| !t.can("js"));
        // a rate limit is a wait, never the tier's answer (bd's `rate_limited` → `provider:bd`)
        let hops = all_hops.iter().filter(|h| h.code != "rate_limited" && (all || !gate(h) && !error(h)));
        let tiers = hops.filter_map(|h| Tier::parse(&h.provider)).chain(Tier::ALL.into_iter().filter(barred));
        tiers.map(|t| t.spec().short).collect::<Vec<_>>()
    };
    let all = done(true);
    let rest = Tier::ALL.iter().any(|t| t.spec().price > 0.0 && !all.contains(&t.spec().short));
    if rest { all } else { done(false) }
}

/// THE settle rule: the tiers (names) a provider's error settled for `m`'s URL: its own 4xx (the same request is
/// refused again: Oxylabs' 400-404 are not retryable) or its own retries given up (`faulted`). The hop keeps no
/// status, so it is read from `why`, which `provider::reason` starts with it (`400 url: value not supported`).
/// `m`'s own hops settle; another copy's (`cuts`) settles every copy when it gave up, is a 404 or names the `url`,
/// else only when the arrow's own request (`country` its `country:` mod) sends that tier the country the refused
/// call sent (`provider::sent`: `400 geo_location: …` refuses that country again, never another).
pub fn settled<'a>(m: &'a Meta, cuts: &'a [HopRow], country: Option<&str>) -> Vec<&'a str> {
    let is = |h: &HopRow| h.code == "provider_error" && (h.why.starts_with('4') || h.why.contains("faulted"));
    let all = |h: &HopRow| h.why.contains("faulted") || h.why.starts_with("404") || h.why.contains(" url: ");
    let ask = Ask { url: m.url.clone(), country: country.map(String::from), ..Ask::default() };
    let same = |h: &HopRow| Tier::parse(&h.provider).is_some_and(|t| sent(&ask, t).unwrap_or_default() == h.country);
    let hops = m.hops.iter().chain(cuts).filter(|h| is(h) && (all(h) || same(h)));
    hops.map(|h| h.provider.as_str()).collect()
}

/// A call WE cut (`upto`: no answer) is no provider's answer: no `app_page` vote.
fn ours(code: &str, upto: f64) -> bool {
    code == "provider_timeout" && upto > 0.0
}

/// The cut a failed run's move and reason follow when the run ended in OUR cut, whatever code
/// heads the row (`outcome` puts a wall any tier met there): the tiers decide, not the head. Of two cuts, the first
/// render's of the table (`js:true`'s order, one provider at a time: bd), else the last.
pub fn cut_of(m: &Meta) -> Option<&HopRow> {
    let last = m.hops.iter().rfind(|h| h.code != "abandoned")?;
    let ended =
        last.code == "provider_timeout" && (m.code == last.code || m.code.ends_with("_wall") || last.upto > 0.0);
    let all = m.hops.iter().enumerate().filter(|(_, h)| ended && h.code == "provider_timeout");
    let first = Tier::ALL.into_iter().find(|t| t.can("js")).map(|t| t.spec().name);
    all.max_by_key(|(i, h)| (Some(h.provider.as_str()) == first, *i)).map(|(_, h)| h)
}

/// The code a failed run's move follows (`lang::fix`): a timeout's when it ended in our cut (`cut_of`), else its own.
pub fn move_code(m: &Meta) -> &str {
    if cut_of(m).is_some() { "provider_timeout" } else { &m.code }
}

/// `{arg}` of a failed fetch's fix (`lang::fix`), THE timeout move, judged on the session's hops of the URL (`cuts`
/// = its other copies'): after the agent's own `timeout:` cut shorter than the tier's time, none (`fresh:true`
/// = the default time, never `nothing left to try`); with no other keyed tier left untried, None = a COUNT, no
/// arrow, the run's end (no longer call after our cut: 0 of 131 saved renders answered in 60-150 s, the
/// four `timeout:150` calls that served did so in under 60 s), as after an error page with nothing left;
/// else a render (`js:true`, `lang::fix` names the keyed tier left; shells + timeouts = `no_copy`).
pub fn fix_arg(m: &Meta, cuts: &[HopRow]) -> Option<&'static str> {
    let (cut, done) = (cut_of(m), spent(m, cuts, &keyed()));
    // in a timeout's move a tier WE cut is tried too, else two cut renders defer to each other (`js:true`)
    let tried = |t: &Tier| cut.is_some() && m.hops.iter().chain(cuts).any(|h| h.provider == t.spec().name);
    let open = |t: &Tier| t.spec().price > 0.0 && !done.contains(&t.spec().short);
    let left = |not: Tier| Tier::ALL.iter().any(|t| open(t) && *t != not && !tried(t));
    // the certificate's answer (`provider::plain`): the keyed no-JS tier by name (`provider:oxy`), else none
    let no_js = Tier::ALL.iter().find(|t| open(t) && !t.can("js")).map(|t| ASK[*t as usize].as_str());
    let short = |h: &HopRow, t: Option<Tier>| t.is_none_or(|t| h.timeout_s < t.spec().timeout.as_secs());
    // The tier whose own failure ended the run, still unspent (`spent`: its error, the one tier left), by name
    let err = |h: &HopRow| Tier::parse(&h.provider).filter(|t| h.code == m.code && !done.contains(&t.spec().short));
    match cut.map(|h| (h, Tier::parse(&h.provider))) {
        None if m.code == "tls_failed" => no_js,
        // The site refused every keyed provider with its error page: a count; so is a site's 4xx
        None if (m.code == "error_page" || m.code.starts_with("http_4")) && !left(Tier::Plain) => None,
        // a settled tier is never asked again: `fail_row`'s gate (`settled`) names the tier left
        None => Some(m.hops.iter().find_map(err).map_or("", |t| &ASK[t as usize])),
        Some((h, t)) if h.why.starts_with("timeout:") && short(h, t) => Some(""),
        Some((_, Some(t))) if !left(t) => None,
        _ => Some("js:true"),
    }
}

/// The re-render rule (`rendered`) for `url` applied to `p`, from the URL's newest stored copy.
pub fn route_of(c: &Cache, url: &str, p: &mut Plan) {
    let prior = c.lookup(url).and_then(|(dir, _)| c.meta(&dir));
    p.avoid = prior.filter(|_| p.js).and_then(|m| rendered(&m, &p.keys));
}

/// The most an arrow (`fats u1 fresh:true …`, the words after the binary) can spend on `url`
/// ("" = its id's page, or a search), with the tiers it would call in order (`outline::js_next` names the first,
/// `fail_row`'s settle gate reads them all); None = it spends nothing. THE `≤$X` of every paid arrow
/// (`lang::priced`): `dry:true`'s `up to` for that command, the same plan (`Plan::of`, `route_of`) through the same
/// `route`.
pub fn worst(arrow: &str, url: &str) -> Option<Route> {
    // The words after the binary that heads the arrow or a `· ` part of it (`check the certificate · fats u1 …`),
    // never after its name inside them, before its price
    let at = format!(" · {arrow}").split_once(&format!(" · {} ", bin())).map(|p| p.1.to_string()).unwrap_or_default();
    let words = at.split(" ≤$").next().unwrap_or_default();
    let cmd = crate::lang::parse(&[words.to_string()]);
    // a command that can spend: a fetch or search past the cache (`account`, `setup`: free reads)
    // GitHub's search is free (a re-read once quoted ≤$0.0025, a web search's)
    let spends = ["fresh", "js", "provider", "timeout"].iter().any(|k| cmd.get(k).is_some());
    if !spends || !cmd.reserved.is_empty() || crate::cli::is_gh(&cmd) {
        return None;
    }
    let page = cmd.urls.first().map(String::as_str).or((!url.is_empty() || !cmd.ids.is_empty()).then_some(url));
    let mut p = Plan::of(&cmd, page);
    let s = crate::settings::get();
    route_of(&Cache::at(s.cache.value.clone(), s.session.value.clone()), page.unwrap_or_default(), &mut p);
    Some(route(&p))
}

/// THE settle gate, every printed paid arrow (`fail_row`, `outline::js_next`, `outline::geo_flags`, `budget_row`):
/// false when the arrow's own dry (`worst`: its tiers, by name or ladder) plans a tier settled for `m`'s URL
/// (`settled`, for the country the arrow's request sends), the hops of the URL's `others` copies read too.
pub fn clear(arrow: &str, m: &Meta, others: &[Meta]) -> bool {
    let cuts: Vec<HopRow> = others.iter().flat_map(|o| o.hops.clone()).collect();
    let sett = settled(m, &cuts, crate::lang::parse(&[arrow.into()]).get("country"));
    worst(arrow, &m.url).is_none_or(|r| r.0.iter().all(|t| !sett.contains(&t.0.spec().name)))
}

/// A failed record's count once another record of the same URL or query in this session (`others`) failed the
/// same way: ` · failed k/n here` (the same failures / the records). With a fix (`arg`) the same way = the same
/// move code and the same LAST tier, code and country (a lap ending where an earlier one ended, whatever
/// tiers ran before or time each call had), and the caller drops the arrow (it would send the call that failed
/// again: a bd outage = an endless $0 loop; consent.google's 400). With no fix (None) every copy failed with the
/// same move code counts, whatever path, said from the first (`failed 1/1 here` on a one-key error page, the row
/// with no arrow). A code is the move's (`move_code`: an older build's stored `no_copy` whose renders were both cut is
/// the timeout it was).
pub fn fail_count<'a>(m: &Meta, others: impl Iterator<Item = &'a Meta>, arg: Option<&str>) -> Option<String> {
    let same: Vec<&Meta> = others.filter(|s| s.url == m.url).collect();
    let last = |s: &Meta| s.hops.last().map(|h| [&h.provider, &h.code, &h.country].map(String::clone));
    let way = |s: &Meta| move_code(s) == move_code(m) && (arg.is_none() || last(s) == last(m));
    let k = 1 + same.iter().filter(|s| !s.ok && way(s)).count();
    (arg.is_none() || k > 1).then(|| format!(" · failed {k}/{} here", same.len() + 1))
}

/// D1: THE failed copy's row parts, for `cli::no_doc` and `outline::page_flags`: why it failed (`fail_detail`, `page`
/// when no reason), its count (`fail_count`), its fix (`lang::fix`, None = a count or no fix), the accounts named.
/// No count when a newer copy `served` (its row points there, never "failed" beside the page).
pub fn fail_row(m: &Meta, others: &[Meta], page: &str, id: &str, served: bool) -> FailRow {
    let cuts: Vec<HopRow> = others.iter().flat_map(|o| o.hops.clone()).collect();
    let done = spent(m, &cuts, &keyed());
    let arg = fix_arg(m, &cuts);
    let at = At { id, url: &m.url, arg: arg.unwrap_or_default(), done: &done };
    // the settle gate (`clear`): an arrow that plans a settled tier names the keyed paid tier left whose dry plans
    // none (`left_ask`), else none: a count from the first copy (oxy-only: `failed 1/1`)
    let gate = |a: &str| clear(a, m, others);
    let (arg, arrow) = match arg.map(|_| fix(move_code(m), &at)) {
        Some(a) if !gate(&a) => left_ask(id, &m.url, &Tier::ALL, &done, gate).unzip(),
        arrow => (arg, arrow),
    };
    let again = fail_count(m, others.iter(), arg).filter(|_| !served);
    // a run the cap stopped: its `budget_hit` row (`skipped`) is the one move while it has one
    let budget = budget_row(m, id, others).is_some_and(|r| r.contains(" → "));
    // a repeat drops the arrow only when it changes nothing: the same call again (an empty `arg`); a tier never
    // asked (both keys: `provider:bd` after oxyjs cut twice) stays the move beside the count
    let arrow = arrow.filter(|_| !(again.is_some() && arg == Some("")) && !budget);
    // a key this run lacked could still serve it (its `no_key` row): that is the move, never "nothing left" or none
    let none = arrow.as_ref().is_none_or(|a| a.starts_with("nothing left")) && !budget && !no_key_left(m).is_empty();
    let arrow = if none { Some(fix("no_key", &At::default())) } else { arrow };
    let (why, owners) = fail_detail(m, page);
    (why, again.unwrap_or_default(), arrow, owners)
}

/// `fail_row`: (why, ` · failed k/n here`, the fix, the accounts named).
pub type FailRow = (String, String, Option<String>, Vec<&'static str>);

/// THE paid ask by name: the "keyed paid tier left" arrow (`lang::fix` after a render or a refusal, `fail_row`'s
/// settle gate; `tiers` = the table) and a `budget_hit` row's provider ask (`budget_row`): the first of `tiers` that
/// is paid and not `done` whose `fats <id> fresh:true <ask>` passes `ok`, priced (`lang::priced`), and its ask.
pub fn left_ask(id: &str, url: &str, tiers: &[Tier], done: &[&str], ok: impl Fn(&str) -> bool) -> Asked {
    let left = tiers.iter().copied().filter(|t| t.spec().price > 0.0 && !done.contains(&t.spec().short));
    let mut arrows = left.map(|t| (ASK[t as usize].as_str(), format!("{} {id} fresh:true {}", bin(), ASK[t as usize])));
    arrows.find(|a| ok(&a.1)).map(|(a, arrow)| (a, priced(&arrow, url)))
}

/// `left_ask`'s answer: the ask (`provider:oxy`) and the whole priced arrow; None = no tier passes.
pub type Asked = Option<(&'static str, String)>;

/// A tier asked by name (`provider:oxy js:true`: its owner, and `js:true` for an owner's render rung).
static ASK: LazyLock<Vec<String>> = LazyLock::new(|| {
    let ask = |t: Tier| format!("provider:{}{}", t.owner().spec().short, if t.renders() { " js:true" } else { "" });
    Tier::ALL.map(ask).to_vec()
});

/// What a failed run's `!` row names: a refused key is named as `fats config` lists it (`oxy`, `bd`); a failure the
/// provider gave a reason token for (`Hop::why`) names the account and the token (`bd proxy_error`, `oxy retry in 5s`,
/// `bd cut at 40 s (3m ago)`); never the provider's answer itself. Any other failure names `page`. Also the accounts
/// the row names, for their dashboard pages (`provider::pages`). THE reason of every failed copy's row, with or without
/// a doc.
pub fn fail_detail(m: &Meta, page: &str) -> (String, Vec<&'static str>) {
    // the account a hop bills (`oxy` for oxyjs), a tier's or one of its own (`gh`: GitHub)
    let owner = |h: &HopRow| {
        let tier = Tier::parse(&h.provider).map(|t| t.owner().spec().short);
        tier.or(crate::provider::links(&h.provider).map(|l| l.short))
    };
    if m.code != "bad_key" {
        let code = move_code(m);
        let last = cut_of(m).or(m.hops.iter().rev().find(|h| h.code == code));
        // Our cut, stored as `took > 40 s here` (0.1.6 rows too), reads its time and age: `cut at 40 s (3m ago)`;
        // No age under a minute (the answer that made the cut said `(0s ago)`)
        let age = crate::cache::now() - m.t;
        let age = if age < 60.0 { String::new() } else { format!(" ({} ago)", fmt_age(age)) };
        let ours = |h: &HopRow| h.why.strip_prefix("took > ").map(|_| format!("cut at {} s{age}", h.timeout_s));
        let why = last.map(|h| ours(h).unwrap_or(h.why.clone()));
        // a certificate's refusal names its reason, never the http copy's: `certificate-verify-failed · $0`
        let tls = why.clone().filter(|_| code == "tls_failed").and_then(|w| Some(w.strip_prefix("tls:")?.to_owned()));
        // a call we cut or a provider's error names its tier (`oxyjs cut at 40 s`, `oxyjs 400 url: value not
        // supported`), a provider's other reason its account (`oxy retry in 5s`)
        let cut = last.and_then(|h| Tier::parse(&h.provider));
        let cut = cut.filter(|_| matches!(code, "provider_timeout" | "provider_error"));
        return match (last.and_then(owner), why.filter(|w| !w.is_empty())) {
            _ if let Some(t) = tls => (format!("{} · $0", t.split('+').next().unwrap_or_default()), Vec::new()),
            (Some(o), Some(w)) => (format!("{} {w}", cut.map_or(o, |t| t.spec().short)), vec![o]),
            _ => (page.into(), Vec::new()),
        };
    }
    // the accounts whose hop ended with the run's code, in call order
    let keys = uniq(m.hops.iter().filter(|h| h.code == m.code).filter_map(owner), |k| *k);
    (keys.join(" "), keys)
}

/// Every provider error answer of a run, one block each (`# oxylabs 500 provider_error`), for `error.txt`.
pub fn error_text(hops: &[Hop]) -> Option<Vec<u8>> {
    let block =
        |h: &Hop| [format!("# {} {} {}\n", h.tier.spec().name, h.status, h.code).as_bytes(), &h.err, b"\n"].concat();
    let out: Vec<u8> = hops.iter().filter(|h| !h.err.is_empty()).flat_map(block).collect();
    (!out.is_empty()).then_some(out)
}

/// The chain's result as a snapshot: the winner (or the best failed copy), meta with every hop.
pub struct Outcome {
    pub meta: Meta,
    pub file: Option<(String, Vec<u8>)>,
    pub doc: Option<Doc>,
    /// `error_text` of the hops: stored as `error.txt` (0600), never printed.
    pub error: Option<Vec<u8>>,
}

pub fn outcome(url: &str, hops: &mut [Hop], ms: u64) -> Outcome {
    // the first full answer, else the first thin one (`next` waited for the other provider)
    let win = hops.iter().position(|h| h.ok && !thin(h)).or(hops.iter().position(|h| h.ok));
    // no winner: the cut PDF with the most pages read, else the HTML answer with the most visible text
    let pages = |h: &Hop| h.doc.as_ref().map_or(0, |d| d.sections.len() - crate::pdf::empty_pages(d).len());
    let score = |h: &Hop| match h.kind {
        "pdf" if h.doc.is_some() => Some((1, pages(h))),
        "html" if !h.body.is_empty() => Some((0, h.vis)),
        _ => None,
    };
    let wall = hops.iter().rfind(|h| h.code.ends_with("_wall"));
    let best = win.or_else(|| hops.iter().enumerate().filter_map(|(i, h)| Some((score(h)?, i))).max().map(|x| x.1));
    // a failed run is named by its last answer; a cut PDF shown as the best copy by its own code (`pdf_truncated`)
    let code = match (win, best.filter(|&i| hops[i].kind == "pdf")) {
        (Some(_), _) => "ok".to_string(),
        (None, Some(i)) => hops[i].code.clone(),
        // An app page (`app_page`): no copy from two providers, not a fault a retry fixes
        _ if app_page(hops) => "no_copy".into(),
        // A wall any tier met says more than the last tier's timeout (oxy-only Joomla: `cloudflare_wall`, not
        // `provider_timeout`)
        _ => match (hops.iter().rev().find(|h| h.code != "abandoned"), wall) {
            // a `one_render` gate names the run only when its render met it too (a shell, the gate), never a
            // render cut or refused: that one keeps its own fix (the other keyed provider, else the run's end)
            (Some(l), Some(w)) if CHECK.one_render.contains(&w.code.as_str()) => {
                if l.code == "js_shell" { w } else { l }.code.clone()
            }
            (Some(l), Some(w)) if l.code == "provider_timeout" => w.code.clone(),
            (last, _) => last.map_or("no_key".into(), |h| h.code.clone()),
        },
    };
    let error = if win.is_some() { None } else { error_text(hops) };
    let cost = spend(hops.iter().map(|h| h.cost));
    let landed = best.and_then(|i| hops[i].landed.clone());
    let refreshed_from = best.and_then(|i| hops[i].refreshed_from.clone());
    let (tier, kind, final_url, body, doc, tree, charset) = match best {
        Some(i) => {
            let h = &mut hops[i];
            (h.tier, h.kind, h.final_url.clone(), std::mem::take(&mut h.body), h.doc.take(), h.tree.take(), h.charset)
        }
        None => (hops.last().map_or(Tier::Plain, |h| h.tier), "html", String::new(), Vec::new(), None, None, ""),
    };
    let render = tier.renders();
    let fin = if final_url.is_empty() { url } else { &final_url };
    // `judge` parsed every body once (a PDF into its doc); a failed HTML answer is cleaned here from that parse
    let doc = doc.or_else(|| tree.and_then(|t| guarded(|| Ok(clean::build_doc(&t, url, fin, render))).ok()));
    let meta = Meta {
        url: url.into(),
        final_url: (!final_url.is_empty() && final_url != url).then_some(final_url),
        landed,
        refreshed_from,
        provider: tier.spec().name.into(),
        ms: ms.saturating_sub(hops.iter().map(|h| h.waited).sum()),
        cost: (cost * 1e6).round() / 1e6,
        ok: win.is_some(),
        code,
        t: crate::cache::now(),
        bytes: body.len() as u64,
        kind: kind.into(),
        hops: hops.iter().map(Hop::row).collect(),
        render: Some(render),
        charset: charset.into(),
        ..Meta::default()
    };
    let file = (!body.is_empty()).then(|| (raw_name(kind), body));
    Outcome { meta, file, doc, error }
}

// ---------------------------------------------------------------- content check

/// The content check's tables (`src/data/check.json`). `rules` = the walls, tried in order before anything else
/// (`Wall`); `stop` = title words that say nothing about the page; `js_needed` = sentences asking for JavaScript
/// (they name the page, "enable javascript to see google maps", so they never count as the page showing its
/// title); `render_first` = plain's codes after which a provider's own no-JS fetch is skipped for its render
/// (`Plan::ladder`), `no_js_first` = the ones after which one no-JS try runs before the other provider's render,
/// `sign_in` = the words that make a moved landing a sign-in (`sign_in_gate`; whole words, a compound's last).
/// `one_render` = the ones after which ONE render runs (a sign-in gate answers every render alike, `outcome`).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, bound(deserialize = "'de: 'static"))]
pub struct Check {
    pub rules: Vec<Wall>,
    stop: Vec<&'static str>,
    js_needed: Vec<&'static str>,
    render_first: Vec<&'static str>,
    no_js_first: Vec<&'static str>,
    one_render: Vec<&'static str>,
    sign_in: Vec<&'static str>,
}

pub static CHECK: LazyLock<Check> = LazyLock::new(|| crate::settings::table("check"));

/// A wall rule: a page whose `on` part holds any of `any` (a header line by its start, the rest anywhere), with
/// fewer than `under` visible chars when set, is `code` ("" = the HTTP status's own code, `status_code`). Bot-wall
/// vendors (DataDome & co.) tag every answer of a protected site: only a short page is their challenge (etsy: 14k
/// chars, ok); Cloudflare's `cf-mitigated: challenge` is one at any length. No row walls a copy that holds the page
/// (`check`: its own words shown).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wall {
    on: On,
    any: Vec<&'static str>,
    under: Option<usize>,
    pub code: &'static str,
}

/// Where a wall rule looks: response header lines, the raw HTML (unescaped, folded), the visible text, or the HTTP
/// status (an error status, ≥ 400, is a wall of its own).
#[derive(serde::Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum On {
    Header,
    Html,
    Text,
    Status,
}

/// A page's language: its `<html lang>` primary tag when that is 2–3 ASCII letters, lower case (a 10 kB
/// `lang` reached every printed read); "" when it names none.
fn page_lang(tree: &Tree) -> String {
    let tag = tree.nodes.iter().find(|n| n.tag == "html").and_then(|n| n.attr("lang")?.split(['-', '_']).next());
    let tag = tag.map(str::trim).filter(|l| (2..=3).contains(&l.len()) && l.bytes().all(|b| b.is_ascii_alphabetic()));
    tag.unwrap_or_default().to_lowercase()
}

/// Body check for one answer → a hop (ok, code, visible chars, doc when it passed). DETECT (`clean::kind_of`) runs
/// on every body, any tier. A PDF is read here: a body without its end that lost a page (`pdf::cut_short`) is
/// `pdf_truncated`, its doc kept as the best copy. A text document (`clean::Lists::text`) that the site sent with a
/// success status is served as it is, so the ladder stops: no content check, no provider. In a PDF run (`pdf`) an
/// answer without the PDF magic is `not_pdf` (plain's keeps its wall code when the check names one): the next
/// tier is tried.
pub fn judge(tier: Tier, raw: Raw, url: &str, js: bool, pdf: bool) -> Hop {
    let (kind, magic) = (clean::kind_of(&raw.ctype, &raw.body), clean::sniff(&raw.body));
    let mut h = Hop { tier, kind, status: raw.status, charset: raw.charset, why: raw.why.clone(), ..Hop::default() };
    h.final_url.clone_from(&raw.final_url);
    h.refreshed_from.clone_from(&raw.refreshed_from);
    let fin = if raw.final_url.is_empty() { url } else { &raw.final_url };
    if pdf && magic != Some("pdf") && (tier != Tier::Plain || clean::LISTS.text.contains(&kind)) {
        // the site's own error status (Bright Data's `x-brd-status-code`) says more than "not the file"
        h.code = if raw.status >= 400 { status_code(raw.status) } else { "not_pdf".into() };
    } else if kind != "html" && raw.status >= 400 {
        h.code = status_code(raw.status);
    } else if kind == "pdf" && magic.is_none() {
        h.code = "not_pdf".into(); // a content type that named a PDF over a body that is not one
    } else if kind == "pdf" {
        match guarded(|| crate::pdf::build_pdf(&raw.body, url, fin, limit("pdf_budget_s"))) {
            Ok(d) => {
                // only Oxylabs' copy is rebuilt from text (`provider::file_bytes`), so only its offsets can shift
                let oxy = matches!(tier, Tier::Oxy | Tier::OxyJs);
                let code = match () {
                    _ if crate::pdf::cut_short(&raw.body, &d) => "pdf_truncated",
                    _ if oxy && !crate::pdf::xref_ok(&raw.body) => "pdf_mangled",
                    _ => "ok",
                };
                (h.ok, h.code, h.doc) = (code == "ok", code.into(), Some(d));
            }
            Err(_) => h.code = "parse_failed".into(),
        }
    } else if kind != "html" {
        (h.ok, h.code) = (true, "ok".into());
        if kind != "zip" {
            h.doc = crate::cache::build(&raw.body, url, fin, js, kind).ok();
        }
        // a server's Markdown says how it may be used (`content-signal: ai-train=yes, …`): kept for `stats`
        let signal = raw.headers.lines().find_map(|l| l.strip_prefix("content-signal: "));
        if let (Some(d), Some(s)) = (h.doc.as_mut(), signal.filter(|_| kind == "md")) {
            d.info.insert("signal".into(), s.into());
        }
    } else {
        let text = String::from_utf8_lossy(&raw.body);
        let tree = crate::html::parse(&text);
        let (ok, code, vis, shown) = check(&tree, &text, &raw, url);
        // the site's own language only from a copy of the site (served, or a shell/thin page below 400), never a
        // wall's or a challenge's (a CF challenge says `en-US` on any site)
        h.lang = if ok || (raw.status < 400 && !code.ends_with("_wall")) { page_lang(&tree) } else { h.lang };
        (h.ok, h.code, h.vis) = (ok, code, vis);
        // https refused for its certificate (`provider::plain`'s `tls:` why) and an error page over http: the site
        // is there, its certificate is the answer, no paid tier is asked (`FINAL`); http's other answers: `plain`
        h.code = if h.code == "error_page" && raw.why.starts_with("tls:") { "tls_failed".into() } else { h.code };
        // a page served under an error status says so in its tier path (`oxyjs✓403`)
        h.why = if ok && raw.status >= 400 { raw.status.to_string() } else { h.why };
        // A provider is named, never plain (no provider: its error page is the page's own answer)
        h.why = if h.code == "error_page" && tier != Tier::Plain { "refused here (error page)".into() } else { h.why };
        // Bright Data names a redirect only sometimes (`x-unblocker-redirected-to`): without it, the page's own
        // same-site canonical link says where the ask landed (`outline::lost_page` judges it)
        if tier == Tier::Bd && h.final_url == url {
            h.landed = canonical(&tree, url);
        }
        if ok {
            match guarded(|| Ok(clean::build_doc(&tree, url, &h.final_url, js))) {
                // a page that cleans to fewer than `thin_chars` is a shell unless its title is visibly there
                // (example.com)
                Ok(d) if d.loss.pruned_chars < limit("thin_chars") && !shown => {
                    (h.ok, h.code, h.doc) = (false, "thin_content".into(), Some(d))
                }
                // a page that passes for one in a PDF run is not the file (a bot check titled as itself)
                Ok(d) if pdf => (h.ok, h.code, h.doc) = (false, "not_pdf".into(), Some(d)),
                Ok(d) => h.doc = Some(d),
                Err(_) => (h.ok, h.code) = (false, "parse_failed".into()),
            }
        } else {
            h.tree = Some(tree);
        }
    }
    h.body = raw.body;
    h
}

fn status_code(s: u16) -> String {
    match s {
        404 | 410 => "not_found".into(),
        401 | 403 | 429 | 451 => format!("blocked_{s}"),
        _ => format!("http_{s}"),
    }
}

/// A page's `<link rel=canonical>`, absolute, when it is on the same site as `url`.
fn canonical(t: &Tree, url: &str) -> Option<String> {
    let rel = |n: &&Node| n.attr("rel").is_some_and(|r| r.eq_ignore_ascii_case("canonical"));
    let href = t.nodes.iter().filter(|n| n.tag == "link").find(rel)?.attr("href")?;
    let c = url::Url::parse(url).ok()?.join(href).ok()?.to_string();
    (site(&c) == site(url)).then_some(c)
}

/// Text a no-JS browser shows (everything but head, script, style, template and svg), the same without the text
/// nodes that ask for JavaScript, and whether one of those, outside the page's chrome (noscript, nav, header,
/// footer, aside), is a paragraph (`clean::paragraph`): a text of the page's own.
pub fn visible(t: &Tree) -> (String, String, bool) {
    let skip = ["head", "script", "style", "template", "svg", "noscript", "nav", "header", "footer", "aside"];
    let asks_js = |f: &String| CHECK.js_needed.iter().any(|m| f.contains(m));
    let all: Vec<String> = t.texts(0, &skip[..5]).map(clean::fold).collect();
    let body: Vec<&str> = all.iter().filter(|f| !asks_js(f)).map(String::as_str).collect();
    let own = t.texts(0, &skip).map(clean::fold).any(|f| clean::paragraph(&f) && !asks_js(&f));
    (clean::collapse(&all.join(" ")), clean::collapse(&body.join(" ")), own)
}

/// A search page (a URL whose query names the search: `q=`, a key ending in `query`) is judged by its results, by
/// no engine's markup: distinct link targets whose text holds ≥ `link_words` words. None = no search page.
fn serp_results(url: &str, t: &Tree) -> Option<usize> {
    let u = url::Url::parse(url).ok()?;
    u.query_pairs().any(|(k, _)| k == "q" || k.ends_with("query")).then_some(())?;
    let links = t.nodes.iter().enumerate().filter_map(|(i, n)| {
        let href = n.attr("href").filter(|h| n.tag == "a" && !h.starts_with('#'))?;
        let words = t.texts(i, &[]).flat_map(str::split_whitespace).count();
        (words >= limit("link_words")).then_some(href)
    });
    Some(links.collect::<std::collections::HashSet<_>>().len())
}

/// The page's own words in `s` (THE rule, with `outline::anchor`): its distinct folded words of ≥ 4
/// letters outside `CHECK.stop` (`access`, `denied`, `page`: a wall's or a 404's title says nothing of the page).
pub fn words_of(s: &str) -> Vec<String> {
    let f = clean::fold(s);
    let long = f.split(|c: char| !(clean::is_word(c) && !clean::is_digit(c) && c != '_'));
    uniq(long.filter(|w| w.chars().count() >= 4 && !CHECK.stop.contains(w)).map(String::from), String::clone)
}

/// The page title up to its site suffix (`Title | Site`, `Title – Site`).
pub fn title_head(s: &str) -> &str {
    let cs: Vec<(usize, char)> = s.char_indices().collect();
    cs.windows(3).find(|w| py_space(w[0].1) && "|·–—-".contains(w[1].1) && py_space(w[2].1)).map_or(s, |w| &s[..w[0].0])
}

/// The asked URL's last non-empty path segment (`/comments/x1/job_interview_tips` → `job_interview_tips`).
fn slug_id(url: &str) -> Option<String> {
    url::Url::parse(url).ok().and_then(|u| Some(u.path_segments()?.rfind(|s| !s.is_empty())?.to_string()))
}

/// A sign-in gate, judged before `redirected` whatever the id, on a landing that moved off the asked path: the
/// landing's path holds a segment that ends in a `Check::sign_in` word (`/login`, `/customer-login`,
/// `/users/sign_in`, `/signin.php`), or a query pair whose name ends in one hands the asked page back
/// (`?redirect_after_login=%2Fa%2Fall`: a path, or a URL on the asked site). Words are whole: every mark parts
/// them, and a compound is named by its last word, so `/login-free`, `/signing-guide`, `/authors` and
/// `/docs/authentication` name none. A 404 bounce `?from=`, a country chooser `?return=`, a consent `?continue=`
/// hand the page back too and are no sign-in: `redirected`, both renders.
fn sign_in_gate(asked: &str, landed: &str) -> bool {
    let (Ok(a), Ok(f)) = (url::Url::parse(asked), url::Url::parse(landed)) else { return false };
    let path = |u: &url::Url| u.path().trim_end_matches('/').to_owned();
    let words = |s: &str| {
        let s = s.rsplit_once('.').map_or(s, |(name, _)| name).to_lowercase();
        s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_owned).collect::<Vec<_>>()
    };
    let named = |s: &str| CHECK.sign_in.iter().any(|w| words(s).ends_with(&words(w)));
    let back = |v: &str| f.join(v).is_ok_and(|v| site(v.as_str()) == site(asked) && path(&v) == path(&a));
    let gate =
        f.path_segments().is_some_and(|mut s| s.any(named)) || f.query_pairs().any(|(k, v)| named(&k) && back(&v));
    path(&f) != path(&a) && gate
}

/// Is the main content there? Walls, consent and login pages by their markers, on a thin copy only (short, and no
/// title shown in a 2xx); with no expected words to look for, the title's distinct words stand in for them (missing
/// from the visible text = the body is a JS shell). A short page is real when its title is visibly there.
/// (ok, code, visible chars, title shown).
pub fn check(tree: &Tree, html: &str, raw: &Raw, url: &str) -> (bool, String, usize, bool) {
    let (vis, body, own) = visible(tree);
    let chars = vis.chars().count();
    let folded = std::cell::OnceCell::new();
    let title = tree.meta("og:title").map(str::to_string).unwrap_or_else(|| tree.title.clone());
    // the page's own words are shown when at least half (and 2 or more) are in the visible text: its title's, or
    // its URL's last path segment's (`/comments/x1/job_interview_tips`: a site-wide title says nothing)
    let seen = |w: &[String]| w.len() >= 2 && 2 * w.iter().filter(|w| body.contains(w.as_str())).count() >= w.len();
    let words = words_of(title_head(&title));
    let slug =
        url::form_urlencoded::parse(slug_id(url).unwrap_or_default().as_bytes()).next().map(|(s, _)| words_of(&s));
    let shown = seen(&words) || slug.is_some_and(|s| seen(&s));
    // a copy that holds the page is served whatever its status or the scripts it loads: its own words shown in a
    // 2xx, or in a 4xx/5xx body of `page_chars` or more (a wall = wall text AND no page words),
    // never a 404/410: a missing page echoes the URL's words (Wikipedia's, 2,034 chars outside its h1, vs
    // a 403 Q&A's 1,767: no length tells them apart); a served page is never a wall, wherever it landed
    let gone = matches!(raw.status, 404 | 410);
    let page = shown && !gone && (raw.status < 400 || chars >= limit("page_chars"));
    let hit = |w: &Wall| {
        let found = |s: &str| w.any.iter().any(|p| s.contains(p));
        !page
            && w.under.is_none_or(|n| chars < n)
            && match w.on {
                On::Header => raw.headers.lines().any(|l| w.any.iter().any(|p| l.starts_with(p))),
                On::Html => found(folded.get_or_init(|| clean::fold(&crate::html::unescape(html)))),
                On::Text => found(&vis),
                On::Status => raw.status >= 400,
            }
    };
    if let Some(w) = CHECK.rules.iter().find(|w| hit(w)) {
        let code = if w.code.is_empty() { status_code(raw.status) } else { w.code.into() };
        return (false, code, chars, false);
    }
    if !page && sign_in_gate(url, &raw.final_url) {
        return (false, "login_wall".into(), chars, false);
    }
    // no wall path named: the copy landed where the asked id is gone (`lost_page`), and shows none of its own
    // words: a front page, a search; never the page
    if !page && lost_page(url, &raw.final_url).is_some() {
        return (false, "redirected".into(), chars, false);
    }
    if let Some(n) = serp_results(url, tree) {
        let ok = n >= limit("serp_links");
        return (ok, if ok { "ok" } else { "serp_empty" }.into(), chars, false);
    }
    // a shell loads JavaScript: a short one with none (no `<script>`, no refresh) and none of its own words is the
    // site's own answer, an error page (Akamai's 534 B "Site Maintenance"; no phrase list). A copy that shows a
    // text of its own in `thin_chars` or more is the page, whatever its title (one word, a site's label): a shell
    // shows chrome, notices and short rows only
    if !shown && !(own && chars >= limit("thin_chars")) && (words.len() >= 2 || chars < 800) {
        let refresh = |n: &Node| n.attr("http-equiv").is_some_and(|v| v.eq_ignore_ascii_case("refresh"));
        let bare = chars < limit("page_chars") && !tree.nodes.iter().any(|n| n.tag == "script" || refresh(n));
        return (false, if bare { "error_page" } else { "js_shell" }.into(), chars, false);
    }
    // a short copy that shows little more than its own title is a state page, an error page too ("Post
    // isn't available • Instagram", 111 chars; no phrase: visible text under `title_ratio` × the title's chars)
    let state = chars < limit("page_chars") && chars < limit::<usize>("title_ratio") * title.trim().chars().count();
    (!state, if state { "error_page" } else { "ok" }.into(), chars, shown && !state)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two providers the unit tests model (oxy, bd); a third key has its own (`a_third_provider_takes_its_rungs`)
    pub const KEYED: [Tier; 4] = [Tier::Plain, Tier::Bd, Tier::Oxy, Tier::OxyJs];

    fn plan(mode: Mode) -> Plan {
        Plan { mode, cap: COST_CAP, keys: KEYED.to_vec(), hedge: HEDGE, ..Plan::default() }
    }

    /// A search has no gate: Oxylabs runs alone for the hedge, then Bright Data starts beside it (or at once when
    /// Oxylabs failed); the first answer stops both. Search prices, search timeout, rows headed by no id.
    #[test]
    fn search_hedges_bright_data() {
        let p = Plan { keys: vec![Tier::Oxy, Tier::Bd], ..Plan::search(Mode::Auto, COST_CAP, None) };
        let s = Duration::from_secs;
        assert_eq!(next(&p, &[], &[]), Step::Start(Tier::Oxy));
        assert_eq!(next(&p, &[], &[(Tier::Oxy, s(1))]), Step::Wait(Duration::from_millis(1500)));
        assert_eq!(next(&p, &[], &[(Tier::Oxy, SEARCH_HEDGE)]), Step::Start(Tier::Bd));
        assert_eq!(next(&p, &[hop(Tier::Oxy, "provider_error")], &[]), Step::Start(Tier::Bd));
        assert_eq!(next(&p, &[hop(Tier::Oxy, "ok")], &[(Tier::Bd, s(1))]), Step::Stop);
        let late = [(Tier::Oxy, s(36)), (Tier::Bd, s(36))];
        assert_eq!(next(&p, &[], &late), Step::Stop, "detached: twice 15 s at once (`one_call`) + grace");
        assert_eq!(route(&p), (vec![(Tier::Oxy, None), (Tier::Bd, Some(SEARCH_HEDGE))], 0.0025));
        let poor =
            Plan { cap: 0.001, ..Plan { keys: vec![Tier::Oxy, Tier::Bd], ..Plan::search(Mode::Auto, 0.0, None) } };
        assert_eq!(route(&poor), (vec![(Tier::Oxy, None)], 0.001));
        let only = Plan { keys: vec![Tier::Oxy, Tier::Bd], ..Plan::search(Mode::Only(Tier::Bd), COST_CAP, None) };
        assert_eq!(route(&only).0, vec![(Tier::Bd, None)], "bd alone, detached: one call at twice the time");
        assert_eq!(only.timeout_of(Tier::Bd, &[]), s(30));
        // bd-only detached (no `wait:`) ran 15,004 + 14,999 ms: one call at 2×; an inline one under the 30 s
        // default `wait:` keeps its retry (S6: the default once made every inline search one call)
        let only_bd = || Plan { keys: vec![Tier::Oxy, Tier::Bd], ..Plan::search(Mode::Only(Tier::Bd), COST_CAP, None) };
        let (detached, inline) = (only_bd(), Plan { wait: Some(s(30)), ..only_bd() });
        assert_eq!((route(&detached).0, detached.timeout_of(Tier::Bd, &[])), (vec![(Tier::Bd, None)], s(30)));
        assert_eq!((route(&inline).0, inline.timeout_of(Tier::Bd, &[])), (vec![(Tier::Bd, None); 2], s(15)));
        let cut = Hop { ms: 30_000, ..hop(Tier::Bd, "provider_timeout") };
        assert_eq!(next(&detached, &[cut], &[]), Step::Stop, "one hop, never a retry");
        let waits = Plan { wait: Some(s(5)), ..only };
        assert_eq!(route(&waits).0, vec![(Tier::Bd, None), (Tier::Bd, None)], "inside wait: a retry");
        // bd-only `timeout:1` made 2 hops for one cut: the agent's cut is the answer, asked once
        let cut = Plan { timeout: Some(s(1)), ..waits };
        assert_eq!(next(&cut, &[hop(Tier::Bd, "provider_timeout")], &[]), Step::Stop, "no retry under the same cut");
        let nobd = Plan { keys: vec![Tier::Oxy], ..Plan::search(Mode::Auto, COST_CAP, None) };
        let bd = Plan { keys: vec![Tier::Bd], ..Plan::search(Mode::Auto, COST_CAP, None) };
        let wait = |p: &Plan, asked: u64| p.search_wait(s(asked)).as_secs();
        assert_eq!(
            [wait(&p, 5), wait(&nobd, 5), wait(&bd, 5), wait(&bd, 1), wait(&bd, 30)],
            [5, 5, 10, 10, 30],
            "the search patience"
        );
        let rows = skipped(&nobd, &[hop(Tier::Oxy, "provider_error")], "", "rust news", "");
        let bd = "! no_key bd · would try web search ≤$0.0015";
        assert!(
            rows.len() == 1 && rows[0].starts_with(bd),
            "a failed search names the key that could serve it: {rows:?}"
        );
        assert!(
            rows[0].ends_with(&format!(" → run {} setup bd, give the user its 3 lines", bin())),
            "its key form: {rows:?}"
        );
    }

    fn hop(t: Tier, code: &str) -> Hop {
        let cost = t.spec().price;
        Hop { tier: t, ok: code == "ok", code: code.into(), kind: "html", cost, ..Hop::default() }
    }

    const S: fn(u64) -> Duration = Duration::from_secs;

    /// host class × tier × outcome → next tier.
    #[test]
    fn decision_table() {
        use Step::*;
        use Tier::*;
        let auto = plan(Mode::Auto);
        type Row<'a> = (&'a str, &'a Plan, Vec<Hop>, Vec<(Tier, Duration)>, Step);
        let rows: Vec<Row> = vec![
            ("start with plain", &auto, vec![], vec![], Start(Plain)),
            ("plain running: wait for its verdict", &auto, vec![], vec![(Plain, S(1))], Wait(S(14))),
            ("plain ok: done", &auto, vec![hop(Plain, "ok")], vec![], Stop),
            ("plain js shell → bd, never oxy no-JS", &auto, vec![hop(Plain, "js_shell")], vec![], Start(Bd)),
            ("plain 403 → bd", &auto, vec![hop(Plain, "blocked_403")], vec![], Start(Bd)),
            ("plain captcha → oxy no-JS first", &auto, vec![hop(Plain, "captcha_wall")], vec![], Start(Oxy)),
            ("oxy no-JS running: its verdict", &auto, vec![hop(Plain, "captcha_wall")], vec![(Oxy, S(1))], Wait(S(14))),
            ("oxy no-JS failed → bd", &auto, vec![hop(Plain, "captcha_wall"), hop(Oxy, "x")], vec![], Start(Bd)),
            ("plain timeout → bd", &auto, vec![hop(Plain, "provider_timeout")], vec![], Start(Bd)),
            ("plain 404: a provider will not fix it", &auto, vec![hop(Plain, "not_found")], vec![], Stop),
            (
                "plain and bd shells: one provider's shell is its failure, the other renders (airbnb)",
                &auto,
                vec![hop(Plain, "js_shell"), hop(Oxy, "js_shell"), hop(Bd, "js_shell")],
                vec![],
                Start(OxyJs),
            ),
            (
                "plain and bd shells: the render beside them runs on",
                &auto,
                vec![hop(Plain, "js_shell"), hop(Oxy, "js_shell"), hop(Bd, "js_shell")],
                vec![(OxyJs, S(1))],
                Wait(S(44)),
            ),
            (
                "bd wall → oxyjs",
                &auto,
                vec![hop(Plain, "js_shell"), hop(Oxy, "x"), hop(Bd, "captcha_wall")],
                vec![],
                Start(OxyJs),
            ),
            ("bd just started: oxyjs waits the hedge", &auto, both(), vec![(Bd, S(0))], Wait(S(5))),
            ("bd 2 s in: 3 s left", &auto, both(), vec![(Bd, S(2))], Wait(S(3))),
            ("bd 5 s in: oxyjs starts beside it", &auto, both(), vec![(Bd, S(5))], Start(OxyJs)),
            ("bd failed fast → oxyjs now", &auto, and(vec![hop(Bd, "captcha_wall")]), vec![], Start(OxyJs)),
            ("both running: wait", &auto, both(), vec![(Bd, S(6)), (OxyJs, S(1))], Wait(S(39) + S(5))),
            ("oxyjs won: stop", &auto, and(vec![hop(OxyJs, "ok")]), vec![(Bd, S(9))], Stop),
            ("all failed: stop", &auto, and(vec![hop(Bd, "x"), hop(OxyJs, "x")]), vec![], Stop),
            ("stuck past timeout+grace: stop", &auto, both(), vec![(Bd, S(66))], Stop),
            (
                "shell + bd timeout: one provider failed, the other renders",
                &auto,
                vec![hop(Plain, "js_shell"), hop(Oxy, "js_shell"), hop(Bd, "provider_timeout")],
                vec![],
                Start(OxyJs),
            ),
        ];
        for (what, p, done, running, want) in rows {
            assert_eq!(next(p, &done, &running), want, "{what}");
        }
    }

    /// The decision table's named cases: gov hosts, zip documents, `js:true`, missing keys, `provider:`, refused
    /// keys, the cost cap, `hedge:0`, `timeout:`.
    #[test]
    fn decision_cases() {
        use Step::*;
        use Tier::*;
        let auto = plan(Mode::Auto);
        let gov = Plan { gov: true, ..plan(Mode::Auto) };
        assert_eq!(gov.ladder(&[]), [Plain, Oxy, OxyJs, DecodoJs], "gov skips bd: no other render, oxy no-JS runs");
        let zip = Plan { doc: Some("zip"), ..plan(Mode::Auto) };
        assert_eq!(next(&zip, &[hop(Tier::Plain, "blocked_403")], &[]), Stop, "zip documents: plain only");
        assert!(skipped(&zip, &[hop(Tier::Plain, "blocked_403")], "u1", "u1", "").is_empty());
        let js = Plan { js: true, ..plan(Mode::Auto) };
        assert_eq!(js.ladder(&[]), [Bd, OxyJs, DecodoJs], "js:true skips plain and the no-JS rungs");
        assert_eq!(next(&js, &[], &[]), Start(Bd));
        // A paid ask runs ONE provider; the next starts only on its failure, never beside it (hedge:off)
        assert_eq!(next(&js, &[], &[(Bd, S(30))]), Wait(S(15)), "no oxyjs beside a slow bd");
        assert_eq!(next(&js, &[hop(Bd, "js_shell")], &[]), Start(OxyJs), "it failed: oxyjs");
        assert_eq!(next(&js, &[hop(Bd, "not_found")], &[]), Stop);
        assert_eq!((route(&js), js.hedged(), auto.hedged()), ((vec![(Bd, None), (OxyJs, None)], 0.003), false, true));
        assert!(!plan(Mode::Only(Bd)).hedged(), "a named tier is a paid ask too");
        assert!(plan(Mode::Only(Plain)).hedged(), "provider:plain is free: no hedge:off");
        let nobd = Plan { keys: vec![Plain, Oxy, OxyJs], ..plan(Mode::Auto) };
        // one key: that provider's every rung, oxy no-JS gating the render (measured: a news page in 1.9 s)
        assert_eq!(next(&nobd, &both(), &[]), Start(Oxy), "no bd key → oxy no-JS");
        assert_eq!(next(&nobd, &[hop(Plain, "blocked_403"), hop(Oxy, "captcha_wall")], &[]), Start(OxyJs));
        let nooxy = Plan { keys: vec![Plain, Bd], ..plan(Mode::Auto) };
        assert_eq!(next(&nooxy, &[hop(Plain, "js_shell")], &[]), Start(Bd), "no oxy key → bd, gated by plain");
        let only = plan(Mode::Only(Plain));
        assert_eq!(next(&only, &[hop(Plain, "js_shell")], &[]), Stop, "provider:plain never escalates");
        let oxy = plan(Mode::Only(Oxy));
        assert_eq!(oxy.ladder(&[]), [Oxy, OxyJs], "provider:oxy keeps both oxy tiers");
        assert_eq!(next(&oxy, &[hop(Oxy, "provider_error")], &[]), Start(OxyJs));
        // a refused or rate-limited key is never sent again by a tier that shares it; a tier with its own key runs
        for code in KEY_CODES {
            assert_eq!(next(&oxy, &[hop(Oxy, code)], &[]), Stop, "provider:oxy {code}: no oxyjs");
            assert!(skipped(&oxy, &[hop(Oxy, code)], "u1", "u1", "").is_empty(), "{code}: no budget row for oxyjs");
            let refused_oxy = vec![hop(Plain, "js_shell"), hop(Oxy, code)];
            assert_eq!(next(&auto, &refused_oxy, &[]), Start(Bd), "oxy {code} → bd");
            assert_eq!(next(&auto, &refused_oxy, &[(Bd, S(9))]), Wait(S(36)), "{code}: bd alone, no oxyjs beside it");
            let mut all = refused_oxy;
            all.push(hop(Bd, "captcha_wall"));
            assert_eq!(next(&auto, &all, &[]), Stop, "{code}: bd failed, oxyjs still not tried");
            let search = Plan { keys: vec![Oxy, Bd], ..Plan::search(Mode::Auto, COST_CAP, None) };
            assert_eq!(next(&search, &[hop(Oxy, code)], &[]), Start(Bd), "search {code}: bd has its own key");
            assert_eq!(next(&search, &[hop(Oxy, code), hop(Bd, code)], &[]), Stop);
        }
        assert_eq!(Plan { js: true, ..plan(Mode::Only(Oxy)) }.ladder(&[]), [OxyJs]);
        let poor = Plan { cap: 0.002, ..plan(Mode::Auto) };
        assert_eq!(next(&poor, &both(), &[(Bd, S(6))]), Wait(S(39)), "no budget for oxyjs too");
        assert_eq!(next(&poor, &both(), &[(Bd, S(1))]), Wait(S(44)), "no hedge wake without budget");
        let race = Plan { hedge: S(0), ..plan(Mode::Auto) };
        assert_eq!(next(&race, &both(), &[(Bd, S(0))]), Start(OxyJs), "hedge:0 = both at once");
        let slow = Plan { timeout: Some(S(180)), ..plan(Mode::Auto) };
        assert_eq!(next(&slow, &[], &[(Plain, S(1))]), Wait(S(14)), "timeout: leaves plain at 10 s");
        let named = Plan { timeout: Some(S(180)), ..plan(Mode::Only(Oxy)) };
        assert_eq!(next(&named, &[], &[(Oxy, S(3))]), Wait(S(22)), "and named oxy no-JS at its 20 s");
        let late = [(Bd, S(66)), (OxyJs, S(61))];
        assert_eq!(next(&slow, &both(), &late), Wait(S(124)), "bd and oxyjs get the 180 s");
        let zero = Plan { cap: 0.0, ..plan(Mode::Auto) };
        assert_eq!(next(&zero, &[hop(Plain, "js_shell")], &[]), Stop, "cost:0 = plain only");
        // a final answer ends the run with a call in flight; a provider's refusal (bd 502) does not
        for code in ["not_found", "too_big", "no_host", "private_address"] {
            assert_eq!(next(&auto, &[hop(Bd, code)], &[(OxyJs, S(70))]), Stop, "{code}");
        }
        assert_eq!(next(&auto, &[hop(Plain, "no_host")], &[]), Stop, "no tier after a dead host");
        assert_ne!(next(&auto, &[hop(Bd, "provider_refused")], &[(OxyJs, S(9))]), Stop, "bd 502 is not final");
    }

    /// Three keys (decodo beside oxy and bd): its no-JS rung after oxy's behind plain's captcha, never after a shell;
    /// its render joins bd ∥ oxyjs one hedge after the last start; `js:true` = every render, one at a time.
    #[test]
    fn a_third_provider_takes_its_rungs() {
        use Step::*;
        use Tier::*;
        let three = Plan { keys: Tier::ALL.to_vec(), ..plan(Mode::Auto) };
        assert_eq!(next(&three, &[hop(Plain, "captcha_wall")], &[]), Start(Oxy));
        assert_eq!(next(&three, &[hop(Plain, "captcha_wall"), hop(Oxy, "x")], &[]), Start(Decodo));
        let slow = [hop(Plain, "captcha_wall"), hop(Oxy, "provider_timeout")];
        assert_eq!(next(&three, &slow, &[]), Start(Bd), "a no-JS tier cut: the renders, never decodo's no-JS");
        let shell = [hop(Plain, "js_shell")];
        assert_eq!(next(&three, &shell, &[]), Start(Bd), "a shell skips both no-JS rungs");
        assert_eq!(next(&three, &shell, &[(Bd, S(5))]), Start(OxyJs));
        assert_eq!(next(&three, &shell, &[(Bd, S(6)), (OxyJs, S(1))]), Wait(S(4)), "a hedge after the last start");
        assert_eq!(next(&three, &shell, &[(Bd, S(10)), (OxyJs, S(5))]), Start(DecodoJs));
        let failed = [hop(Plain, "js_shell"), hop(Bd, "x"), hop(OxyJs, "x"), hop(DecodoJs, "x")];
        assert_eq!(next(&three, &failed, &[]), Stop);
        let js = Plan { js: true, keys: Tier::ALL.to_vec(), ..plan(Mode::Auto) };
        assert_eq!(route(&js).0, [(Bd, None), (OxyJs, None), (DecodoJs, None)], "one at a time, table order");
        let decodo = Plan { keys: vec![Plain, Decodo, DecodoJs], ..plan(Mode::Only(Decodo)) };
        assert_eq!(decodo.ladder(&[]), [Decodo, DecodoJs], "provider:decodo keeps both decodo tiers");
    }

    /// Behind Bright Data with no redirect header, a same-site canonical link is where the ask landed.
    #[test]
    fn canonical_names_a_bd_landing() {
        let page = |canon: &str| {
            let html = format!(
                "<html><head><title>Base Set</title><link rel=canonical href=\"{canon}\"></head><body><h1>Base Set</h1>\
                 <p>{}</p><a href=\"2\">Next page</a></body></html>",
                "Singles from the Base Set, in stock today. ".repeat(20)
            );
            Raw {
                status: 200,
                body: html.into_bytes(),
                ctype: "text/html".into(),
                final_url: "https://a.example/i/9-x".into(),
                ..Raw::default()
            }
        };
        let fin = |t: Tier, canon: &str| judge(t, page(canon), "https://a.example/i/9-x", false, false).landed;
        assert_eq!(fin(Tier::Bd, "/singles").as_deref(), Some("https://a.example/singles"));
        assert_eq!(fin(Tier::Bd, "https://other.org/x"), None, "another site's canonical: syndication");
        assert_eq!(fin(Tier::Plain, "/singles"), None, "plain follows the redirects itself");
        // The landing is the redirect judgement only; links resolve against where the bytes came from
        let h = judge(Tier::Bd, page("/singles"), "https://a.example/i/9-x", false, false);
        let doc = h.doc.expect("a page");
        assert_eq!(
            (doc.final_url.as_str(), doc.links.first().map(String::as_str)),
            ("https://a.example/i/9-x", Some("https://a.example/i/2"))
        );
    }

    /// Per key set: a run that fails at every rung leaves no keyed rung untried, and its fix
    /// never sends a provider without a key, nor `js:true` when no keyed render is left.
    #[test]
    fn every_keyed_rung_runs_and_no_hint_names_a_missing_key() {
        use Tier::*;
        for keys in [vec![Plain, Oxy, OxyJs], vec![Plain, Bd], Tier::ALL.to_vec()] {
            let p = Plan { keys: keys.clone(), ..plan(Mode::Auto) };
            for code in ["captcha_wall", "js_shell", "provider_timeout", "provider_error", "blocked_403", "login_wall"]
            {
                let mut hops = Vec::new();
                while let Step::Start(t) = next(&p, &hops, &[]) {
                    hops.push(hop(t, code));
                }
                let untried = |t: &Tier| keys.contains(t) && !hops.iter().any(|h| h.tier == *t);
                let left: Vec<Tier> = p.ladder(&hops).into_iter().filter(untried).collect();
                assert!(left.is_empty(), "{keys:?} {code}: {left:?} never ran");
                let render = Tier::ALL.into_iter().any(|t| t.can("js") && untried(&t));
                let m = outcome("https://a.example/", &mut hops, 1).meta;
                let done = spent(&m, &[], &keys);
                let fix = crate::lang::fix(
                    &m.code,
                    &crate::lang::At { id: "u1", url: &m.url, arg: fix_arg(&m, &[]).unwrap_or_default(), done: &done },
                );
                assert!(!fix.contains("provider:oxy") || keys.contains(&Oxy), "{keys:?} {code}: {fix}");
                assert!(!fix.contains("provider:bd") || keys.contains(&Bd), "{keys:?} {code}: {fix}");
                assert!(!fix.contains("js:true") || render, "{keys:?} {code}: {fix}");
            }
        }
    }

    /// A thin first answer waits for the other provider's running call, and a full answer wins.
    #[test]
    fn thin_answer_waits_for_the_other_provider() {
        use Step::*;
        use Tier::*;
        let auto = plan(Mode::Auto);
        let loss = || clean::Loss { visible_chars: 9_000, ..clean::Loss::default() };
        let thin = || Hop { doc: Some(Doc { loss: loss(), ..Doc::default() }), ..hop(Bd, "ok") };
        assert!(crate::outline::is_thin(thin().doc.as_ref().expect("doc")), "the fixture is thin");
        let done = vec![hop(Plain, "js_shell"), thin()];
        assert_eq!(next(&auto, &done, &[(OxyJs, S(1))]), Wait(S(5)), "oxyjs running: wait the hedge window for it");
        // The window runs from the thin answer's landing, never to the other call's timeout
        let landed = |ago| vec![hop(Plain, "js_shell"), Hop { end: Instant::now().checked_sub(S(ago)), ..thin() }];
        assert_eq!(next(&auto, &landed(6), &[(OxyJs, S(3))]), Stop, "the window is over: the thin copy is served");
        assert!(matches!(next(&auto, &landed(2), &[(OxyJs, S(3))]), Wait(d) if d <= S(3)), "3 s of the window left");
        assert_eq!(next(&auto, &done, &[]), Stop, "nothing running: the thin answer is served");
        let full = vec![hop(Plain, "js_shell"), hop(Bd, "ok")];
        assert_eq!(next(&auto, &full, &[(OxyJs, S(1))]), Stop, "a full answer stops the ladder");
        let mut both: Vec<Hop> = done.into_iter().chain([hop(OxyJs, "ok")]).collect();
        assert_eq!(next(&auto, &both, &[]), Stop);
        assert_eq!(
            outcome("https://a.example/", &mut both, 1).meta.provider,
            "oxylabs-js",
            "the full answer is served"
        );
    }

    /// A render cut at its own 40 s ends the run, whatever the keys: 8/131 served renders took 40-60 s, a second
    /// one at twice the time and price is the silent 60-100 s block; `dry:true` prices the one call. A network
    /// error still gets its retry where one provider is left.
    #[test]
    fn a_render_cut_is_never_retried() {
        use Step::*;
        use Tier::*;
        let nooxy = Plan { keys: vec![Plain, Bd], ..plan(Mode::Auto) };
        let bd = |code: &str| vec![hop(Plain, "blocked_403"), hop(Bd, code)];
        assert_eq!(next(&nooxy, &bd("provider_timeout"), &[]), Stop, "bd cut: the run ends");
        assert_eq!(route(&nooxy), (vec![(Plain, None), (Bd, None)], 0.0015), "dry: no bd → bd retry");
        let oxy = Plan { keys: vec![Plain, Oxy, OxyJs], ..plan(Mode::Auto) };
        assert_eq!(next(&oxy, &[hop(Plain, "js_shell"), hop(OxyJs, "provider_timeout")], &[]), Stop, "oxyjs cut");
        // one provider on the LADDER, both keys present
        let gov = Plan { gov: true, ..plan(Mode::Auto) };
        let js = |code: &str| vec![hop(Plain, "js_shell"), hop(Oxy, "js_shell"), hop(OxyJs, code)];
        assert_eq!(next(&gov, &js("net_error"), &[]), Start(OxyJs), "a gov host: Oxylabs alone, once more");
        assert_eq!(next(&gov, &js("provider_timeout"), &[]), Stop, "but never a render after its cut");
        assert_eq!(next(&plan(Mode::Only(Bd)), &bd("net_error"), &[]), Start(Bd), "provider:bd: once more");
        assert_eq!(next(&plan(Mode::Only(Bd)), &bd("provider_timeout"), &[]), Stop, "provider:bd cut: the end");
    }

    /// One paid key: ONE retry on a network error or a no-JS tier's timeout, never on a wall or a shell.
    #[test]
    fn one_key_retries_once() {
        use Step::*;
        use Tier::*;
        let (auto, nooxy) = (plan(Mode::Auto), Plan { keys: vec![Plain, Bd], ..plan(Mode::Auto) });
        let bd = |code: &str| vec![hop(Plain, "blocked_403"), hop(Bd, code)];
        assert_eq!(next(&nooxy, &bd("net_error"), &[]), Start(Bd), "a network error: once more");
        // the no-JS retry stays: an Oxylabs-only PDF, oxy cut at its 30 s, gets twice the time under the wall
        let pdf = Plan { doc: Some("pdf"), keys: vec![Plain, Oxy, OxyJs], ..plan(Mode::Auto) };
        let ran = |gone: u64| {
            let now = Instant::now();
            let plain = Hop { end: now.checked_sub(S(gone - 10)), ms: 10_000, ..hop(Plain, "blocked_403") };
            vec![plain, Hop { end: Some(now), ms: (gone - 10) * 1000, ..hop(Oxy, "provider_timeout") }]
        };
        assert_eq!(pdf.timeout_of(Oxy, &[hop(Oxy, "provider_timeout")]), S(60), "with twice the time");
        // plain 10 s + oxy 30 s: 50 s of the 90 s one-key wall left ≥ the 30 s oxy had: the retry gets the 50 s
        assert_eq!((next(&pdf, &ran(40), &[]), pdf.timeout_of(Oxy, &ran(40))), (Start(Oxy), S(50)));
        // plain 10 s + oxy 60 s: 20 s of the wall left < the 60 s oxy had: no retry (a shorter retry serves
        // nothing: one key on 20 hard pages, 5 retries after a 60 s cut got 23.9-30 s under the wall and served 0)
        assert_eq!(next(&pdf, &ran(70), &[]), Stop, "20 s left < the 60 s oxy had: the run ends here");
        let capped = Plan { timeout: Some(S(30)), keys: vec![Plain, Bd], ..plan(Mode::Auto) };
        assert_eq!(capped.timeout_of(Bd, &bd("provider_timeout")).as_secs(), 30, "timeout:30 stays 30");
        // `timeout:` is a hard budget, no retry under it: bd-only `timeout:60` = plain 10 s
        // + bd 60 s, then the end (was a 60 s retry: ~130 s, past the 90 s wall)
        let sixty = Plan { timeout: Some(S(60)), keys: vec![Plain, Bd], ..plan(Mode::Auto) };
        assert_eq!(sixty.timeout_of(Bd, &[hop(Plain, "blocked_403")]), S(60));
        // plain keeps its 10 s under a longer `timeout:` and gets the agent's time under a shorter one (`timeout:2`
        // on a 12 s page ran 12 s)
        let two = Plan { timeout: Some(S(2)), keys: vec![Plain, Bd], ..plan(Mode::Auto) };
        assert_eq!((sixty.timeout_of(Plain, &[]), two.timeout_of(Plain, &[])), (S(10), S(2)));
        assert_eq!(next(&sixty, &bd("provider_timeout"), &[]), Stop, "timeout: set: no retry");
        let pdf60 = Plan {
            timeout: Some(S(60)),
            ..Plan { doc: Some("pdf"), keys: vec![Plain, Oxy, OxyJs], ..plan(Mode::Auto) }
        };
        assert_eq!(next(&pdf60, &ran(30), &[]), Stop, "not even with the wall far off");
        // an oxy-only Joomla site: plain's JS challenge skips oxy no-JS (0/33 after a JS
        // challenge or shell), plain's 403 or captcha keeps it (11/26)
        let oxy = Plan { keys: vec![Plain, Oxy, OxyJs], ..plan(Mode::Auto) };
        for (wall, first) in [("cloudflare_wall", OxyJs), ("js_shell", OxyJs), ("serp_empty", OxyJs)] {
            assert_eq!(next(&oxy, &[hop(Plain, wall)], &[]), Start(first), "{wall}");
        }
        for wall in ["blocked_403", "captcha_wall"] {
            assert_eq!(next(&oxy, &[hop(Plain, wall)], &[]), Start(Oxy), "{wall}");
        }
        // a provider's own refusal served 0/7 on a retry, our cut 3/9 (a refusal is final)
        assert_eq!(next(&nooxy, &bd("provider_error"), &[]), Stop, "bd 5xx: its answer");
        assert_eq!(next(&nooxy, &bd("captcha_wall"), &[]), Stop, "a wall: never again");
        assert_eq!(next(&nooxy, &bd("js_shell"), &[]), Stop, "a shell: never again");
        // the site's own 5xx is its answer: a bd-only listings site bought
        // `brightdata http_502` twice for the same failure; a wall served as 503 is a wall
        for (code, status) in [("http_502", 502), ("http_500", 500), ("cloudflare_wall", 503)] {
            let site = vec![hop(Plain, "captcha_wall"), Hop { status, ..hop(Bd, code) }];
            assert_eq!(next(&nooxy, &site, &[]), Stop, "{code} {status}: the site answered");
        }
        let twice: Vec<Hop> = bd("net_error").into_iter().chain([hop(Bd, "net_error")]).collect();
        assert_eq!(next(&nooxy, &twice, &[]), Stop, "one retry only");
        let tight = Plan { cap: 0.002, ..Plan { keys: vec![Plain, Bd], ..plan(Mode::Auto) } };
        assert_eq!(next(&tight, &bd("net_error"), &[]), Stop, "the retry is inside cost:");
        let both_failed: Vec<Hop> =
            bd("provider_timeout").into_iter().chain([hop(OxyJs, "provider_timeout")]).collect();
        assert_eq!(next(&auto, &both_failed, &[]), Stop, "two keys: the other provider was the retry");
        // Bd refused the URL (`europa.eu`, no gov label): Oxylabs is the one provider left, its failure gets the retry
        let refused = vec![hop(Plain, "js_shell"), hop(Oxy, "x"), hop(Bd, "provider_refused"), hop(OxyJs, "net_error")];
        assert_eq!(next(&auto, &refused, &[]), Start(OxyJs), "a refused owner covers no failure");
        let no_key = vec![hop(Plain, "js_shell"), hop(Oxy, "x"), hop(Bd, "bad_key"), hop(OxyJs, "net_error")];
        assert_eq!(next(&auto, &no_key, &[]), Start(OxyJs), "nor does a refused key");
    }

    /// A PDF run: plain → oxy no-JS (given the slow timeout) → bd, each waiting for the last one's verdict, never
    /// oxyjs; the URL's suffix says so, or an answer does (magic, content type); a missing file stays missing. A
    /// `/pdf/` path (`/doi/pdf/…` holds it) is only a hint: the page ladder, with oxy's file timeout.
    #[test]
    fn pdf_ladder() {
        use Step::*;
        use Tier::*;
        let doc = |u: &str| {
            let p = Plan::new(u, Mode::Auto, false, COST_CAP, None);
            (p.doc, p.hint)
        };
        assert_eq!(doc("https://arxiv.org/pdf/2405.15793"), (None, true), "a /pdf/ segment: a hint");
        assert_eq!(doc("https://dl.acm.org/doi/pdf/10.1145/3290605.3300233"), (None, true));
        assert_eq!(doc("https://x.org/docs/pdf/merge.html"), (None, false), "a page under /pdf/");
        assert_eq!(doc("https://x.org/pdf/a.pdf"), (Some("pdf"), false));
        assert_eq!(doc("https://x.org/A.PDF?dl=1"), (Some("pdf"), false));
        assert_eq!(doc("https://static.zara.net/static/pdfs/ES/a.html"), (None, false), "`pdfs` is not `pdf`");
        assert_eq!(doc("https://x.org/report.docx"), (Some("zip"), false));
        let hint = Plan::new("https://arxiv.org/pdf/2405.15793", Mode::Auto, false, COST_CAP, None);
        assert_eq!(hint.ladder(&[]), [Plain, Oxy, Decodo, Bd, OxyJs, DecodoJs], "the page ladder");
        assert_eq!(next(&hint, &[hop(Plain, "js_shell")], &[(Oxy, S(9))]), Wait(S(26)), "oxy has 30 s");
        let pdf = Plan { doc: Some("pdf"), ..plan(Mode::Auto) };
        let p403 = || hop(Plain, "blocked_403");
        assert_eq!(route(&pdf), (vec![(Plain, None), (Oxy, None), (Bd, None)], 0.0025));
        assert_eq!(next(&pdf, &[p403()], &[]), Start(Oxy));
        assert_eq!(next(&pdf, &[p403()], &[(Oxy, S(9))]), Wait(S(26)), "oxy has 30 s for a file");
        assert_eq!(next(&pdf, &[p403(), hop(Oxy, "not_pdf")], &[]), Start(Bd));
        assert_eq!(next(&pdf, &[p403(), hop(Oxy, "not_pdf")], &[(Bd, S(9))]), Wait(S(36)), "no oxyjs beside bd");
        assert_eq!(next(&pdf, &[p403(), hop(Oxy, "x"), hop(Bd, "pdf_truncated")], &[]), Stop);
        assert_eq!(next(&pdf, &[hop(Plain, "not_found")], &[]), Stop);
        assert_eq!(Plan { js: true, ..Plan { doc: Some("pdf"), ..plan(Mode::Auto) } }.ladder(&[]), [Plain, Oxy, Bd]);
        assert_eq!(Plan { gov: true, ..Plan { doc: Some("pdf"), ..plan(Mode::Auto) } }.ladder(&[]), [Plain, Oxy]);
        assert_eq!(Plan { doc: Some("pdf"), ..plan(Mode::Only(Oxy)) }.ladder(&[]), [Oxy]);
        // no `.pdf` in the URL: plain's failed answer says PDF (content type), so the run becomes one
        let page = plan(Mode::Auto);
        let said = Hop { kind: "pdf", ..p403() };
        assert_eq!(next(&page, &[said], &[]), Start(Oxy));
        let said = || Hop { kind: "pdf", ..p403() };
        assert_eq!(next(&page, &[said(), hop(Oxy, "provider_error"), hop(Bd, "x")], &[]), Stop, "no oxyjs");
        let rows =
            skipped(&Plan { keys: vec![Plain, Oxy], ..plan(Mode::Auto) }, &[said(), hop(Oxy, "x")], "u1", "u1", "");
        assert!(rows.len() == 1 && rows[0].starts_with("u1 ! no_key bd · "), "bd's PDF bytes, no oxyjs row: {rows:?}");
        // the driver tells each call it is a PDF run once an answer said so
        let seen = std::sync::Mutex::new(Vec::new());
        let call = |t: Tier, done: &[Hop]| -> Call {
            seen.lock().expect("lock").push((t, page.pdf_run(done)));
            let h = match t {
                Plain => Hop { kind: "pdf", ..hop(Plain, "blocked_403") },
                Oxy => hop(Oxy, "not_pdf"),
                _ => hop(t, "ok"),
            };
            Box::pin(async move { h })
        };
        let path: Vec<(Tier, bool)> = drive(&page, &call, Vec::new(), &drop).iter().map(|h| (h.tier, h.ok)).collect();
        assert_eq!(path, [(Plain, false), (Oxy, false), (Bd, true)]);
        assert_eq!(seen.into_inner().expect("lock"), [(Plain, false), (Oxy, true), (Bd, true)]);
    }

    /// DETECT by magic on every body: a PDF is read in `judge` (whole, an end-of-file line short, cut through a
    /// page), a zip is a document, a provider's page in a PDF run is `not_pdf`, an Oxylabs copy whose offsets moved
    /// is `pdf_mangled`; the best failed copy is the cut PDF.
    #[test]
    fn judge_reads_documents() {
        let pdf: &[u8] = include_bytes!("../tests/fixtures/three-pages.pdf");
        let mid = pdf.windows(7).position(|w| w == b"5 0 obj").expect("page 2") + 40;
        let raw = |b: &[u8], ct: &str| Raw { status: 200, body: b.to_vec(), ctype: ct.into(), ..Raw::default() };
        let at = |t: Tier, b: &[u8], ct: &str, pdf: bool| {
            let h = judge(t, raw(b, ct), "https://a.example/x.pdf", false, pdf);
            (h.ok, h.code.clone(), h.kind, h.doc.as_ref().map(|d| d.sections.len()))
        };
        let page = b"<html><head><title>Sign in</title></head><body>Sign in</body></html>";
        assert_eq!(at(Tier::Oxy, pdf, "", true), (true, "ok".into(), "pdf", Some(3)));
        assert_eq!(at(Tier::Oxy, &pdf[..pdf.len() - 6], "", true), (true, "ok".into(), "pdf", Some(3)));
        assert_eq!(at(Tier::Bd, &pdf[..mid], "", true), (false, "pdf_truncated".into(), "pdf", Some(3)));
        assert_eq!(at(Tier::Oxy, page, "", true), (false, "not_pdf".into(), "html", None));
        let gone = judge(Tier::Bd, Raw { status: 404, ..raw(page, "") }, "https://a.example/x.pdf", false, true);
        assert_eq!(gone.code, "not_found", "the site said 404: the file is missing, not walled");
        assert_eq!(at(Tier::Plain, page, "application/pdf", false), (false, "not_pdf".into(), "pdf", None));
        assert_eq!(at(Tier::Plain, b"PK\x03\x04rest", "", false), (true, "ok".into(), "zip", None));
        // a text document is served as it is, from any tier; in a PDF run it is not the file
        assert_eq!(at(Tier::Plain, b"a\nb", "text/plain", false), (true, "ok".into(), "text", Some(1)));
        assert_eq!(at(Tier::Bd, b"{\"a\":1}", "", false), (true, "ok".into(), "json", Some(1)));
        assert_eq!(at(Tier::Plain, b"a", "text/plain", true).1, "not_pdf");
        let gone = judge(
            Tier::Plain,
            Raw { status: 500, ..raw(b"{}", "application/json") },
            "https://a.example/x",
            false,
            false,
        );
        assert_eq!(
            (gone.ok, gone.code.as_str(), gone.kind),
            (false, "http_500", "json"),
            "a site's error is no document"
        );
        assert_eq!(at(Tier::Oxy, pdf, "", false).1, "ok", "a PDF found by magic outside a PDF run");
        let lit: &[u8] = include_bytes!("../tests/fixtures/literal-escape.pdf");
        let i = lit.windows(4).position(|w| w == b"\\x9c").expect("literal");
        let moved = [&lit[..i], &[0x9c], &lit[i + 4..]].concat();
        assert_eq!(at(Tier::Oxy, &moved, "", true), (false, "pdf_mangled".into(), "pdf", Some(3)));
        assert_eq!(at(Tier::Plain, &moved, "", true).1, "ok", "plain's bytes are the server's own");
        let check = b"<html><head><title>Voight-Kampff Browser Test</title></head><body><h1>Voight-Kampff Browser \
                      Test</h1><p>Checking your browser before you reach the file.</p><p>This takes a few seconds; the \
                      download starts by itself once the test is done.</p></body></html>";
        assert_eq!(at(Tier::Plain, check, "text/html", false).1, "ok", "as a page it passes");
        assert_eq!(at(Tier::Plain, check, "text/html", true).1, "not_pdf", "in a PDF run it is not the file");
        let mut hops = vec![
            judge(Tier::Plain, Raw { status: 403, ..raw(page, "text/html") }, "https://a.example/x.pdf", false, true),
            judge(Tier::Oxy, raw(&pdf[..mid], ""), "https://a.example/x.pdf", false, true),
            judge(Tier::Bd, raw(page, ""), "https://a.example/x.pdf", false, true),
        ];
        let o = outcome("https://a.example/x.pdf", &mut hops, 1);
        assert_eq!(
            (o.meta.code.as_str(), o.meta.kind.as_str(), o.meta.provider.as_str()),
            ("pdf_truncated", "pdf", "oxylabs")
        );
        assert_eq!((o.file.map(|f| f.0), o.doc.map(|d| d.mode)), (Some("raw.pdf".into()), Some("pdf".into())));
    }

    /// A Reddit thread (its meta.json as saved): bd's copy was thin, the hint said `js:true`, and
    /// `js:true` went to bd for the same copy. The render that sent the shown copy is never asked again while the
    /// other provider's render has a key.
    #[test]
    fn a_render_is_not_bought_twice_for_the_same_copy() {
        let m: Meta = serde_json::from_str(
            r#"{"url":"https://www.reddit.com/r/rust/comments/63wts9/x/","provider":"brightdata","ok":true,"code":"ok",
            "hops":[{"provider":"plain","code":"captcha_wall"},{"provider":"brightdata","code":"ok","cost":0.0015}]}"#,
        )
        .unwrap_or_default();
        let mut p = Plan { js: true, ..plan(Mode::Auto) };
        p.avoid = rendered(&m, &p.keys);
        assert_eq!(p.ladder(&[]), [Tier::OxyJs, Tier::DecodoJs], "the other renders only");
        assert_eq!(rendered(&m, &[Tier::Plain, Tier::Bd]), None, "one key: bd is still the only render");
    }

    /// No kept run takes a tier off the ladder or its retry (nothing keeps runs): every cross-run skip policy lost
    /// 2–4 of 17 oxy no-JS serves to save ~2 s. Oxy-only: oxy no-JS timed out on r/rust, oxyjs ran after it,
    /// and the failed run names the missing keys whose renders could still serve it.
    #[test]
    fn no_kept_run_wears_a_tier() {
        use Tier::*;
        let oxy = Plan { keys: vec![Plain, Oxy, OxyJs], ..plan(Mode::Auto) };
        let hops = [hop(Plain, "cloudflare_wall"), hop(Oxy, "provider_timeout"), hop(OxyJs, "x")];
        assert_eq!(route(&oxy).0.iter().map(|r| r.0).collect::<Vec<_>>(), [Plain, Oxy, OxyJs]);
        let rows = skipped(&oxy, &hops, "", "u1", "");
        assert!(rows.len() == 2 && rows[0].starts_with("! no_key bd · would try "), "{rows:?}");
    }

    /// With a fake transport: `js:true` calls exactly ONE provider when it answers, however slow (past the 5 s
    /// hedge, scaled to 200 ms); the second only after a failure; an `auto` ladder still hedges
    /// (`slow_first_provider_is_hedged`).
    #[test]
    fn paid_ask_runs_one_provider() {
        let p = Plan { js: true, hedge: Duration::from_millis(50), ..plan(Mode::Auto) };
        for (first, want) in
            [("ok", vec![(Tier::Bd, "ok")]), ("js_shell", vec![(Tier::Bd, "js_shell"), (Tier::OxyJs, "ok")])]
        {
            // bd answers past the hedge (200 ms), oxyjs at once
            let (ms, code) =
                (|t| if t == Tier::Bd { 200 } else { 5 }, move |t| if t == Tier::Bd { first } else { "ok" });
            let call = move |t: Tier, _: &[Hop]| -> Call {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(ms(t))).await;
                    hop(t, code(t))
                })
            };
            let hops = drive(&p, &call, Vec::new(), &drop);
            let path: Vec<(Tier, &str)> = hops.iter().map(|h| (h.tier, h.code.as_str())).collect();
            assert_eq!(path, want, "bd {first}");
        }
    }

    #[test]
    fn site_is_the_registrable_domain() {
        for (url, want) in [
            ("https://m.youtube.com/watch?v=1", "youtube.com"),
            ("https://www.bbc.co.uk/news", "bbc.co.uk"),
            ("https://data.gov.uk/x", "data.gov.uk"),
            ("https://www.bundestag.de/portal", "bundestag.de"),
            ("https://www.tripadvisor.ca/", "tripadvisor.ca"),
            ("https://ex.com/ok", "ex.com"),
            ("http://10.0.0.7:8080/", "10.0.0.7"),
            ("http://localhost/", "localhost"),
            ("https://co.uk/", "co.uk"),
            ("https://alice.github.io/blog/", "alice.github.io"),
            ("https://a.b.github.io/", "b.github.io"),
            ("https://leapcell.medium.com/x", "medium.com"),
            ("https://medium.com/@bob/x", "medium.com"),
            ("https://www.medium.com/x", "medium.com"),
            ("https://my-app.vercel.app/", "my-app.vercel.app"),
            ("https://github.io/", "github.io"),
        ] {
            assert_eq!(site(url), want, "{url}");
        }
    }

    /// A government host by its labels, no host list: the last label, or the one before a two-letter
    /// country code with a host under it (`go.pl` is a site, `www.gov.uk` is not). `europa.eu` and `bundestag.de` left
    /// with the list (BD refused 3 of 6 sampled gov hosts the labels catch; a miss costs one $0 hop).
    #[test]
    fn gov_is_a_label_rule() {
        let yes = ["https://www.sec.gov/x", "https://apps.dtic.mil/", "https://data.gov.uk/x", "https://www.gov.pl/"];
        let more = ["https://x.gouv.fr/", "https://dof.gob.mx/", "https://www.go.jp/", "https://ird.govt.nz/"];
        assert!(yes.iter().chain(&more).chain(&["https://www.gv.at/"]).all(|u| gov(u)));
        for no in
            ["https://go.pl/", "https://eur-lex.europa.eu/x", "https://www.bundestag.de/", "https://gov.example.com/"]
        {
            assert!(!gov(no), "{no}");
        }
        assert!(Plan::new("https://get.data.gov.uk/a", Mode::Auto, false, 1.0, None).gov);
    }

    /// Plain failed (no JS shell, so a shell from bd is no app page): bd and oxyjs are next.
    fn both() -> Vec<Hop> {
        vec![hop(Tier::Plain, "blocked_403")]
    }

    fn and(more: Vec<Hop>) -> Vec<Hop> {
        both().into_iter().chain(more).collect()
    }

    #[test]
    fn dry_route_and_skip_rows() {
        let all = vec![(Tier::Plain, None), (Tier::Oxy, None), (Tier::Bd, None), (Tier::OxyJs, Some(HEDGE))];
        assert_eq!(route(&plan(Mode::Auto)), (all, 0.004), "oxy no-JS after plain's captcha, the worst case");
        // Both keys, oxy no-JS after a captcha only; after a shell the page needs JS
        let after = |code| plan(Mode::Auto).ladder(&[hop(Tier::Plain, code)]);
        let renders = [Tier::Bd, Tier::OxyJs, Tier::DecodoJs];
        assert_eq!(after("js_shell")[1..], renders, "a shell: plain → bd ∥ oxyjs (→ decodojs with its key)");
        assert_eq!(after("captcha_wall")[..2], [Tier::Plain, Tier::Oxy], "plain → oxy → decodo → bd ∥ oxyjs");
        let race = Plan { hedge: Duration::ZERO, ..plan(Mode::Auto) };
        assert_eq!(route(&race).0[3], (Tier::OxyJs, Some(Duration::ZERO)));
        // bd is given up on (1 s + grace) before a 60 s hedge: oxyjs follows it instead of racing it
        let late = Plan { hedge: Duration::from_secs(60), timeout: Some(Duration::from_secs(1)), ..plan(Mode::Auto) };
        assert_eq!(route(&late).0[3], (Tier::OxyJs, None));
        let nokey = Plan { keys: vec![Tier::Plain], ..plan(Mode::Auto) };
        assert_eq!(route(&nokey).0, vec![(Tier::Plain, None)]);
        let rows = skipped(&nokey, &[hop(Tier::Plain, "blocked_403")], "u1", "u1", "");
        assert_eq!(rows.len(), 3, "one row per missing key, the cheapest first: {rows:?}");
        let oxy = "u1 ! no_key oxy · would try no-JS fetch + JS render ≤$0.0025 → ";
        assert!(rows[1].starts_with(oxy) && rows[0].starts_with("u1 ! no_key bd · would try unblock+JS"), "{rows:?}");
        assert!(rows[2].contains("no_key decodo · would try no-JS fetch + JS render ≤$0.0025"));
        // after a JS shell oxy no-JS served 0/15, so a key would buy the render only
        let rows = skipped(&nokey, &[hop(Tier::Plain, "js_shell")], "u1", "u1", "");
        assert!(rows[1].starts_with("u1 ! no_key oxy · would try JS render ≤$0.0015 → "), "{rows:?}");
        assert!(skipped(&nokey, &[hop(Tier::Plain, "not_found")], "u1", "u1", "").is_empty());
        let poor = Plan { cap: 0.001, ..plan(Mode::Auto) };
        let walled = [hop(Tier::Plain, "captcha_wall"), hop(Tier::Oxy, "x")];
        let rows = skipped(&poor, &walled, "u1", "u1", "");
        let want = format!(
            "u1 ! budget_hit bd oxyjs not tried · $0.0040 > cost:0.001 → {} u1 fresh:true cost:0.004 ≤$0.0040",
            crate::lang::bin()
        );
        let decodo = "u1 ! no_key decodo · would try no-JS fetch + JS render ≤$0.0025";
        assert!(rows[0].starts_with(decodo) && rows[1..] == [want], "decodo's key could still serve it: {rows:?}");
        // three renders are $0.0045, never the float sum's $0.0046
        let renders = Plan { keys: vec![Tier::Plain, Tier::Bd, Tier::OxyJs, Tier::DecodoJs], ..poor.clone() };
        let rows = skipped(&renders, &[hop(Tier::Plain, "js_shell")], "u1", "u1", "");
        assert!(rows.iter().any(|r| r.contains(" not tried · $0.0045 > cost:0.001 ")), "{rows:?}");
        // Priced for the URL's own plan, as its `dry:true` (a .gov page: no bd, `up to $0.0025`)
        let gov = skipped(&poor, &walled, "u1", "u1", "https://www.usa.gov/x");
        assert!(gov[1].ends_with(" cost:0.004 ≤$0.0025"), "{gov:?}");
        // The words after the binary heading the arrow, never after its name further in
        let arrow = |head: &str| worst(&format!("{head}{} u1 fresh:true", crate::lang::bin()), "");
        assert!(arrow("").is_some_and(|r| r.1 > 0.0 && r.0[0].0 == Tier::Plain), "{:?}", arrow(""));
        assert!(arrow("x").is_none() && arrow("top up, then ").is_none());
        assert!(skipped(&poor, &[hop(Tier::Plain, "too_big")], "u1", "u1", "").is_empty());
        let mobile = Ask { mobile: true, url: "https://a.example/".into(), ..Ask::default() };
        let row = ignored(&mobile, &[hop(Tier::Plain, "js_shell"), hop(Tier::Bd, "ok")], "u1");
        let want = "u1 ! ignored device bd · Bright Data has no device setting: desktop page → {b} u1 fresh:true \
                    provider:oxy device:mobile ≤$0.0025";
        assert_eq!(row, [want.replace("{b}", crate::lang::bin())]);
        assert!(ignored(&mobile, &[hop(Tier::Oxy, "ok")], "u1").is_empty());
        assert!(ignored(&Ask::default(), &[hop(Tier::Bd, "ok")], "u1").is_empty());
    }

    /// Geo copies (`<html lang>`, title and first lines of the saved bytes, hand-trimmed): Reddit in
    /// Italian and in Arabic, Booking in pt-BR (oxyjs). The expected language is the page's own
    /// (`country:`, its URL's locale, plain's copy when plain held the page), never a guess. Behind plain's wall
    /// (what live runs get) only the URL can name it; where plain held the English page, they are flagged; bd's
    /// English Reddit is not; a Spanish `/es/es/` page on a `.com` is not.
    #[test]
    fn a_geo_copy_is_flagged() {
        use Tier::*;
        let page = |t: Tier, html: &str, url: &str, status: u16| {
            let body = html.as_bytes().to_vec();
            judge(t, Raw { status, ctype: "text/html".into(), body, ..Raw::default() }, url, t == OxyJs, false)
        };
        let reddit = "https://www.reddit.com/r/rust/comments/63wts9/why_are_there_so_many_linear_algebra_crates/";
        let ask = |url: &str| Ask { url: url.into(), ..Ask::default() };
        let why = |h: Hop, url: &str, done: &[Hop]| geo(h, &langs(&ask(url), done)).why;
        let en = include_str!("../tests/fixtures/geo-reddit-en.html");
        let held = page(Plain, en, reddit, 200);
        assert_eq!((held.ok, held.lang.as_str()), (true, "en"), "plain held the English page");
        let odd =
            |lang: &str| page(Plain, &en.replacen("<html", &format!("<html lang=\"{lang}\" x"), 1), reddit, 200).lang;
        assert_eq!([odd(&"a".repeat(10_000)), odd("e1-US"), odd(" pt-BR ")], ["", "", "pt"], "bounded");
        let it = || page(OxyJs, include_str!("../tests/fixtures/geo-reddit-it.html"), reddit, 200);
        assert!(it().ok, "the Italian copy passes every content check");
        // the path real runs take: plain gets a wall (plain held Reddit 0 of 29, Booking 0 of 14), whose
        // `lang` is not the site's, at any status; the expectation is then the URL's alone (ccTLD, `xx.`, first
        // segment), and a `.com` with none is no flag: the copy's language cannot be judged
        let wall =
            |url: &str, status: u16| page(Plain, include_str!("../tests/fixtures/js-challenge.html"), url, status);
        for status in [200, 403] {
            let w = wall(reddit, status);
            assert_eq!((w.ok, w.code.ends_with("_wall"), w.lang.as_str()), (false, true, ""), "{status}");
        }
        assert_eq!(why(it(), reddit, &[wall(reddit, 200)]), "", "a walled .com: no expectation, no flag");
        assert_eq!(why(it(), reddit, &[]), "", "no plain copy (js:true): no expectation, no flag");
        let eu = "https://www.reddit.com/r/europe/comments/1tb1ts5/vorstellungsgesprach_beim_energieversorger/";
        let ar = || page(OxyJs, include_str!("../tests/fixtures/geo-reddit-ar.html"), eu, 200);
        assert_eq!(why(ar(), eu, &[wall(eu, 403)]), "");
        let booking = "https://www.booking.com/hotel/fr/example-hotel.html";
        let pt = || page(OxyJs, include_str!("../tests/fixtures/geo-booking-ptbr.html"), booking, 200);
        assert_eq!(why(pt(), booking, &[wall(booking, 403)]), "", "/hotel/fr/ names no language");
        for url in ["https://www.reddit.fr/r/x", "https://it.example.org/x", "https://ex.com/it/x"] {
            assert_eq!(why(ar(), url, &[wall(url, 403)]), "lang:ar", "walled, the URL names it: {url}");
        }
        // Zara-class: plain held the English page, so its `lang` is the site's own
        assert_eq!(why(it(), reddit, std::slice::from_ref(&held)), "lang:it");
        assert_eq!(why(ar(), eu, &[page(Plain, en, eu, 200)]), "lang:ar");
        assert_eq!(why(pt(), booking, &[page(Plain, en, booking, 200)]), "lang:pt");
        // only the FIRST path segment names a locale: Booking's `/hotel/fr/` is the hotel's country (in a replay
        // 10 of 16 flags were Booking's English copies flagged `lang:en`)
        assert_eq!(why(page(Oxy, en, booking, 200), booking, &[]), "", "/hotel/fr/ in English: no flag");
        assert_eq!(why(page(Bd, en, reddit, 200), reddit, &[held]), "", "the English control");
        let zara = "https://www.zara.com/es/es/x.html";
        let es_page = include_str!("../tests/fixtures/geo-zara-es.html");
        assert_eq!(why(page(Oxy, es_page, zara, 200), zara, &[]), "", "/es/es/ names es: the page asked for");
        let en_zara = page(Oxy, en, zara, 200);
        assert_eq!(why(en_zara, zara, &[]), "lang:en", "/es/es/ in English is a geo copy");
        for (url, want) in [
            ("https://www.zara.es/x", ""),
            ("https://de.example.org/wiki/x", "lang:es"),
            ("https://en-gb.example.org/x", "lang:es"),
            ("https://ex.com/pt-br/x", "lang:es"),
            ("https://ex.com/shop/pt-br/x", ""),
            ("https://ex.com/go/es-page", ""),
        ] {
            assert_eq!(why(page(Oxy, es_page, url, 200), url, &[]), want, "{url}");
        }
        let cc = Ask { country: Some("it".into()), ..ask(reddit) };
        assert_eq!(geo(it(), &langs(&cc, &[])).why, "", "country:it names it");
    }

    /// A URL naming no locale, fetched with no country sent (bd at home US), expects `en`; an
    /// Oxylabs call sends US, a query value or `country:` names one: no expectation (`a_geo_copy_is_flagged`).
    #[test]
    fn a_no_locale_copy_sent_none_expects_english() {
        use Tier::*;
        let booking = "https://www.booking.com/hotel/fr/example-hotel.html";
        let page = |t: Tier, html: &str, url: &str| {
            let (body, final_url) = (html.as_bytes().to_vec(), url.into());
            let raw = Raw { status: 200, ctype: "text/html".into(), body, final_url, ..Raw::default() };
            judge(t, raw, url, t == OxyJs, false)
        };
        let why = |h: Hop, url: &str, country: Option<&str>| {
            geo(h, &langs(&Ask { url: url.into(), country: country.map(Into::into), ..Ask::default() }, &[])).why
        };
        let es = |t: Tier, url: &str| page(t, include_str!("../tests/fixtures/geo-booking-es.html"), url);
        assert_eq!(why(es(Bd, booking), booking, None), "lang:es", "bd sent none");
        let en = page(Bd, include_str!("../tests/fixtures/geo-reddit-en.html"), booking);
        assert_eq!(why(en, booking, None), "", "bd in English");
        assert_eq!(why(es(OxyJs, booking), booking, None), "", "oxyjs sent US");
        let hl = "https://www.booking.com/hotel/fr/example-hotel.html?lang=es-es";
        assert_eq!(why(es(Bd, hl), hl, None), "", "the query names es");
        assert_eq!(why(es(Bd, booking), booking, Some("es")), "", "country:es");
        // a page whose canonical link is the URL asked is the site's own, in its own language: no flag, no paid
        // fix, an `own:` trace; a geo copy names its language's URL or no canonical at all (above), and stays
        // flagged
        for (file, url, want) in [
            (
                include_str!("../tests/fixtures/geo-own-es.html"),
                "https://www.pisos.example/alquiler/madrid/?orden=precio",
                "own:es",
            ),
            (
                include_str!("../tests/fixtures/geo-own-de.html"),
                "https://shop.example.com/produkt/kaffeemuehle-42",
                "own:de",
            ),
            (
                include_str!("../tests/fixtures/geo-copy-es.html"),
                "https://www.hotels.example/hotel/fr/example-hotel.html",
                "lang:es",
            ),
            (
                include_str!("../tests/fixtures/geo-own-es.html"),
                "https://www.pisos.example/alquiler/madrid/",
                "lang:es",
            ),
        ] {
            assert_eq!(why(page(Bd, file, url), url, None), want, "{url}");
        }
        let own = page(
            Bd,
            include_str!("../tests/fixtures/geo-own-es.html"),
            "https://www.pisos.example/alquiler/madrid/?orden=precio",
        );
        assert_eq!((own.ok, own.lang.as_str()), (true, "es"), "a served Spanish page");
    }

    /// The driver with a fake transport: plain fails, oxy without JS too (one try after a captcha), bd is slow; oxyjs
    /// starts when the hedge runs out (5 s in `next`, scaled to 200 ms here), wins, and bd is cancelled, its list price
    /// unverified.
    #[test]
    fn slow_first_provider_is_hedged() {
        let p = Plan { hedge: Duration::from_millis(200), ..plan(Mode::Auto) };
        let started = std::sync::Mutex::new(Vec::new());
        let t0 = Instant::now();
        let call = |t: Tier, _: &[Hop]| -> Call {
            started.lock().expect("lock").push((t, t0.elapsed()));
            Box::pin(async move {
                let wait = if t == Tier::Bd { 5_000 } else { 5 };
                tokio::time::sleep(Duration::from_millis(wait)).await;
                hop(t, ["captcha_wall", "", "provider_error", "ok"][t as usize])
            })
        };
        let hops = drive(&p, &call, Vec::new(), &drop);
        let path: Vec<(Tier, &str)> = hops.iter().map(|h| (h.tier, h.code.as_str())).collect();
        let want = [
            (Tier::Plain, "captcha_wall"),
            (Tier::Oxy, "provider_error"),
            (Tier::OxyJs, "ok"),
            (Tier::Bd, "abandoned"),
        ];
        assert_eq!(path, want);
        let started = started.into_inner().expect("lock");
        let at = |t: Tier| started.iter().find(|s| s.0 == t).map(|s| s.1).unwrap_or_default();
        assert!(at(Tier::OxyJs) >= at(Tier::Bd) + Duration::from_millis(200), "oxyjs waited the hedge: {started:?}");
        assert!(t0.elapsed() < Duration::from_secs(1), "the loser was not awaited");
        assert!(at(Tier::Oxy) < at(Tier::Bd), "oxy no-JS before the renders after a captcha: {started:?}");
        // The cancelled loser may be finished and billed by the provider: its price is unverified, not spent
        assert_eq!((hops[3].cost, hops[3].upto), (0.0, 0.0015), "the cancelled call: unverified");
        let mut hops = hops;
        let o = outcome("https://a.example/", &mut hops, 9);
        let tried = "plain:captcha_wall✗ oxy:provider_error✗$0.0010 oxyjs✓ bd:abandoned✗≤$0.0015 (unverified)";
        assert_eq!((o.meta.cost, o.meta.tried().as_str(), o.meta.render), (0.0025, tried, Some(true)));
    }

    /// With the free fetch off (plain not keyed) there is no free verdict to wait for: oxy without JS, its 10 s row, is
    /// the first rung, then bd ∥ oxyjs as after plain's captcha; a no-JS answer ends the run before a render starts.
    /// A page both renders cut at 60 s served without JS in 6.4 s.
    #[test]
    fn free_off_starts_at_the_no_js_rung() {
        use Tier::*;
        let off = Plan { keys: vec![Bd, Oxy, OxyJs], ..plan(Mode::Auto) };
        assert_eq!((next(&off, &[], &[]), off.timeout_of(Oxy, &[])), (Step::Start(Oxy), S(10)));
        assert_eq!(next(&off, &[], &[(Oxy, S(1))]), Step::Wait(S(14)), "a gate: its verdict first");
        assert_eq!(next(&off, &[hop(Oxy, "captcha_wall")], &[]), Step::Start(Bd));
        assert_eq!(next(&off, &[hop(Oxy, "provider_timeout")], &[]), Step::Start(Bd), "cut: the renders");
        assert_eq!(next(&off, &[hop(Oxy, "js_shell")], &[(Bd, S(5))]), Step::Start(OxyJs), "then the hedge");
        let all = vec![(Oxy, None), (Bd, None), (OxyJs, Some(HEDGE))];
        assert_eq!(route(&off), (all.clone(), 0.004));
        assert_eq!(crate::cli::path_of(&all), "oxy → bd ∥ oxyjs after 5s", "dry:true and config");
        let one = Plan { keys: vec![Oxy, OxyJs], ..plan(Mode::Auto) };
        assert_eq!(route(&one).0, [(Oxy, None), (OxyJs, None)], "one key: unchanged");
        let started = std::sync::Mutex::new(Vec::new());
        let call = |t: Tier, _: &[Hop]| -> Call {
            started.lock().expect("lock").push(t);
            Box::pin(async move { hop(t, "ok") })
        };
        let hops = drive(&off, &call, Vec::new(), &drop);
        assert_eq!(hops.iter().map(|h| (h.tier, h.ok)).collect::<Vec<_>>(), [(Oxy, true)]);
        assert_eq!(started.into_inner().expect("lock"), [Oxy], "no render started");
    }

    /// THE billing rule for both providers: an answer is billed, a call WE cut (no status: our timeout, `wait:`,
    /// abandoned) is unverified at list price, the provider's own refusal, 408 or 5xx envelope is $0 (bd
    /// billed 15 s search cuts that `fats` printed `$0 (not charged)`).
    #[test]
    fn a_cut_is_unverified_a_refusal_is_free() {
        let at = |bill: bool, code: &str, status: u16| {
            let h = billed(
                Instant::now(),
                bill,
                0.0015,
                Hop { tier: Tier::Bd, code: code.into(), status, ..Hop::default() },
            );
            (h.cost, h.upto)
        };
        assert_eq!(at(true, "ok", 200), (0.0015, 0.0));
        for code in ["provider_timeout", "abandoned"] {
            assert_eq!(at(false, code, 0), (0.0, 0.0015), "{code}: we cut it");
        }
        assert_eq!(at(false, "provider_timeout", 408), (0.0, 0.0), "the provider's own 408");
        assert_eq!(at(false, "provider_error", 500), (0.0, 0.0), "its error envelope");
        assert_eq!(at(false, "http_502", 502), (0.0, 0.0), "bd's inner 5xx");
        let cut = Hop { upto: 0.0015, cost: 0.0, ..hop(Tier::Bd, "provider_timeout") };
        let m = Meta { hops: vec![cut.row()], ..Meta::default() };
        assert_eq!((m.tried().as_str(), m.upto()), ("bd:provider_timeout✗≤$0.0015 (unverified)", 0.0015));
        assert_eq!(crate::provider::bill(0.0225, 0.006), "$0.0225 + ≤$0.0060 unverified");
    }

    /// A timeout's fix is a render while one is left to try; a render that ran out its time with none left is the
    /// end (no arrow), the agent's own `timeout:` cut the default time; shells from two providers are
    /// `no_copy` with no paid next step.
    #[test]
    fn timeout_fix_never_repeats_a_render() {
        use Tier::*;
        let b = crate::lang::bin();
        let (js, retry) = (format!("{b} u1 fresh:true js:true ≤$0.0030"), format!("{b} u1 fresh:true ≤$0.0040"));
        let free = format!("{retry} · last try not charged");
        let none = "nothing left to try · open https://a.example/ in a browser";
        let (bd, oxy) = (
            format!("{b} u1 fresh:true provider:bd ≤$0.0015"),
            format!("{b} u1 fresh:true provider:oxy js:true ≤$0.0015"),
        );
        type Path<'a> = &'a [(Tier, &'a str)];
        let rows: &[(Path, &str, &str)] = &[
            (&[(Plain, "provider_timeout")], "provider_timeout", &js),
            // one provider's timeout on a shell is that provider's failure: a render is left
            (&[(Plain, "js_shell"), (Oxy, "provider_timeout")], "provider_timeout", &js),
            (&[(Plain, "x"), (Oxy, "x"), (Bd, "provider_timeout")], "provider_timeout", &js),
            (&[(Plain, "js_shell"), (Oxy, "x"), (Bd, "x"), (OxyJs, "provider_timeout")], "provider_timeout", ""),
            (&[(Plain, "js_shell"), (Oxy, "js_shell")], "js_shell", &oxy),
            // shells from two providers: an app page
            (&[(Plain, "js_shell"), (Bd, "js_shell"), (OxyJs, "js_shell")], "no_copy", none),
            (
                &[(Plain, "js_shell"), (Oxy, "js_shell"), (Bd, "provider_timeout"), (OxyJs, "abandoned")],
                "no_copy",
                none,
            ),
            (&[(Plain, "provider_timeout"), (Bd, "js_shell")], "js_shell", &oxy),
            (&[(Plain, "provider_timeout"), (Oxy, "provider_error")], "provider_error", &free),
            // oxy-only Joomla: the wall plain met heads, not oxyjs's timeout after it
            (
                &[(Plain, "cloudflare_wall"), (OxyJs, "provider_error"), (OxyJs, "provider_timeout")],
                "cloudflare_wall",
                &bd,
            ),
        ];
        for (path, code, want) in rows {
            let mut hops: Vec<Hop> = path.iter().map(|(t, c)| hop(*t, c)).collect();
            let m = outcome("https://a.example/", &mut hops, 1).meta;
            let done = spent(&m, &[], &KEYED);
            let at = |arg| crate::lang::At { id: "u1", url: &m.url, arg, done: &done };
            let fix = fix_arg(&m, &[]).map_or(String::new(), |a| crate::lang::fix(move_code(&m), &at(a)));
            assert_eq!((m.code.as_str(), fix.as_str()), (*code, *want), "{path:?}");
        }
        // `timeout:8` once said "nothing left to try": the agent's cut → the default time; a tier's own cut row
        let p = Plan { timeout: Some(S(8)), ..plan(Mode::Auto) };
        let cut = |s: u64| cut_why(&p, Hop { timeout_s: s, ..hop(Bd, "provider_timeout") });
        assert_eq!((cut(8).why.as_str(), cut(40).why.as_str()), ("timeout:8s", "took > 40 s here"));
        let mut hops = vec![hop(Plain, "blocked_403"), hop(Oxy, "x"), cut(8), Hop { tier: OxyJs, ..cut(8) }];
        let m = outcome("https://a.example/", &mut hops, 1).meta;
        let at = crate::lang::At {
            id: "u1",
            url: &m.url,
            arg: fix_arg(&m, &[]).unwrap_or_default(),
            done: &spent(&m, &[], &KEYED),
        };
        assert_eq!(crate::lang::fix(&m.code, &at), retry, "after `timeout:8`: fresh:true, never nothing left");
        let mut hops = vec![hop(Plain, "blocked_403"), hop(OxyJs, "x"), cut(40)];
        let m = outcome("https://a.example/", &mut hops, 1).meta;
        // Read as `cut at 40 s (5m ago)`, its age from the copy's time; none under a minute
        assert_eq!(fail_detail(&m, "").0, "bd cut at 40 s");
        let old = Meta { t: m.t - 300.0, ..m.clone() };
        assert_eq!(fail_detail(&old, "").0, "bd cut at 40 s (5m ago)");
        assert_eq!(fix_arg(&m, &[]), Some("js:true"), "oxy no-JS left: `lang::fix` names it");
    }

    /// No longer call follows our cut: a render cut at 40 s (or at the agent's `timeout:150`) with no
    /// keyed tier left is the run's end, a count with no arrow; the row names the cut.
    #[test]
    fn a_cut_render_is_the_end() {
        use Tier::*;
        let p = |s: u64| Plan { timeout: (s == 150).then_some(S(s)), ..plan(Mode::Auto) };
        let cut = |s: u64| cut_why(&p(s), Hop { timeout_s: s, ..hop(Bd, "provider_timeout") });
        for (s, why) in [(40, "bd cut at 40 s"), (150, "bd timeout:150s")] {
            let m = outcome("https://a.example/", &mut [hop(Plain, "js_shell"), hop(OxyJs, "x"), cut(s)], 1).meta;
            assert_eq!((fix_arg(&m, &[]), fail_detail(&m, "").0.as_str()), (None, why));
        }
    }

    /// Spotify: the provider refused the render (its 4xx) or gave up (`faulted`): settled, never asked again by
    /// name or by an arrow's own dry (`fail_row`'s gate); the keyed tier left by name, else a count; a bare
    /// 5xx is a flake. eBay: bd's rate limit is a wait, so with oxyjs settled bd is the move, never `provider:oxy`
    /// (its dry plans oxyjs). Another copy's 4xx settles this one only for `url`, a 404 or the same country.
    #[test]
    fn a_settled_tier_is_spent() {
        use Tier::*;
        let b = crate::lang::bin();
        let erred = |why: &str| Hop { why: why.into(), country: "US".into(), ..hop(OxyJs, "provider_error") };
        let m = |why: &str| outcome("https://a.example/", &mut [hop(Plain, "js_shell"), erred(why)], 1).meta;
        let row = |m: &Meta, others: &[Meta]| {
            let r = fail_row(m, others, "", "u1", false);
            (r.2.unwrap_or_default(), r.1)
        };
        let bd = outcome("https://a.example/", &mut [hop(Bd, "js_shell")], 1).meta;
        let to_bd = format!("{b} u1 fresh:true provider:bd ≤$0.0015");
        for why in ["400 url: value not supported", "404", "500 faulted: request failed after too many retries"] {
            assert_eq!(row(&m(why), &[]).0, to_bd, "{why}");
            assert_eq!(row(&m(why), std::slice::from_ref(&bd)), (String::new(), " · failed 1/2 here".into()), "{why}");
            assert!(spent(&m(why), &[], &KEYED).contains(&"oxyjs"), "{why}");
        }
        assert_eq!(fix_arg(&m("500"), &[bd.hops[0].clone()]), Some("provider:oxy js:true"), "a flake: the named retry");
        assert_eq!(fail_detail(&m("400 url: value not supported"), "").0, "oxyjs 400 url: value not supported");
        let ebay = |why: &str| {
            let mut hops = [hop(Bd, "rate_limited"), erred(why)];
            outcome("https://a.example/", &mut hops, 1).meta
        };
        assert_eq!(row(&ebay("500 faulted: request failed"), &[]).0, to_bd, "a rate limit is a wait");
        // another copy's 400 on another field, sent FR, settles nothing for an arrow whose request sends another
        // country (`fresh:true` may plan oxyjs), but does for one sending FR (`provider::sent`); one naming the url,
        // a 404 or a give-up settles every copy: its dry may not plan it
        let fr = |why: &str| Hop { country: "FR".into(), ..erred(why) }.row();
        let geo = |why: &str| Meta { url: "http://a.example/".into(), hops: vec![fr(why)], ..m("x") };
        let reset = outcome("http://a.example/", &mut [hop(Plain, "connect_failed")], 1).meta;
        let arrow = |why: &str| row(&reset, &[geo(why)]).0;
        assert_eq!(arrow("400 geo_location: not supported"), format!("{b} u1 fresh:true ≤$0.0040"), "scoped");
        let cut = [geo("400 geo_location: not supported")];
        assert!(!clear(&format!("{b} u1 fresh:true country:fr"), &reset, &cut), "the refused country again");
        assert_eq!(arrow("400 url: value not supported"), to_bd.as_str(), "every copy");
        assert_eq!(arrow("404"), to_bd.as_str(), "every copy");
    }

    /// 4-hop walks on two sites: the 150 s call failing as the 60 s call did is the same
    /// failure: a count, no second `fresh:true`; another tier or country is another way.
    #[test]
    fn the_longer_call_failing_the_same_way_is_a_count() {
        use Tier::*;
        let meta = |s: u64, cc: &str| {
            let bd = Hop { timeout_s: s, country: cc.into(), ..hop(Bd, "http_502") };
            outcome("https://a.example/", &mut [hop(Plain, "captcha_wall"), bd], 1).meta
        };
        let (u1, u3) = (meta(60, "FR"), meta(150, "FR"));
        assert_eq!(fix_arg(&u3, &[]), Some(""));
        assert_eq!(fail_count(&u3, [&u1].into_iter(), Some("")).as_deref(), Some(" · failed 2/2 here"));
        assert_eq!(fail_count(&u3, [&meta(60, "US")].into_iter(), Some("")), None, "another country");
    }

    /// A tier whose only signal was OUR cut (`upto`) casts no app-page vote, but it is tried: with nothing else
    /// left the run ends (TikTok: bd cut at 60 s; both renders cut), another keyed tier is the move (a Reddit
    /// thread: oxy no-JS cut at 10 s → oxyjs, then oxy no-JS after the renders' captchas).
    #[test]
    fn a_cut_tier_is_tried() {
        use Tier::*;
        let b = crate::lang::bin();
        let ours = |t: Tier, s: u64| Hop { upto: t.spec().price, timeout_s: s, ..hop(t, "provider_timeout") };
        let fix = |m: &Meta, cuts: &[HopRow]| {
            let arg = fix_arg(m, cuts);
            let at =
                crate::lang::At { id: "u1", url: &m.url, arg: arg.unwrap_or_default(), done: &spent(m, &[], &KEYED) };
            arg.map(|_| crate::lang::fix(&m.code, &at))
        };
        let mut tiktok = [hop(Plain, "js_shell"), hop(OxyJs, "js_shell"), ours(Bd, 60)];
        let m = outcome("https://a.example/", &mut tiktok, 1).meta;
        assert_eq!((m.code.as_str(), fix(&m, &[])), ("provider_timeout", None), "not no_copy: the end");
        let mut reddit =
            [hop(Plain, "blocked_403"), ours(Oxy, 10), hop(Bd, "captcha_wall"), hop(OxyJs, "captcha_wall")];
        let m = outcome("https://a.example/", &mut reddit, 1).meta;
        assert_eq!(fix(&m, &[]), Some(format!("{b} u1 fresh:true provider:oxy ≤$0.0025")), "oxy, then oxyjs");
        // 8 saved copies: both renders cut at 60 s = both tried, the end, never js:true
        let mut both = [hop(Plain, "js_shell"), ours(Bd, 60), ours(OxyJs, 60)];
        let m = outcome("https://a.example/", &mut both, 1).meta;
        assert_eq!(fix(&m, &[]), None, "never js:true");
        let again = outcome("https://a.example/", &mut [ours(Oxy, 10)], 1).meta;
        let cuts: Vec<HopRow> = m.hops.clone();
        assert_eq!(fix(&again, &cuts), None, "the renders another copy cut: a count");
        assert_eq!(fix(&again, &[]).as_deref(), Some(format!("{b} u1 fresh:true js:true ≤$0.0030").as_str()));
    }

    /// A Joomla copy with only `meta.code` changed: a wall at plain then OUR cut on the paid tier
    /// prints the same reason and move whatever code heads the row: the cut tier decides, never the head.
    #[test]
    fn a_wall_headed_cut_keeps_the_timeout_move() {
        use Tier::*;
        let b = crate::lang::bin();
        let ours =
            || Hop { upto: 0.0015, timeout_s: 40, why: "took > 40 s here".into(), ..hop(OxyJs, "provider_timeout") };
        let row = |m: &Meta| {
            let done = spent(m, &[], &KEYED);
            let at = |arg| crate::lang::At { id: "u1", url: &m.url, arg, done: &done };
            (fail_detail(m, "").0, fix_arg(m, &[]).map_or(String::new(), |a| crate::lang::fix(move_code(m), &at(a))))
        };
        let paid = Hop { cost: 0.0015, ..hop(Bd, "captcha_wall") };
        let bd = format!("{b} u1 fresh:true provider:bd ≤$0.0015");
        for (mut hops, want) in [
            (vec![hop(Plain, "cloudflare_wall"), ours()], bd),
            (vec![hop(Plain, "cloudflare_wall"), paid, ours()], String::new()),
        ] {
            let wall = outcome("https://a.example/", &mut hops, 1).meta;
            let cut = Meta { code: "provider_timeout".into(), ..wall.clone() };
            assert!(wall.code.ends_with("_wall"), "the wall heads it: {}", wall.code);
            let (why, fix) = row(&wall);
            assert!(why == "oxyjs cut at 40 s" && fix == want, "{}: {why} {fix}", wall.tried());
            assert_eq!(row(&wall), row(&cut), "the same row apart from the code word");
        }
        // behind a sign-in gate (`one_render`) the run is the gate's only when its render met it too: a cut or
        // refused render names the run, so its own move (the other provider, the end) survives
        for (last, want) in
            [("js_shell", "login_wall"), ("provider_timeout", "provider_timeout"), ("provider_error", "provider_error")]
        {
            let mut hops = vec![hop(Plain, "login_wall"), hop(Bd, last)];
            assert_eq!(outcome("https://a.example/a/all", &mut hops, 1).meta.code, want, "{last}");
        }
    }

    /// ONE render behind a sign-in gate whoever met it, plain or a render (plain's 403, then bd on the gate, once
    /// rendered twice); a render the provider refused rendered nothing, so the next one may try.
    #[test]
    fn a_sign_in_gate_gets_one_render() {
        use Tier::*;
        let p = Plan { keys: Tier::ALL.to_vec(), ..plan(Mode::Auto) };
        let rest = |mut hops: Vec<Hop>| {
            while let Step::Start(t) = next(&p, &hops, &[]) {
                hops.push(hop(t, "js_shell"));
            }
            hops.iter().filter(|h| h.tier.can("js")).map(|h| h.tier).collect::<Vec<_>>()
        };
        assert_eq!(rest(vec![hop(Plain, "login_wall")]), [Bd], "plain's gate");
        assert_eq!(rest(vec![hop(Plain, "blocked_403"), hop(Bd, "login_wall")]), [Bd], "bd met the gate");
        assert_eq!(rest(vec![hop(Plain, "login_wall"), hop(Bd, "provider_refused")]), [Bd, OxyJs], "bd refused");
    }

    /// 16 saved Akamai pages, 534-536 B, were `js_shell` before: a short page that loads no JavaScript and
    /// shows none of its own words is the site refusing this provider (no phrase list, any language; 1,323 saved
    /// bodies: 0 verdicts move); the fix is the other keyed provider, else a count.
    #[test]
    fn an_error_page_is_no_shell() {
        use Tier::*;
        let akamai = "<html><head><title>Site Maintenance</title></head><body><h1>Oops! Something went wrong.</h1>\
                      <p>Reference #18.5f3c1102.1760500000.1a2b3c</p></body></html>";
        assert_eq!(
            check(akamai, 200, "", "", "https://www.tiktok.com/@example_creator/video/7000000000000000002").1,
            "error_page"
        );
        let bare = "<html><head><title>Preise und Angebote</title></head><body><h1>Entschuldigung</h1>\
                    <p>Die Website ist vorübergehend nicht erreichbar.</p></body></html>";
        assert_eq!(check(bare, 200, "", "", "").1, "error_page", "any language");
        let moved = bare.replace("<body>", r#"<body><meta http-equiv="Refresh" content="0; url=/k">"#);
        assert_eq!(check(&moved, 200, "", "", "").1, "js_shell", "a refresh moves on");
        let post = "<h1>Something went wrong with my build</h1><p>The error code was E0502, borrow twice.</p>\
                <p>It builds on stable but not on beta; the borrow checker rejects the second loop.</p>";
        let own =
            format!("<html><head><title>Something went wrong with my build</title></head><body>{post}</body></html>");
        assert_eq!(check(&own, 200, "", "", "https://a.example/").1, "ok", "a page about an error shows its own words");
        let b = crate::lang::bin();
        let fix = |hops: &mut [Hop]| {
            let m = outcome("https://a.example/", hops, 1).meta;
            let arg = fix_arg(&m, &[]);
            let at =
                crate::lang::At { id: "u1", url: &m.url, arg: arg.unwrap_or_default(), done: &spent(&m, &[], &KEYED) };
            (fail_detail(&m, "").0, arg.map(|_| crate::lang::fix(&m.code, &at)))
        };
        let one = fix(&mut [
            hop(Plain, "js_shell"),
            Hop { why: "refused here (error page)".into(), ..hop(OxyJs, "error_page") },
        ]);
        let want =
            ("oxy refused here (error page)".to_string(), Some(format!("{b} u1 fresh:true provider:bd ≤$0.0015")));
        assert_eq!(one, want);
        assert_eq!(
            fix(&mut [hop(Plain, "js_shell"), hop(Bd, "error_page"), hop(OxyJs, "error_page")]).1,
            None,
            "a count"
        );
    }

    fn check(h: &str, status: u16, headers: &str, fin: &str, url: &str) -> (bool, String, usize, bool) {
        let r = Raw { status, headers: headers.into(), final_url: fin.into(), ..Raw::default() };
        super::check(&crate::html::parse(h), h, &r, url)
    }

    /// No phrase, no host: a short copy whose visible text is under `title_ratio` × its
    /// title's chars says only its own name, a state page ("Post isn't available"): `error_page`, so `spent` bars
    /// that provider. A short real page (example.com: two paragraphs under a 14-char title) stays served.
    #[test]
    fn a_title_only_copy_is_an_error_page() {
        let state = include_str!("../tests/fixtures/state-page.html");
        let url = "https://photos.example/p/DW3NZXxDJvB/";
        assert_eq!(check(state, 200, "", url, url).1, "error_page");
        let example = "<html><head><title>Example Domain</title></head><body><div><h1>Example Domain</h1>\
                       <p>This domain is for use in illustrative examples in documents. You may use this domain in \
                       literature without prior coordination or asking for permission.</p><p><a \
                       href=\"https://www.iana.org/domains/example\">More information...</a></p></div></body></html>";
        assert_eq!(check(example, 200, "", "https://example.com/", "https://example.com/").1, "ok");
        // a short page that serves its own title is the page, whatever script it loads; at 4xx the marker walls it
        let pin = "<html><head><title>Hubble Sees the Eye | Pins</title><script src=/a.js></script></head><body>\
                   <h1>Hubble Sees the Eye</h1><p>A spiral galaxy seen face-on by the Hubble Space Telescope, its \
                   arms full of young blue stars.</p><script src=/challenge.js></script></body></html>";
        assert_eq!(check(pin, 200, "", "", "").1, "ok", "a served copy is not a wall");
        assert_eq!(check(pin, 403, "", "", "").1, "captcha_wall", "a 4xx with the marker is");
    }

    /// No wall path is named (LinkedIn's `/authwall` row gone): a copy that landed where the asked id (8+ letters or
    /// digits) is gone and shows none of its own words is `redirected`; one that shows them is served (the outline
    /// flags it), and a short id or a kept one is no redirect. A sign-in gate hands the asked path back in its query
    /// and names a sign-in: `login_wall` before `redirected`, whatever the id; a landing that only lost the page,
    /// or a bounce that names no sign-in or hands back another site's page, is no gate.
    #[test]
    fn a_lost_id_is_redirected() {
        let url = "https://www.linkedin.com/company/example-data-insights/";
        let wall = "<html><head><title>تسجيل | LinkedIn</title></head><body><h1>انضم</h1><script src=/a.js></script></body></html>";
        let fin = "https://www.linkedin.com/authwall?trk=bf&sessionRedirect=https%3A%2F%2Fwww.linkedin.com%2Fcompany";
        assert_eq!(check(wall, 200, "", fin, url).1, "redirected");
        let own = "<html><head><title>Example Data Insights | LinkedIn</title></head><body><h1>Example Data Insights</h1><p>Data analytics and business intelligence consulting in Lyon, France.</p><p>IT services · 51-200 employees · Lyon · 1,204 followers</p></body></html>";
        assert_eq!(check(own, 200, "", fin, url).1, "ok", "its own words: the page, wherever it landed");
        assert_eq!(
            check(wall, 200, "", fin, "https://x.com/company/abc/").1,
            "js_shell",
            "a 3-char id, not handed back"
        );
        let gate = "https://x.com/i/jf/onboarding/web?redirect_after_login=%2Fsomeone%2Fall&mode=login";
        assert_eq!(check(wall, 200, "", gate, "https://x.com/someone/all").1, "login_wall", "the path handed back");
        let back =
            "https://www.linkedin.com/uas/authenticate?r=https%3A%2F%2Flinkedin.com%2Fcompany%2Fexample-data-insights";
        assert_eq!(check(wall, 200, "", back, url).1, "login_wall", "before `redirected`, a full URL handed back");
        // live copies once misread as `redirected`: an index page's root, a renamed slug, a dropped sub-tab, a short
        // link's shell
        for (fin, asked) in [
            ("https://docs.ex.com/", "https://docs.ex.com/index.html"),
            ("https://ex.com/p/new-name-of-it?ref=old", "https://ex.com/p/oldnameofthething"),
            ("https://ex.com/u/someone", "https://ex.com/u/someone/likes"),
            ("https://ex.com/a/b?utm=x", "https://ex.link/xyz"),
            // bounces that hand the page back and name no sign-in, and a sign-in that hands back another site's
            ("https://ex.com/notfound?from=%2Fp%2Fthing-that-moved", "https://ex.com/p/thing-that-moved"),
            ("https://ex.com/choose-country?return=%2Fitem%2Fabcdefghij", "https://ex.com/item/abcdefghij"),
            ("https://ex.com/consent?continue=https%3A%2F%2Fex.com%2Fwatch", "https://ex.com/watch"),
            ("https://ex.com/p?next_login=https%3A%2F%2Fother.org%2Fwatch", "https://ex.com/watch"),
        ] {
            assert_ne!(check(wall, 200, "", fin, asked).1, "login_wall", "{asked} → {fin}");
        }
        assert_eq!(check(wall, 200, "", url, url).1, "js_shell", "the id kept");
    }

    /// The sign-in words are whole words of the landing's path or its query key, never a piece of one, and a
    /// compound is named by its last word: docs about authentication, an author's page, a page on designing, a
    /// consent page offering a login-free mode, a signing guide. The asked path itself is no landing.
    #[test]
    fn a_sign_in_is_a_whole_word() {
        for (fin, asked) in [
            (
                "https://ex.com/docs/authentication?from=%2Fdocs%2Fapi%2Foauth-tokens",
                "https://ex.com/docs/api/oauth-tokens",
            ),
            ("https://ex.com/authors/jane?ref=%2Fblog%2Fx", "https://ex.com/blog/x"),
            ("https://ex.com/designing?from=%2Fguide%2Fdesigning-apis", "https://ex.com/guide/designing-apis"),
            ("https://ex.com/consent/login-free?continue=https%3A%2F%2Fex.com%2Fwatch", "https://ex.com/watch"),
            ("https://ex.com/account?next=%2Fwatch", "https://ex.com/watch"),
            ("https://ex.com/signing-guide?from=%2Fwatch", "https://ex.com/watch"),
            ("https://ex.com/doc/login-best-practices", "https://ex.com/doc/login-best-practices"),
        ] {
            assert!(!sign_in_gate(asked, fin), "{asked} → {fin}");
        }
        for (fin, asked) in [
            ("https://ex.com/users/sign_in?redirect=%2Fwatch", "https://ex.com/watch"),
            ("https://ex.com/i/flow?redirect_after_login=%2Fwatch", "https://ex.com/watch"),
            ("https://ex.com/oauth/authorize?return_to=%2Fwatch", "https://ex.com/watch"),
            ("https://ex.com/sso?r=https%3A%2F%2Fex.com%2Fwatch", "https://ex.com/watch"),
            ("https://ex.com/customer-login?redirect=%2Fwatch", "https://ex.com/watch"),
            ("https://ex.com/en/signin.php", "https://ex.com/watch"),
            ("https://ex.com/sso?continue=https%3A%2F%2Fother.org%2Fwatch", "https://ex.com/watch"),
        ] {
            assert!(sign_in_gate(asked, fin), "{asked} → {fin}");
        }
    }

    /// No consent host is named; the landing alone is no wall.
    #[test]
    fn consent_is_a_notice() {
        let page =
            |title: &str, body: &str| format!("<html><head><title>{title}</title></head><body>{body}</body></html>");
        let long = "Lorem ipsum dolor sit amet. ".repeat(40);
        let ok = page("Preise und Angebote | Shop", &format!("<h1>Preise und Angebote</h1><p>{long}</p>"));
        assert_eq!(check(&ok, 200, "", "https://consent.youtube.com/m?continue=x", "").1, "ok");
        // the interstitial titled "Before you continue to YouTube" with ~3.1k chars of cookie text (3/3 saved copies
        // the url row caught): its title is no page word, so the notice walls it under 4000 chars
        let terms = "<p>We use cookies and data to deliver and maintain Google services and measure audiences.</p>";
        let interstitial = page(
            "Before you continue to YouTube",
            &format!("<h1>Before you continue to YouTube</h1>{}", terms.repeat(36)),
        );
        let wall = check(&interstitial, 200, "", "https://consent.youtube.com/m", "https://www.youtube.com/@Brian");
        assert_eq!((wall.1.as_str(), wall.2 > 3000), ("consent_wall", true));
    }

    #[test]
    fn content_check() {
        let page = |title: &str, body: &str| {
            format!("<html><head><title>{title}</title><script src=/a.js></script></head><body>{body}</body></html>")
        };
        let long = "Lorem ipsum dolor sit amet. ".repeat(40);
        let ok = page("Preise und Angebote | Shop", &format!("<h1>Preise und Angebote</h1><p>{long}</p>"));
        assert_eq!(check(&ok, 200, "", "https://a.example/", "").1, "ok");
        let shell = page("Preise und Angebote", &format!("<div id=app></div><noscript>{long}</noscript>"));
        assert_eq!(check(&shell, 200, "", "", "").1, "js_shell", "title words only in <head>");
        let cf = page("Just a moment...", "<div class=cf-chl-widget></div>");
        assert_eq!(check(&cf, 403, "", "", "").1, "cloudflare_wall");
        assert_eq!(check(&ok, 403, "cf-mitigated: challenge\n", "", "").1, "cloudflare_wall");
        assert_eq!(check(&page("Etsy", "<p>Blocked</p>"), 403, "x-datadome: protected\n", "", "").1, "captcha_wall");
        let big = page("Preise und Angebote", &format!("<h1>Preise und Angebote</h1>{}", long.repeat(2)));
        assert_eq!(check(&big, 200, "x-datadome: protected\n", "", "").1, "ok", "a protected site that answered");
        // a challenge is a wall, never a shell: the ladder goes on past it (Reddit's no-JS copy is a
        // hidden solver form, its rendered copy "Prove your humanity"); a shell that only loads a captcha script stays
        // a shell (Facebook, Instagram: the two-shell stop)
        let challenge = include_str!("../tests/fixtures/js-challenge.html");
        assert_eq!(check(challenge, 200, "", "", "").1, "captcha_wall", "the no-JS challenge form");
        for words in ["Prove your humanity", "Please enable JavaScript and cookies to continue"] {
            assert_eq!(check(&page("Reddit", &format!("<p>{words}</p>")), 200, "", "", "").1, "captcha_wall");
        }
        let app = page("Instagram", "<div id=app></div><script src=/rsrc/recaptcha.js></script>");
        assert_eq!(check(&app, 200, "", "", "").1, "js_shell", "a captcha script is no wall");
        let robot = page("Fehler", "<p>Are you a robot?</p>");
        assert_eq!(check(&robot, 200, "", "", "").1, "captcha_wall", "folded marker");
        assert_eq!(check(&ok, 403, "", "", "").1, "blocked_403");
        assert_eq!(check(&ok, 404, "", "", "").1, "not_found");
        let login = "https://x.com/login?next=/";
        assert_eq!(check(&shell, 200, "", login, "https://x.com/watch").1, "login_wall", "a moved landing");
        assert_eq!(check(&ok, 200, "", login, "https://x.com/watch").1, "ok", "a served page is no sign-in");
        // the wall rows run in file order before the sign-in, consent by a short notice (`consent_is_a_notice`)
        assert_eq!(check(&shell, 403, "", login, "https://x.com/watch").1, "blocked_403");
        let notice = page("YouTube", "<p>Before you continue to YouTube</p>");
        assert_eq!(check(&notice, 200, "", "", "").1, "consent_wall");
        assert_eq!(
            check(&page("Preise und Angebote", "<p>Before you continue</p>"), 200, "", "", "").1,
            "consent_wall"
        );
        assert_eq!(title_head("Preise – Shop | Brand"), "Preise");
        // the two no-JS bodies Google serves (Maps, Search): the title words are visible, the content is not
        let maps = page("Google Maps", "<noscript>Enable JavaScript to see Google Maps.</noscript>");
        assert_eq!(check(&maps, 200, "", "", "").1, "js_shell");
        let stub = page("Google Search", "If you're having trouble accessing Google Search, please click here");
        let q = "https://www.google.fr/search?q=lyon";
        assert_eq!(check(&stub, 200, "", "", q).1, "serp_empty", "a SERP is judged by its results");
        let hits: String =
            (0..5).map(|i| format!("<a href=\"/url?q=https%3A%2F%2Fsite{i}.fr%2F\">Lyon old town {i}</a>")).collect();
        assert_eq!(check(&page("lyon - Google Search", &hits), 200, "", "", q).1, "ok");
        let nav: String =
            (0..5).map(|i| format!("<a href=\"/p{i}\">Home</a><a href=\"#r{i}\">jump to result {i}</a>")).collect();
        assert_eq!(check(&page("lyon - Google Search", &nav), 200, "", "", q).1, "serp_empty", "no result rows");
        let bsky = page(
            "@mariaa.bsky.social on Bluesky",
            "<p>Maria mariaa.bsky.social wrote a post.</p><p>The lake froze overnight for the first time in years; we went skating at dawn.</p>\
             <noscript>JavaScript is required.</noscript>",
        );
        assert_eq!(check(&bsky, 200, "", "", "").1, "ok", "the post is there despite the notice");
        let real = page(
            "Preise und Angebote",
            &format!("<noscript>Enable JavaScript</noscript><h1>Preise und Angebote</h1>{long}"),
        );
        assert_eq!(check(&real, 200, "", "", "").1, "ok", "a notice on a long page is not a shell");
    }

    /// A would-be shell (two title words, or under 800 visible chars) whose title words are not shown is a JS shell
    /// whatever text sits around them: nav and footer rows, a `<noscript>` notice, a footer's address, a cookie
    /// banner over an h1, a `role=contentinfo` footer, a `div.legal`, "requires JavaScript" beside a nav link,
    /// "activate scripting", "Loading Acme…".
    #[test]
    fn shell_fixtures_are_shells() {
        let shells = [
            include_str!("../tests/fixtures/shell-nav-footer.html"),
            include_str!("../tests/fixtures/shell-noscript.html"),
            include_str!("../tests/fixtures/shell-footer.html"),
            include_str!("../tests/fixtures/shell-blocks.html"),
            include_str!("../tests/fixtures/shell-notices.html"),
        ];
        let pages = shells.iter().flat_map(|f| f.split("<!-- page -->"));
        let codes: Vec<String> = pages.map(|h| check(h, 200, "", "", "https://acme.example/app").1).collect();
        assert_eq!(codes, ["js_shell"; 9]);
    }

    /// A short copy with one script whose title words are not on screen (a one-word title head, a site's label) is
    /// the page when it shows a text of its own: one text of ≥ `paragraph_words` words outside noscript, nav,
    /// header, footer and aside that asks nothing of JavaScript, in ≥ `thin_chars` visible chars. One word less
    /// and it stays a shell. The thin net after `check` keeps the thread served (446 pruned chars).
    #[test]
    fn a_text_of_its_own_is_the_page() {
        let thread = include_str!("../tests/fixtures/thread-short.html");
        let url = "https://forum.example/item?id=1";
        assert_eq!(check(thread, 200, "", "", url).1, "ok", "a 16-word comment under a one-word title");
        let h = judge(Tier::Plain, Raw { status: 200, body: thread.into(), ..Raw::default() }, url, false, false);
        assert_eq!((h.ok, h.code.as_str()), (true, "ok"), "the thin net keeps it");
        let rows = "<li>posted in the town square by a neighbour</li>".repeat(6);
        let page = |text: &str| {
            let nav = "<nav>home new past ask show submit login</nav>";
            let head = "<title>Zephyr | Board</title><script src=board.js></script>";
            format!("<html><head>{head}</head><body>{nav}<ul>{rows}</ul><p>{text}</p></body></html>")
        };
        let twelve = "the ferry runs twice a day from the old pier in summer";
        let (ok, code, chars, _) = check(&page(twelve), 200, "", "", url);
        assert_eq!((ok, code.as_str()), (true, "ok"), "12 words, {chars} chars");
        let eleven = "the ferry runs twice a day from the old pier in";
        assert_eq!(check(&page(eleven), 200, "", "", url).1, "js_shell", "11 words: no text of its own");
        let chrome = page("").replace("<nav>", &format!("<nav>{twelve} "));
        assert_eq!(check(&chrome, 200, "", "", url).1, "js_shell", "12 words in the nav are chrome");
    }

    /// On trimmed saved copies: a copy holding the page is served whatever its status or the wall
    /// scripts it loads; a search page by its result rows, whatever the engine's markup.
    #[test]
    fn a_copy_holding_the_page_is_served() {
        let so = include_str!("../tests/fixtures/served-403.html");
        let q = "https://qa.example.com/questions/1000/package-with-both-a-library-and-a-binary";
        assert_eq!(check(so, 403, "", "", q).1, "ok", "the Q&A under a 403 (was blocked_403)");
        let h = judge(Tier::OxyJs, Raw { status: 403, body: so.into(), ..Raw::default() }, q, false, false);
        assert_eq!((h.ok, h.row().why.as_str()), (true, "403"), "the status stays in the tier path");
        let denied = format!(
            "<html><head><title>Access Denied</title></head><body>Access Denied {}</body></html>",
            "x ".repeat(900)
        );
        assert_eq!(check(&denied, 403, "", "", "").1, "blocked_403", "1 title word is no page");
        let yt = include_str!("../tests/fixtures/serp-links.html");
        let serp = "https://www.youtube.com/results?search_query=rust+in+100+seconds";
        assert_eq!(check(yt, 200, "", "", serp).1, "ok", "6 result rows (was serp_empty)");
        let post = include_str!("../tests/fixtures/slug-served.html");
        let thread =
            "https://www.reddit.com/r/europe/comments/1tb1ts5/vorstellungsgespr%C3%A4ch_beim_energieversorger/";
        assert_eq!(check(post, 200, "", "", thread).1, "ok", "a site-wide title, the URL's words shown");
        assert_eq!(check(post, 200, "", "", "https://www.reddit.com/r/europe/").1, "cloudflare_wall");
        assert_eq!(check(post, 403, "", "", thread).1, "cloudflare_wall", "a short 4xx copy");
        // Wikipedia's missing article (saved 404, trimmed) echoes the URL's words, longer
        // outside its h1 (2,034 chars) than the 403 Q&A (1,767): a 404 is final, never the page
        let wiki = include_str!("../tests/fixtures/missing-404.html");
        let missing = "https://en.wikipedia.org/wiki/Rust_html_markdown_crate_zzqx";
        assert_eq!(check(wiki, 404, "", "", missing).1, "not_found");
        assert_eq!(check(wiki, 410, "", "", missing).1, "not_found");
    }

    /// A tiny complete page is ok; a long nav around an empty body is thin (one title word proves nothing).
    #[test]
    fn short_pages_and_thin_bodies() {
        let raw = |h: String| Raw { status: 200, body: h.into_bytes(), ..Raw::default() };
        let tiny = "<html><head><title>Example Domain</title></head><body><p>This domain is for use in documentation \
                    examples without needing permission.</p></body></html>";
        let parses = crate::html::PARSES.with(|n| n.get());
        let h = judge(Tier::Plain, raw(tiny.into()), "https://example.com/", false, false);
        assert_eq!((h.ok, h.code.as_str()), (true, "ok"));
        assert_eq!(crate::html::PARSES.with(|n| n.get()) - parses, 1, "the check and the cleaner share one parse");
        // the real page (a league game report): 1.4k visible chars of menus, 71 chars of body
        let nav: String = (0..45).map(|i| format!("<li><a href=/n{i}>menu item {i}</a></li>")).collect();
        let shell = format!(
            "<html><head><title>Home - Away - League</title></head><body><nav><ul>{nav}</ul></nav>\
             <p>Home 81:77</p><footer><ul>{nav}</ul></footer></body></html>"
        );
        let h = judge(Tier::Plain, raw(shell), "https://league.example/g/1", false, false);
        assert_eq!(h.code, "thin_content", "vis {}", h.vis);
        // a failed answer is stored as the best copy: cleaned from the check's parse, not parsed again
        let parses = crate::html::PARSES.with(|n| n.get());
        let wall = "<html><head><title>Just a moment...</title></head><body><p>checking</p></body></html>";
        let mut hops = vec![judge(Tier::Plain, raw(wall.into()), "https://a.example/", false, false)];
        let o = outcome("https://a.example/", &mut hops, 1);
        assert_eq!((o.meta.code.as_str(), o.doc.is_some()), ("cloudflare_wall", true));
        assert_eq!(crate::html::PARSES.with(|n| n.get()) - parses, 1, "one parse for the check and the stored copy");
    }

    /// A run killed between its reserve and its give-back (`pooled`) holds no floor (its lock let go,
    /// `cache::run_at`); its hold stays (a call it started may be billed), and the next page gets its rung. A live
    /// run's floor holds; a queued page's too (its lane's lock). Floors over the cap are even shares (`seed`).
    #[test]
    fn a_dead_runs_floor_goes_back() {
        let dir = std::env::temp_dir().join(format!("fats-pool-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let at = dir.join("u1.pool.json");
        let pool = Pool::from([(1, [0.0015, 0.0]), (2, [0.0015, 0.0]), (3, [0.0015, 0.001]), (4, [0.0015, 0.0])]);
        crate::cache::edit(&at, |p: &mut Pool| *p = pool).ok();
        let run = |n: usize, by: usize| {
            crate::cache::jsave(&dir.join(format!("u{n}.json")), &crate::cache::Detached { by, ..<_>::default() })
        };
        assert!([(1, 0), (2, 0), (3, 0), (4, 1)].iter().all(|&(n, by)| run(n, by).is_ok()));
        let lock = |n: usize| std::fs::File::create(dir.join(format!("u{n}.lock"))).and_then(|f| f.lock().map(|()| f));
        let mine = lock(1).ok();
        let claims_of =
            || claims(&at, &crate::cache::edit(&at, |p: &mut Pool| p.clone()).unwrap_or_default(), 1, false);
        assert_eq!(claims_of(), [(3, 0.001, ""), (4, 0.0015, " held")], "u2 and u3 died, u4 waits in u1's lane");
        let p = Plan { pool: Some((at.clone(), 1)), cap: 0.004, ..plan(Mode::Auto) };
        assert_eq!(pooled(&p, &[hop(Tier::Plain, "blocked_403")], &[]), Step::Start(Tier::Bd), "u2's floor went back");
        let held = lock(2).ok();
        let live = claims_of();
        assert!(live.iter().any(|c| c == &(2, 0.0015, " held")), "a live run keeps its floor: {live:?}");
        drop((mine, held));
        // 2 × $0.0015 over a $0.002 cap: an even share each, never the first page's whole floor and none for the next
        assert!(seed(&at, [(1, plan(Mode::Auto)), (2, plan(Mode::Auto))].into_iter(), 0.002).is_ok());
        let pool = crate::cache::edit(&at, |p: &mut Pool| p.clone()).unwrap_or_default();
        assert_eq!(pool, Pool::from([(1, [0.001, 0.0]), (2, [0.001, 0.0])]));
        std::fs::remove_dir_all(&dir).ok();
    }
}
