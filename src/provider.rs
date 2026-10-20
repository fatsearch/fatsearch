//! PROVIDERS: how a URL becomes bytes. `plain` = a direct GET with Safari 26's TLS, HTTP/2 and header profile;
//! `bd` = Bright Data Web Unlocker; `oxy` / `oxyjs` = Oxylabs Web API `/v1/scrape` without / with JavaScript
//! rendering (one key, two prices, two timeouts); `decodo` / `decodojs` = Decodo Web Data API `/v1/scrape` the same
//! way. One table holds names, keys, timeouts and list prices; the router (fetch.rs) decides which to call.

use serde_json::{Value, json};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tier {
    #[default]
    Plain,
    Bd,
    Oxy,
    OxyJs,
    Decodo,
    DecodoJs,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// meta.json / stats name (`brightdata`); `short` is the `provider:` value and the tier-path name (`bd`).
    pub name: &'static str,
    pub short: &'static str,
    pub env: &'static str,
    pub timeout: Duration,
    /// USD per request at list price (no API returns a cost).
    pub price: f64,
    /// THE capability row the router builds every ladder from (`fetch::Plan::ladder`): `page` (fetches a page),
    /// `js` (renders JavaScript), `search`, `pdf` (a file's exact bytes), `gov` (government hosts served),
    /// `auto_geo` (its unset exit country is the best for the target: `sent` names none by default).
    can: Vec<&'static str>,
    /// The provider bills a finished job that relays the site's own 5xx (`charged`): Oxylabs' "Not
    /// charged" is its own API error (`500 REQUEST_FAILED_AFTER_TOO_MANY_RETRIES`, an error envelope, never a
    /// `results` answer); Bright Data's inner 5xx is an unlock failure (zone counter +0).
    pub bills_site_5xx: bool,
    /// What a call adds, for the `no_key` row (`unblock+JS`).
    pub buys: &'static str,
    /// A search's typical answer time, s: the default `wait:` of a search this tier leads (`fetch::Plan::search`).
    pub search_s: u64,
}

/// The provider table (`src/data/providers.json`): `specs` indexed by `Tier as usize`, `apis` by `Api as usize`,
/// `links` per account, `cctld` = TLD → `"<exit country> <its page languages>"` (`fetch::langs`).
/// Prices: Oxylabs Web API $1.00/1k, $1.50/1k with `run_js` (smallest paid plan); Bright Data Web Unlocker
/// ~$1.50/1k; Decodo $1.00/1k, $1.50/1k with JS. Timeouts: plain 10 s, Oxylabs without JS 10 s (its wins took
/// 1.4–5.6 s on 40 hard pages, 6 s cut 2 of 3 Reddit threads; a page rung after plain's captcha with both keys and
/// on every rung with one key, also files, prefetch, `provider:oxy`), Decodo without JS 15 s, BD, Oxylabs and
/// Decodo with JS 60 s each (`timeout:` sets the renders).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, bound(deserialize = "'de: 'static"))]
pub struct Providers {
    pub specs: Vec<Spec>,
    pub apis: Vec<ApiRow>,
    pub links: Vec<Links>,
    pub cctld: std::collections::HashMap<&'static str, &'static str>,
}

pub static PROVIDERS: LazyLock<Providers> = LazyLock::new(|| crate::settings::table("providers"));

impl Tier {
    pub const ALL: [Tier; 6] = [Tier::Plain, Tier::Bd, Tier::Oxy, Tier::OxyJs, Tier::Decodo, Tier::DecodoJs];

    pub fn spec(self) -> &'static Spec {
        &PROVIDERS.specs[self as usize]
    }

    /// A tier's short or stored name (`oxyjs`, `oxylabs-js`) → tier; the `provider:` mod refuses `oxyjs`.
    pub fn parse(s: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.spec().short == s || t.spec().name == s)
    }

    /// The tier whose key this one sends, as `fats config` lists it (`oxyjs` → `oxy`): the first with its env.
    pub fn owner(self) -> Tier {
        Tier::ALL.into_iter().find(|t| t.spec().env == self.spec().env).unwrap_or(self)
    }

    /// The JS rung of a provider whose key also fetches without JS (oxyjs, decodojs): the call asks for a render
    /// (`Ask::js`) and the copy drops `<noscript>` (`clean::build_doc`); bd renders on its own.
    pub fn renders(self) -> bool {
        self.owner() != self
    }

    /// The tier's row says it does `job` (`Spec::can`).
    pub fn can(self, job: &str) -> bool {
        self.spec().can.contains(&job)
    }

    /// The name `fats setup <name> on|off` switches: the key's owner, `free` for plain.
    pub fn switch(self) -> &'static str {
        if self == Tier::Plain { "free" } else { self.owner().spec().short }
    }

    /// The key this tier sends (`settings::key`; Bright Data's needs the zone too); None = not configured, or
    /// switched off (`settings::on`). Plain needs none: an empty key.
    pub fn key(self) -> Option<String> {
        let key = || crate::settings::key(self.switch()).map(|k| k.0);
        match self {
            _ if !crate::settings::on(self.switch()) => None,
            Tier::Plain => Some(String::new()),
            Tier::Bd => crate::settings::zone().and_then(|_| key()),
            _ => key(),
        }
    }
}

/// Short name for a stored provider name (`brightdata` → `bd`); unknown names pass through.
pub fn short(name: &str) -> &str {
    PROVIDERS.specs.iter().find(|s| s.name == name).map_or(name, |s| s.short)
}

/// What to fetch and how.
#[derive(Clone, Default)]
pub struct Ask {
    pub url: String,
    /// A render (`Tier::renders`): Oxylabs `run_js`, Decodo `headless`.
    pub js: bool,
    /// `country:` as given, ISO 3166-1 alpha-2, lower case (`fr`); see `country` for what each tier sends.
    pub country: Option<String>,
    pub timeout: Option<Duration>,
    /// Body cap in bytes (`max_bytes:`).
    pub max_bytes: u64,
    /// `device:mobile`: Oxylabs emulates a phone's user agent and viewport; Bright Data has no such setting.
    pub mobile: bool,
    /// A PDF run (fetch `pdf_run`): Bright Data sends the file's bytes (`format: raw`), not a JSON string.
    pub doc: bool,
    /// Plain sends the browser's own `Accept`, without `MD_ACCEPT`: `fetch::run`'s second call after a weak Markdown
    /// answer, and a picture download (`cli::download`).
    pub html: bool,
}

/// Plain's `Accept` (`FATS_ACCEPT_MD`, not in a PDF run): Markdown first, then Safari's own list, so a server that
/// converts its pages (Cloudflare's Markdown for Agents, `text/markdown`) skips our CONVERT and any other server
/// answers as it does a browser.
pub const MD_ACCEPT: &str = "text/markdown;q=1.0, text/html;q=0.9, application/xhtml+xml;q=0.9, */*;q=0.8";

/// A target's answer, whoever carried it. `headers` = lowercase `name: value` lines (bot-wall vendors live there).
#[derive(Default)]
pub struct Raw {
    pub status: u16,
    pub final_url: String,
    pub ctype: String,
    pub headers: String,
    pub body: Vec<u8>,
    /// What the body was decoded from (`windows-1250`; "" = UTF-8, as provider APIs return JSON strings).
    pub charset: &'static str,
    /// The provider's request budget, when its answer said (`Limit`).
    pub limit: Option<Limit>,
    /// The page whose meta refresh plain followed (`refresh`) to this one (`Meta::refreshed_from`): kept to be
    /// seen, never a base, never what `outline::lost_page` judges (the target is, as any landing).
    pub refreshed_from: Option<String>,
    /// Plain's https failure an http copy stands in for (`plain`): `tls:wrong-version-number+http:200`.
    pub why: String,
}

/// A provider's own request budget from its answer: the two numbers only. Oxylabs Web API sends
/// `x-ratelimit-<bucket>-<account number>-limit` and `-remaining` (search 10, scrape 250; undocumented, window
/// unknown; observed, not documented). The header names carry the account number, so they are never kept.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limit {
    pub limit: u32,
    pub remaining: u32,
}

/// Seconds before a rate-limited call is worth retrying when the answer gives none (`retry-after`).
pub const RETRY_AFTER: u64 = 5;

/// A failed call: the stable code, and the provider's own answer when it sent one (HTTP status, body). The body
/// is never printed: it is masked here and kept only as the snapshot's `error.txt` (a provider may echo the
/// key in it: ScrapingBee's 401 does).
#[derive(Debug, Default, PartialEq)]
pub struct Fail {
    pub code: &'static str,
    pub status: u16,
    pub body: Vec<u8>,
    /// What the `!` row may add to the code: a short token read from the answer (`proxy_error`), never raw text.
    pub why: String,
    pub limit: Option<Limit>,
}

impl From<&'static str> for Fail {
    fn from(code: &'static str) -> Fail {
        Fail { code, ..Fail::default() }
    }
}

/// One call. Err = a stable code. `billed` = the provider charges for it (`charged`).
pub async fn get(t: Tier, ask: &Ask, key: &str) -> (Result<Raw, Fail>, bool) {
    let wait = ask.timeout.unwrap_or(t.spec().timeout);
    let r = match t {
        Tier::Plain => plain(ask, wait).await,
        Tier::Bd => brightdata(ask, key, wait).await,
        Tier::Oxy | Tier::OxyJs => oxylabs(ask, key, wait).await,
        Tier::Decodo | Tier::DecodoJs => decodo(ask, key, wait).await,
    };
    let billed = charged(t, r.as_ref().map(|raw| raw.status));
    (r.and_then(|raw| if raw.body.len() as u64 > ask.max_bytes { Err("too_big".into()) } else { Ok(raw) }), billed)
}

/// Run `f` to its end on a fresh single-thread runtime: the one bridge from the blocking CLI to async calls.
/// None = no runtime could be built (the caller's `net_error`).
pub fn block<F: Future>(f: F) -> Option<F::Output> {
    Some(tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?.block_on(f))
}

/// A command answers by `wait:`: `calls` run side by side on one runtime, each answer in order;
/// None = still running at `wait` (given up, the row says `late`).
pub fn all_within<F>(calls: Vec<F>, wait: Duration) -> Vec<Option<F::Output>>
where
    F: Future<Output: Send> + Send + 'static,
{
    let mut got: Vec<Option<F::Output>> = calls.iter().map(|_| None).collect();
    block(async {
        let mut set = tokio::task::JoinSet::new();
        for (i, call) in calls.into_iter().enumerate() {
            set.spawn(async move { (i, call.await) });
        }
        let end = tokio::time::Instant::now() + wait;
        while let Ok(Some(Ok((i, answer)))) = tokio::time::timeout_at(end, set.join_next()).await {
            got[i] = Some(answer);
        }
    });
    got
}

/// A call given up at the command's `wait:` (`all_within`, a search's deadline): `provider_timeout wait:5s`.
pub fn late(wait: Duration) -> Fail {
    Fail { why: format!("wait:{}s", wait.as_secs()), ..Fail::from("provider_timeout") }
}

/// The second resolver, asked only when the system resolver found no address: Cloudflare's DNS over HTTPS (JSON,
/// developers.cloudflare.com 1.1.1.1 dns-json: GET `name` + `type`, `accept: application/dns-json`; `Status` = the
/// DNS RCODE, `Answer` the records, `type` 1 = A, 28 = AAAA). Some(true) = an address (A, else AAAA: asked only
/// on no A), Some(false) = none: NXDOMAIN (RCODE 3) or NODATA (NOERROR, no address: a dead name in a signed zone
/// answers so), None = no verdict (unreachable, SERVFAIL).
pub async fn exists(host: &str) -> Option<bool> {
    for (ty, code) in [("A", 1), ("AAAA", 28)] {
        let (body, ..) = api(Api::Doh, "", json!({"name": host, "type": ty}), 64 << 10, DOH_WAIT).await.ok()?;
        let j = serde_json::from_slice::<Value>(&body).ok()?;
        let records = j["Answer"].as_array().into_iter().flatten();
        match j["Status"].as_u64()? {
            0 if records.into_iter().any(|r| r["type"] == code) => return Some(true),
            0 => {}
            3 => return Some(false),
            _ => return None,
        }
    }
    Some(false)
}

/// A keyless call to fats's own endpoint (`telemetry`): the 200's body, else what failed (the HTTP status, or the
/// code when no reply came), never a row.
pub async fn ours(to: Api, req: Value, wait: Duration) -> Result<Vec<u8>, String> {
    let fail = |f: Fail| if f.status > 0 { f.status.to_string() } else { f.code.to_string() };
    api(to, "", req, 64 << 10, wait).await.map(|a| a.0).map_err(fail)
}

pub const DOH_WAIT: Duration = Duration::from_millis(1500);

/// THE host check, before any tier runs (`js:true` skips plain too): the system resolver, and on no address the second
/// resolver (`exists`, asked on that failure only). A host with no address from both (NXDOMAIN or NODATA alike) is a
/// plain hop `no_host`, one whose every address is private `private_address`: final, $0, no provider call. No paid call
/// before a verdict on the address: none within `dns_wait` seconds is a final `dns_timeout` (when the providers ran at
/// a shorter wait, a name of 127.0.0.1 billed a render of the provider's own loopback); worst case `dns_wait` + the
/// second resolver's two calls (A, then AAAA on no A), `DOH_WAIT` 1.5 s each: ≤ 3 s. One lookup: the system resolver
/// retries on its own, and a second one for the same name only joins the first (macOS mDNSResponder: an unanswered
/// query is sent again at ≥ 1 s, doubling, uDNS.c `UDNSBackOffMultiplier`; the same question is a `DuplicateOf` the
/// first, mDNS.c; glibc: `attempts:2` × `timeout:5`, resolv.conf(5)). Within the agent's `timeout:` (`agent_left`) when
/// shorter: the lookup is no part of a call, the wall holds it all the same. None = the host resolves public, or the
/// URL names an IP address; else the hop's code and why (`fetch::host_hop`). A real run's only: `dry:true` looks
/// nothing up (`lang::real_host`, its shape rule `askable` here too).
pub fn host_check(url: &str, wall: Option<Duration>) -> Option<(&'static str, String)> {
    let url::Host::Domain(host) = url::Url::parse(url).ok()?.host()?.to_owned() else { return None };
    let (t0, (tx, rx), name) = (Instant::now(), std::sync::mpsc::channel(), host.clone());
    // a name no resolver can ask for (`lang::askable`: an empty or 64-char label, a hyphen at a label's end) is
    // looked up by neither resolver: `no_host` at once, $0, never the network's fault
    let ask = crate::lang::askable(&host);
    ask.then(|| std::thread::spawn(move || tx.send(lookup(&name))));
    let all = rx
        .recv_timeout(Duration::from_secs(crate::settings::limit("dns_wait")).min(wall.unwrap_or(Duration::MAX)))
        .ok();
    let fast = t0.elapsed() < Duration::from_millis(crate::settings::limit("dns_fast_ms"));
    // Ok = the system resolver's word (true: every address private); on no address, Err = the second resolver's
    // (Some(true): it holds an address the system's did not give; Some(false): none, NXDOMAIN or NODATA) and the
    // system resolver's error
    let private = |all: &[std::net::SocketAddr]| all.iter().all(|a| private_ip(a.ip()));
    let second = || block(exists(&host)).flatten();
    let got = all.map(|all| match all {
        Ok(all) if !all.is_empty() => Ok(private(&all)),
        all => Err((second(), all.err())),
    });
    // an error back under `dns_fast_ms` was no wait cut short: `dns_wait:` cures nothing there
    let (code, why) = match got {
        _ if !ask => ("no_host", None),
        Some(Ok(true)) if !crate::settings::get().allow_private.value => ("private_address", None),
        Some(Ok(_) | Err((Some(true), _))) => return None,
        Some(Err((Some(false), _))) => ("no_host", None),
        Some(Err((None, why))) if fast => ("dns_failed", why),
        _ => ("dns_timeout", None),
    };
    Some((code, why.unwrap_or_default()))
}

/// The system resolver's addresses for `host`, or its own error (getaddrinfo's words, `said_short`: `nodename nor
/// servname provided, or not known`). Under a test's fake transport, the whole network: that error, a `lag.*`
/// name's after 2 s (`host_check`'s wait).
pub fn lookup(host: &str) -> Result<Vec<std::net::SocketAddr>, String> {
    if in_process() {
        std::thread::sleep(Duration::from_secs(if host.starts_with("lag.") { 2 } else { 0 }));
        return Err(said_short("nodename nor servname provided, or not known"));
    }
    let said = |e: std::io::Error| said_short(&e.to_string().rsplit(": ").next().unwrap_or_default().to_lowercase());
    std::net::ToSocketAddrs::to_socket_addrs(&(host, 443)).map(Iterator::collect).map_err(said)
}

/// THE billing rule, one for both providers: an answer below 500 is billed; a refusal, a 5xx or a timeout is not,
/// but a site's 5xx relayed in a finished job is where `bills_site_5xx` (Oxylabs, printed `(assumed)` until one is
/// reconciled). Oxylabs: "All upstream engines failed. Not charged." (developers.oxylabs.io/products/web-api/
/// troubleshooting, its own error envelope, a `provider_error` here); its no-JS and 60 s cuts end in that 500 (10 of
/// 11 re-run at 60 s, measured 2026-10-14). Bright Data: the zone counter moved +20 for ~28 calls, its 3 `http_502` and
/// 6 timeouts unbilled. A search answer with no result list is not (bd: +0
/// for 2 such `provider_error`s). A call WE cut has no answer: its price is unverified (`fetch::billed`:
/// bd billed 15 s search cuts; "a cut is free" held for 1-s cuts only).
pub fn charged(t: Tier, status: Result<u16, &Fail>) -> bool {
    t != Tier::Plain && status.is_ok_and(|s| s < 500 || t.spec().bills_site_5xx)
}

/// THE list of the provider's own refusals (its error envelope, bd's `x-brd-error`, a rate limit): no answer, so
/// never billed (`charged` gets them as an `Err`); a tried path says `$0 (not charged)`, a priced retry after one
/// ` · last try not charged`.
pub const REFUSED: &[&str] = &["provider_error", "provider_refused", "rate_limited"];

/// THE money total: from +0.0, so nothing spent prints `$0.0000` (`sum::<f64>()` of nothing is −0.0 → `$-0.0000`).
pub fn spend(costs: impl IntoIterator<Item = f64>) -> f64 {
    costs.into_iter().fold(0.0, |total, cost| total + cost)
}

/// A transport error's code; a TLS handshake that failed says so in `why`: `tls:<reason>` from BoringSSL's error
/// stack, whose text is `[REASON] …` (btls 0.5.6 `ErrorStack`'s Display: `tls:certificate-verify-failed`).
fn net(e: &wreq::Error) -> Fail {
    let mut src: Option<&(dyn std::error::Error + 'static)> = Some(e);
    let mut tls = String::new();
    while let Some(s) = src {
        if s.is::<Private>() {
            return Fail::from("private_address");
        }
        let said = s.to_string();
        if let Some(r) = said.strip_prefix('[').and_then(|r| r.split(']').next()) {
            tls = format!("tls:{}", r.to_lowercase().replace('_', "-"));
        }
        src = s.source();
    }
    let code = match () {
        _ if e.is_timeout() => "provider_timeout",
        _ if e.is_connect() || e.is_dns() || e.is_tls() => "connect_failed",
        _ => "net_error",
    };
    Fail { why: said_short(&tls), ..Fail::from(code) }
}

/// A header map as (name, value) pairs; names are lower case already.
fn pairs(h: &wreq::header::HeaderMap) -> Vec<(String, String)> {
    h.iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into())).collect()
}

/// Headers as `Raw::headers` keeps them: `name: value` lines, all lower case (wall vendors are matched on them).
fn header_lines<'a>(h: impl Iterator<Item = (&'a String, &'a str)>) -> String {
    h.map(|(k, v)| format!("{}: {}\n", k.to_lowercase(), v.to_lowercase())).collect()
}

async fn plain(ask: &Ask, wait: Duration) -> Result<Raw, Fail> {
    let client = wreq::Client::builder()
        .emulation(browser())
        .redirect(wreq::redirect::Policy::none())
        .dns_resolver(Fence)
        .timeout(wait)
        .build()
        .map_err(|_| "net_error")?;
    let send = |u: String| {
        let req = client.get(&u);
        let md = crate::settings::get().accept_md.value && !ask.doc && !ask.html;
        let req = if md { req.header(wreq::header::ACCEPT, MD_ACCEPT) } else { req };
        async move {
            let r = req.send().await.map_err(|e| net(&e))?;
            let to = r.headers().get("location").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
            Ok::<_, Fail>((r.status().as_u16(), to, r))
        }
    };
    let read = |at: String| {
        let send = &send;
        async move {
            let (final_url, r) = follow(&at, send).await?;
            let (status, headers) =
                (r.status().as_u16(), header_lines(pairs(r.headers()).iter().map(|h| (&h.0, h.1.as_str()))));
            let ctype = r.headers().get("content-type").map(|v| String::from_utf8_lossy(v.as_bytes()).to_lowercase());
            let ctype = ctype.unwrap_or_default();
            let body = capped(r, ask.max_bytes).await?;
            let declared =
                ctype.split("charset=").nth(1).map(|c| c.split(';').next().unwrap_or(c).trim_matches(['"', ' ']));
            // a picture's bytes are never text (`cli::asset`)
            let (body, charset) = if ctype.starts_with("image/") { (body, "") } else { decode_body(body, declared) };
            Ok::<Raw, Fail>(Raw { status, final_url, ctype, headers, body, charset, ..Raw::default() })
        }
    };
    // https the host's listener would not take (`connect_failed`: TLS, reset, refused; the host resolved,
    // `host_check`) is no wall: every tier meets the same listener. The same URL over http, once, in the same turn
    // of the brake (a redirect's way). Its why keeps the https cause first, then what http answered
    // (`tls:wrong-version-number+http:200`); a 2xx copy goes through the content check as any plain body
    // (`fetch::judge`), any other answer leaves the https failure. A certificate's refusal (expired, wrong host,
    // untrusted: `tls:certificate-*`) under it is the site's answer (`tls_failed`, final): every tier meets the same
    // certificate, a paid one only when asked (expired.badssl.com: http 301 back to https, then oxy + oxyjs, $0.0025)
    let mut first = read(ask.url.clone()).await;
    if let Err(f) = &mut first
        && f.code == "connect_failed"
        && let Some(r) = ask.url.strip_prefix("https:")
    {
        let cause = if f.why.is_empty() { String::new() } else { format!("{}+", f.why) };
        match read(format!("http:{r}")).await {
            Ok(raw) if (200..300).contains(&raw.status) => {
                first = Ok(Raw { why: format!("{cause}http:{}", raw.status), ..raw })
            }
            h => {
                f.why = format!("{cause}http:{}", h.map_or_else(|h| h.code.into(), |r| r.status.to_string()));
                f.code = if f.why.starts_with("tls:certificate-") { "tls_failed" } else { f.code };
            }
        }
    }
    let first = first?;
    let Some(to) = refresh(&first) else { return Ok(first) };
    // the site's own move in its HTML is followed once, as a `Location` is; a target that errs or answers no 2xx
    // leaves the refreshing page the page (its host was reached: never thrown away for the target's failure)
    match read(to).await {
        Ok(raw) if (200..300).contains(&raw.status) => Ok(Raw { refreshed_from: Some(first.final_url), ..raw }),
        _ => Ok(first),
    }
}

/// The site's own redirect in its HTML (`plain` follows it once, $0): a 2xx HTML page that shows under
/// `page_chars`, loads no script and holds `<meta http-equiv=refresh content="0; url=…">` to its own origin
/// (scheme, host and port; http → https the one scheme change) (the content parsed as the WHATWG "shared
/// declarative refresh steps": a time of digits, `;` or `,`, an optional `url=`, the URL, quotes off). A delay,
/// a sign, another origin, a script beside it or no URL (`url=` empty reloads the page) is no redirect: a
/// challenge's 360 s `<noscript>` refresh (no URL), a scripted page's no-JS fallback (an "enable JavaScript"
/// page). Only a body holding the bytes `http-equiv` is parsed. Some = the target.
fn refresh(raw: &Raw) -> Option<String> {
    let html = raw.status < 300 && crate::clean::kind_of(&raw.ctype, &raw.body) == "html";
    let marked = html && raw.body.windows(10).any(|w| w.eq_ignore_ascii_case(b"http-equiv"));
    let tree = crate::html::parse(&String::from_utf8_lossy(marked.then_some(&raw.body[..])?));
    let tag = |n: &&crate::html::Node| n.attr("http-equiv").is_some_and(|v| v.eq_ignore_ascii_case("refresh"));
    let (delay, rest) = tree.nodes.iter().find(tag)?.attr("content")?.split_once([';', ','])?;
    let rest = rest.split_once('=').filter(|(k, _)| k.trim().eq_ignore_ascii_case("url")).map_or(rest, |(_, v)| v);
    // both quote marks, the double one escaped: a bare one would pair this file's literals off (`lang` error_table)
    let target = rest.trim().trim_matches(['\x22', '\'']);
    let at = url::Url::parse(&raw.final_url).ok()?;
    let delay = delay.trim();
    let to = at.join(target).ok().filter(|_| !target.is_empty())?;
    let scheme = at.scheme() == to.scheme() || (at.scheme(), to.scheme()) == ("http", "https");
    let origin = scheme && (at.host_str(), at.port()) == (to.host_str(), to.port());
    let alone = !tree.nodes.iter().any(|n| n.tag == "script") && origin;
    let small = crate::fetch::visible(&tree).0.chars().count() < crate::settings::limit("page_chars");
    let zero = !delay.is_empty() && delay.bytes().all(|b| b == b'0');
    (zero && alone && small).then(|| to.into())
}

/// Text is kept as UTF-8: a body with no file magic is decoded from what the header or page declares (else
/// sniffed); a file (PDF, zip) stays bytes. The one rule for every carrier of a page's bytes (plain, Bright Data
/// raw, a local file).
pub fn decode_body(body: Vec<u8>, declared: Option<&str>) -> (Vec<u8>, &'static str) {
    if crate::clean::sniff(&body).is_some() { (body, "") } else { crate::clean::decode(body, declared) }
}

// ---------------------------------------------------------------- private-address fence

/// Not a public unicast address: inside a block of `sites.json` `fence`; IPv4 inside IPv6 (mapped, NAT64,
/// compatible, 6to4) is judged as the IPv4 it carries.
pub fn private_ip(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr::{V4, V6};
    let bits = |ip| match ip {
        V4(a) => (u128::from(a.to_bits()), 32),
        V6(a) => (a.to_bits(), 128),
    };
    let block = |(net, n): (&str, &str)| Some((bits(net.parse().ok()?), n.parse::<u32>().ok()?));
    let (at, width) = bits(ip);
    // inside a block: the same family, and the address's first `n` bits are the block's
    let blocks = crate::fetch::SITES.fence.iter().filter_map(|b| block(b.split_once('/')?));
    let within = || blocks.clone().any(|((net, w), n)| w == width && (at ^ net).checked_shr(w - n.min(w)) == Some(0));
    let V6(a) = ip else { return within() };
    let s = a.segments();
    // an IPv4 inside: mapped `::ffff:a.b.c.d`, NAT64 `64:ff9b::/96`, compatible `::/96`, 6to4 `2002::/16`
    let v4 = |hi: u16, lo: u16| Some(std::net::Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo)));
    let v4 = a.to_ipv4_mapped().or(match s {
        [0x64, 0xff9b, 0, 0, 0, 0, ..] | [0, 0, 0, 0, 0, 0, ..] => v4(s[6], s[7]),
        [0x2002, ..] => v4(s[1], s[2]),
        _ => None,
    });
    v4.map_or_else(within, |v4| private_ip(V4(v4)))
}

/// The URL's own host is private (an IP literal in any WHATWG spelling, or `localhost`); DNS names are judged
/// by `Fence` when they resolve. Always false with `FATS_ALLOW_PRIVATE=1`.
pub fn private_host(url: &str) -> bool {
    !crate::settings::get().allow_private.value && private_name(url)
}

fn private_name(url: &str) -> bool {
    match url::Url::parse(url).ok().as_ref().and_then(url::Url::host) {
        Some(url::Host::Ipv4(a)) => private_ip(a.into()),
        Some(url::Host::Ipv6(a)) => private_ip(a.into()),
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.');
            d == "localhost" || d.ends_with(".localhost")
        }
        None => true,
    }
}

/// THE home-IP brake (plain only, never a provider: they are the other IPs): one plain call to a host at a time
/// and one start per `plain_host_pace_ms`, across processes (a lock file per host under the cache, `pace/<host>`,
/// holding the last start, epoch seconds). A plain call WAITS its turn, keys or not: the brake protects the home IP,
/// it never buys a provider call for a page plain serves free (12 JSON pages of one host: $0 waiting, $0.015
/// skipping). 0 = no brake. It blocks before the call starts, so the wait is no part of the call's ms. Returns (the
/// turn, held while the file is open; the ms waited, `Hop::waited`); a file that will not open or lock = no turn, no
/// wait: the call runs unbraked rather than not at all.
pub fn pace(url: &str) -> (Option<std::fs::File>, u64) {
    use std::io::{Seek, Write};
    let (gap, t0) = (Duration::from_millis(crate::settings::limit("plain_host_pace_ms")), Instant::now());
    let at = crate::settings::get().cache.value.join("pace").join(crate::clean::host_of(url));
    let open = || std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&at);
    let file = at.parent().map(std::fs::create_dir_all).and_then(Result::ok).and_then(|()| open().ok());
    let Some(mut f) = file.filter(|f| !gap.is_zero() && f.lock().is_ok()) else { return (None, 0) };
    // the start is read and written under the lock (a new host = an empty file: lanes that opened it before the
    // first start still wait), never the file's mtime (another clock, which a copy or a sync moves); 20 bytes wide,
    // so each write covers the last whole and a crash mid-write leaves no shorter mix (an older, longer file's
    // tail is never read)
    let last = std::io::read_to_string(&f).ok().and_then(|s| s.get(..20)?.parse::<f64>().ok());
    // a clock stepped back counts as no time gone
    let since = last.and_then(|t| Duration::try_from_secs_f64((crate::cache::now() - t).max(0.0)).ok());
    std::thread::sleep(since.and_then(|s| gap.checked_sub(s)).unwrap_or_default());
    f.rewind().and_then(|()| write!(f, "{:020.6}", crate::cache::now())).ok();
    (Some(f), t0.elapsed().as_millis() as u64)
}

/// The resolver for plain fetches: the system's answer minus private addresses, so a name that resolves (or
/// re-resolves, DNS rebinding) to one never gets a connection.
struct Fence;

#[derive(Debug)]
struct Private;

impl std::fmt::Display for Private {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("resolves to a private address")
    }
}

impl std::error::Error for Private {}

impl wreq::dns::Resolve for Fence {
    fn resolve(&self, name: wreq::dns::Name) -> wreq::dns::Resolving {
        let all = wreq::dns::GaiResolver::new().resolve(name);
        Box::pin(async move {
            let open = crate::settings::get().allow_private.value;
            let keep: Vec<_> = all.await?.filter(|a| open || !private_ip(a.ip())).collect();
            if keep.is_empty() {
                return Err(Box::new(Private) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(keep.into_iter()) as wreq::dns::Addrs)
        })
    }
}

/// Redirects, followed here rather than by the client so every hop's host passes `private_host` (an IP literal
/// in a `Location` never reaches the resolver). `send` = one request → (status, Location, response).
/// Ok = (final URL, response); at most 10 redirects.
async fn follow<R, E: From<&'static str>, F>(url: &str, send: impl Fn(String) -> F) -> Result<(String, R), E>
where
    F: Future<Output = Result<(u16, Option<String>, R), E>>,
{
    let mut at = url.to_string();
    for _ in 0..=10 {
        if private_host(&at) {
            return Err("private_address".into());
        }
        match send(at.clone()).await? {
            (301 | 302 | 303 | 307 | 308, Some(to), _) => {
                at = url::Url::parse(&at).and_then(|u| u.join(&to)).map_err(|_| "net_error")?.into();
            }
            (_, _, r) => return Ok((at, r)),
        }
    }
    Err("redirect_loop".into())
}

/// The body, frame by frame, refused as soon as it passes `cap` bytes: a chunked or lying server never fills
/// memory (Content-Length, when sent, refuses it before the first frame, its size the why).
async fn capped(mut r: wreq::Response, cap: u64) -> Result<Vec<u8>, Fail> {
    use http_body::Body;
    if let Some(n) = r.content_length().filter(|&n| n > cap) {
        return Err(Fail { why: format!("{}B", crate::lang::ksize(n as usize)), ..Fail::from("too_big") });
    }
    let mut v = Vec::new();
    while let Some(f) = std::future::poll_fn(|cx| std::pin::Pin::new(&mut r).poll_frame(cx)).await {
        let Ok(d) = f.map_err(|e| net(&e))?.into_data() else { continue };
        if (v.len() + d.len()) as u64 > cap {
            return Err("too_big".into());
        }
        v.extend_from_slice(&d);
    }
    Ok(v)
}

// ---------------------------------------------------------------- provider APIs: the one table, the one sender

/// Every provider API this binary sends a key to. A call names an `Api`, never a URL string, so a key can only
/// go to a URL in `Providers::apis`: scrape, search, and the account reads `fats account` makes. Account endpoints that
/// return secrets are never listed (Bright Data `GET /zone` and `/zone/passwords` return zone passwords,
/// Webshare `/proxy/list` returns proxy passwords).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Api {
    BdRequest,
    OxyScrape,
    OxySearch,
    /// Account reads (`fats account`): Oxylabs has no balance or usage API for the Web API key; the scraper list
    /// answers 200 or 401, so it is the key check (developers.oxylabs.io products/web-api).
    OxyScrapers,
    /// Bright Data Account Management API (docs.brightdata.com api-reference/account-management-api): balance in
    /// USD, active zones `[{name, type}]`, the cost export (a read, documented as POST), one zone's month.
    BdBalance,
    BdZones,
    BdCosts,
    BdZoneCost,
    /// DNS over HTTPS (`exists`): no key.
    Doh,
    /// GitHub repository search and one account's repositories (`gh_repos`): the GitHub token, or none.
    GhRepos,
    GhUser,
    /// Decodo Web Data API (github.com/Decodo/sdk-ts `src/client.ts`, `Authorization: Bearer <api key>`): pages,
    /// and the key check (`check_call`).
    DecodoScrape,
    /// fats's own endpoints (`telemetry`): the daily update check and the weekly counts; no key.
    Telemetry,
    TelemetryL2,
}

/// A row of `Providers::apis`: the method, the URL, and what an account read gets (`account dry:true`, `{from}`
/// `{to}` = the month). A GET sends the request's fields as the query string.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiRow {
    pub api: Api,
    pub method: &'static str,
    pub url: &'static str,
    pub what: &'static str,
    /// The `accept` header the API asks for (DoH: `application/dns-json`); empty = none.
    #[serde(default)]
    pub accept: &'static str,
}

impl Api {
    pub fn row(self) -> &'static ApiRow {
        &PROVIDERS.apis[self as usize]
    }

    /// The URL a call to `self` goes to: a GET's `req` string fields as the query (`send`, `account dry:true`).
    pub fn url(self, req: &Value) -> String {
        let get = req.as_object().into_iter().flatten().filter(|_| self.row().method == "GET");
        let pairs = get.map(|(k, v)| (k, v.as_str().unwrap_or_default()));
        let q = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
        // a path's `{q}` (GhUser's login, `[A-Za-z0-9-]` only) is the request's `q`, also sent in the query
        let url = crate::lang::draw(self.row().url, &[("q", req["q"].as_str().unwrap_or_default())]);
        if q.is_empty() { url } else { format!("{url}?{q}") }
    }
}

/// A provider's dashboard pages, printed after a `!` row's fix (`→ <link>`); never requested, never sent a key.
/// Only pages the providers' own docs give as a URL (read 2026-10-12);
/// a page the docs reach only by menu path is None, never a guessed URL. In rank order (`fats setup` lists them so,
/// best first), with the setup card's words.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Links {
    pub short: &'static str,
    pub name: &'static str,
    pub signup: &'static str,
    pub mint: &'static str,
    pub keys: Option<&'static str>,
    pub billing: Option<&'static str>,
    pub usage: Option<&'static str>,
    pub rank: &'static str,
    pub free: &'static str,
    pub cost: &'static str,
    pub step: &'static str,
}

/// A provider's pages by any tier name that uses its key (`oxyjs` → Oxylabs).
pub fn links(short: &str) -> Option<&'static Links> {
    let owner = Tier::parse(short).map_or(short, |t| t.owner().spec().short);
    PROVIDERS.links.iter().find(|l| l.short == owner)
}

/// THE page a `!` row about a provider's key or money ends with: `no_key` → sign-up, `bad_key` → the keys
/// page, `low_balance` → billing. None = another code, or the docs give no URL for that page.
pub fn page(code: &str, short: &str) -> Option<String> {
    let l = links(short)?;
    match code {
        "no_key" => Some(l.signup.into()),
        "bad_key" => l.keys.map(String::from),
        "low_balance" => l.billing.map(String::from),
        _ => None,
    }
}

/// `page` for every provider a row names, appended after its fix: ` · bd https://…` ("" when none has one).
pub fn pages<'a>(code: &str, names: impl IntoIterator<Item = &'a str>) -> String {
    names.into_iter().filter_map(|n| page(code, n).map(|p| format!(" · {n} {p}"))).collect()
}

/// A provider API's answer before it is read: HTTP status, headers (lower-case names), body. Headers live only
/// here: `read` takes the few values it needs, nothing stores or prints them.
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|h| h.0 == name).map(|h| h.1.as_str())
    }

    /// Bright Data's refusal: HTTP 200 (also for `format: raw` with an empty body) and the reason in `x-brd-error`
    /// (`x-luminati-error` before the rename), its code in `x-brd-error-code` (observed 2026-10-12).
    fn refused(&self) -> bool {
        self.header("x-brd-error").or(self.header("x-luminati-error")).is_some()
    }

    /// Seconds until a rate-limited call is worth retrying, None = not a rate limit that says so. Oxylabs: a 429
    /// titled `RATE_LIMIT_EXCEEDED` (`retry-after`, else `RETRY_AFTER`). GitHub (docs.github.com rest
    /// "Exceeding the rate limit"): a 403 or 429 with `retry-after`, or with `x-ratelimit-remaining: 0` until
    /// `x-ratelimit-reset` (UTC epoch seconds).
    fn retry_in(&self) -> Option<u64> {
        let num = |name: &str| self.header(name).and_then(|s| s.trim().parse::<u64>().ok());
        let after = num("retry-after").filter(|s| (1..=3600).contains(s));
        let title =
            serde_json::from_slice::<Value>(&self.body).ok().and_then(|j| j["title"].as_str().map(String::from));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let reset = num("x-ratelimit-reset").filter(|_| num("x-ratelimit-remaining") == Some(0));
        let rate = self.status == 429 && title.as_deref() == Some("RATE_LIMIT_EXCEEDED");
        after.or(if rate { Some(RETRY_AFTER) } else { reset.map(|t| t.saturating_sub(now).max(1)) })
    }

    /// The account's own `x-ratelimit-…-limit` / `-remaining` pair (not the `system_` one every client shares).
    fn limit(&self) -> Option<Limit> {
        let num = |end: &str| {
            let mine = |n: &str| n.starts_with("x-ratelimit-") && !n.contains("system_") && n.ends_with(end);
            self.headers.iter().find(|h| mine(&h.0)).and_then(|h| h.1.trim().parse().ok())
        };
        Some(Limit { limit: num("-limit")?, remaining: num("-remaining")? })
    }
}

/// The provider's own reason for its error answer (`Fail::why`, the `!` row's): its status, then what an RFC 9457
/// problem document names (Oxylabs Web API, developers.oxylabs.io/api-reference/errors-and-status-codes): the
/// first field error's pointer and detail (`400 url: value not supported`), else `faulted` when its own retries
/// gave up (`state`) and the title (`500 faulted: request failed after too many retries`). Fields only, never raw
/// text, each masked (`lang::redact`) as DECODED (a key spelled in JSON escapes is a key) before it is lowered:
/// words of `[a-z0-9.,:/#-]` (a masked `***` drops), whole words up to 50 chars (the tier's name before it stays
/// ≤ 60); the bare status when the body names nothing (Oxylabs' 401: empty; HTML). `fetch::settled` reads the
/// status back.
fn reason(status: u16, body: &[u8], secrets: &[String]) -> String {
    let j: Value = serde_json::from_slice(body).unwrap_or_default();
    let text = |v: &Value| crate::lang::redact(v.as_str().unwrap_or_default(), secrets).to_lowercase();
    let e = &j["errors"][0];
    let faulted = if j["state"] == "faulted" { "faulted".into() } else { String::new() };
    let (label, said) = match text(&e["detail"]) {
        d if !d.is_empty() => (text(&e["pointer"]).rsplit('/').next().unwrap_or_default().to_string(), d),
        _ => (faulted, text(&j["title"]).replace('_', " ")),
    };
    let label = if label.is_empty() { label } else { label + ":" };
    said_short(&format!("{status} {label} {said}"))
}

/// Words as a printable reason: those of `[a-z0-9.,:/#-]` only, at most `reason_chars` chars, no trailing `.` or `:`.
fn said_short(words: &str) -> String {
    let ok = |w: &&str| w.bytes().all(|b| b.is_ascii_alphanumeric() || b".,:/#-".contains(&b));
    let (mut n, cap) = (0, crate::settings::limit::<usize>("reason_chars") + 1);
    let fits = words.split_whitespace().filter(ok).take_while(|w| {
        n += w.len() + 1;
        n <= cap
    });
    let fits: Vec<&str> = fits.collect();
    fits.join(" ").trim_end_matches(['.', ':']).to_string()
}

/// A header value as a printable token: `[a-z0-9_]`, at most 32 chars; anything else is dropped (empty).
fn token(v: Option<&str>) -> String {
    let v = v.unwrap_or_default().trim().to_ascii_lowercase();
    let ok =
        !v.is_empty() && v.len() <= 32 && v.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if ok { v } else { String::new() }
}

/// The fake network of tests (tests/redact.rs): (api, key, request) → reply, set once per process. Only
/// in builds with the `fake` feature; the shipped binary has no such hook.
#[cfg(feature = "fake")]
pub type Fake = fn(Api, &str, &Value) -> Reply;
#[cfg(feature = "fake")]
static FAKE: std::sync::OnceLock<Fake> = std::sync::OnceLock::new();

#[cfg(feature = "fake")]
pub fn fake(f: Fake) {
    FAKE.set(f).ok(); // the first fake holds
}

/// A test's fake transport answers (`fake`): it lives in this process only, so a detached fetch runs on a
/// thread here, not in a new process (`cli::detach`).
pub fn in_process() -> bool {
    #[cfg(feature = "fake")]
    if FAKE.get().is_some() {
        return true;
    }
    false
}

/// Call a provider API; Ok = the body of a 200. `cap` = the largest body kept (a JSON-escaped page can be twice
/// its size: fetches pass twice their page cap).
async fn api(to: Api, key: &str, req: Value, cap: u64, wait: Duration) -> Result<Answer, Fail> {
    #[cfg(feature = "fake")]
    if let Some(f) = FAKE.get() {
        // a fake answer may ask to be slow (`x-fake-delay-ms`): the router's cancel and wait paths under test
        let r = f(to, key, &req);
        let ms = r.header("x-fake-delay-ms").and_then(|v| v.parse().ok()).unwrap_or(0);
        tokio::time::sleep(Duration::from_millis(ms).min(wait)).await;
        return if ms > wait.as_millis() as u64 { Err("provider_timeout".into()) } else { read(to, r) };
    }
    read(to, send(to, key, req, cap, wait).await?)
}

/// The only request that carries a key (`bearer_auth`; none when empty), to `to.url()` only: a POST with `req` as
/// JSON, or a GET with `req`'s string fields as the query. Every call names this tool (`user-agent`).
async fn send(to: Api, key: &str, req: Value, cap: u64, wait: Duration) -> Result<Reply, &'static str> {
    let client = wreq::Client::builder().timeout(wait).build().map_err(|_| "net_error")?;
    let r = match to.row().method {
        "GET" => client.get(to.url(&req)),
        _ => client.post(to.row().url).header("content-type", "application/json").body(req.to_string()),
    };
    let r = r.header("user-agent", concat!("fatsearch/", env!("CARGO_PKG_VERSION")));
    let r = if to.row().accept.is_empty() { r } else { r.header("accept", to.row().accept) };
    let r = if key.is_empty() { r } else { r.bearer_auth(key) };
    let r = r.send().await.map_err(|e| net(&e).code)?;
    let status = r.status().as_u16();
    let mut reply = Reply { status, headers: pairs(r.headers()), body: Vec::new() };
    reply.body = match (reply.refused(), status) {
        (false, 200) => capped(r, cap).await.map_err(|f| f.code)?,
        // an error body over the cap is not worth keeping; the code says what happened
        _ => capped(r, crate::settings::limit("err_cap_bytes")).await.unwrap_or_default(),
    };
    Ok(reply)
}

/// A provider API's 200: the body, the request budget (`Reply::limit`), and the target's own status where the API
/// reports it in a header: Bright Data's `/request` answers 200 once its unlocker ran and puts the site's status in
/// `x-brd-status-code` (docs.brightdata.com products/web-unlocker/error-codes, "Direct API").
type Answer = (Vec<u8>, Option<Limit>, Option<u16>);

/// A reply → the 200 body, or the code + the masked body for `error.txt`.
/// A 200 with an empty body is a failure too: no API answers a good call with nothing. A rate limit that says
/// when to retry (`Reply::retry_in`) gets `retry in Ns`; another 429 may be an exhausted quota (Oxylabs' docs give
/// no title for it), so it gets no retry time; another 403 is a refused key.
fn read(to: Api, r: Reply) -> Result<Answer, Fail> {
    let limit = r.limit();
    let target = r.header("x-brd-status-code").and_then(|s| s.trim().parse().ok());
    // our setup: Bright Data's 400 naming the zone we sent (`zone "nozone" not found`:
    // its own APIs, the name quoted, a 200 never decoded), its zone-cost read's 422 `Unknown zone` (live
    // 2026-10-15, naming none)
    let named = |z: &str| match (to, r.status) {
        (Api::BdZoneCost, 422) => true,
        (Api::BdRequest | Api::BdZoneCost, 400) => String::from_utf8_lossy(&r.body).contains(&format!("zone \"{z}\"")),
        _ => false,
    };
    let zone = crate::settings::zone().filter(|z| !z.is_empty() && named(z));
    let (code, why) = match (r.refused(), r.status) {
        (false, 200) if !r.body.is_empty() => return Ok((r.body, limit, target)),
        (false, 200) => ("provider_error", "empty".into()),
        (true, _) => bd_refused(token(r.header("x-brd-error-code")), r.header("x-brd-status-code"), r.retry_in()),
        (_, 400 | 422) if let Some(z) = zone => ("bad_zone", z.into()),
        (_, 403 | 429) if let Some(s) = r.retry_in() => ("rate_limited", format!("retry in {s}s")),
        (_, 401 | 403) => ("bad_key", String::new()),
        (_, 408) => ("provider_timeout", String::new()),
        (_, 429) => ("rate_limited", String::new()),
        _ => ("provider_error", String::new()),
    };
    let secrets = crate::settings::secrets();
    let why = if code == "provider_error" && why.is_empty() { reason(r.status, &r.body, &secrets) } else { why };
    // a JSON answer is read as decoded (a key spelled in `\u00XX` escapes is a key), then masked
    let text = serde_json::from_slice::<Value>(&r.body)
        .map_or_else(|_| String::from_utf8_lossy(&r.body).into(), |j| j.to_string());
    let body = crate::lang::redact(&text, &secrets).into_bytes();
    Err(Fail { code, status: r.status, body, why, limit })
}

/// Bright Data's refusal (`x-brd-error-code`, the `why`) → our code: a rate limit is a wait, not a no
/// (`bucket_rate_limit`, the same eBay URL served 4 min later), with its retry time when sent; a site status it
/// relayed (`http_status` + `x-brd-status-code: 403`: a 0-byte body, the zone counter +0) is the
/// site's block; anything else its own policy (`provider_refused`). None of them is billed (`charged`: no answer).
fn bd_refused(why: String, site: Option<&str>, retry: Option<u64>) -> (&'static str, String) {
    const SITE: [&str; 4] = ["blocked_401", "blocked_403", "blocked_429", "blocked_451"];
    let relayed = site.and_then(|s| SITE.into_iter().find(|c| c[8..] == *s.trim())).filter(|_| why == "http_status");
    match retry {
        _ if !why.contains("rate_limit") => (relayed.unwrap_or("provider_refused"), why),
        Some(s) => ("rate_limited", format!("{why} retry in {s}s")),
        None => ("rate_limited", why),
    }
}

/// Web Unlocker REST: `{zone, url, format: "json", country}` → `{status_code, headers, body}` (the BD CLI's
/// request shape; `country` = lower-case ISO alpha-2, docs.brightdata.com api-reference unlock-website; sent only
/// when one is named, `bd_body`). The page's
/// final URL is `headers["x-unblocker-redirected-to"]` when the site redirected (observed live 2026-10-13 on a
/// wrong product slug; not in the unlock-website docs); absent = the asked URL.
async fn brightdata(ask: &Ask, key: &str, wait: Duration) -> Result<Raw, Fail> {
    let (body, _, target) = api(Api::BdRequest, key, bd_body(ask), 2 * ask.max_bytes, wait).await?;
    if ask.doc {
        // the target's body and status as sent (a failure is a header + empty body, `read`). A page (a `/pdf/`
        // path that was not a file) is decoded to UTF-8 like plain's
        let (body, charset) = decode_body(body, None);
        let status = target.unwrap_or(200);
        return Ok(Raw { status, final_url: ask.url.clone(), body, charset, ..Raw::default() });
    }
    let j: Value = serde_json::from_slice(&body).map_err(|_| "provider_error")?;
    // its own policy refusal inside the answer: `x-brd-error` among the headers beside a `status_code` below 500
    // that is Bright Data's, not the site's (a bad port: 403, empty body; zone counter +0 four times).
    // Beside a 5xx it is an unlock failure (`reject_block`), retried as the 5xx it is (`fetch::FINAL`)
    let inner = |k: &str| j["headers"][k].as_str();
    if inner("x-brd-error").is_some() && j["status_code"].as_u64().is_some_and(|s| s < 500) {
        let why = token(inner("x-brd-err-code").or(inner("x-brd-error-code")));
        let (code, why) = bd_refused(why, inner("x-brd-status-code"), None);
        return Err(Fail { why, ..Fail::from(code) });
    }
    relayed(&j, "body", j["headers"]["x-unblocker-redirected-to"].as_str().unwrap_or(&ask.url))
}

/// A target's answer a provider relayed as JSON, `{status_code, headers, <body>}` (Bright Data `body`, Decodo
/// `content`): no `status_code` = 200; one that is no HTTP status is a broken answer, never truncated (65940 → 404).
fn relayed(j: &Value, body: &str, final_url: &str) -> Result<Raw, Fail> {
    let headers =
        header_lines(j["headers"].as_object().into_iter().flatten().map(|(k, v)| (k, v.as_str().unwrap_or(""))));
    let status = j["status_code"].as_u64().map_or(Ok(200), u16::try_from).map_err(|_| "provider_error")?;
    let body = j[body].as_str().unwrap_or_default().as_bytes().to_vec();
    Ok(Raw { status, final_url: final_url.into(), headers, body, ..Raw::default() })
}

/// The country a page's domain names, upper case: its ccTLD's `Providers::cctld` row (sites that answer best from
/// inside the country; `.uk` → GB); a gTLD names none.
pub fn domain_country(url: &str) -> Option<String> {
    let host = crate::clean::host_of(url);
    PROVIDERS.cctld.get(host.rsplit('.').next().unwrap_or_default()).map(|c| c[..2].to_uppercase())
}

/// `FATS_COUNTRY`, upper case: `US` unless set, the country of the Accept-Language plain sends (`settings`).
pub fn home() -> String {
    crate::settings::get().country.value.to_uppercase()
}

/// THE geo rule (from a replay of saved copies): the country a page call of `t` sends, upper
/// case: the one named (`country:`; an empty one names none), else the domain's (`domain_country`), else `home`:
/// always on Oxylabs, whose unset `location` is a random exit (developers.oxylabs.io products/web-api/scrape; 196
/// saved paid copies: none sent = 18 of 110 in another language, a country sent = 0 of 8; Reddit's captcha
/// came at either exit, 0 of 10), on Bright Data only when it is not the default `US`, else none there: its unset
/// default is the best location for the target (docs.brightdata.com products/web-unlocker features; 2 of 77 saved
/// copies foreign). Plain sends none. A copy in another language is flagged (`fetch::geo`) with a fix that changes
/// the request (`outline::geo_fix`). What the serving call sent is the page's `meta.json` country (`stats` `@FR`).
pub fn sent(ask: &Ask, t: Tier) -> Option<String> {
    let named = ask.country.as_ref().filter(|c| !c.is_empty()).map(|c| c.to_uppercase());
    let home = Some(home()).filter(|h| h != "US" || !t.can("auto_geo"));
    (t != Tier::Plain).then(|| named.or_else(|| domain_country(&ask.url)).or(home)).flatten()
}

/// Web Unlocker `/request` body: `country` only when one is `sent`, `format: raw` for a file's bytes.
fn bd_body(ask: &Ask) -> Value {
    let format = if ask.doc { "raw" } else { "json" };
    let mut req = json!({"zone": crate::settings::zone(), "url": ask.url, "format": format});
    if let Some(c) = sent(ask, Tier::Bd) {
        req["country"] = c.to_lowercase().into();
    }
    req
}

/// Web API `/v1/scrape` request: `{url, output: ["html"], run_js, location, device}` (developers.oxylabs.io
/// products/web-api/scrape: `location` = ISO alpha-2, `sent`, never empty; `device` = `desktop` (default) or
/// `mobile`). `run_js` is always sent: the same page says an omitted `run_js` takes "the optimal execution path",
/// which may render (and bill) JavaScript, and to pass `"run_js": false` for static pages.
fn oxy_body(ask: &Ask) -> Value {
    let mut req = json!({"url": ask.url, "output": ["html"], "run_js": ask.js, "location": sent(ask, Tier::Oxy)});
    if ask.mobile {
        req["device"] = "mobile".into();
    }
    req
}

/// `/v1/scrape` → `results[0].html`; a file (PDF, zip magic) comes back as bytes (`file_bytes`). The JSON cap is
/// 5× the page cap: a file byte can take 5 JSON bytes (`\\x9c`).
async fn oxylabs(ask: &Ask, key: &str, wait: Duration) -> Result<Raw, Fail> {
    let (body, limit, _) = api(Api::OxyScrape, key, oxy_body(ask), 5 * ask.max_bytes, wait).await?;
    let j: Value = serde_json::from_slice(&body).map_err(|_| "provider_error")?;
    let r = &j["results"][0];
    let html = r["html"].as_str().ok_or("provider_error")?.as_bytes();
    // the target's own answer (a 404 page is a 404, not a page): `metadata.statuses.http_request.code`, observed
    // live 2026-10-13, not in the /scrape docs (developers.oxylabs.io products/web-api/scrape.md); absent = 200
    let status = r["metadata"]["statuses"]["http_request"]["code"].as_u64().and_then(|c| u16::try_from(c).ok());
    Ok(Raw {
        status: status.unwrap_or(200),
        final_url: r["metadata"]["url"].as_str().unwrap_or(&ask.url).to_string(),
        body: if crate::clean::sniff(html).is_some() { file_bytes(html) } else { html.to_vec() },
        limit,
        ..Raw::default()
    })
}

/// Web Data API `/v1/scrape`, `universal` target (github.com/Decodo/sdk-ts `src/generated/request-schemas.ts`):
/// `{url, target, geo, headless, device_type}` → `results[0].{content, status_code, url, headers}`. HTML, never
/// `markdown: true`: a Markdown answer skips the content check (walls, shells) and loses the page's own data.
/// `geo` = `sent` as Decodo's own CLI passes it (`--geo de`, github.com/Decodo/cli); `headless: "html"` = the JS
/// rung; `device_type: "mobile"` (Decodo/Web-Scraping-API README).
fn decodo_body(ask: &Ask) -> Value {
    let mut req = json!({"url": ask.url, "target": "universal"});
    if let Some(c) = sent(ask, Tier::Decodo) {
        req["geo"] = c.to_lowercase().into();
    }
    if ask.js {
        req["headless"] = "html".into();
    }
    if ask.mobile {
        req["device_type"] = "mobile".into();
    }
    req
}

async fn decodo(ask: &Ask, key: &str, wait: Duration) -> Result<Raw, Fail> {
    let (body, limit, _) = api(Api::DecodoScrape, key, decodo_body(ask), 2 * ask.max_bytes, wait).await?;
    let j: Value = serde_json::from_slice(&body).map_err(|_| "provider_error")?;
    let r = &j["results"][0];
    r["content"].as_str().ok_or("provider_error")?;
    Ok(Raw { limit, ..relayed(r, "content", r["url"].as_str().unwrap_or(&ask.url))? })
}

/// A file's bytes from Oxylabs' `html` string. It is the file decoded as UTF-8 with every byte that does not
/// decode written as the 4 characters `\xNN` (Python's `backslashreplace`: lower-case hex, and only bytes
/// 0x80–0xff, since every byte below 0x80 decodes). So only `\x[89a-f][0-9a-f]` is an escape; a `\x48` is the
/// file's own text. Measured: arXiv 2405.15793 (4,963,538 B) holds two such runs; reading every `\xNN` as an
/// escape left it 6 bytes short; this rule gives it back byte for byte.
/// Still ambiguous: the file's own text `\x80`–`\xff`; a PDF copy it broke fails `pdf::xref_ok` (`pdf_mangled`), so
/// the ladder goes on to Bright Data's raw bytes.
fn file_bytes(s: &[u8]) -> Vec<u8> {
    let hex = |c: u8| (c as char).to_digit(16).filter(|_| !c.is_ascii_uppercase()).map(|d| d as u8);
    let (mut out, mut i) = (Vec::with_capacity(s.len()), 0);
    while i < s.len() {
        match (&s[i..], s.get(i + 2).copied().and_then(hex), s.get(i + 3).copied().and_then(hex)) {
            ([b'\\', b'x', ..], Some(h @ 8..), Some(l)) => {
                out.push(h << 4 | l);
                i += 4;
            }
            _ => {
                out.push(s[i]);
                i += 1;
            }
        }
    }
    out
}

// ---------------------------------------------------------------- web search

/// One web search result.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Hit {
    pub url: String,
    pub title: String,
    pub snippet: String,
}

/// Tiers that search = the `search` rows (`Spec::can`): Oxylabs Web API `/v1/search`, then Google results through
/// the Bright Data zone. Results per call: 1–20 (`max_results`, Oxylabs search docs); one wait for both.
pub const SEARCH_MAX: usize = 20;
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);

/// THE spend text: `$0.0225`, and `+ ≤$0.0060 unverified` when calls we cut may have been billed.
pub fn bill(cost: f64, upto: f64) -> String {
    if upto > 0.0 { format!("${cost:.4} + ≤${upto:.4} unverified") } else { format!("${cost:.4}") }
}

/// The request for one search: (endpoint, body). Oxylabs (developers.oxylabs.io products/web-api/search):
/// `{query, max_results, location}`, `location` = ISO alpha-2, unknown fields are a 400. Bright Data: a Google
/// URL with `brd_json=1` (parsed JSON, as the BD CLI's `search` sends it) and `gl` for the country, through the
/// `/request` API (`country` = proxy location, BD SERP API reference). Google returns about 10 results a page.
fn search_req(t: Tier, q: &str, limit: usize, country: Option<&str>) -> (Api, Value) {
    let c = search_country(country);
    if t == Tier::Bd {
        let p = [("q", q.to_string()), ("brd_json", "1".into()), ("gl", c.to_lowercase())];
        let url =
            url::Url::parse_with_params("https://www.google.com/search", &p).map(String::from).unwrap_or_default();
        let req = json!({"zone": crate::settings::zone(), "url": url, "format": "json", "country": c.to_lowercase()});
        return (Api::BdRequest, req);
    }
    (Api::OxySearch, json!({"query": q, "max_results": limit.min(SEARCH_MAX), "location": c}))
}

/// The country a search sends, both providers: `country:`, else `home` (never none: Google's `gl`).
pub fn search_country(country: Option<&str>) -> String {
    country.filter(|c| !c.is_empty()).map_or_else(home, str::to_uppercase)
}

/// A search answer → hits, best first. Oxylabs: `results[].{url, title, outline[]}`. Bright Data: `organic[].{link,
/// title, description}`, top level from a SERP zone, inside the `body` string from an unlocker zone.
pub fn hits(t: Tier, body: &[u8], limit: usize) -> Result<Vec<Hit>, &'static str> {
    let j: Value = serde_json::from_slice(body).map_err(|_| "provider_error")?;
    let s = |v: &Value| crate::clean::collapse(v.as_str().unwrap_or_default());
    let (list, url, snip) = match t {
        Tier::Bd => {
            let inner = j["body"].as_str().and_then(|b| serde_json::from_str::<Value>(b).ok()).unwrap_or_default();
            let j = if j["organic"].is_array() { &j } else { &inner };
            (j["organic"].as_array().cloned().ok_or("provider_error")?, "link", "description")
        }
        _ => (j["results"].as_array().cloned().ok_or("provider_error")?, "url", "outline"),
    };
    let hits = list.iter().filter(|r| r[url].is_string()).take(limit);
    Ok(hits
        .map(|r| Hit {
            url: s(&r[url]),
            title: s(&r["title"]),
            snippet: if r[snip].is_array() { s(&r[snip][0]) } else { s(&r[snip]) },
        })
        .collect())
}

/// One search call. `billed` as in `get`.
pub async fn search(
    t: Tier,
    q: &str,
    limit: usize,
    country: Option<&str>,
    wait: Duration,
) -> (Result<(Vec<Hit>, Option<Limit>), Fail>, bool) {
    let key = t.key().unwrap_or_default();
    let (to, req) = search_req(t, q, limit, country);
    let r = api(to, &key, req, 2 * crate::cli::MAX_BYTES, wait).await;
    let r = r.and_then(|(body, lim, _)| hits(t, &body, limit).map(|h| (h, lim)).map_err(Fail::from));
    // an answer without a result list is bd's failed SERP: the zone counter +0 for 2
    let billed = charged(t, r.as_ref().map(|_| 200));
    (r, billed)
}

/// `fats gh <words>`: GitHub REST `GET /search/repositories` sorted by stars (docs.github.com rest/search "Search
/// repositories": `q`, `sort=stars`, `order`, `per_page`), free: 30 searches a minute with a token, 10 without.
/// A login-shaped word (`[A-Za-z0-9-]`, ≤ 39: GitHub's username rule) also lists that account's repos, at once,
/// its top 100 by stars (`q=user:<login>`, one more search; `GET /users/{q}/repos` holds only its 100 last pushed,
/// so a ★71 repo of an account with 164 was missing), that list only for an account the search leaves out (422
/// with or without a token, seen live on a real account) or none (then 404): a login's own repos rarely bear it in
/// their text (0 hits, or repos about the person). The lists merge by stars, so a
/// squatter account of a topic word (★0 repos) stays below the topic's hits. A repository → a hit: its page,
/// `★12.4k owner/repo`, `description (80 chars) · pushed YYYY-MM-DD`. Ok = the hits + each call's code and `=` line
/// note (the search's, then the account's: ` · no user <q>`, ` · user lookup: rate_limited retry in 60s`); the
/// account's failure with no hit at all is the answer's (`! rate_limited gh user <q> …`), never `0 results`.
pub async fn gh_repos(q: String, limit: usize) -> Result<(Vec<Hit>, Vec<(&'static str, String)>), Fail> {
    let (key, per) = (crate::settings::key("gh").unwrap_or_default().0, limit.min(crate::settings::limit("gh_max")));
    let call = |to: Api, req: Value| api(to, &key, req, 4 << 20, SEARCH_TIMEOUT);
    let login = q.len() <= 39 && !q.contains(|c: char| !c.is_ascii_alphanumeric() && c != '-');
    let user = login.then_some(async {
        let by_stars = json!({"q": format!("user:{q}"), "sort": "stars", "order": "desc", "per_page": "100"});
        match call(Api::GhRepos, by_stars).await {
            Err(f) if f.status == 422 => call(Api::GhUser, json!({"q": q, "sort": "pushed", "per_page": "100"})).await,
            r => r,
        }
    });
    let req = json!({"q": q, "sort": "stars", "order": "desc", "per_page": per.to_string()});
    let (found, user) = tokio::join!(call(Api::GhRepos, req), async { Some(user?.await) });
    let items =
        |r: Result<Answer, Fail>| serde_json::from_slice::<Value>(&r?.0).map_err(|_| Fail::from("provider_error"));
    let items = |r| items(r).map(|j| j["items"].as_array().or(j.as_array()).cloned().unwrap_or_default());
    let mut repos = items(found)?;
    // the account call's hop (`cli::github`): its code and the `=` line's note when it got no list
    let user = match user.map(items) {
        Some(Err(f)) if f.status == 404 => Some(("not_found", format!(" · no user {q}"))),
        Some(Err(f)) if repos.is_empty() => return Err(Fail { why: format!("user {q} {}", f.why), ..f }),
        Some(Err(f)) => Some((f.code, format!(" · user lookup: {} {}", f.code, f.why).trim_end().into())),
        theirs => theirs.map(|r| repos.extend(r.unwrap_or_default())).map(|()| ("ok", String::new())),
    };
    repos.sort_by_key(|r| std::cmp::Reverse(r["stargazers_count"].as_u64()));
    let s = |v: &Value| crate::clean::collapse(v.as_str().unwrap_or_default());
    let hit = |r: &Value| {
        let stars = crate::lang::ksize(r["stargazers_count"].as_u64().unwrap_or(0) as usize);
        let pushed = format!("pushed {}", crate::lang::cut(&s(&r["pushed_at"]), 10));
        let about: Vec<String> =
            [crate::lang::short(&s(&r["description"]), 80), pushed].into_iter().filter(|x| !x.is_empty()).collect();
        Hit { url: s(&r["html_url"]), title: format!("★{stars} {}", s(&r["full_name"])), snippet: about.join(" · ") }
    };
    let on_github =
        |h: &Hit| url::Url::parse(&h.url).is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("github.com"));
    let hits = crate::lang::uniq(repos.iter().map(hit).filter(on_github), |h| h.url.clone());
    Ok((hits.into_iter().take(per).collect(), [("ok", String::new())].into_iter().chain(user).collect()))
}

// ---------------------------------------------------------------- account (`fats account`)

/// The providers `fats account` reads, in row order (Decodo documents no usage read for its API key).
pub const ACCOUNTS: [Tier; 2] = [Tier::Oxy, Tier::Bd];
/// Every provider with a key, in the order `fats setup` and the status line name them.
pub const KEYS: [Tier; 3] = [Tier::Oxy, Tier::Bd, Tier::Decodo];
/// Per call; the account APIs answered in 0.1–0.5 s (measured 2026-10-12).
pub const ACCOUNT_TIMEOUT: Duration = Duration::from_secs(10);
/// The largest account answer kept (the scraper list is ~14 KB).
const ACCOUNT_CAP: u64 = 1 << 20;

/// One provider's account as read from its answers (never the answers themselves). None = not offered or not read.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Account {
    /// `oxy`, `bd`.
    pub provider: String,
    /// `ok`, or the failed call's code (`bad_key`, `rate_limited` …); `why` = its printable token.
    pub code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub why: String,
    pub ms: u64,
    pub calls: usize,
    pub scrapers: Option<usize>,
    /// USD.
    pub balance: Option<f64>,
    /// USD this calendar month (UTC).
    pub spend: Option<f64>,
    pub reqs_unblocker: Option<u64>,
    pub reqs_serp: Option<u64>,
    /// USD this month on the zone fats calls (`BRIGHTDATA_UNLOCKER_ZONE`): what `stats` reconciles against.
    pub zone_cost: Option<f64>,
    pub zones: Vec<String>,
    /// The reads a scoped key may not make (403 on that one only), by endpoint (`stats::READS` names them).
    pub denied: Vec<Api>,
}

/// The calls one provider's account takes, in order, with their request fields. `month` = (its first day, the
/// next month's first day), `YYYY-MM-DD` UTC (Bright Data: `to` is exclusive). The zone's month needs the
/// configured zone and is skipped without one.
pub fn account_calls(t: Tier, month: (&str, &str), zone: Option<&str>) -> Vec<(Api, Value)> {
    let (from, to) = month;
    match t {
        Tier::Bd => {
            let mut v = vec![
                (Api::BdBalance, json!({})),
                (Api::BdZones, json!({})),
                (Api::BdCosts, json!({"dimension": "products", "filters": {}, "from": from, "to": to})),
            ];
            v.extend(zone.map(|z| (Api::BdZoneCost, json!({"from": from, "to": to, "zone": z}))));
            v
        }
        _ => vec![(Api::OxyScrapers, json!({}))],
    }
}

/// One account answer into `a`. Bright Data's cost export has a key per day **and** a `total` key: the month's
/// spend is the sum of `total` only (summing every key counts each day twice). A zone's month is keyed by
/// customer, then plan: requests are summed over the plans.
pub fn account_read(a: &mut Account, to: Api, body: &[u8]) {
    let j: Value = serde_json::from_slice(body).unwrap_or_default();
    match to {
        Api::OxyScrapers => a.scrapers = j["scrapers"].as_array().map(Vec::len),
        Api::BdBalance => a.balance = j["balance"].as_f64(),
        Api::BdZones => {
            let z = j.as_array().into_iter().flatten().filter_map(|z| z["type"].as_str().map(String::from));
            a.zones = z.collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        }
        Api::BdCosts => a.spend = j["total"].as_object().map(|t| spend(t.values().filter_map(Value::as_f64))),
        Api::BdZoneCost => {
            let customers = j.as_object().into_iter().flat_map(|c| c.values());
            let plans: Vec<&Value> = customers.filter_map(Value::as_object).flat_map(|p| p.values()).collect();
            let sum = |k: &str| plans.iter().map(|p| p[k].as_u64()).sum::<Option<u64>>();
            let n = |k: &str| sum(k).filter(|_| !plans.is_empty());
            (a.reqs_unblocker, a.reqs_serp) = (n("reqs_unblocker"), n("reqs_serp"));
            a.zone_cost = plans.iter().map(|p| p["cost"].as_f64()).sum::<Option<f64>>().filter(|_| !plans.is_empty());
        }
        _ => {}
    }
}

/// Read one provider's account: its calls in order; the first failure stops the rest (a refused key fails them
/// all). A 403 on one read is a key scoped without it (Bright Data's per-permission keys: `denied`, the rest
/// read on); a 401, or a 403 on every read, is a bad key. Answers are parsed and dropped; an error body is never
/// kept.
pub async fn account(t: Tier, key: String, month: (String, String), zone: Option<&str>) -> Account {
    let t0 = std::time::Instant::now();
    let mut a = Account { provider: t.spec().short.into(), code: "ok".into(), ..Account::default() };
    for (to, req) in account_calls(t, (&month.0, &month.1), zone) {
        a.calls += 1;
        match api(to, &key, req, ACCOUNT_CAP, ACCOUNT_TIMEOUT).await {
            Ok((body, ..)) => account_read(&mut a, to, &body),
            Err(f) if f.status == 403 && f.code == "bad_key" => {
                a.denied.push(to);
            }
            Err(f) => {
                (a.code, a.why) = (f.code.into(), f.why);
                break;
            }
        }
    }
    if a.denied.len() == a.calls {
        a.code = "bad_key".into();
    }
    a.ms = t0.elapsed().as_millis() as u64;
    a
}

/// A target that does not exist: Decodo's key check scrapes it (`check_call`); a key is never sent there.
pub const NO_HOST: &str = "https://does-not-exist.decodo.com";

/// `fats setup`'s free key check of `t`'s key: Oxylabs the scraper list, Bright Data the zone list (names and types,
/// never an endpoint with a zone password), Decodo a `universal` scrape of `NO_HOST`, its CLI's own check
/// (github.com/Decodo/cli `src/scrape/services/auth-validation.ts`): 401 = a bad key, a 400 = a good one.
pub fn check_call(t: Tier) -> (Api, Value) {
    match t.owner() {
        Tier::Bd => (Api::BdZones, json!({})),
        Tier::Decodo => (Api::DecodoScrape, json!({"url": NO_HOST, "target": "universal"})),
        _ => (Api::OxyScrapers, json!({})),
    }
}

/// `t`'s key check (`check_call`): what it shows (Oxylabs' scraper count, else `key ok`) and Bright Data's Web
/// Unlocker zones in the list's order (its rows carry `name` and `type` only); Decodo's 400 on `NO_HOST` = a good key.
pub async fn check_key(t: Tier, key: String) -> Result<(String, Vec<String>), Fail> {
    let (to, req) = check_call(t);
    let j: Value = match api(to, &key, req, ACCOUNT_CAP, ACCOUNT_TIMEOUT).await {
        Err(f) if to == Api::DecodoScrape && f.status == 400 => json!({}),
        r => serde_json::from_slice(&r?.0).unwrap_or_default(),
    };
    let zones = j.as_array().into_iter().flatten().filter(|z| z["type"] == "unblocker");
    let what = j["scrapers"].as_array().map_or("key ok".into(), |s| format!("{} scrapers", s.len()));
    Ok((what, zones.filter_map(|z| z["name"].as_str().map(String::from)).collect()))
}

/// Safari 26 on macOS (TLS ClientHello, HTTP/2 settings, navigation headers, as maintained by wreq-util). Measured
/// on 121 test URLs, one run each: Safari 26 passed 85, Chrome 149 73 (Cloudflare challenged it more).
fn browser() -> wreq_util::Emulation {
    wreq_util::Emulation::builder().profile(wreq_util::Profile::Safari26).platform(wreq_util::Platform::MacOS).build()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE home-IP brake: a second plain call to a host waits while the first holds its turn, and until
    /// `plain_host_pace_ms` after its start (unit tests run with every key: a key never skips the wait).
    #[test]
    fn plain_calls_to_one_host_are_paced() {
        let url = format!("https://pace-{}.example/a", std::process::id());
        let (first, waited) = pace(&url);
        assert!(first.is_some() && waited < 500, "the first call runs at once: {waited} ms");
        let second = std::thread::spawn(move || pace(&url).1);
        std::thread::sleep(Duration::from_millis(300));
        drop(first);
        let waited = second.join().expect("second call");
        assert!((600..2000).contains(&waited), "the held turn, then 1 s from the first start: {waited} ms");
        // the start is written over the last one, fixed-width: a longer old file's tail is never read
        let other = format!("https://pace-tail-{}.example/", std::process::id());
        let at = crate::settings::get().cache.value.join("pace").join(crate::clean::host_of(&other));
        std::fs::create_dir_all(at.parent().expect("pace dir")).ok();
        std::fs::write(&at, format!("1.0{}", "x".repeat(40))).ok();
        drop(pace(&other));
        let kept = std::fs::read_to_string(&at).unwrap_or_default();
        assert!(kept.get(..20).and_then(|k| k.parse::<f64>().ok()).is_some_and(|t| t > 1e9), "one start: {kept}");
        assert_eq!(kept.len(), 43, "written over, never cut or grown");
    }

    /// THE billing rule (bd's zone counter): an answer below 500 is billed, a 5xx (bd's
    /// `http_502`), a timeout (a search's too: counter +0 for 2) or a refusal is not, for both providers.
    #[test]
    fn charged_only_on_a_charged_reply() {
        let (late, err) = (Fail::from("provider_timeout"), Fail::from("provider_error"));
        for t in [Tier::Oxy, Tier::OxyJs, Tier::Bd] {
            let fetch = |s| charged(t, s);
            assert!(fetch(Ok(200)) && fetch(Ok(404)), "{t:?}: an answer");
            // Oxylabs' finished job relaying the site's 502 is billed; Bright Data's inner 5xx is not
            assert_eq!(fetch(Ok(502)), t != Tier::Bd, "{t:?}: a site's 5xx");
            assert!(!fetch(Err(&late)) && !fetch(Err(&err)), "{t:?}: a timeout or refusal is free");
        }
        assert!(!charged(Tier::Plain, Ok(200)), "plain is free");
    }

    /// The Oxylabs request always says `run_js` and a `location` (`US` where nothing names one: an unset one is a
    /// random exit), and carries `device` only for `device:mobile` (desktop is the API's default).
    #[test]
    fn oxy_request_body() {
        let ask = Ask { url: "https://ex.com/a".into(), ..Ask::default() };
        let want = json!({"url": "https://ex.com/a", "output": ["html"], "run_js": false, "location": "US"});
        assert_eq!(oxy_body(&ask), want);
        let ask = Ask { url: "https://ex.com/a".into(), js: true, mobile: true, ..Ask::default() };
        assert_eq!(
            oxy_body(&ask),
            json!({"url": "https://ex.com/a", "output": ["html"], "run_js": true, "location": "US", "device": "mobile"})
        );
        assert_eq!(oxy_body(&Ask { url: "https://www.lemonde.fr/".into(), ..Ask::default() })["location"], "FR");
    }

    /// Oxylabs' escaped file → the file: only lower-case `\x80`–`\xff` are escapes; the file's own `\x48`, an
    /// upper-case `\x9C`, a cut `\x9` and UTF-8 stay as they are (the arXiv case: two literal runs, 0 bytes lost).
    #[test]
    fn oxylabs_file_bytes() {
        let file: &[u8] = b"%PDF-1.5 \x9c\xff\\x48 \xc3\xa9 \\x9C \\x9";
        let escaped = r"%PDF-1.5 \x9c\xff\x48 é \x9C \x9";
        assert_eq!(file_bytes(escaped.as_bytes()), file);
    }

    /// A key goes only to `Providers::apis`: scrape, search and account reads on the two provider API hosts, never an
    /// endpoint that returns secrets (Bright Data `GET /zone`, `/zone/passwords`; Webshare `/proxy/list`), and a
    /// POST only where the docs make it a query. `send` is the only request with a key, and no provider URL is
    /// written outside `apis` and `links` in providers.json (pages printed for people, never requested): a new one is a
    /// visible row.
    #[test]
    fn keys_go_only_to_listed_endpoints() {
        // 1.1.1.1 (DoH) and fatsearch.com (telemetry, `ours`) are sent no key: an empty one, which `send` never puts
        // in a header
        const HOSTS: &[&str] = &[
            "api.brightdata.com", "webapi.oxylabs.io", "1.1.1.1", "api.github.com", "data.decodo.com", "fatsearch.com",
        ];
        const NEVER: &[&str] = &["password", "/proxy/list", "/billing", "/account", "/users"];
        const POST: &[&str] = &["/request", "/scrape", "/search", "/costs/export/json", "/v1/l2"];
        for (i, ApiRow { api, method, url, .. }) in PROVIDERS.apis.iter().enumerate() {
            assert_eq!(*api as usize, i, "apis is indexed by Api");
            let u = url::Url::parse(url).expect("url");
            assert_eq!(u.scheme(), "https", "{url}");
            assert!(HOSTS.contains(&u.host_str().unwrap_or_default()), "{url}: unlisted host");
            let p = u.path();
            // GitHub's `/users/{login}/repos` lists an account's public repositories, nothing of the caller's
            let public = u.host_str() == Some("api.github.com") && p.ends_with("/repos");
            assert!(p != "/zone" && (public || NEVER.iter().all(|n| !p.contains(n))), "{url}: returns secrets");
            assert!(*method == "GET" || (*method == "POST" && POST.iter().any(|e| p.ends_with(e))), "{url}");
        }
        let links: Vec<&str> = PROVIDERS
            .links
            .iter()
            .flat_map(|l| [Some(l.signup), Some(l.mint), l.keys, l.billing, l.usage])
            .flatten()
            .collect();
        assert!(links.iter().all(|l| l.starts_with("https://")));
        let src: String = [
            include_str!("provider.rs"),
            include_str!("fetch.rs"),
            include_str!("cli.rs"),
            include_str!("stats.rs"),
            include_str!("settings.rs"),
            include_str!("mcp.rs"),
            include_str!("data/providers.json"),
        ]
        .iter()
        .map(|f| f.split("#[cfg(test)]").next().unwrap_or_default())
        .collect();
        assert_eq!(src.matches("bearer_auth(").count(), 1, "one request carries a key");
        assert!(!src.contains("basic_auth(") && !src.to_ascii_lowercase().contains("\"authorization\""));
        let listed: Vec<&str> = PROVIDERS.apis.iter().map(|a| a.url).chain(links).collect();
        let provider = [
            "oxylabs.io", "brightdata.com", "luminati", "webshare.io", "decodo.com", "1.1.1.1", "github.com",
            "fatsearch.com",
        ];
        let lits: Vec<String> =
            src.split("\"http").skip(1).filter_map(|r| r.split('"').next()).map(|r| format!("http{r}")).collect();
        // Decodo's key check target is a page asked for, no place a key goes
        let found: Vec<&String> =
            lits.iter().filter(|l| *l != NO_HOST && provider.iter().any(|h| l.contains(h))).collect();
        assert_eq!(found.len(), listed.len(), "each provider URL is written once, in apis or links: {found:?}");
        assert!(found.iter().all(|l| listed.contains(&l.as_str())), "provider URL outside apis/links: {found:?}");
    }

    /// A provider's answer → a code; an error body is kept for `error.txt`, the 200 body is the answer.
    #[test]
    fn replies_become_codes() {
        let r = |status, refused: bool, body: &str| {
            let h = [
                ("x-brd-error", "Access denied: x.gov is classified as Government"),
                ("x-brd-error-code", "proxy_error"),
            ];
            let headers = h.iter().filter(|_| refused).map(|(k, v)| (k.to_string(), v.to_string())).collect();
            read(Api::BdRequest, Reply { status, headers, body: body.into() })
        };
        assert_eq!(r(200, false, "{}"), Ok((b"{}".to_vec(), None, None)));
        let raw = Reply { status: 200, headers: vec![("x-brd-status-code".into(), "404".into())], body: b"<p>".into() };
        assert_eq!(read(Api::BdRequest, raw), Ok((b"<p>".to_vec(), None, Some(404))), "Bright Data's target status");
        let empty = r(200, false, "").expect_err("200 + empty body");
        assert_eq!((empty.code, empty.why.as_str()), ("provider_error", "empty"));
        let gov = r(200, true, "").expect_err("Bright Data refusal");
        assert_eq!(
            (gov.code, gov.why.as_str()),
            ("provider_refused", "proxy_error"),
            "the code header, not the message"
        );
        // A status bd relayed is the site's block, a rate limit a wait; neither is bd's policy
        let brd = |code: &str, site: &str| {
            let h = [("x-brd-error", "rejected"), ("x-brd-error-code", code), ("x-brd-status-code", site)];
            let f = read(
                Api::BdRequest,
                Reply { status: 200, headers: h.map(|(k, v)| (k.into(), v.into())).to_vec(), body: vec![] },
            );
            f.map_err(|f| (f.code, f.why)).expect_err("refused")
        };
        assert_eq!(brd("http_status", "403"), ("blocked_403", "http_status".into()), "a relayed site 403");
        assert_eq!(brd("http_status", "502").0, "provider_refused", "no site block row");
        assert_eq!(brd("bucket_rate_limit", "").0, "rate_limited", "a rate limit");
        assert_eq!(bd_refused("bucket_rate_limit".into(), None, Some(30)).1, "bucket_rate_limit retry in 30s");
        assert!(!charged(Tier::Bd, Err(&Fail::from("blocked_403"))), "a relayed status is no answer: not billed");
        assert_eq!(token(Some("Access denied: x.gov")), "", "free text is never a token");
        assert_eq!(token(Some(" Proxy_Error ")), "proxy_error");
        let f = r(401, false, "Invalid api key").expect_err("401");
        assert_eq!((f.code, f.status, f.body.as_slice()), ("bad_key", 401, &b"Invalid api key"[..]));
        assert_eq!(r(403, false, "").map_err(|f| f.code), Err("bad_key"));
        assert_eq!(r(408, false, "").map_err(|f| f.code), Err("provider_timeout"));
        assert_eq!(r(429, false, "").map_err(|f| (f.code, f.why)), Err(("rate_limited", String::new())), "quota?");
        let acct = |h: &[(&str, &str)]| Reply {
            status: 429,
            headers: h.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            body: br#"{"status":429,"title":"RATE_LIMIT_EXCEEDED","detail":"Too many requests."}"#.to_vec(),
        };
        let h = [
            ("x-ratelimit-search_total_dynamic_europe-1234567-limit", "10"),
            ("x-ratelimit-search_total_dynamic_europe-1234567-remaining", "0"),
            ("x-ratelimit-system_search_total_dynamic_europe-remaining", "9999"),
        ];
        let f = read(Api::OxyScrape, acct(&h)).expect_err("429");
        assert_eq!((f.why.as_str(), f.limit), ("retry in 5s", Some(Limit { limit: 10, remaining: 0 })));
        let f = read(Api::OxyScrape, acct(&[("retry-after", "7")])).expect_err("429");
        assert_eq!((f.why.as_str(), f.limit), ("retry in 7s", None), "retry-after wins; no budget sent");
        assert_eq!(
            read(Api::OxyScrape, acct(&[("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT")])).map_err(|f| f.why),
            Err("retry in 5s".into())
        );
        assert_eq!(r(500, false, "<html>oops").map_err(|f| (f.code, f.why)), Err(("provider_error", "500".into())));
    }

    /// The provider's own reason (`reason`): an RFC 9457 field error, its faulted title; fields only, ≤ 50 chars.
    #[test]
    fn a_refusal_keeps_its_reason() {
        let why = |status, body: &str| {
            let r = Reply { status, headers: vec![], body: body.into() };
            read(Api::OxyScrape, r).map_err(|f| f.why).expect_err("refused")
        };
        let url =
            r##"{"title":"INVALID_BODY_PROPERTY","errors":[{"detail":"Value not supported.","pointer":"#/url"}]}"##;
        assert_eq!(why(400, url), "400 url: value not supported");
        let faulted =
            r#"{"status":500,"state":"faulted","title":"REQUEST_FAILED_AFTER_TOO_MANY_RETRIES","detail":"x"}"#;
        assert_eq!(why(500, faulted), "500 faulted: request failed after too many retries");
        let long = r#"{"title":"A_B","errors":[{"detail":"key=*** <b>is</b> wrong and this sentence goes on far too long to fit"}]}"#;
        assert_eq!(why(400, long), "400 wrong and this sentence goes on far too long", "masked and markup words drop");
        // a key in JSON escapes is masked once decoded, before it is lowered: no run of it in the reason
        let key = ["Ab3dEf6hIj9kLm2nOp".to_string()];
        let escaped: String = key[0].chars().map(|c| format!("\\u{:04x}", c as u32)).collect();
        let body = format!(r##"{{"errors":[{{"detail":"key {escaped} refused","pointer":"#/key"}}]}}"##);
        assert_eq!(reason(400, body.as_bytes(), &key), "400 key: key refused");
    }

    /// Search requests carry only the documented fields; the parser reads both providers' saved answers.
    #[test]
    fn search_requests_and_answers() {
        let (u, r) = search_req(Tier::Oxy, "heidelberg university", 50, Some("fr"));
        assert_eq!(u.row().url, "https://webapi.oxylabs.io/v1/search");
        assert_eq!(r, json!({"query": "heidelberg university", "max_results": 20, "location": "FR"}));
        assert_eq!(search_req(Tier::Oxy, "a", 10, None).1, json!({"query": "a", "max_results": 10, "location": "US"}));
        let none = search_req(Tier::Bd, "a", 10, None).1;
        assert_eq!(
            (none["country"].as_str(), none["url"].as_str()),
            (Some("us"), Some("https://www.google.com/search?q=a&brd_json=1&gl=us"))
        );
        let (u, r) = search_req(Tier::Bd, "a b&c", 10, Some("fr"));
        assert_eq!(u.row().url, "https://api.brightdata.com/request");
        assert_eq!(r["url"], "https://www.google.com/search?q=a+b%26c&brd_json=1&gl=fr");
        assert_eq!((r["format"].as_str(), r["country"].as_str()), (Some("json"), Some("fr")));
        let oxy = hits(Tier::Oxy, include_bytes!("../tests/fixtures/oxy-search.json"), 10).expect("oxy fixture");
        assert_eq!(oxy.len(), 5);
        assert_eq!(
            (oxy[0].url.as_str(), oxy[0].title.as_str()),
            ("https://www.uni-heidelberg.de/en", "Heidelberg University")
        );
        assert!(oxy[0].snippet.starts_with("Heidelberg University – the oldest in Germany"));
        assert_eq!(hits(Tier::Oxy, include_bytes!("../tests/fixtures/oxy-search.json"), 2).map(|h| h.len()), Ok(2));
        let bd = hits(Tier::Bd, include_bytes!("../tests/fixtures/bd-serp.json"), 10).expect("bd fixture");
        assert_eq!(bd.len(), 3);
        assert_eq!(bd[0].url, "https://pizza.example.com/delivery-springfield");
        assert!(bd[0].snippet.starts_with("Example Pizzeria offers fast pizza delivery"));
        assert_eq!(hits(Tier::Oxy, b"<html>", 10), Err("provider_error"));
        assert_eq!(hits(Tier::Oxy, br#"{"state":"done","results":[]}"#, 10), Ok(vec![]));
    }

    /// Account answers in the providers' shapes (made-up numbers) → the fields; BD spend = the `total` key only
    /// (summing every key gives 11.00: each day counted twice).
    #[test]
    fn account_answers() {
        let mut a = Account::default();
        account_read(&mut a, Api::OxyScrapers, include_bytes!("../tests/fixtures/account-oxy-scrapers.json"));
        assert_eq!(a.scrapers, Some(351));
        account_read(&mut a, Api::BdBalance, include_bytes!("../tests/fixtures/account-bd-balance.json"));
        account_read(&mut a, Api::BdZones, include_bytes!("../tests/fixtures/account-bd-zones.json"));
        account_read(&mut a, Api::BdCosts, include_bytes!("../tests/fixtures/account-bd-costs.json"));
        account_read(&mut a, Api::BdZoneCost, include_bytes!("../tests/fixtures/account-bd-zone-cost.json"));
        assert_eq!((a.balance, format!("{:.2}", a.spend.unwrap_or_default())), (Some(1234.56), "5.50".into()));
        assert_eq!((a.reqs_unblocker, a.reqs_serp, a.zone_cost), (Some(1500), Some(700), Some(3.21)));
        let all: Value = serde_json::from_slice(include_bytes!("../tests/fixtures/account-bd-costs.json")).unwrap();
        let every: f64 = all
            .as_object()
            .unwrap()
            .values()
            .flat_map(|d| d.as_object().unwrap().values())
            .filter_map(Value::as_f64)
            .sum();
        assert_eq!(format!("{every:.2}"), "11.00", "the double count the `total` rule avoids");
        assert_eq!(a.zones, ["dc", "serp", "unblocker"], "zone types, sorted, each once");
        let mut b = Account::default();
        account_read(&mut b, Api::BdZoneCost, b"{}");
        account_read(&mut b, Api::BdBalance, b"<html>");
        assert_eq!((b.reqs_unblocker, b.balance, b.zone_cost), (None, None, None), "no answer = n/a, never 0");
        let calls = |z| account_calls(Tier::Bd, ("2026-10-01", "2026-11-01"), z);
        assert_eq!(calls(None).len(), 3, "no zone, no zone month");
        let all = calls(Some("unblocker"));
        assert_eq!(all[3].1, json!({"zone": "unblocker", "from": "2026-10-01", "to": "2026-11-01"}));
        assert_eq!(account_calls(Tier::Oxy, ("", ""), None)[0].0.row().url, "https://webapi.oxylabs.io/v1/scrapers");
    }

    /// `!` row pages: documented URLs only (the Oxylabs keys page is not), by any tier name of the key.
    #[test]
    fn row_pages() {
        assert_eq!(
            pages("no_key", ["oxyjs", "bd"]),
            " · oxyjs https://dashboard.oxylabs.io/en/registration · bd https://brightdata.com/cp/start"
        );
        assert_eq!(pages("bad_key", "oxy bd".split(' ')), format!("{K} · bd https://brightdata.com/cp/setting/users"));
        // developers.oxylabs.io/products/web-api/authentication "Getting a key": only a Web API key works
        const K: &str = " · oxy https://dashboard.oxylabs.io (a Web API instance's key)";
        assert_eq!(pages("bad_key", ["oxy"]), K);
        assert_eq!(pages("provider_error", ["bd"]), "", "only key and money codes");
        assert_eq!(page("low_balance", "bd").as_deref(), Some("https://brightdata.com/cp/billing"));
        assert_eq!(page("no_key", "plain"), None);
    }

    /// ccTLD → country for both providers; `country:` wins; a gTLD names `home` on Oxylabs, none on Bright Data;
    /// plain never has one.
    #[test]
    fn country_from_tld() {
        let at = |url: &str, c: Option<&str>, t: Tier| {
            sent(&Ask { url: url.into(), country: c.map(String::from), ..Ask::default() }, t)
        };
        for (url, want) in [
            ("https://www.lemonde.fr/a", Some("FR")),
            ("https://www.spiegel.de/", Some("DE")),
            ("https://www.bbc.co.uk/news", Some("GB")),
            ("https://www.idealo.fr/x", Some("FR")),
            ("https://ex.gr/", Some("GR")),
            ("https://example.com/", None),
            ("https://europa.eu/", None),
            ("https://crates.io/", None),
            ("https://ex.us/", None),
        ] {
            assert_eq!(at(url, None, Tier::Bd).as_deref(), want, "{url}");
            for t in [Tier::Oxy, Tier::OxyJs] {
                assert_eq!(at(url, None, t).as_deref(), want.or(Some("US")), "{url} {t:?}");
            }
        }
        assert_eq!(at("https://www.lemonde.fr/", Some("de"), Tier::Oxy).as_deref(), Some("DE"), "country: wins");
        assert_eq!(at("https://ex.com/", Some("pl"), Tier::Bd).as_deref(), Some("PL"));
        assert_eq!(at("https://www.lemonde.fr/", Some("de"), Tier::Plain), None);
    }

    /// An Oxylabs page call always names a `location`
    /// (unset = a random exit: 18 of 110 saved copies foreign); a Bright Data page names a country only when one is
    /// named or the domain gives one (unset = its best location for the target).
    #[test]
    fn oxylabs_always_names_a_location_bd_only_a_named_one() {
        let ask = |url: &str, c: Option<&str>| Ask { url: url.into(), country: c.map(String::from), ..Ask::default() };
        for (url, c, oxy, bd) in [
            ("https://example.com/", None, "US", None),
            ("https://example.com/", Some(""), "US", None),
            ("https://www.lemonde.fr/a", None, "FR", Some("fr")),
            ("https://example.com/", Some("de"), "DE", Some("de")),
        ] {
            let a = ask(url, c);
            assert_eq!(oxy_body(&a)["location"].as_str(), Some(oxy), "{url} {c:?}");
            assert_eq!(bd_body(&a).get("country").and_then(Value::as_str), bd, "{url} {c:?}");
            assert_eq!(decodo_body(&a)["geo"].as_str(), Some(oxy.to_lowercase().as_str()), "Decodo as Oxylabs");
        }
        // Decodo: `universal`, HTML (no `markdown`), `headless: html` on its render only, a mobile ask as its device
        let js = Ask { js: true, mobile: true, ..ask("https://ex.com/", None) };
        let mut want = json!({"url": "https://ex.com/", "target": "universal", "geo": "us"});
        assert_eq!(decodo_body(&ask("https://ex.com/", None)), want);
        (want["headless"], want["device_type"]) = ("html".into(), "mobile".into());
        assert_eq!(decodo_body(&js), want);
    }

    /// Every spelling of a private address is private; public ones pass.
    #[test]
    fn private_addresses() {
        for ip in [
            "127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0",
            "255.255.255.255", "224.0.0.1", "240.0.0.1", "198.18.0.1", "192.0.0.8", "::1", "::", "fe80::1", "fc00::1",
            "fd12::1", "::ffff:127.0.0.1", "::ffff:10.0.0.1", "64:ff9b::a9fe:a9fe", "2001:db8::1", "::7f00:1",
            "::a9fe:a9fe", "2002:7f00:1::", "2002:c0a8:101::1", "203.0.113.9", "fec0::1", "ff02::1", "192.0.2.1",
            "198.51.100.7",
        ] {
            assert!(private_ip(ip.parse().unwrap()), "{ip} is private");
        }
        // every block parses: one that does not is silently no block (`private_ip`'s `filter_map`)
        let block =
            |b: &str| b.split_once('/').map(|(ip, n)| (ip.parse::<std::net::IpAddr>().is_ok(), n.parse::<u32>()));
        let bad: Vec<&&str> =
            crate::fetch::SITES.fence.iter().filter(|b| !matches!(block(b), Some((true, Ok(_))))).collect();
        assert!(bad.is_empty(), "{bad:?}");
        for ip in [
            "8.8.8.8", "172.32.0.1", "100.128.0.1", "198.20.0.1", "2606:4700::1111", "::ffff:8.8.8.8",
            "2002:808:808::1",
        ] {
            assert!(!private_ip(ip.parse().unwrap()), "{ip} is public");
        }
        for url in [
            "http://localhost:8080/",
            "http://api.localhost/",
            "http://127.0.0.1/",
            "http://2130706433/",
            "http://0x7f.1/",
            "http://[::1]/",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.1:9000/x",
            "data:text/html,hi",
        ] {
            assert!(private_host(url), "{url} is refused");
        }
        assert!(!private_host("https://example.com/") && !private_host("http://8.8.8.8/"));
    }

    /// A meta refresh is a redirect only as the page's one move: no delay, its own host, no script, under
    /// `page_chars` shown, a 2xx page. A challenge's `<noscript>` refresh (a time, no URL) and a scripted page's
    /// no-JS fallback are no redirect (the 27 saved plain copies with a refresh were all of those kinds).
    #[test]
    fn a_meta_refresh_moves_only_alone() {
        let page = |status: u16, head: &str, body: &str| Raw {
            status,
            ctype: "text/html".into(),
            body: format!("<html><head>{head}</head><body>{body}</body></html>").into_bytes(),
            final_url: "https://a.example/releases/latest/".into(),
            ..Raw::default()
        };
        let tag = |c: &str| format!("<meta http-equiv=\"Refresh\" content=\"{c}\">");
        let to = |status, head: &str, body: &str| refresh(&page(status, head, body));
        let at = Some("https://a.example/2026/10/15/one/".to_string());
        assert_eq!(to(200, &tag("0; url=/2026/10/15/one/"), ""), at);
        assert_eq!(to(200, &tag("0;URL='../../2026/10/15/one/'"), ""), at, "WHATWG: any case, quotes, relative");
        assert_eq!(to(200, &tag("0, https://a.example/2026/10/15/one/"), ""), at, "a comma, no url=");
        assert_eq!(to(200, &tag("5; url=/2026/10/15/one/"), ""), None, "a delay");
        assert_eq!(to(200, &tag("0; url=https://b.example/one/"), ""), None, "another host");
        // one origin: scheme, host and port; http → https the one scheme move
        assert_eq!(to(200, &tag("0; url=https://a.example:8443/one/"), ""), None, "another port");
        assert_eq!(to(200, &tag("0; url=http://a.example/one/"), ""), None, "https → http");
        let up = Raw { final_url: "http://a.example/x".into(), ..page(200, &tag("0; url=https://a.example/one/"), "") };
        assert_eq!(refresh(&up).as_deref(), Some("https://a.example/one/"), "http → https");
        // a time of digits only; an empty `url=` reloads the page
        for c in ["+0; url=/one/", "-0; url=/one/", "0.0; url=/one/", " ; url=/one/", "0; url=", "0; url=''"] {
            assert_eq!(to(200, &tag(c), ""), None, "{c}");
        }
        assert_eq!(to(200, &tag("00 ; url=/one/"), "").as_deref(), Some("https://a.example/one/"));
        // the bytes `http-equiv` gate the parse: a page that merely says refresh is no candidate
        assert_eq!(to(200, "<meta name=refresh content='0; url=/one/'>", ""), None, "no http-equiv");
        assert_eq!(to(200, &tag("0; url=/one/"), "<script src=/app.js></script>"), None, "a script");
        assert_eq!(to(200, "", &format!("<noscript>{}</noscript>", tag("360"))), None, "no URL");
        assert_eq!(to(404, &tag("0; url=/one/"), ""), None, "no 2xx");
        assert_eq!(to(200, &tag("0; url=/one/"), &"<p>A page of its own. </p>".repeat(80)), None, "a page");
    }

    /// Redirects run through the fence: a public page that bounces to the metadata address is refused before
    /// the second request; relative Locations resolve; an endless chain stops.
    #[test]
    fn redirects_are_fenced() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let calls = std::cell::RefCell::new(Vec::<String>::new());
        let fake = |to: Option<&'static str>| {
            let calls = &calls;
            move |u: String| {
                calls.borrow_mut().push(u);
                let reply = match to {
                    Some(t) => (302, Some(t.to_string()), ()),
                    None => (200, None, ()),
                };
                std::future::ready(Ok::<_, &'static str>(reply))
            }
        };
        let r = rt.block_on(follow("https://ex.com/a", fake(Some("http://169.254.169.254/latest/meta-data/"))));
        assert_eq!(r.err(), Some("private_address"));
        assert_eq!(*calls.borrow(), ["https://ex.com/a"], "the private hop is never requested");
        calls.borrow_mut().clear();
        assert_eq!(
            rt.block_on(follow("https://ex.com/a", fake(Some("http://[::ffff:7f00:1]/")))).err(),
            Some("private_address")
        );
        calls.borrow_mut().clear();
        assert_eq!(rt.block_on(follow("https://ex.com/a", fake(Some("/b")))).err(), Some("redirect_loop"));
        assert_eq!(calls.borrow().len(), 11, "the first request + 10 redirects");
        assert!(calls.borrow()[1..].iter().all(|u| u == "https://ex.com/b"));
        let ok = rt.block_on(follow("https://ex.com/a", fake(None)));
        assert_eq!(ok.map(|r| r.0), Ok("https://ex.com/a".to_string()));
    }

    /// A DNS name that resolves only to loopback gets no address (`localhost` resolves from the hosts file,
    /// no network), and the error is recognised as `private_address`.
    #[test]
    fn resolver_drops_private_answers() {
        use wreq::dns::Resolve;
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let e = rt.block_on(Fence.resolve("localhost".into())).err().expect("refused");
        assert!(e.is::<Private>());
    }
}
