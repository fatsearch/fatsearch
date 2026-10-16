//! FETCH: the router. `auto` = plain first; a page that fails the content check goes to Bright Data, and to
//! Oxylabs with JavaScript rendering when Bright Data has not answered within the hedge delay (`hedge:`, 5 s).
//! Oxylabs without JavaScript is no page tier (it served 3 of 20 pages plain failed on, 2026-10-14 spike); it
//! stays for a search, a PDF's bytes and `provider:oxy`.
//! The first good body wins and the other call is cancelled, its bill unverified (`billed`). Government hosts skip Bright Data
//! (it refuses them). A PDF (`pdf_run`) goes plain → Oxylabs without JS → Bright Data for its bytes, never
//! JavaScript; a `/pdf/` path (`Plan::hint`) only gets the file timeout and raw bytes, page rules otherwise; a zip
//! document (docx, xlsx) never goes to a provider, and a text document plain served (`clean::Lists::text`) stops it. `next` is the whole decision table, pure, so
//! it is tested without a network.
//! Every page fetch walks the same ladder; `js:true` walks the table's render order (`JS_QUOTE`).

use crate::cache::{Cache, HopRow, Meta};
use crate::clean::{self, Doc};
use crate::html::Tree;
use crate::provider::{Ask, Hit, Raw, Tier, spend};
use crate::settings::limit;
use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// A page that passes the check but cleans to fewer body characters is a JS shell.
pub const THIN_CHARS: usize = 300;
/// Default `cost:` cap per command, USD.
pub const COST_CAP: f64 = 0.05;
/// The URL tables (`src/data/sites.json`).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, bound(deserialize = "'de: 'static"))]
pub struct Sites {
    /// Government labels (`gov()`), a TLD convention, no site: Bright Data refuses those hosts, so `auto` sends
    /// them straight to Oxylabs. The first two are TLDs; the rest are public suffixes under a country code.
    pub gov: Vec<&'static str>,
    /// URL suffixes that name a document, and its kind (what `clean::kind_of` finds in the answer). A text
    /// document's suffix only keeps a `/pdf/` path from being a PDF hint (`Plan::hint`): the answer says what it is.
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
    match flag {
        Some(v) => format!("hedge {v} s · flag hedge:{v}"),
        None => format!("hedge search {} s · fetch {} s · default", SEARCH_HEDGE.as_secs_f64(), HEDGE.as_secs_f64()),
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Auto,
    Only(Tier),
}

#[derive(Default)]
pub struct Plan {
    pub mode: Mode,
    /// `js:true`: skip plain (and Oxylabs without JS, where it runs); Bright Data, then Oxylabs rendering JavaScript.
    pub js: bool,
    pub cap: f64,
    pub gov: bool,
    /// The URL names a document by its suffix: `pdf`, `zip`, a text document (`Sites::doc_ext`).
    pub doc: Option<&'static str>,
    /// The path may name a file (`Sites::pdf_path`, no `.htm(l)` suffix): Oxylabs gets `PDF_OXY` and Bright Data sends raw
    /// bytes, but a page that passes the content check is served as a page; only an answer that says PDF makes it
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
}

impl Plan {
    pub fn new(url: &str, mode: Mode, js: bool, cap: f64, timeout: Option<Duration>) -> Plan {
        let gov = gov(url);
        let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
        let keys = keyed();
        let doc = SITES.doc_ext.iter().find(|e| path.ends_with(e.0)).map(|e| e.1);
        let page = path.ends_with(".html") || path.ends_with(".htm");
        let hint = doc.is_none() && !page && SITES.pdf_path.iter().any(|p| path.contains(p));
        Plan { mode, js, cap, gov, doc, hint, keys, timeout, hedge: HEDGE, ..Plan::default() }
    }

    /// A web search: the search rows (Oxylabs, then Bright Data beside it after the hedge); `provider:` = that one only.
    pub fn search(mode: Mode, cap: f64, timeout: Option<Duration>) -> Plan {
        let mut p = Plan { search: true, hedge: SEARCH_HEDGE, ..Plan::new("", mode, false, cap, timeout) };
        p.keys.retain(|t| t.can("search"));
        p
    }

    /// THE plan of a command's mods (`provider:`, `js:`, `cost:` less what this run `spent`, `timeout:`, `hedge:`),
    /// a page's for `url`, else a search's: what a fetch runs, `dry:true` routes and an arrow quotes (`worst`).
    pub fn of(cmd: &crate::lang::Cmd, url: Option<&str>, spent: f64) -> Plan {
        let mode = cmd.get("provider").and_then(Tier::parse).map_or(Mode::Auto, Mode::Only);
        let cap = (cmd.get("cost").and_then(|v| v.parse::<f64>().ok()).unwrap_or(COST_CAP) - spent).max(0.0);
        let timeout = cmd.get("timeout").and_then(|v| v.parse().ok()).map(Duration::from_secs);
        let mut p = match url {
            Some(u) => Plan::new(u, mode, cmd.flag("js"), cap, timeout),
            None => Plan::search(mode, cap, timeout),
        };
        p.hedge = cmd.get("hedge").and_then(|v| v.parse().ok()).map_or(p.hedge, Duration::from_secs);
        p
    }

    /// THE document rule: this run fetches a PDF when its URL names one (`.pdf`) or an answer said
    /// so (magic, or the content type on a failure). A PDF is never rendered: no oxyjs, and `js:true` does not
    /// skip plain.
    pub fn pdf_run(&self, done: &[Hop]) -> bool {
        !self.search && (self.doc == Some("pdf") || done.iter().any(|h| h.kind == "pdf"))
    }

    /// Tiers in the order they may run, before any answer.
    pub fn tiers(&self) -> Vec<Tier> {
        self.ladder(&[])
    }

    /// THE ladder rule: the tiers in the order they may run, given the finished hops (`pdf_run`). `auto` = the
    /// capability rows (`provider::Spec::can`) that do this job, cheapest first (table order on a tie: bd before
    /// oxyjs); a paid no-JS tier leaves a page's ladder when another provider's render has a key (it served 3 of 20
    /// pages plain failed on), so with one provider's key every rung of that provider runs, unless
    /// plain's wall needs a render (`Check::render_first`: after a JS challenge or shell oxy no-JS served 0/33, after
    /// a 403 or captcha 11/26). With both keys it stays, before the renders,
    /// after plain's captcha only (`Check::no_js_first`: Reddit 25/34 in ~4.4 s for $0.0010, a render $0.0015); one
    /// try, no site log. Never after a shell, the page needs JS (0/3 served, each +$0.0010, +4–10 s).
    /// `provider:oxy` = both Oxylabs tiers, without JS first.
    fn ladder(&self, done: &[Hop]) -> Vec<Tier> {
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
        // a paid no-JS tier yields to another provider's render but after plain's `no_js_first` wall, to its own
        // render after a `render_first` one
        let yields = |t: Tier| if render(t) { !plain(&CHECK.no_js_first) } else { plain(&CHECK.render_first) };
        let all = match self.mode {
            _ if self.prefetch => vec![Tier::Plain],
            Mode::Only(Tier::Oxy) if !self.search => vec![Tier::Oxy, Tier::OxyJs],
            Mode::Only(t) => vec![t],
            Mode::Auto => {
                rows.iter().copied().filter(|&t| !(page && !t.can("js") && t.spec().price > 0.0 && yields(t))).collect()
            }
        };
        // behind a `one_render` gate, plain's or a render's, ONE render: it answers every render alike (bd and oxyjs
        // both billed for one shell); a render the provider refused or erred on rendered nothing, so the next may try
        let gate = done.iter().any(|h| CHECK.one_render.contains(&h.code.as_str()));
        let refused =
            |t: &Tier| done.iter().any(|h| h.tier == *t && crate::provider::REFUSED.contains(&h.code.as_str()));
        let one = all.iter().copied().find(|t| t.can("js") && self.keys.contains(t) && !refused(t)).filter(|_| gate);
        let keep = |t: Tier| t.can(job) && self.avoid != Some(t);
        all.into_iter().filter(|&t| keep(t) && one.is_none_or(|o| o == t || !t.can("js"))).collect()
    }

    /// A tier's timeout, given the finished hops. `timeout:` stretches only the slow tiers: the 10 s plain and oxy
    /// no-JS gates exist to hand a hard page on quickly, so they keep their time, but never past what the agent's
    /// wall left (`agent_left`; plain runs first, so its cap is `timeout:` itself: `timeout:2` on a 12 s page ran
    /// 12 s; `provider:oxy timeout:1` ran its no-JS call 20 s, 21 s in all). In a PDF run, or on a `/pdf/` path (`hint`), Oxylabs gets
    /// `PDF_OXY`. The one-key retry of a timed-out call (`retries`) gets twice OUR time ("bd, then bd
    /// with a longer timeout"), never twice the agent's `timeout:` (a cap it asked for), and never past the
    /// one-key wall (`wall_left`). A named no-JS tier (`provider:oxy`, the cut tier's offer) gates nothing to hand
    /// on quickly: twice its time, as a cut one's retry.
    fn timeout_of(&self, t: Tier, done: &[Hop]) -> Duration {
        let cut = done.iter().find(|h| h.tier == t && h.code == "provider_timeout");
        let k = 1 + u32::from(one_call(self) || cut.is_some() || (self.mode == Mode::Only(t) && !t.can("js")));
        let wall = cut.and_then(|h| wall_left(done, h)).unwrap_or(Duration::MAX);
        let left = self.agent_left(done);
        let ours = |d: Duration| left.unwrap_or((d * k).min(wall));
        match t {
            _ if self.search => ours(crate::provider::SEARCH_TIMEOUT),
            Tier::Bd | Tier::OxyJs => ours(t.spec().timeout),
            Tier::Oxy if self.pdf_run(done) || self.hint => ours(PDF_OXY),
            Tier::Plain | Tier::Oxy => (t.spec().timeout * k).min(wall).min(left.unwrap_or(Duration::MAX)),
        }
    }

    /// Plain and oxy without JS (the no-JS rows, `!t.can("js")`) are gates of a fetch: what follows waits for their
    /// verdict, and only some failures are worth it. In a paid ask (`js:true`, `provider:`) every tier is a gate: ONE provider runs, the next
    /// only on its failure (`hedge:off`; a hedge bills both). Bd leads a render: it won 14/14 of the
    /// races it answered, and fails fast (js_shell ~2.3 s) where oxyjs fails slow (68-71 s timeouts over
    /// 192 runs).
    fn gate(&self, t: Tier) -> bool {
        !self.search && (!t.can("js") || !self.hedged() || self.timeout.is_some())
    }

    /// With `timeout:8` and both keys, bd was cut at 8 s and oxyjs, hedged at 5 s, cut 8 s later: 13.3 s. So
    /// the agent's `timeout:` is ONE wall for the provider calls of a page run, one at a time (`gate`): what the
    /// finished ones left of it (plain, free, capped by it but not counted, and the host check before it); the run stops
    /// with < 1 s left (`next`). None = no `timeout:`.
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
        let lead = self.tiers().into_iter().find(|t| self.keys.contains(t)).map_or(0, |t| t.spec().search_s);
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
    let n = labels.len();
    let shared = SITES.shared.iter().find(|s| host.ends_with(&format!(".{s}")));
    let keep = match labels.as_slice() {
        _ if let Some(s) = shared => s.split('.').count() + 1,
        [.., sld, tld] if n >= 3 && tld.len() == 2 && SITES.sld.contains(sld) => 3,
        _ => 2,
    };
    labels[n.saturating_sub(keep)..].join(".")
}

/// What `js:true` takes: a render's time (oxyjs renders measured 14-32 s, 2026-10); its price = `worst`.
pub const JS_QUOTE: &str = "~25s";

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
    pub limit: Option<crate::provider::Limit>,
    /// When the call landed (`billed`): a thin answer waits the hedge window from here (`next`).
    pub end: Option<Instant>,
    /// The HTML answer's `<html lang>` primary subtag, lower case (`it`); empty = none declared (`langs`).
    pub lang: String,
}

impl Hop {
    /// A call the provider failed: its code, status, reason token, budget and (masked) answer.
    fn failed(tier: Tier, f: crate::provider::Fail) -> Hop {
        Hop { tier, code: f.code.into(), status: f.status, err: f.body, why: f.why, limit: f.limit, ..Hop::default() }
    }

    pub fn row(&self) -> crate::cache::HopRow {
        crate::cache::HopRow {
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
/// either), a host that does not exist (`host_hop`). Not final: a provider's refusal or unlock failure, such as
/// Bright Data's 502 / `x-brd-error-code` (`reject_block`, `resolve_failed_*`: "each attempt uses a different
/// peer, and the same request has been observed to succeed on a later attempt", docs.brightdata.com
/// products/web-unlocker/error-codes) or its policy refusals (another provider may serve that host).
const FINAL: &[&str] = &["not_found", "too_big", "private_address", "no_host"];

fn is_final(h: &Hop) -> bool {
    FINAL.contains(&h.code.as_str())
}

/// A failed gate hop (plain, or oxy without JS) worth the next tier: not a zip document, not a final answer.
fn escalates(h: &Hop) -> bool {
    !h.ok && h.kind != "zip" && !is_final(h)
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
    if over || spent_wall || done.iter().any(|h| h.ok || is_final(h)) {
        return Step::Stop;
    }
    let started = |t: Tier| done.iter().any(|h| h.tier == t) || running.iter().any(|r| r.0 == t);
    let spent = spend(done.iter().map(|h| h.cost).chain(running.iter().map(|r| r.0.spec().price)));
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
            Some((q, None))
                if p.gate(q) && (p.doc == Some("zip") || !done.iter().any(|h| h.tier == q && escalates(h))) =>
            {
                break;
            }
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
/// host or `provider:bd` with both, or the other one refused this run) there is no other provider to cover a transient failure, so its
/// failed call gets ONE retry after OUR cut (`provider_timeout`) or a network error, never after the provider's own
/// refusal (`provider_error`, a non-200 API answer, `provider::read`: a final refusal; retries served 0/7 vs 3/9
/// after our cut), never on the site's own status (`Hop::status`: a 5xx page, a wall served as
/// 503), a wall or a shell (the same answer again), nor in a prefetch (its page fetch covers it), nor a detached
/// search (`one_call`: it got the retry's time at once, `timeout_of`), nor under the agent's `timeout:` (a hard budget;
/// a search cut there is not asked again either); inside `cost:` (`next`'s `can`); never with less
/// time than the call it repeats had (bd-only retries after a 60 s cut got 23.9-30 s under the wall, served 0/5):
/// the run ends at the first timeout, its row buys more time (`fix_arg`).
fn retries(p: &Plan, done: &[Hop], h: &Hop) -> bool {
    let keyed = p.tiers().into_iter().filter(|t| p.keys.contains(t) && t.spec().price > 0.0 && !refused(done, *t));
    let paid = crate::lang::uniq(keyed, |t| t.owner());
    let transient = matches!(h.code.as_str(), "provider_timeout" | "net_error") && !one_call(p) && p.timeout.is_none();
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
    h.ok && h.doc.as_ref().is_some_and(crate::outline::is_thin)
}

/// An app page (`no_copy`): a JS shell, and shells or timeouts from two different paid providers (one
/// provider's shell is that provider's failure, airbnb: bd shells, Oxylabs renders it; OUR short cut is no
/// provider's answer). The ladder runs on
/// until then, so this only names the outcome.
fn app_page(done: &[Hop]) -> bool {
    let ran = |h: &&Hop| h.tier.spec().price > 0.0 && !ours(&h.code, h.upto, h.timeout_s);
    let paid = |h: &&Hop| ran(h) && matches!(h.code.as_str(), "js_shell" | "provider_timeout");
    done.iter().any(|h| h.code == "js_shell")
        && crate::lang::uniq(done.iter().filter(paid), |h| h.tier.owner()).len() > 1
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
/// `done` = hops settled before the first call (`host_hop`); `seen` gets the tiers so far each time one starts or lands.
pub fn drive(p: &Plan, call: &dyn Fn(Tier, &[Hop]) -> Call, mut done: Vec<Hop>, seen: Seen) -> Vec<Hop> {
    crate::provider::block(async {
        let mut set = tokio::task::JoinSet::new();
        let mut running = Vec::<Running>::new();
        let (end, mut cut) = (p.wait.map(|w| Instant::now() + w), None);
        loop {
            let now: Vec<(Tier, Duration)> = running.iter().map(|r| (r.0, r.1.elapsed())).collect();
            let step = next(p, &done, &now);
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
                    if let Ok(Some(r)) = tokio::time::timeout(d, set.join_next_with_id()).await
                        && let Some(h) = landed(r, &mut running)
                    {
                        done.push(cut_why(p, h));
                    }
                }
                Step::Stop => break,
            }
            let tried = done
                .iter()
                .map(|h| format!("{}{}", h.tier.spec().short, if h.ok { "✓".into() } else { format!(":{}✗", h.code) }));
            let busy = running.iter().map(|r| format!("{} running", r.0.spec().short));
            // the newest running call's start, and the last running call's deadline (its timeout as it started)
            let at = |i: Instant| crate::cache::now() - i.elapsed().as_secs_f64();
            let since = running.iter().map(|r| at(r.1)).fold(0.0, f64::max);
            let until = running.iter().map(|r| at(r.1) + r.3.as_secs_f64()).fold(0.0, f64::max);
            seen((tried.chain(busy).collect::<Vec<_>>().join(" · "), since, until));
        }
        set.abort_all();
        let gone = |t: Tier| match cut {
            Some(wait) => Hop::failed(t, crate::provider::late(wait)),
            None => Hop { tier: t, code: "abandoned".into(), ..Hop::default() },
        };
        for (t, t0, _, d) in running {
            done.push(cut_why(p, Hop { timeout_s: d.as_secs(), ..billed(t0, false, t.spec().price, gone(t)) }));
        }
        done
    })
    .unwrap_or_else(|| vec![Hop { code: "net_error".into(), ..Hop::default() }])
}

/// Why WE cut a call (`Hop::why`, named by `fail_detail`): `timeout:8s` = the agent's `timeout:` (its fix is
/// the default time), else `took > 60 s here` (`fail_detail` reads it as `cut at 60 s (3m ago)`).
fn cut_why(p: &Plan, mut h: Hop) -> Hop {
    let s = h.timeout_s;
    if h.code == "provider_timeout" && h.status == 0 && h.why.is_empty() {
        // under `timeout:` a render gets what the agent's wall left (`agent_left`): its cut is named as the agent typed it
        let user = p.timeout.map(|w| w.as_secs()).filter(|&w| (h.tier.can("js") && s <= w) || s == w);
        h.why = user.map_or_else(|| format!("took > {s} s here"), |w| format!("timeout:{w}s"));
    }
    h
}

/// How long the system resolver gets before the run goes on without its verdict (`dns_timeout`).
pub const DNS_WAIT: Duration = Duration::from_millis(1500);

/// THE host check, before any tier runs (`js:true` skips plain too): the system resolver, and on no
/// address the second resolver (`provider::exists`, asked on that failure only). A host both call missing is a
/// plain hop `no_host`: final, $0, no provider call. A resolver that gives no verdict in time is a plain hop
/// `dns_timeout` (not a missing host): plain is skipped (it would wait on the same lookup), the providers run.
/// None = the host resolves, or the URL names an IP address.
pub fn host_hop(url: &str) -> Option<Hop> {
    let Some(url::Host::Domain(host)) = url::Url::parse(url).ok()?.host().map(|h| h.to_owned()) else {
        return None;
    };
    let (t0, (tx, rx)) = (Instant::now(), std::sync::mpsc::channel());
    // a test's stand-in transport is the whole network: its resolver (the second one) is the only one asked
    let (name, fake) = (host.clone(), crate::provider::in_process());
    std::thread::spawn(move || {
        tx.send(!fake && std::net::ToSocketAddrs::to_socket_addrs(&(name.as_str(), 443)).is_ok())
    });
    let code = match rx.recv_timeout(DNS_WAIT) {
        Ok(true) => return None,
        Ok(false) => match crate::provider::block(crate::provider::exists(&host)).flatten() {
            Some(true) => return None,
            Some(false) => "no_host",
            None => "dns_timeout",
        },
        Err(_) => "dns_timeout",
    };
    let ms = t0.elapsed().as_millis() as u64;
    Some(Hop { tier: Tier::Plain, code: code.into(), ms, ..Hop::default() })
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
type Called<T> = (Result<T, crate::provider::Fail>, bool);

/// The network transport: one provider call, then the content check. A `/pdf/` path (`hint`) asks Bright Data for
/// raw bytes like a PDF run, but only a PDF run fails a page as `not_pdf`.
pub fn run(p: &Plan, ask: &Ask, seen: Seen) -> Vec<Hop> {
    let call = |t: Tier, done: &[Hop]| -> Call {
        let (mut ask, pdf) = (ask.clone(), p.pdf_run(done));
        (ask.js, ask.doc) = (t == Tier::OxyJs, pdf || p.hint);
        ask.timeout = Some(p.timeout_of(t, done));
        let (key, price, prior, card) = (t.key().unwrap_or_default(), t.spec().price, p.prior.clone(), !p.js && !pdf);
        let langs = langs(&ask, done);
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
            geo(if card && !ask.js { og_card(first, &ask.url) } else { first }, &langs)
        })
    };
    let hops = drive(p, &call, host_hop(&ask.url).into_iter().collect(), seen).into_iter();
    hops.map(|h| Hop { country: crate::provider::sent(ask, h.tier).unwrap_or_default(), ..h }).collect()
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
    let own = crate::lang::uniq(words.chain(plain), String::clone);
    let query = u
        .query_pairs()
        .any(|(_, v)| shaped(&&*v) && v.split(['-', '_']).any(|w| !locale(&w.to_lowercase()).is_empty()));
    let bare = (named.is_empty() && own.is_empty() && !query).then(|| ask.clone());
    (if named.is_empty() { own } else { named }, bare)
}

/// The languages a locale word names through `Providers::cctld`: a ccTLD (`uk`) or a country (`gb`) → its row's
/// (`ch` → de fr it), a row's language (`en`) → itself; any other word names none.
fn locale(word: &str) -> Vec<String> {
    let rows = &crate::provider::PROVIDERS.cctld;
    let langs = |r: &&str| r.split(' ').skip(1).map(String::from).collect::<Vec<_>>();
    match rows.get(word).or_else(|| rows.values().find(|r| r.split(' ').next() == Some(word))) {
        Some(r) => langs(r),
        None => rows.values().flat_map(langs).filter(|l| l == word).take(1).collect(),
    }
}

/// A provider's copy outside `langs` (none = no expectation; `en` when it names no locale and the call sent no
/// country) says so in its `why`, after a status it carries: `lang:it`, `403+lang:it`. A copy whose own canonical
/// link names the asked URL (`landed`) is the site's page in the site's language, so no `en` is expected of it: a
/// geo copy names its language's URL (`x.es.html`) or another site. The dropped expectation leaves a trace, never
/// a flag: `own:es` in its `why` (`bd✓own:es` in the tried path: `stats`, meta.json's hops).
fn geo(mut h: Hop, (langs, bare): &(Vec<String>, Option<Ask>)) -> Hop {
    let own = |a: &Ask| url::Url::parse(&a.url).is_ok_and(|u| h.landed.as_deref() == Some(u.as_str()));
    let unsent = bare.as_ref().filter(|a| crate::provider::sent(a, h.tier).is_none());
    let langs = if unsent.is_some() { &vec!["en".into()] } else { langs };
    if h.ok && h.tier != Tier::Plain && !h.lang.is_empty() && !langs.is_empty() && !langs.contains(&h.lang) {
        let tag = if unsent.is_some_and(own) { "own" } else { "lang" };
        h.why = format!("{}{}{tag}:{}", h.why, if h.why.is_empty() { "" } else { "+" }, h.lang);
    }
    h
}

/// THE Markdown guard: a server's own `text/markdown` answer (`clean::kind_of` `md`) is not served when it is thin
/// (`THIN_CHARS` of prose) or under a fifth of the prose of the copy stored before, or flat (no heading) where
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
    let thin = d.loss.pruned_chars < THIN_CHARS || d.tokens.clean * 5 < tok;
    thin.then_some("md:thin").or((flat && n >= 3).then_some("md:flat"))
}

/// og_only, THE card rule (any tier but the render, oxyjs, and never in a `js:true` or PDF run): a JS shell, or a
/// 4xx that is not a missing page, whose head carries a page's own og card is served now as that card: `mode` og,
/// ok, the ladder stops (`outline` adds `! js_shell → … js:true` while a render is left). A page's own card = an
/// og:description of ≥ 40 chars, an og:title or og:image beside it, and an og:title that is not the og:site_name:
/// a wall or an app shell carrying the site-wide card (a maps shell, a login wall) is not the page. Stored as kind
/// `og` (`clean::card`), so a rebuild of the body makes the card again.
fn og_card(mut h: Hop, url: &str) -> Hop {
    let shell = !h.ok && (h.code == "js_shell" || (400..500).contains(&h.status) && !is_final(&h));
    let own = |t: &Tree| {
        let (title, site) = (t.meta("og:title").map(clean::fold), t.meta("og:site_name").map(clean::fold));
        let desc = t.meta("og:description").map_or(0, |d| d.trim().chars().count());
        desc >= 40 && (title.is_some() || t.meta("og:image").is_some()) && (title.is_none() || title != site)
    };
    let Some(tree) = h.tree.as_ref().filter(|t| shell && own(t)) else { return h };
    let fin = if h.final_url.is_empty() { url } else { &h.final_url };
    if let Ok(card) = crate::cache::guarded(|| Ok(clean::card(clean::build_doc(tree, url, fin, false)))) {
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
    let cc = crate::provider::search_country(country);
    drive(p, &call, Vec::new(), &drop).into_iter().map(|h| Hop { country: cc.clone(), ..h }).collect()
}

/// What `dry:true` prints: the tiers a fully failing run would try, in order, each with the delay after which it
/// starts beside the call before it (`next` hedges it) or None (it waits for that call's verdict), and the
/// worst-case cost.
pub fn route(p: &Plan) -> (Vec<(Tier, Option<Duration>)>, f64) {
    // a worst case: every call runs out its time (a clock, so the one-key wall decides a retry); under the
    // agent's one wall every render fails at once, so each one left gets to start and bill; plain meets the
    // wall that keeps every rung after it (a captcha keeps oxy no-JS with both keys)
    let fail = |t: Tier, ms: Duration, end| {
        let ms = if p.timeout.is_some() { 0 } else { ms.as_millis() as u64 };
        let code = if t == Tier::Plain { CHECK.no_js_first[0] } else { "provider_timeout" }.into();
        Hop { tier: t, code, kind: "html", cost: t.spec().price, ms, end, ..Hop::default() }
    };
    let (mut done, mut path, mut clock) = (Vec::new(), Vec::new(), Instant::now());
    while let Step::Start(t) = next(p, &done, &[]) {
        // a tier hedged at or past a search's `wait:` never starts beside: it runs only after a fast failure
        let after = p.hedge;
        let beside = match next(p, &done, &[(t, after)]) {
            Step::Start(u) if p.wait.is_none_or(|w| after < w) => Some((u, Some(after))),
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
    let at = crate::lang::At { id, url: &ask.url, arg: "device:mobile", ..Default::default() };
    let what = "device bd · Bright Data has no device setting: desktop page";
    (ask.mobile && bd).then(|| crate::lang::bang(id, "ignored", what, &at)).into_iter().collect()
}

/// `!` rows for tiers a failed run never tried: one per missing key, one for the cap with what the rest would
/// cost. `subject` heads the rows (a snapshot's id, empty for a search); `id` = what the hint reruns, on `url` ("" =
/// a search; the cap's arrow priced for the URL's own plan, as its `dry:true`).
pub fn skipped(p: &Plan, hops: &[Hop], subject: &str, id: &str, url: &str) -> Vec<String> {
    use crate::lang::{At, bang};
    let zip = p.doc == Some("zip") && p.tiers()[0] == Tier::Plain;
    if hops.iter().any(|h| h.ok) || zip || hops.iter().any(|h| p.gate(h.tier) && !escalates(h)) {
        return Vec::new();
    }
    let left: Vec<Tier> = p.ladder(hops).into_iter().filter(|t| !hops.iter().any(|h| h.tier == *t)).collect();
    let mut rows = no_key(p, &left, subject, false);
    let over: Vec<Tier> = left.into_iter().filter(|t| p.keys.contains(t) && !refused(hops, *t)).collect();
    // the same rule as `next`: a tier the cap stopped (one cut at a search's `wait:` was not)
    let spent = spend(hops.iter().map(|h| h.cost));
    if over.iter().any(|t| spent + t.spec().price > p.cap + 1e-9) {
        let want = spent + spend(over.iter().map(|t| t.spec().price));
        let names: Vec<&str> = over.iter().map(|t| t.spec().short).collect();
        let (cap, want) = (p.cap, (want * 1e4).ceil() / 1e4);
        let what = format!("{} not tried · ${want:.4} > cost:{cap}", names.join(" "));
        rows.push(bang(subject, "budget_hit", &what, &At { id, url, arg: &want.to_string(), ..At::default() }));
    }
    rows
}

/// The tiers with a key configured (`Tier::key`). Unit tests run with every key: the key dir of the machine that
/// runs them never changes an answer (the binary's tests set their own).
pub fn keyed() -> Vec<Tier> {
    Tier::ALL.into_iter().filter(|t| cfg!(test) || t.key().is_some()).collect()
}

/// THE `no_key` row (fetch, search, dry, setup, account), one per provider of `left` without a
/// key: what its key would add to this run and the price, `! no_key bd · would try unblock+JS ≤$0.0015 · set
/// BRIGHTDATA_API_KEY + … → fats setup · bd <sign-up>`; `setup` = said inside `fats setup`: no `→ fats setup`.
pub fn no_key(p: &Plan, left: &[Tier], subject: &str, setup: bool) -> Vec<String> {
    let row = |o: &Tier| {
        let (s, mine) = (o.owner().spec(), left.iter().filter(|t| t.owner() == o.owner()));
        let buys =
            if p.search { vec!["web search"] } else { crate::lang::uniq(mine.clone().map(|t| t.spec().buys), |b| *b) };
        let need = if o.owner() == Tier::Bd { format!("{} + BRIGHTDATA_UNLOCKER_ZONE", s.env) } else { s.env.into() };
        let cost = spend(mine.map(|t| t.spec().price));
        let what = format!("{} · would try {} ≤${cost:.4} · set {need}", s.short, buys.join(" + "));
        let next = if setup { String::new() } else { crate::lang::fix("no_key", &crate::lang::At::default()) };
        let row = crate::lang::Flag { code: "no_key".into(), what, next, row: true }.bang(subject);
        row + &crate::provider::pages("no_key", [s.short])
    };
    // the tiers without a key, one per key (both Oxylabs tiers share one)
    crate::lang::uniq(left.iter().filter(|t| !p.keys.contains(t)), |t| t.owner()).into_iter().map(row).collect()
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
/// tier is left, a tier whose only signal was OUR cut (`ours`: it never said no) is not spent: it is the move.
/// `cuts` = the hops of the URL's other copies this session (cli `seen`): a tier another copy of the URL tried is no
/// fix either (a `provider:oxy` offer never shows again after the `provider:oxy` copy).
pub fn spent(m: &Meta, cuts: &[HopRow], keys: &[Tier]) -> Vec<&'static str> {
    let all_hops: Vec<&HopRow> = m.hops.iter().chain(cuts).collect();
    let js_wall = all_hops.iter().any(|h| CHECK.render_first.contains(&h.code.as_str()));
    let site_no =
        |t: &Tier| all_hops.iter().any(|h| h.code == "error_page" && Tier::parse(&h.provider) == Some(t.owner()));
    let barred = |t: &Tier| !keys.contains(t) || gov(&m.url) && !t.can("gov") || js_wall && !t.can("js") || site_no(t);
    // Nor, then, one whose only word was its own error (`provider_error`) unless another copy's too
    let error = |h: &HopRow| h.code == "provider_error" && !cuts.iter().any(|c| c.provider == h.provider);
    let done = |all: bool| {
        let hops = all_hops.iter().filter(|h| all || !ours(&h.code, h.upto, h.timeout_s) && !error(h));
        let tiers = hops.filter_map(|h| Tier::parse(&h.provider)).chain(Tier::ALL.into_iter().filter(barred));
        tiers.map(|t| t.spec().short).collect::<Vec<_>>()
    };
    let all = done(true);
    let rest = Tier::ALL.iter().any(|t| t.spec().price > 0.0 && !all.contains(&t.spec().short));
    if rest { all } else { done(false) }
}

/// A call WE cut (`upto`: no answer; bd cut at our 60 s served the page in 36.7 s 3 min later) under
/// `render_fix_s` is no "tried": no `app_page` vote, not `spent` when it is the last tier left.
fn ours(code: &str, upto: f64, timeout_s: u64) -> bool {
    code == "provider_timeout" && upto > 0.0 && timeout_s < limit("render_fix_s")
}

/// The cut a failed run's move and reason follow when the run ended in OUR cut, whatever code
/// heads the row (`outcome` puts a wall any tier met there): the tiers decide, not the head. Of two cuts, bd's
/// (`js:true`'s order, one provider at a time), else the last.
pub fn cut_of(m: &Meta) -> Option<&HopRow> {
    let last = m.hops.iter().rfind(|h| h.code != "abandoned")?;
    let ended =
        last.code == "provider_timeout" && (m.code == last.code || m.code.ends_with("_wall") || last.upto > 0.0);
    let all = m.hops.iter().enumerate().filter(|(_, h)| ended && h.code == "provider_timeout");
    all.max_by_key(|(i, h)| (h.provider == Tier::Bd.spec().name, *i)).map(|(_, h)| h)
}

/// The code a failed run's move follows (`lang::fix`): a timeout's when it ended in our cut (`cut_of`), else its own.
pub fn move_code(m: &Meta) -> &str {
    if cut_of(m).is_some() { "provider_timeout" } else { &m.code }
}

/// `{arg}` of a failed fetch's fix (`lang::fix`), THE timeout move, judged on the session's hops of the URL (`cuts`
/// = its other copies'): after the agent's own `timeout:` cut shorter than the tier's time, none (`fresh:true`
/// = the default time, never `nothing left to try`); with no other keyed tier left untried, the cut render ONCE a
/// session (until a call of `render_fix_s` ran, cut or answered) at its longer time and price (`OFFER`;
/// bd took over 60 s on TikTok 9 of 9 times), then None = a COUNT, no arrow (`timed_out`), as after a no-JS
/// cut or an error page with nothing left; else a render (`js:true`; shells + timeouts = `no_copy`).
pub fn fix_arg(m: &Meta, cuts: &[HopRow]) -> Option<&'static str> {
    let (cut, keys) = (cut_of(m), keyed());
    let done = spent(m, cuts, &keys);
    // in a timeout's move a tier WE cut is tried too, else two cut renders defer to each other (`js:true`)
    let tried = |t: &Tier| cut.is_some() && m.hops.iter().chain(cuts).any(|h| h.provider == t.spec().name);
    let left = |js: bool, not: Tier| {
        let untried = |t: &&Tier| t.spec().price > 0.0 && !done.contains(&t.spec().short) && **t != not && !tried(t);
        Tier::ALL.iter().filter(untried).any(|t| t.can("js") || !js)
    };
    let short = |h: &HopRow, t: Option<Tier>| t.is_none_or(|t| h.timeout_s < t.spec().timeout.as_secs());
    let long = |t: Tier, h: &HopRow| h.timeout_s >= limit("render_fix_s") || !t.can("js");
    let ran = |t: Tier| m.hops.iter().chain(cuts).any(|h| h.provider == t.spec().name && long(t, h));
    // two keys name the provider, unless the tier answered itself (a 408: `lang::fix` bars a named spent tier)
    let two = crate::lang::uniq(keys.iter().filter(|t| t.spec().price > 0.0), |t| t.owner()).len() > 1;
    let named = |t: Tier| usize::from(two && !done.contains(&t.spec().short));
    // The tier whose own failure ended the run, still unspent (`spent`: its error, the one tier left), by name
    let err = |h: &HopRow| Tier::parse(&h.provider).filter(|t| h.code == m.code && !done.contains(&t.spec().short));
    match cut.map(|h| (h, Tier::parse(&h.provider))) {
        // The site refused every keyed provider with its error page: a count; so is a site's 4xx
        None if (m.code == "error_page" || m.code.starts_with("http_4")) && !left(false, Tier::Plain) => None,
        None => Some(m.hops.iter().find_map(err).map_or("", |t| &OFFER[t as usize][2])),
        Some((h, t)) if h.why.starts_with("timeout:") && short(h, t) => Some(""),
        Some((_, Some(t))) if !left(t.can("js"), t) && !ran(t) => Some(&OFFER[t as usize][named(t)]),
        Some((_, Some(t))) if !left(false, t) => None,
        _ => Some("js:true"),
    }
}

/// The re-render rule (`rendered`) for `url` applied to `p`, from the URL's newest stored copy.
pub fn route_of(c: &Cache, url: &str, p: &mut Plan) {
    let prior = c.lookup(url).and_then(|(dir, _)| c.meta(&dir));
    p.avoid = prior.filter(|_| p.js).and_then(|m| rendered(&m, &p.keys));
}

/// The most an arrow (`fats u1 fresh:true …`, the words after the binary) can spend on `url`
/// ("" = its id's page, or a search), 0 when it spends nothing; THE `≤$X` of every paid arrow (`lang::priced`):
/// `dry:true`'s `up to` for that command, the same plan (`Plan::of`, `route_of`) through the same `route`.
pub fn worst(arrow: &str, url: &str) -> f64 {
    // The words after the binary that heads the arrow, never after its name inside them
    let cmd = crate::lang::parse(&[arrow.strip_prefix(&format!("{} ", crate::lang::bin())).unwrap_or("").to_string()]);
    // a command that can spend: a fetch or search past the cache (`account`, `setup`: free reads)
    // GitHub's search is free (a re-read once quoted ≤$0.0025, a web search's)
    let spends = ["fresh", "js", "provider", "timeout"].iter().any(|k| cmd.get(k).is_some());
    if !spends || !cmd.reserved.is_empty() || crate::cli::is_gh(&cmd) {
        return 0.0;
    }
    let page = cmd.urls.first().map(String::as_str).or((!url.is_empty() || !cmd.ids.is_empty()).then_some(url));
    let mut p = Plan::of(&cmd, page, 0.0);
    let s = crate::settings::get();
    route_of(&Cache::at(s.cache.value.clone(), s.session.value.clone()), page.unwrap_or_default(), &mut p);
    route(&p).1
}

/// `60 s + 150 s here` (`fail_detail` names the tier): every time this session cut the URL's timed-out tier at (this
/// copy's hops and `cuts`), once one reached `render_fix_s` (the longer time ran, `fix_arg` never offers it
/// again); `cut at 60 s · 150 s answered` once a longer call served (Zara: in 16 s), else `150 s failed
/// (502)` (its code); else "".
pub fn timed_out(m: &Meta, cuts: &[HopRow]) -> String {
    let (Some(last), fix) = (cut_of(m), limit::<u64>("render_fix_s")) else { return String::new() };
    // A call of that provider at the longer time ran, cut OR answered (`true`, said apart)
    let same = |h: &&HopRow| h.provider == last.provider && (h.code == last.code || h.timeout_s >= fix);
    // The longer call `answered` only when it served (a serve sorts last), else it `failed (502)`
    let key = |h: &HopRow| (h.code != last.code, h.timeout_s, h.code == "ok", h.code.replace("http_", ""));
    let s: std::collections::BTreeSet<_> = m.hops.iter().chain(cuts).filter(same).map(key).collect();
    let times = |r| s.iter().filter(|t| t.0 == r).map(|t| format!("{} s", t.1)).collect::<Vec<_>>().join(" + ");
    match s.last() {
        Some((true, _, true, _)) => format!("cut at {} · {} answered", times(false), times(true)),
        Some((true, .., c)) => format!("cut at {} · {} failed ({c})", times(false), times(true)),
        Some((false, t, ..)) if *t >= fix => format!("{} here", times(false)),
        _ => String::new(),
    }
}

/// A failed record's count once another record of the same URL or query in this session (`others`)
/// failed the same way, the same code along the same tiers, codes and countries (whatever time each call was
/// given, so the 150 s call failing as the 60 s one did counts, `http_502`): ` · failed 2/2 here` (the same
/// failures / the records). Its arrow would send the call that failed again (a bd outage = an endless $0 loop;
/// consent.google's 400): said only when its fix (`arg`) changes nothing (empty), the caller drops the
/// arrow. The end state (no fix, None) counts every copy failed the same way, whatever path; a code is the
/// move's (`move_code`: a 0.1.6 `no_copy` both-cut copy is the timeout it was), said from the first (`failed 1/1
/// here` on a one-key error page, the row with no arrow).
pub fn fail_count<'a>(m: &Meta, others: impl Iterator<Item = &'a Meta>, arg: Option<&str>) -> Option<String> {
    let same: Vec<&Meta> = others.filter(|s| s.url == m.url).collect();
    let tiers =
        |s: &Meta| s.hops.iter().map(|h| [&h.provider, &h.code, &h.country].map(String::clone)).collect::<Vec<_>>();
    let way = |s: &Meta| move_code(s) == move_code(m) && (arg.is_none() || tiers(s) == tiers(m));
    let k = 1 + same.iter().filter(|s| !s.ok && way(s)).count();
    arg.is_none_or(|a| k > 1 && a.is_empty()).then(|| format!(" · failed {k}/{} here", same.len() + 1))
}

/// D1: THE failed copy's row parts, for `cli::no_doc` and `outline::page_flags`: why it failed (`fail_detail`, `page`
/// when no reason), its count (`fail_count`), its fix (`lang::fix`, None = a count or no fix), the accounts named.
/// No count when a newer copy `served` (its row points there, never "failed" beside the page).
pub fn fail_row(m: &Meta, others: &[Meta], page: &str, id: &str, served: bool) -> FailRow {
    let cuts: Vec<HopRow> = others.iter().flat_map(|o| o.hops.clone()).collect();
    let (done, arg) = (spent(m, &cuts, &keyed()), fix_arg(m, &cuts));
    let again = fail_count(m, others.iter(), arg).filter(|_| !served);
    let at = crate::lang::At { id, url: &m.url, arg: arg.unwrap_or_default(), done: &done };
    let fix = arg.filter(|_| again.is_none()).map(|_| crate::lang::fix(move_code(m), &at));
    let (why, owners) = fail_detail(m, page, &cuts);
    // The longer call offered says so before its arrow (`bd cut at 60 s (3m ago) · one more try → …`)
    let more = (!served && arg.is_some_and(|a| a.contains("timeout:"))).then(|| " · one more try".into());
    (why, again.or(more).unwrap_or_default(), fix, owners)
}

/// `fail_row`: (why, ` · failed k/n here` or the fix's ` · one more try`, the fix, the accounts named).
pub type FailRow = (String, String, Option<String>, Vec<&'static str>);

/// `fix_arg`'s one more call of a cut render at `render_fix_s`, one key / two keys (two name the provider):
/// `timeout:150` / `provider:bd timeout:150` (`lang::fix` quotes it, `worst`). A no-JS tier is offered by
/// `lang::fix` at its own time (`timeout:` stretches renders only); third, the tier named at its own time.
static OFFER: LazyLock<Vec<[String; 3]>> = LazyLock::new(|| {
    let s: u64 = limit("render_fix_s");
    Tier::ALL.map(|t| [format!("timeout:{s}"), format!("{} timeout:{s}", t.ask()), t.ask()]).to_vec()
});

/// What a failed run's `!` row names: a refused key is named as `fats config` lists it (`oxy`, `bd`); a failure
/// the provider gave a reason token for (`Hop::why`) names the account and the token (`bd proxy_error`,
/// `oxy retry in 5s`, `bd cut at 60 s (3m ago)`, after the longer time ran every time tried: `timed_out`); never
/// the provider's answer itself. Any other failure names `page`. Also the accounts the row names, for their
/// dashboard pages (`provider::pages`). THE reason of every failed copy's row, with or without a doc.
pub fn fail_detail(m: &Meta, page: &str, cuts: &[HopRow]) -> (String, Vec<&'static str>) {
    // the account a hop bills (`oxy` for oxyjs), a tier's or one of its own (`gh`: GitHub)
    let owner = |h: &HopRow| {
        Tier::parse(&h.provider)
            .map(|t| t.owner().spec().short)
            .or(crate::provider::links(&h.provider).map(|l| l.short))
    };
    if m.code != "bad_key" {
        let code = move_code(m);
        let last = cut_of(m).or(m.hops.iter().rev().find(|h| h.code == code));
        // Our cut, stored as `took > 60 s here` (0.1.6 rows too), reads its time and age: `cut at 60 s (3m ago)`;
        // No age under a minute (the answer that made the cut said `(0s ago)`)
        let age = crate::cache::now() - m.t;
        let age = if age < 60.0 { String::new() } else { format!(" ({} ago)", crate::outline::fmt_age(age)) };
        let ours = |h: &HopRow| h.why.strip_prefix("took > ").map(|_| format!("cut at {} s{age}", h.timeout_s));
        let why = Some(timed_out(m, cuts)).filter(|t| !t.is_empty()).or(last.map(|h| ours(h).unwrap_or(h.why.clone())));
        // a call we cut names its tier (`oxyjs cut at 60 s`), a provider's reason its account (`oxy retry in 5s`)
        let cut = last.and_then(|h| Tier::parse(&h.provider)).filter(|_| code == "provider_timeout");
        return match (last.and_then(owner), why.filter(|w| !w.is_empty())) {
            (Some(o), Some(w)) => (format!("{} {w}", cut.map_or(o, |t| t.spec().short)), vec![o]),
            _ => (page.into(), Vec::new()),
        };
    }
    // the accounts whose hop ended with the run's code, in call order
    let keys = crate::lang::uniq(m.hops.iter().filter(|h| h.code == m.code).filter_map(owner), |k| *k);
    (keys.join(" "), keys)
}

/// Every provider error answer of a run, one block each (`# oxylabs 500 provider_error`), for `error.txt`.
pub fn error_text(hops: &[Hop]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for h in hops.iter().filter(|h| !h.err.is_empty()) {
        out.extend(format!("# {} {} {}\n", h.tier.spec().name, h.status, h.code).into_bytes());
        out.extend(&h.err);
        out.push(b'\n');
    }
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
    let best = win.or_else(|| hops.iter().enumerate().filter_map(|(i, h)| Some((score(h)?, i))).max().map(|x| x.1));
    // a failed run is named by its last answer; a cut PDF shown as the best copy by its own code (`pdf_truncated`)
    let code = match (win, best.filter(|&i| hops[i].kind == "pdf")) {
        (Some(_), _) => "ok".to_string(),
        (None, Some(i)) => hops[i].code.clone(),
        // An app page (`app_page`): no copy from two providers, not a fault a retry fixes
        _ if app_page(hops) => "no_copy".into(),
        // A wall any tier met says more than the last tier's timeout (oxy-only Joomla: `cloudflare_wall`, not
        // `provider_timeout`)
        _ => {
            match (hops.iter().rev().find(|h| h.code != "abandoned"), hops.iter().rfind(|h| h.code.ends_with("_wall")))
            {
                // a `one_render` gate names the run only when its render met it too (a shell, the gate), never a
                // render cut or refused: that one keeps its own fix (the 150 s offer, the other provider)
                (Some(l), Some(w)) if CHECK.one_render.contains(&w.code.as_str()) => {
                    if l.code == "js_shell" { w } else { l }.code.clone()
                }
                (Some(l), Some(w)) if l.code == "provider_timeout" => w.code.clone(),
                (last, _) => last.map_or("no_key".into(), |h| h.code.clone()),
            }
        }
    };
    let error = if win.is_some() { None } else { error_text(hops) };
    let cost = spend(hops.iter().map(|h| h.cost));
    let landed = best.and_then(|i| hops[i].landed.clone());
    let (tier, kind, final_url, body, doc, tree, charset) = match best {
        Some(i) => {
            let h = &mut hops[i];
            (h.tier, h.kind, h.final_url.clone(), std::mem::take(&mut h.body), h.doc.take(), h.tree.take(), h.charset)
        }
        None => (hops.last().map_or(Tier::Plain, |h| h.tier), "html", String::new(), Vec::new(), None, None, ""),
    };
    let render = tier == Tier::OxyJs;
    let fin = if final_url.is_empty() { url } else { &final_url };
    // `judge` parsed every body once (a PDF into its doc); a failed HTML answer is cleaned here from that parse
    let doc =
        doc.or_else(|| tree.and_then(|t| crate::cache::guarded(|| Ok(clean::build_doc(&t, url, fin, render))).ok()));
    let meta = Meta {
        url: url.into(),
        final_url: (!final_url.is_empty() && final_url != url).then_some(final_url),
        landed,
        provider: tier.spec().name.into(),
        ms,
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
    let file = (!body.is_empty()).then(|| (crate::cache::raw_name(kind), body));
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

/// Where a wall rule looks: response header lines, the raw HTML (unescaped, folded), the visible text, or the HTTP status (an error status, ≥ 400, is a wall of its own).
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
fn page_lang(tree: &crate::html::Tree) -> String {
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
    let (final_url, charset) = (raw.final_url.clone(), raw.charset);
    let mut h = Hop { tier, status: raw.status, final_url, kind, charset, ..Hop::default() };
    let fin = if raw.final_url.is_empty() { url } else { &raw.final_url };
    if pdf && magic != Some("pdf") && (tier != Tier::Plain || clean::LISTS.text.contains(&kind)) {
        // the site's own error status (Bright Data's `x-brd-status-code`) says more than "not the file"
        h.code = if raw.status >= 400 { status_code(raw.status) } else { "not_pdf".into() };
    } else if kind != "html" && raw.status >= 400 {
        h.code = status_code(raw.status);
    } else if kind == "pdf" && magic.is_none() {
        h.code = "not_pdf".into(); // a content type that named a PDF over a body that is not one
    } else if kind == "pdf" {
        match crate::cache::guarded(|| crate::pdf::build_pdf(&raw.body, url, fin, limit("pdf_budget_s"))) {
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
        if ok || (raw.status < 400 && !code.ends_with("_wall")) {
            h.lang = page_lang(&tree);
        }
        (h.ok, h.code, h.vis) = (ok, code, vis);
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
            match crate::cache::guarded(|| Ok(clean::build_doc(&tree, url, &h.final_url, js))) {
                // a page that cleans to almost nothing is a shell unless its title is visibly there (example.com)
                Ok(d) if d.loss.pruned_chars < THIN_CHARS && !shown => {
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
    let rel = |n: &&crate::html::Node| n.attr("rel").is_some_and(|r| r.eq_ignore_ascii_case("canonical"));
    let href = t.nodes.iter().filter(|n| n.tag == "link").find(rel)?.attr("href")?;
    let c = url::Url::parse(url).ok()?.join(href).ok()?.to_string();
    (site(&c) == site(url)).then_some(c)
}

/// Text a no-JS browser shows (everything but head, script, style, template and svg), and the same without the
/// text nodes that ask for JavaScript.
fn visible(t: &Tree) -> (String, String) {
    let (mut out, mut body) = (String::new(), String::new());
    for s in t.texts(0, &["head", "script", "style", "template", "svg"]) {
        let f = clean::fold(s);
        if !CHECK.js_needed.iter().any(|m| f.contains(m)) {
            body.push(' ');
            body.push_str(&f);
        }
        out.push(' ');
        out.push_str(&f);
    }
    (clean::collapse(&out), clean::collapse(&body))
}

/// A search page: a URL whose query names the search (`q=`, a key ending in `query`), judged by its results.
fn is_serp(url: &str) -> bool {
    let u = url::Url::parse(url).ok();
    u.is_some_and(|u| u.query_pairs().any(|(k, _)| k == "q" || k.ends_with("query")))
}

/// Results on a search page, by no engine's markup: distinct link targets whose text holds ≥ `link_words` words.
fn serp_results(t: &Tree) -> usize {
    let links = t.nodes.iter().enumerate().filter_map(|(i, n)| {
        let href = n.attr("href").filter(|h| n.tag == "a" && !h.starts_with('#'))?;
        let words = t.texts(i, &[]).flat_map(str::split_whitespace).count();
        (words >= limit("link_words")).then_some(href)
    });
    links.collect::<std::collections::HashSet<_>>().len()
}

/// The page's own words in `s` (THE rule, with `outline::anchor`): its distinct folded words of ≥ 4
/// letters outside `CHECK.stop` (`access`, `denied`, `page`: a wall's or a 404's title says nothing of the page).
pub fn words_of(s: &str) -> Vec<String> {
    let f = clean::fold(s);
    let long = f.split(|c: char| !(clean::is_word(c) && !clean::is_digit(c) && c != '_'));
    crate::lang::uniq(
        long.filter(|w| w.chars().count() >= 4 && !CHECK.stop.contains(w)).map(String::from),
        String::clone,
    )
}

/// The page title up to its site suffix (`Title | Site`, `Title – Site`).
pub fn title_head(s: &str) -> &str {
    let cs: Vec<(usize, char)> = s.char_indices().collect();
    cs.windows(3)
        .find(|w| crate::html::py_space(w[0].1) && "|·–—-".contains(w[1].1) && crate::html::py_space(w[2].1))
        .map_or(s, |w| &s[..w[0].0])
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
    let (vis, body) = visible(tree);
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
    // no wall path named: the copy landed where the asked id (its last path segment, 8+ letters or
    // digits) is gone, and shows none of its own words: a front page, a search; never the page
    let id = slug_id(url).is_some_and(|s| s.bytes().filter(u8::is_ascii_alphanumeric).count() >= 8);
    if id && !page && crate::outline::lost_page(url, &raw.final_url).is_some() {
        return (false, "redirected".into(), chars, false);
    }
    if is_serp(url) {
        let ok = serp_results(tree) >= limit("serp_links");
        return (ok, if ok { "ok" } else { "serp_empty" }.into(), chars, false);
    }
    // a shell loads JavaScript: a short one with none (no `<script>`, no refresh) and none of its own words is the
    // site's own answer, an error page (Akamai's 534 B "Site Maintenance"; no phrase list)
    if !shown && (words.len() >= 2 || chars < 800) {
        let refresh = |n: &crate::html::Node| n.attr("http-equiv").is_some_and(|v| v.eq_ignore_ascii_case("refresh"));
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

    fn plan(mode: Mode) -> Plan {
        Plan { mode, cap: COST_CAP, keys: Tier::ALL.to_vec(), hedge: HEDGE, ..Plan::default() }
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
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].starts_with("! no_key bd · would try web search ≤$0.0015 · set BRIGHTDATA_API_KEY"),
            "{rows:?}"
        );
        assert!(rows[0].ends_with(" · bd https://brightdata.com/cp/start"), "the sign-up page: {rows:?}");
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
                Wait(S(64)),
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
            ("both running: wait", &auto, both(), vec![(Bd, S(6)), (OxyJs, S(1))], Wait(S(59) + S(5))),
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
        assert_eq!(gov.tiers(), [Plain, Oxy, OxyJs], "gov host skips bd: no other render, so oxy no-JS runs");
        let zip = Plan { doc: Some("zip"), ..plan(Mode::Auto) };
        assert_eq!(next(&zip, &[hop(Tier::Plain, "blocked_403")], &[]), Stop, "zip documents: plain only");
        assert!(skipped(&zip, &[hop(Tier::Plain, "blocked_403")], "u1", "u1", "").is_empty());
        let js = Plan { js: true, ..plan(Mode::Auto) };
        assert_eq!(js.tiers(), [Bd, OxyJs], "js:true skips plain and oxy no-JS");
        assert_eq!(next(&js, &[], &[]), Start(Bd));
        // A paid ask runs ONE provider; the next starts only on its failure, never beside it (hedge:off)
        assert_eq!(next(&js, &[], &[(Bd, S(30))]), Wait(S(35)), "no oxyjs beside a slow bd");
        assert_eq!(next(&js, &[hop(Bd, "js_shell")], &[]), Start(OxyJs), "it failed: oxyjs");
        assert_eq!(next(&js, &[hop(Bd, "not_found")], &[]), Stop);
        assert_eq!((route(&js), js.hedged(), auto.hedged()), ((vec![(Bd, None), (OxyJs, None)], 0.003), false, true));
        assert!(!plan(Mode::Only(Bd)).hedged(), "a named tier is a paid ask too");
        assert!(plan(Mode::Only(Plain)).hedged(), "provider:plain is free: no hedge:off");
        let nobd = Plan { keys: vec![Plain, Oxy, OxyJs], ..plan(Mode::Auto) };
        // one key: that provider's every rung, oxy no-JS gating the render (the one-key spike: nytimes 1.9 s)
        assert_eq!(next(&nobd, &both(), &[]), Start(Oxy), "no bd key → oxy no-JS");
        assert_eq!(next(&nobd, &[hop(Plain, "blocked_403"), hop(Oxy, "captcha_wall")], &[]), Start(OxyJs));
        let nooxy = Plan { keys: vec![Plain, Bd], ..plan(Mode::Auto) };
        assert_eq!(next(&nooxy, &[hop(Plain, "js_shell")], &[]), Start(Bd), "no oxy key → bd, gated by plain");
        let only = plan(Mode::Only(Plain));
        assert_eq!(next(&only, &[hop(Plain, "js_shell")], &[]), Stop, "provider:plain never escalates");
        let oxy = plan(Mode::Only(Oxy));
        assert_eq!(oxy.tiers(), [Oxy, OxyJs], "provider:oxy keeps both oxy tiers");
        assert_eq!(next(&oxy, &[hop(Oxy, "provider_error")], &[]), Start(OxyJs));
        // a refused or rate-limited key is never sent again by a tier that shares it; a tier with its own key runs
        for code in KEY_CODES {
            assert_eq!(next(&oxy, &[hop(Oxy, code)], &[]), Stop, "provider:oxy {code}: no oxyjs");
            assert!(skipped(&oxy, &[hop(Oxy, code)], "u1", "u1", "").is_empty(), "{code}: no budget row for oxyjs");
            let refused_oxy = vec![hop(Plain, "js_shell"), hop(Oxy, code)];
            assert_eq!(next(&auto, &refused_oxy, &[]), Start(Bd), "oxy {code} → bd");
            assert_eq!(next(&auto, &refused_oxy, &[(Bd, S(9))]), Wait(S(56)), "{code}: bd alone, no oxyjs beside it");
            let mut all = refused_oxy;
            all.push(hop(Bd, "captcha_wall"));
            assert_eq!(next(&auto, &all, &[]), Stop, "{code}: bd failed, oxyjs still not tried");
            let search = Plan { keys: vec![Oxy, Bd], ..Plan::search(Mode::Auto, COST_CAP, None) };
            assert_eq!(next(&search, &[hop(Oxy, code)], &[]), Start(Bd), "search {code}: bd has its own key");
            assert_eq!(next(&search, &[hop(Oxy, code), hop(Bd, code)], &[]), Stop);
        }
        assert_eq!(Plan { js: true, ..plan(Mode::Only(Oxy)) }.tiers(), [OxyJs]);
        let poor = Plan { cap: 0.002, ..plan(Mode::Auto) };
        assert_eq!(next(&poor, &both(), &[(Bd, S(6))]), Wait(S(59)), "no budget for oxyjs too");
        assert_eq!(next(&poor, &both(), &[(Bd, S(1))]), Wait(S(64)), "no hedge wake without budget");
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

    /// One paid key: ONE retry on a 5xx or a timeout, never on a wall or a shell.
    #[test]
    fn one_key_retries_once() {
        use Step::*;
        use Tier::*;
        let (auto, nooxy) = (plan(Mode::Auto), Plan { keys: vec![Plain, Bd], ..plan(Mode::Auto) });
        let bd = |code: &str| vec![hop(Plain, "blocked_403"), hop(Bd, code)];
        assert_eq!(next(&nooxy, &bd("provider_timeout"), &[]), Start(Bd), "bd timed out: once more");
        let longer = |done: &[Hop]| nooxy.timeout_of(Bd, done).as_secs();
        assert_eq!((longer(&bd("provider_error")), longer(&bd("provider_timeout"))), (60, 120), "with twice the time");
        // a bd-only listings site (154 s to fail): the retry fits the 90 s one-key wall from the first call
        let ran = |gone: u64| {
            let now = Instant::now();
            let plain = Hop { end: now.checked_sub(S(gone - 10)), ms: 10_000, ..hop(Plain, "blocked_403") };
            vec![plain, Hop { end: Some(now), ms: (gone - 10) * 1000, ..hop(Bd, "provider_timeout") }]
        };
        assert_eq!(nooxy.timeout_of(Bd, &ran(40)), S(50), "90 s − 40 s gone, not 60 s");
        assert_eq!(next(&nooxy, &ran(40), &[]), Start(Bd), "50 s left ≥ the 30 s bd had");
        // bd-only on 20 hard pages: 5 retries after a 60 s cut got 23.9-30 s under the wall and served 0
        assert_eq!(next(&nooxy, &ran(70), &[]), Stop, "20 s left < the 60 s bd had: the run ends here");
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
        assert_eq!(next(&sixty, &ran(30), &[]), Stop, "not even with the wall far off");
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
        let twice: Vec<Hop> = bd("provider_timeout").into_iter().chain([hop(Bd, "provider_timeout")]).collect();
        assert_eq!(next(&nooxy, &twice, &[]), Stop, "one retry only");
        let tight = Plan { cap: 0.002, ..Plan { keys: vec![Plain, Bd], ..plan(Mode::Auto) } };
        assert_eq!(next(&tight, &bd("provider_timeout"), &[]), Stop, "the retry is inside cost:");
        // Plain 10 s + bd 60 s leave 20 s of the wall, under the 60 s bd had: dry shows no retry either
        assert_eq!(route(&nooxy), (vec![(Plain, None), (Bd, None)], 0.0015), "dry: the run ends at the cut");
        let both_failed: Vec<Hop> =
            bd("provider_timeout").into_iter().chain([hop(OxyJs, "provider_timeout")]).collect();
        assert_eq!(next(&auto, &both_failed, &[]), Stop, "two keys: the other provider was the retry");
        // One provider on the LADDER, both keys present
        let gov = Plan { gov: true, ..plan(Mode::Auto) };
        let js_timeout = vec![hop(Plain, "js_shell"), hop(Oxy, "js_shell"), hop(OxyJs, "provider_timeout")];
        assert_eq!(next(&gov, &js_timeout, &[]), Start(OxyJs), "a gov host: Oxylabs alone, once more");
        assert_eq!(next(&plan(Mode::Only(Bd)), &bd("provider_timeout"), &[]), Start(Bd), "provider:bd: once more");
        // Bd refused the URL (`europa.eu`, no gov label): Oxylabs is the one provider left, its cut gets the retry
        let refused =
            vec![hop(Plain, "js_shell"), hop(Oxy, "x"), hop(Bd, "provider_refused"), hop(OxyJs, "provider_timeout")];
        assert_eq!(next(&auto, &refused, &[]), Start(OxyJs), "a refused owner covers no failure");
        let no_key = vec![hop(Plain, "js_shell"), hop(Oxy, "x"), hop(Bd, "bad_key"), hop(OxyJs, "provider_timeout")];
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
        assert_eq!(hint.tiers(), [Plain, Oxy, Bd, OxyJs], "the page ladder");
        assert_eq!(next(&hint, &[hop(Plain, "js_shell")], &[(Oxy, S(9))]), Wait(S(26)), "oxy has 30 s");
        let pdf = Plan { doc: Some("pdf"), ..plan(Mode::Auto) };
        let p403 = || hop(Plain, "blocked_403");
        assert_eq!(route(&pdf), (vec![(Plain, None), (Oxy, None), (Bd, None)], 0.0025));
        assert_eq!(next(&pdf, &[p403()], &[]), Start(Oxy));
        assert_eq!(next(&pdf, &[p403()], &[(Oxy, S(9))]), Wait(S(26)), "oxy has 30 s for a file");
        assert_eq!(next(&pdf, &[p403(), hop(Oxy, "not_pdf")], &[]), Start(Bd));
        assert_eq!(next(&pdf, &[p403(), hop(Oxy, "not_pdf")], &[(Bd, S(9))]), Wait(S(56)), "no oxyjs beside bd");
        assert_eq!(next(&pdf, &[p403(), hop(Oxy, "x"), hop(Bd, "pdf_truncated")], &[]), Stop);
        assert_eq!(next(&pdf, &[hop(Plain, "not_found")], &[]), Stop);
        assert_eq!(Plan { js: true, ..Plan { doc: Some("pdf"), ..plan(Mode::Auto) } }.tiers(), [Plain, Oxy, Bd]);
        assert_eq!(Plan { gov: true, ..Plan { doc: Some("pdf"), ..plan(Mode::Auto) } }.tiers(), [Plain, Oxy]);
        assert_eq!(Plan { doc: Some("pdf"), ..plan(Mode::Only(Oxy)) }.tiers(), [Oxy]);
        // no `.pdf` in the URL: plain's failed answer says PDF (content type), so the run becomes one
        let page = plan(Mode::Auto);
        let said = Hop { kind: "pdf", ..p403() };
        assert_eq!(next(&page, &[said], &[]), Start(Oxy));
        let said = || Hop { kind: "pdf", ..p403() };
        assert_eq!(next(&page, &[said(), hop(Oxy, "provider_error"), hop(Bd, "x")], &[]), Stop, "no oxyjs");
        let rows =
            skipped(&Plan { keys: vec![Plain, Oxy], ..plan(Mode::Auto) }, &[said(), hop(Oxy, "x")], "u1", "u1", "");
        assert_eq!(rows.len(), 1, "bd's key, no oxyjs budget row: {rows:?}");
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
        assert_eq!(p.tiers(), [Tier::OxyJs], "the other render only");
        assert_eq!(rendered(&m, &[Tier::Plain, Tier::Bd]), None, "one key: bd is still the only render");
    }

    /// No kept run takes a tier off the ladder or its retry (nothing keeps runs): every cross-run skip policy lost
    /// 2–4 of 17 oxy no-JS serves to save ~2 s. Oxy-only:
    /// oxy no-JS timed out on r/rust and the missing bd key is still named.
    #[test]
    fn no_kept_run_wears_a_tier() {
        use Tier::*;
        let oxy = Plan { keys: vec![Plain, Oxy, OxyJs], ..plan(Mode::Auto) };
        let hops = [hop(Plain, "cloudflare_wall"), hop(Oxy, "provider_timeout"), hop(OxyJs, "x")];
        let rows = skipped(&oxy, &hops, "", "u1", "");
        assert!(rows.len() == 1 && rows[0].starts_with("! no_key bd · would try "), "{rows:?}");
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
        assert_eq!(after("js_shell"), [Tier::Plain, Tier::Bd, Tier::OxyJs], "a shell: plain → bd ∥ oxyjs");
        assert_eq!(after("captcha_wall"), [Tier::Plain, Tier::Oxy, Tier::Bd, Tier::OxyJs], "plain → oxy → bd ∥ oxyjs");
        let race = Plan { hedge: Duration::ZERO, ..plan(Mode::Auto) };
        assert_eq!(route(&race).0[3], (Tier::OxyJs, Some(Duration::ZERO)));
        // bd is given up on (1 s + grace) before a 60 s hedge: oxyjs follows it instead of racing it
        let late = Plan { hedge: Duration::from_secs(60), timeout: Some(Duration::from_secs(1)), ..plan(Mode::Auto) };
        assert_eq!(route(&late).0[3], (Tier::OxyJs, None));
        let nokey = Plan { keys: vec![Tier::Plain], ..plan(Mode::Auto) };
        assert_eq!(route(&nokey).0, vec![(Tier::Plain, None)]);
        let rows = skipped(&nokey, &[hop(Tier::Plain, "blocked_403")], "u1", "u1", "");
        assert_eq!(rows.len(), 2, "one row per missing key: {rows:?}");
        let oxy = "u1 ! no_key oxy · would try no-JS fetch + JS render ≤$0.0025 · set OXYLABS_WEB_API_KEY → ";
        assert!(rows[0].starts_with(oxy) && rows[1].starts_with("u1 ! no_key bd · would try unblock+JS"), "{rows:?}");
        // after a JS shell oxy no-JS served 0/15, so a key would buy the render only
        let rows = skipped(&nokey, &[hop(Tier::Plain, "js_shell")], "u1", "u1", "");
        assert!(rows[1].starts_with("u1 ! no_key oxy · would try JS render ≤$0.0015 · "), "{rows:?}");
        assert!(skipped(&nokey, &[hop(Tier::Plain, "not_found")], "u1", "u1", "").is_empty());
        let poor = Plan { cap: 0.001, ..plan(Mode::Auto) };
        let walled = [hop(Tier::Plain, "captcha_wall"), hop(Tier::Oxy, "x")];
        let rows = skipped(&poor, &walled, "u1", "u1", "");
        let want = format!(
            "u1 ! budget_hit bd oxyjs not tried · $0.0040 > cost:0.001 → {} u1 fresh:true cost:0.004 ≤$0.0040",
            crate::lang::bin()
        );
        assert_eq!(rows, [want]);
        // Priced for the URL's own plan, as its `dry:true` (a .gov page: no bd, `up to $0.0025`)
        let gov = skipped(&poor, &walled, "u1", "u1", "https://www.usa.gov/x");
        assert!(gov[0].ends_with(" cost:0.004 ≤$0.0025"), "{gov:?}");
        // The words after the binary heading the arrow, never after its name further in
        let arrow = |head: &str| worst(&format!("{head}{} u1 fresh:true", crate::lang::bin()), "");
        assert!(arrow("") > 0.0 && arrow("x") == 0.0 && arrow("top up, then ") == 0.0);
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

    /// The driver with a fake transport: plain fails, oxy without JS too (one try after a captcha), bd is slow; oxyjs starts when the
    /// hedge runs out (5 s in `next`, scaled to 200 ms here), wins, and bd is cancelled, its list price unverified.
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

    /// A timeout's fix is a render while one is left to try; a render that ran out its time with none left buys a
    /// longer time, the agent's own `timeout:` cut the default time; shells from two providers are
    /// `no_copy` with no paid next step.
    #[test]
    fn timeout_fix_never_repeats_a_render() {
        use Tier::*;
        let b = crate::lang::bin();
        let (js, retry) = (format!("{b} u1 fresh:true js:true ≤$0.0030"), format!("{b} u1 fresh:true ≤$0.0040"));
        let free = format!("{retry} (last try not charged)");
        let none = "nothing left to try · open https://a.example/ in a browser";
        let longer = format!("{b} u1 fresh:true timeout:150 ≤$0.0040");
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
            (&[(Plain, "js_shell"), (Oxy, "x"), (Bd, "x"), (OxyJs, "provider_timeout")], "provider_timeout", &longer),
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
            let at = crate::lang::At {
                id: "u1",
                url: &m.url,
                arg: fix_arg(&m, &[]).unwrap_or_default(),
                done: &spent(&m, &[], &Tier::ALL),
            };
            assert_eq!((m.code.as_str(), crate::lang::fix(move_code(&m), &at).as_str()), (*code, *want), "{path:?}");
        }
        // `timeout:8` once said "nothing left to try": the agent's cut → the default time; a tier's own cut row
        let p = Plan { timeout: Some(S(8)), ..plan(Mode::Auto) };
        let cut = |s: u64| cut_why(&p, Hop { timeout_s: s, ..hop(Bd, "provider_timeout") });
        assert_eq!((cut(8).why.as_str(), cut(60).why.as_str()), ("timeout:8s", "took > 60 s here"));
        let mut hops = vec![hop(Plain, "blocked_403"), hop(Oxy, "x"), cut(8), Hop { tier: OxyJs, ..cut(8) }];
        let m = outcome("https://a.example/", &mut hops, 1).meta;
        let at = crate::lang::At {
            id: "u1",
            url: &m.url,
            arg: fix_arg(&m, &[]).unwrap_or_default(),
            done: &spent(&m, &[], &Tier::ALL),
        };
        assert_eq!(crate::lang::fix(&m.code, &at), retry, "after `timeout:8`: fresh:true, never nothing left");
        let mut hops = vec![hop(Plain, "blocked_403"), hop(OxyJs, "x"), cut(60)];
        let m = outcome("https://a.example/", &mut hops, 1).meta;
        // Read as `cut at 60 s (5m ago)`, its age from the copy's time; none under a minute
        assert_eq!(fail_detail(&m, "", &[]).0, "bd cut at 60 s");
        let old = Meta { t: m.t - 300.0, ..m.clone() };
        assert_eq!(fail_detail(&old, "", &[]).0, "bd cut at 60 s (5m ago)");
        assert_eq!(fix_arg(&m, &[]), Some("timeout:150"));
    }

    /// The longer time runs once a session per URL and tier (bd cut at 150 s on another copy
    /// of the URL): never again; the keyed paid tier left (oxy no-JS here: `lang::fix` names it), else a count with
    /// no arrow; `timeout:150` is no short budget (a short one is a time under the tier's own).
    #[test]
    fn a_longer_time_runs_once() {
        use Tier::*;
        // the 150 s call under the agent's `timeout:150`, the 60 s one under none (under `timeout:` every
        // render cut is the agent's)
        let p = |s: u64| Plan { timeout: (s == 150).then_some(S(s)), ..plan(Mode::Auto) };
        let cut = |s: u64| cut_why(&p(s), Hop { timeout_s: s, ..hop(Bd, "provider_timeout") });
        let long = cut(150);
        assert_eq!((long.why.as_str(), cut(60).why.as_str()), ("timeout:150s", "took > 60 s here"));
        let cuts = [long.row()];
        let meta = |last: Hop, first: &str| {
            outcome("https://a.example/", &mut [hop(Plain, first), hop(OxyJs, "x"), last], 1).meta
        };
        let m = meta(cut(60), "blocked_403");
        assert_eq!((fix_arg(&m, &cuts), timed_out(&m, &cuts).as_str()), (Some("js:true"), "60 s + 150 s here"));
        let m = meta(cut(60), "js_shell");
        assert_eq!((fix_arg(&m, &cuts), fix_arg(&m, &[])), (None, Some("timeout:150")));
        let m = meta(long, "js_shell");
        assert_eq!((fix_arg(&m, &[]), timed_out(&m, &[]).as_str()), (None, "150 s here"));
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

    /// A tier whose only signal was OUR cut (`upto`) is no "tried" and no app-page vote: with
    /// nothing else left it is offered once, named with two keys (TikTok: bd cut at 60 s, served in 36.7 s later;
    /// a Reddit thread: oxy no-JS cut at 10 s, served 6 of 6 later); once its long call ran (cut OR answered), a count.
    #[test]
    fn a_cut_tier_is_not_tried() {
        use Tier::*;
        let b = crate::lang::bin();
        let ours = |t: Tier, s: u64| Hop { upto: t.spec().price, timeout_s: s, ..hop(t, "provider_timeout") };
        let fix = |m: &Meta, cuts: &[HopRow]| {
            let arg = fix_arg(m, cuts);
            let at = crate::lang::At {
                id: "u1",
                url: &m.url,
                arg: arg.unwrap_or_default(),
                done: &spent(m, &[], &Tier::ALL),
            };
            arg.map(|_| crate::lang::fix(&m.code, &at))
        };
        let mut tiktok = [hop(Plain, "js_shell"), hop(OxyJs, "js_shell"), ours(Bd, 60)];
        let m = outcome("https://a.example/", &mut tiktok, 1).meta;
        let offer = format!("{b} u1 fresh:true provider:bd timeout:150 ≤$0.0015");
        assert_eq!((m.code.as_str(), fix(&m, &[])), ("provider_timeout", Some(offer)), "not no_copy, not nothing left");
        let answered = Hop { timeout_s: 150, ..hop(Bd, "js_shell") }.row();
        assert_eq!(fix(&m, std::slice::from_ref(&answered)), None, "the 150 s call answered: a count");
        assert_eq!(timed_out(&m, &[answered]), "cut at 60 s · 150 s failed (js_shell)", "a shell is no serve");
        for (code, said) in [("ok", "answered"), ("http_502", "failed (502)")] {
            let long = Hop { timeout_s: 150, ..hop(Bd, code) }.row();
            assert_eq!(timed_out(&m, &[long]), format!("cut at 60 s · 150 s {said}"), "named apart");
        }
        let mut reddit =
            [hop(Plain, "blocked_403"), ours(Oxy, 10), hop(Bd, "captcha_wall"), hop(OxyJs, "captcha_wall")];
        let m = outcome("https://a.example/", &mut reddit, 1).meta;
        assert_eq!(fix(&m, &[]), Some(format!("{b} u1 fresh:true provider:oxy ≤$0.0025")), "oxy, then oxyjs");
        // 8 saved copies: both renders cut at 60 s = both tried; bd's longer call, priced, once
        let mut both = [hop(Plain, "js_shell"), ours(Bd, 60), ours(OxyJs, 60)];
        let m = outcome("https://a.example/", &mut both, 1).meta;
        assert_eq!(fix(&m, &[]), Some(format!("{b} u1 fresh:true provider:bd timeout:150 ≤$0.0015")), "never js:true");
        let bd150 = Hop { timeout_s: 150, ..ours(Bd, 150) }.row();
        assert_eq!(fix(&m, std::slice::from_ref(&bd150)), None, "bd's 150 s call ran: a count");
        assert_eq!(timed_out(&m, &[bd150]), "60 s + 150 s here");
        let again = outcome("https://a.example/", &mut [ours(Oxy, 10)], 1).meta;
        let cuts: Vec<HopRow> = m.hops.clone();
        assert_eq!(fix(&again, &cuts), None, "offered once: then a count");
        assert_eq!(fix(&again, &[]).as_deref(), Some(format!("{b} u1 fresh:true js:true ≤$0.0030").as_str()));
    }

    /// A Joomla copy with only `meta.code` changed: a wall at plain then OUR cut on the paid tier
    /// prints the same reason and move whatever code heads the row: the cut tier decides, never the head.
    #[test]
    fn a_wall_headed_cut_keeps_the_timeout_move() {
        use Tier::*;
        let b = crate::lang::bin();
        let ours =
            || Hop { upto: 0.0015, timeout_s: 60, why: "took > 60 s here".into(), ..hop(OxyJs, "provider_timeout") };
        let row = |m: &Meta| {
            let done = spent(m, &[], &Tier::ALL);
            let at = crate::lang::At { id: "u1", url: &m.url, arg: fix_arg(m, &[]).unwrap_or_default(), done: &done };
            (fail_detail(m, "", &[]).0, crate::lang::fix(move_code(m), &at))
        };
        let paid = Hop { cost: 0.0015, ..hop(Bd, "captcha_wall") };
        let offer = format!("{b} u1 fresh:true provider:oxy js:true timeout:150 ≤$0.0015");
        let bd = format!("{b} u1 fresh:true provider:bd ≤$0.0015");
        for (mut hops, want) in [
            (vec![hop(Plain, "cloudflare_wall"), ours()], bd),
            (vec![hop(Plain, "cloudflare_wall"), paid, ours()], offer),
        ] {
            let wall = outcome("https://a.example/", &mut hops, 1).meta;
            let cut = Meta { code: "provider_timeout".into(), ..wall.clone() };
            assert!(wall.code.ends_with("_wall"), "the wall heads it: {}", wall.code);
            let (why, fix) = row(&wall);
            assert!(why == "oxyjs cut at 60 s" && fix == want, "{}: {why} {fix}", wall.tried());
            assert_eq!(row(&wall), row(&cut), "the same row apart from the code word");
        }
        // behind a sign-in gate (`one_render`) the run is the gate's only when its render met it too: a cut or
        // refused render names the run, so its own move (the 150 s offer, the other provider) survives
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
            let at = crate::lang::At {
                id: "u1",
                url: &m.url,
                arg: arg.unwrap_or_default(),
                done: &spent(&m, &[], &Tier::ALL),
            };
            (fail_detail(&m, "", &[]).0, arg.map(|_| crate::lang::fix(&m.code, &at)))
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

    /// No wall path is named (LinkedIn's `/authwall` row gone): a copy that landed where the asked id (8+ letters or digits)
    /// is gone and shows none of its own words is `redirected`; one that shows them is served (the outline flags
    /// it), and a short id or a kept one is no redirect. A sign-in gate hands the asked path back in its query and
    /// names a sign-in: `login_wall` before `redirected`, whatever the id; a landing that only lost the page,
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
        // the testers' misfires: an index page's root, a renamed slug, a dropped sub-tab, a short link's shell
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
}
