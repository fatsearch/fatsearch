//! No provider answer and no key is ever printed: the provider calls run in-process against a fake transport
//! (`fats::provider::fake`, no network) that answers like a provider that echoes the key (ScrapingBee's 401
//! does), and every line goes through the one printer and its redactor. One process, one cache; the tests
//! take turns (`lock`) because they share it, each in it fresh, removed after (`Scratch`).

#[path = "../src/scratch.rs"]
mod scratch;

use fats::provider::{Api, Reply};
use scratch::Scratch;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, Once};

/// Every call the fake transport answered: a command that must not touch the network leaves it unchanged.
static CALLS: AtomicUsize = AtomicUsize::new(0);
/// Oxylabs scrapes of `/slow/once`: one URL in flight is paid once.
static ONCE: AtomicUsize = AtomicUsize::new(0);
/// How the account APIs answer: 0 = the saved answers, 1 = Bright Data's 401 that echoes the key, 2 = $0.42 left,
/// 3 = both providers refuse the key.
static ACCOUNT: AtomicU8 = AtomicU8::new(0);
/// Calls to a JavaScript tier (Bright Data, Oxylabs with `run_js`): a prefetch makes none.
static JS: AtomicUsize = AtomicUsize::new(0);
/// Oxylabs scrapes of a `/slowq` page (150 ms each), with when each started: a batch's runs in flight.
static LANES: Mutex<Vec<(String, std::time::Instant)>> = Mutex::new(Vec::new());
/// When `walled()` read each request.
static HITS: Mutex<Vec<std::time::Instant>> = Mutex::new(Vec::new());

/// The Oxylabs key: random-looking, as real keys are.
const OXY: &str = "oxyK3y-7f9QzR2mWb5Lp8TnVc";
/// The Bright Data key is a common word: it must still never print.
const BD: &str = "sunflower";
const JUNK: &str = "<html><body><pre>Traceback (most recent call last): {\"debug\":\"upstream 10.0.0.7 \
    exploded\"}</pre></body></html>";

fn dir() -> PathBuf {
    Scratch::path("redact")
}

/// Bright Data's refusal message: sent in a header, never stored or printed.
/// Bright Data's refusals (200 + empty body + `x-brd-error`) by path word → `x-brd-error-code`: a gov host, a site
/// 403 it relayed, its rate limit.
const BRD: [(&str, &str); 3] =
    [("govblock", "proxy_error"), ("relayed", "http_status"), ("bucket", "bucket_rate_limit")];
const BD_REFUSAL: &str = "Access denied: data.gov.uk is classified as Government and blocked by Bright Data";

/// An account number as a provider may put it in its rate-limit header names.
const ACCT: &str = "8675309001";

/// The GitHub token (`GH_TOKEN`): `fats gh` sends it, nothing prints it.
const GH: &str = "ghp_f4keT0kenQx8LmZr2Wv9";

/// What the providers answer, by the target URL (or query) in the request: `/401` echoes the key in a 401,
/// `/500` sends junk with the key in it, `/long` an Oxylabs page that passes the content check, `govblock` is Bright
/// Data's 200 + empty body + `x-brd-error`, `/empty` a bare 200 with nothing, `/ok` and `okq` an Oxylabs page and
/// search, `burst` a 429 `RATE_LIMIT_EXCEEDED` (`burst now` with `retry-after: 7`), `quota` a 429 without that title;
/// anything else answers 401. Every Oxylabs answer carries rate-limit headers with an account number and a bucket name
/// in the header names.
fn provider(api: Api, key: &str, req: &Value) -> Reply {
    CALLS.fetch_add(1, Ordering::SeqCst);
    let target = req["url"]
        .as_str()
        .or(req["query"].as_str())
        .or(req["name"].as_str())
        .or(req["q"].as_str())
        .unwrap_or_default();
    if api == Api::BdRequest || req["run_js"] == true {
        JS.fetch_add(1, Ordering::SeqCst);
    }
    if api == Api::OxyScrape && target.ends_with("/slow/once") {
        ONCE.fetch_add(1, Ordering::SeqCst);
    }
    if api == Api::OxyScrape && target.contains("/slowq/") {
        LANES.lock().unwrap_or_else(|e| e.into_inner()).push((target.into(), std::time::Instant::now()));
    }
    if api == Api::DecodoScrape {
        return decodo_reply(key, req, target);
    }
    let fake = pdf_reply(api, req, target).or_else(|| prefetch_reply(api, target)).or_else(|| gh_login(api, target));
    if let Some(r) = fake.or_else(|| one_key(api, req, target)).or_else(|| doh(api, req, target)) {
        return r;
    }
    // `/huge-status`: a `status_code` past u16 (65940 mod 65536 = 404)
    if api == Api::BdRequest && target.ends_with("/huge-status") {
        return Reply { status: 200, headers: vec![], body: br#"{"status_code":65940}"#.to_vec() };
    }
    // `nozone`: a zone the account does not have, Bright Data's 400 naming it, the key beside it
    // (an Oxylabs 400 quoting it is Oxylabs' own error, never our zone)
    if matches!(api, Api::BdRequest | Api::OxyScrape) && target.contains("nozone") {
        return Reply { status: 400, headers: vec![], body: format!("zone \"my_unlocker\" not found {key}").into() };
    }
    // The account's zone-cost read of a zone the account does not have (live 2026-10-15: 422, naming none)
    if api == Api::BdZoneCost && ACCOUNT.load(Ordering::SeqCst) == 6 {
        return Reply { status: 422, headers: vec![], body: b"Unknown zone".to_vec() };
    }
    // `/og-js`: plain gets the wall's 403, Oxylabs an empty shell without JS and `/og-own`'s card shell with it
    if api == Api::OxyScrape && target.ends_with("/og-js") {
        let html = if req["run_js"] == true {
            og_md("/og-own", false).unwrap_or_default().1
        } else {
            "<div id=root></div><script src=/app.js></script>".into()
        };
        return Reply {
            status: 200,
            headers: vec![],
            body: serde_json::json!({"results": [{"html": html}]}).to_string().into_bytes(),
        };
    }
    let mut r = reply(api, key, target);
    // a bucket name per API, as the provider returns it in the header names
    let bucket = match api {
        Api::OxySearch => Some(("search_total_dynamic_europe", "10")),
        Api::OxyScrape => Some(("internal_total_default", "250")),
        _ => None,
    };
    if let Some((b, n)) = bucket {
        let left = if r.status == 429 { "0" } else { "3" };
        r.headers.extend(h(&[
            (&format!("x-ratelimit-{b}-{ACCT}-limit"), n),
            (&format!("x-ratelimit-{b}-{ACCT}-remaining"), left),
            ("x-ratelimit-system_search_total_dynamic_europe-remaining", "9999"),
        ]));
    }
    r
}

fn h(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The site's own status in a finished Oxylabs job (`metadata.statuses`): a 404, a relayed 503.
/// `/thin`: a thread whose comments sit in a `<template>` island (a Reddit copy): `! thin → fats uN#k`.
fn site_status(target: &str) -> Option<String> {
    if target.contains("/thin") {
        let c = |i| format!("<p>{}</p>", format!("Comment {i}: ndarray and nalgebra aim at other jobs. ").repeat(50));
        let html = include_str!("fixtures/reddit-thin-template.html")
            .replace("{comments}", &(1..=6).map(c).collect::<String>());
        return Some(serde_json::json!({"results": [{"html": html}]}).to_string());
    }
    let code = [("/gone404", 404), ("/site503", 503)].iter().find(|c| target.contains(c.0))?.1;
    let status = format!(r#""metadata":{{"statuses":{{"http_request":{{"code":{code}}}}}}}"#);
    Some(format!(r#"{{"results":[{{"html":"<html><body>Not here</body></html>",{status}}}]}}"#))
}

/// An account's repositories (`GhUser`; its search `user:<login>` the same): `tokio-rs` three
/// (recorded 2026-10-16, trimmed), `ghnobody` no such account (search 422, list 404), `ghhidden` one the search
/// leaves out (422) whose list holds `ghhidden/oldstar` ★9999, `ghcore`/`ghcore0` the budget spent
/// (`retry-after`; `ghcore0`'s word search finds none); any other login an account with none.
fn gh_login(api: Api, target: &str) -> Option<Reply> {
    let (searched, target) = target.strip_prefix("user:").map_or((false, target), |t| (true, t));
    let old = r#"[{"html_url":"https://github.com/ghhidden/oldstar","full_name":"ghhidden/oldstar","stargazers_count":9999}]"#;
    let (status, headers, body) = match target {
        "ghcore0" if api == Api::GhRepos && !searched => (200, vec![], r#"{"total_count":0,"items":[]}"#.into()),
        _ if api != Api::GhUser && !searched => return None,
        "ghnobody" | "ghhidden" if searched => (422, vec![], r#"{"message":"Validation Failed"}"#.into()),
        "tokio-rs" => (200, vec![], fixture("gh-user")),
        "ghhidden" => (200, vec![], old.into()),
        "ghnobody" => (404, vec![], r#"{"message":"Not Found"}"#.into()),
        "ghcore" | "ghcore0" => (403, h(&[("retry-after", "720")]), r#"{"message":"API rate limit exceeded"}"#.into()),
        _ => (200, vec![], "[]".into()),
    };
    Some(Reply { status, headers, body: body.into_bytes() })
}

/// An account answer's fixture; a missing one fails the test, never an empty answer.
fn fixture(f: &str) -> String {
    let path = format!("tests/fixtures/account-{f}.json");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {path}: {e}"))
}

/// `/500esc`: a JSON answer echoing the key in `\u00XX` escapes and upper-cased.
fn escaped_echo(key: &str) -> String {
    let esc: String = key.chars().map(|c| format!("\\u{:04x}", c as u32)).collect();
    format!(r#"{{"detail":"bad key {esc} {}"}}"#, key.to_uppercase())
}

fn reply(api: Api, key: &str, target: &str) -> Reply {
    let limited = r#"{"status":429,"title":"RATE_LIMIT_EXCEEDED","detail":"Too many requests."}"#;
    let mode = ACCOUNT.load(Ordering::SeqCst);
    let (status, headers, body) = match (api, target) {
        // a refused key: 401 (1, 3), 403 on every read (5); a key scoped without the balance read: 403 there (4)
        (Api::BdBalance | Api::BdZones | Api::BdCosts | Api::BdZoneCost, _) if matches!(mode, 1 | 3 | 5) => {
            (if mode == 5 { 403 } else { 401 }, vec![], format!("Invalid api key: {key}"))
        }
        (Api::BdBalance, _) if mode == 4 => (403, vec![], "Forbidden".into()),
        // GitHub repository search: `ghwait` a secondary limit (`retry-after`), `ghspent` the hourly budget gone
        // (`x-ratelimit-reset` a minute from now), `ghbad` a refused token (the 401 below), `ghcore0` no repo
        // (`gh_login`), any other query two repos (sent the token, or none: unauthenticated)
        (Api::GhRepos, "ghwait") => (403, h(&[("retry-after", "42")]), r#"{"message":"secondary rate limit"}"#.into()),
        (Api::GhRepos, "ghspent") => {
            let reset = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() + 60).to_string();
            (429, h(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", &reset)]), String::new())
        }
        (Api::GhRepos, t) if (key == GH || key.is_empty()) && t != "ghbad" => (200, vec![], fixture("gh-repos")),
        (Api::OxyScrapers, _) if mode == 3 => (401, vec![], format!("Invalid api key: {key}")),
        (Api::BdBalance, _) if mode == 2 => (200, vec![], r#"{"balance":0.42}"#.into()),
        (Api::BdBalance, _) => (200, vec![], fixture("bd-balance")),
        (Api::BdZones, _) => (200, vec![], fixture("bd-zones")),
        (Api::BdCosts, _) => (200, vec![], fixture("bd-costs")),
        (Api::BdZoneCost, _) => (200, vec![], fixture("bd-zone-cost")),
        (Api::OxyScrapers, _) => (200, vec![], fixture("oxy-scrapers")),
        (Api::OxyScrape, t) if let Some(site) = site_status(t) => (200, vec![], site),
        (Api::OxyScrape, t) if t.contains("/ok") => {
            (200, vec![], r#"{"results":[{"html":"<html><head><title>Fine</title></head><body><h1>Fine</h1><p>a page</p></body></html>"}]}"#.into())
        }
        (Api::OxyScrape, t) if t.contains("/long") => {
            let p = "Every learned route starts past plain. ".repeat(30);
            let html = format!("<html><head><title>Learned Route Works</title></head><body><h1>Learned Route Works</h1><p>{p}</p></body></html>");
            (200, vec![], serde_json::json!({"results": [{"html": html}]}).to_string())
        }
        // `/vanished`: Bright Data brings the site's 404 at once; Oxylabs takes 3 s (`x-fake-delay-ms`)
        (Api::BdRequest, t) if t.contains("/vanished") => {
            (200, vec![], r#"{"status_code":404,"headers":{},"body":"<html><body>Not here</body></html>"}"#.into())
        }
        // a wrong product slug: Bright Data lands on the listing and names it in the site's headers (live 2026-10-13)
        (Api::BdRequest, t) if t.contains("/Charizard-V9") => {
            let html = format!("<html><head><title>Base Set</title></head><body><h1>Base Set</h1><p>{}</p></body></html>", "Charizard Blastoise Venusaur singles from the Base Set. ".repeat(20));
            let to = "https://ex.com/Singles/Base-Set";
            (200, vec![], serde_json::json!({"status_code": 200, "headers": {"x-unblocker-redirected-to": to}, "body": html}).to_string())
        }
        // `/stall`: both providers answer after 3 s (past `timeout:1`); bd's `/vanished` matched above
        (Api::OxyScrape | Api::BdRequest, t) if t.contains("/vanished") || t.contains("/stall") => (200, h(&[("x-fake-delay-ms", "3000")]), r#"{"results":[{"html":"<html>late</html>"}]}"#.into()),
        // `/slow`: a good page after 2 s (a fetch past `wait:`); `/slow6` after 6 s (past a batch's `wait:3`);
        // `/slowq` after 150 ms (a batch's lanes)
        (Api::OxyScrape, t) if t.contains("/slow") => {
            let ms = if t.contains("/slow6") { "6000" } else if t.contains("/slowq") { "150" } else { "2000" };
            let html = format!("<html><head><title>Slow Page</title></head><body><h1>Slow Page</h1><p>{}</p></body></html>", "A page that comes late still comes whole, with every number: 12 apples, 3 pears. ".repeat(12));
            (200, h(&[("x-fake-delay-ms", ms)]), serde_json::json!({"results": [{"html": html}]}).to_string())
        }
        // `preq <word>` (and `usage`, a command word searched): six results, their URLs named after the query
        // (each query gets pages of its own)
        (Api::OxySearch, t) if t.starts_with("preq") || t == "usage" => {
            let hit = |i: usize| serde_json::json!({"url": format!("{}/long{i}-{}", walled(), t.replace(' ', "-")), "title": "R"});
            (200, vec![], serde_json::json!({"results": (1..=6).map(hit).collect::<Vec<_>>()}).to_string())
        }
        (Api::OxySearch, "okq") => (200, vec![], r#"{"results":[{"url":"https://ex.com/r","title":"R","outline":["s"]}]}"#.into()),
        (_, "burst now") => (429, h(&[("retry-after", "7")]), limited.into()),
        (_, t) if t.contains("burst") => (429, vec![], limited.into()),
        (_, "quota") => (429, vec![], r#"{"status":429,"title":"SOMETHING_ELSE"}"#.into()),
        (_, t) if t.contains("/500esc") => (500, vec![], escaped_echo(key)),
        (_, t) if t.contains("/500") => (500, vec![], format!("{JUNK} key={key}")),
        (Api::BdRequest, t) if let Some(c) = BRD.iter().find(|c| t.contains(c.0)) => {
            (200, h(&[("x-brd-error", BD_REFUSAL), ("x-brd-error-code", c.1), ("x-brd-status-code", "403")]), String::new())
        }
        (_, t) if t.contains("/empty") => (200, vec![], String::new()),
        _ => (401, vec![], format!("Invalid api key: {key}")),
    };
    Reply { status, headers, body: body.into_bytes() }
}

/// The searches of the `site:` and prefetch tests and their pages.
fn prefetch_reply(api: Api, target: &str) -> Option<Reply> {
    let body: String = match (api, target) {
        // `site:ex.com <words>`: two results on ex.com (one under it), two off it; with `offsite`, only the two off it
        (Api::OxySearch, t) if t.starts_with("site:ex.com") => {
            let on = ["https://ex.com/long-on", "https://docs.ex.com/long-under"];
            let off = ["https://notex.com/long-off", "https://other.org/ex.com"];
            let urls = if t.contains("offsite") { off.to_vec() } else { [on[0], off[0], on[1], off[1]].to_vec() };
            let hits: Vec<Value> = urls.iter().map(|u| serde_json::json!({"url": u, "title": "R"})).collect();
            serde_json::json!({"results": hits}).to_string()
        }
        // `shellq`: two pages on the local wall (plain 403); Oxylabs without JS sends an empty shell, Bright Data the
        // page
        (Api::OxySearch, t) if t.starts_with("shellq") => {
            let hit = |i: usize| serde_json::json!({"url": format!("{}/shell{i}-{}", walled(), t.replace(' ', "-")), "title": "R"});
            serde_json::json!({"results": [hit(1), hit(2)]}).to_string()
        }
        (Api::OxyScrape, t) if t.contains("/shell") || t.contains("/deadshell") => {
            r#"{"results":[{"html":"<html><body><div id=\"root\"></div><script src=/app.js></script></body></html>"}]}"#
                .into()
        }
        // a site's own Spanish page (its canonical = the URL asked), and a geo copy of an English site's page
        (Api::BdRequest, t) if t.contains(".example/") => {
            let html = if t.contains("pisos") {
                include_str!("fixtures/geo-own-es.html")
            } else {
                include_str!("fixtures/geo-copy-es.html")
            };
            serde_json::json!({"status_code": 200, "headers": {}, "body": html}).to_string()
        }
        // `/profile/all`: every render brings the sign-in gate's shell (`login-shell.html`)
        (Api::OxyScrape | Api::BdRequest, t) if t.contains("/profile/all") => {
            serde_json::json!({"status_code": 200, "headers": {}, "body": LOGIN_SHELL, "results": [{"html": LOGIN_SHELL}]}).to_string()
        }
        // `/deadshell`: Bright Data brings the app's JS shell too
        (Api::BdRequest, t) if t.contains("/deadshell") => {
            let shell = "<html><head><title>Loading workspace dashboard</title></head><body><div id=root></div><script src=/app.js></script></body></html>";
            serde_json::json!({"status_code": 200, "headers": {}, "body": shell}).to_string()
        }
        (Api::BdRequest, t) if t.contains("/shell") => {
            let html = format!(
                "<html><head><title>Rendered Shell</title></head><body><h1>Rendered Shell</h1><p>{}</p></body></html>",
                "The app rendered its text at last. ".repeat(20)
            );
            serde_json::json!({"status_code": 200, "headers": {}, "body": html}).to_string()
        }
        // a port Bright Data does not proxy: its own 403 inside the JSON answer (live 2026-10-14)
        (Api::BdRequest, t) if t.contains(":81/") => {
            let h = r#"{"x-brd-error":"Bad Port","x-brd-err-code":"policy_20020","x-brd-err-msg":"Bad Port used"}"#;
            format!(r#"{{"status_code":403,"headers":{h},"body":""}}"#)
        }
        // an unlock failure: Bright Data's 502 + `reject_block` inside the JSON answer (observed live)
        (Api::BdRequest, t) if t.contains("rejectblock") => {
            let h = r#"{"x-brd-error":"captcha or protection page found","x-brd-error-code":"reject_block"}"#;
            format!(r#"{{"status_code":502,"headers":{h},"body":""}}"#)
        }
        // `/holdwall`: every provider sends a Cloudflare challenge showing the page's own words
        (api, t) if t.contains("/holdwall") => {
            let wall = "<html><head><title>Just a moment...</title><meta name=description content=\"Holdwall garden \
                        chairs on sale\"></head><body><p>Holdwall garden chairs on sale</p></body></html>";
            let bd = serde_json::json!({"status_code": 200, "headers": {"cf-mitigated": "challenge"}, "body": wall});
            if api == Api::BdRequest { bd } else { serde_json::json!({"results": [{"html": wall}]}) }.to_string()
        }
        // `nolist`: a 200 that holds no result list (bd's SERP `provider_error`s)
        (_, t) if t.contains("nolist") => r#"{"status_code":502,"body":""}"#.into(),
        _ => return None,
    };
    Some(Reply { status: 200, headers: vec![], body: body.into_bytes() })
}

/// One-key pages on `walled()` (plain: 403): `/k-wall` Oxylabs without JS hits a wall, its render and Bright Data
/// serve; `/k-shell` Oxylabs without JS and Bright Data send a JS shell, the Oxylabs render serves; `/k-flaky`: each
/// provider's first call is a 500, the next serves. `kq`: both providers' search finds one result.
fn one_key(api: Api, req: &Value, target: &str) -> Option<Reply> {
    static FLAKY: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let render = api == Api::BdRequest || req["run_js"] == true;
    let page = format!(
        "<html><head><title>One Key Page</title></head><body><h1>One Key Page</h1><p>{}</p></body></html>",
        "Any one provider's key fetches this page. ".repeat(20)
    );
    let shell = "<html><head><title>Loading workspace dashboard</title></head><body><div id=root></div><script src=/app.js></script></body></html>";
    let html = match target {
        t if t.contains("q=kq") || t.contains("q=kslow") => {
            // `kslow`: Bright Data takes 1.5 s, past `wait:1`
            let late = t.contains("q=kslow").then(|| ("x-fake-delay-ms".to_string(), "1500".to_string()));
            return Some(Reply {
                status: 200,
                headers: late.into_iter().collect(),
                body: br#"{"organic":[{"link":"https://ex.com/bdk","title":"From BD","description":"d"}]}"#.to_vec(),
            });
        }
        "kq" => r#"{"results":[{"url":"https://ex.com/oxyk","title":"From Oxy"}]}"#.to_string(),
        // `/k-capslow`: plain's captcha, Oxylabs without JS past its 10 s cut
        t if t.contains("/k-capslow") && !render => {
            let late = vec![("x-fake-delay-ms".to_string(), "12000".to_string())];
            return Some(Reply { status: 200, headers: late, body: br#"{"results":[{"html":"late"}]}"#.to_vec() });
        }
        t if t.contains("/k-wall") && !render => {
            "<html><head><title>Just a moment...</title></head><body></body></html>".into()
        }
        t if t.contains("/k-sh") && (api == Api::BdRequest || !render) => shell.into(),
        // `/k-sh400`: the Oxylabs render refuses the target (its RFC 9457 400); `/k-shfault`: its
        // own retries gave up (500 `faulted`)
        t if t.contains("/k-sh400") || t.contains("/k-shfault") => {
            let (status, body) = if t.contains("/k-sh400") {
                (
                    400,
                    r##"{"title":"INVALID_BODY_PROPERTY","errors":[{"detail":"Value not supported.","pointer":"#/url"}]}"##,
                )
            } else {
                (500, r#"{"status":500,"title":"REQUEST_FAILED_AFTER_TOO_MANY_RETRIES","state":"faulted"}"#)
            };
            return Some(Reply { status, headers: vec![], body: body.into() });
        }
        // `/k-sherr`: Bright Data's shell, the Oxylabs render's first call a 500 (Instagram)
        t if t.contains("/k-flaky") || t.contains("/k-sherr") => {
            let mut seen = FLAKY.lock().unwrap_or_else(|e| e.into_inner());
            let call = format!("{api:?}{render}{t}");
            if !seen.contains(&call) {
                seen.push(call);
                return Some(Reply { status: 500, headers: vec![], body: b"upstream".to_vec() });
            }
            page
        }
        t if t.contains("/k-") => page,
        _ => return None,
    };
    let body = match (api, target) {
        (_, "kq") => html,
        (Api::BdRequest, _) => serde_json::json!({"status_code": 200, "headers": {}, "body": html}).to_string(),
        _ => serde_json::json!({"results": [{"html": html}]}).to_string(),
    };
    // `/k-wall/late…`: Bright Data serves after the default 5 s hedge, the Oxylabs render at once
    let late =
        (api == Api::BdRequest && target.contains("/k-wall/late")).then(|| ("x-fake-delay-ms".into(), "5600".into()));
    Some(Reply { status: 200, headers: late.into_iter().collect(), body: body.into_bytes() })
}

/// The second resolver's answer to a `kind` (A, AAAA) question: `dead.*` does not exist (NXDOMAIN), `mute.*` gets
/// no verdict (SERVFAIL), `down.*` none either (the resolver unreachable: a 503), `void.*` exists with no address
/// (NODATA), `six.*` has an AAAA only, every other name an A.
fn doh(api: Api, req: &Value, name: &str) -> Option<Reply> {
    let kind = req["type"].as_str().filter(|_| api == Api::Doh)?;
    if name.starts_with("down.") {
        return Some(Reply { status: 503, headers: vec![], body: b"unavailable".to_vec() });
    }
    let rcode = [("dead.", 3), ("mute.", 2)].iter().find(|r| name.starts_with(r.0)).map_or(0, |r| r.1);
    let has = match kind {
        "A" => !name.starts_with("void.") && !name.starts_with("six."),
        _ => name.starts_with("six."),
    };
    let ty = if kind == "A" { 1 } else { 28 };
    let answer = if has { format!(r#","Answer":[{{"name":"{name}","type":{ty},"data":"x"}}]"#) } else { String::new() };
    Some(Reply { status: 200, headers: vec![], body: format!(r#"{{"Status":{rcode}{answer}}}"#).into_bytes() })
}

/// The Decodo key (`DECODO_AUTH_TOKEN`).
const DECODO: &str = "dcd0Tok-Vb7nQ2xLm9RwZ4kP";

/// Decodo's Web Data API: a wrong key = its 401 echoing it; the key check's dead host = its 400; `/k-…` pages the
/// one-key pages (`k-wall`: a wall without `headless`); `/d5xx`: the site's 502 relayed without JS, Decodo's own
/// 500 with it. Every answer is `results[0].{content, status_code, url, headers}`.
fn decodo_reply(key: &str, req: &Value, target: &str) -> Reply {
    let js = req["headless"] == "html";
    let echo = |status| Reply { status, headers: vec![], body: format!("{JUNK} token={key}").into_bytes() };
    let (status, html) = match target {
        _ if key != DECODO => return echo(401),
        fats::provider::NO_HOST => return echo(400),
        t if t.contains("/d5xx") && js => return echo(500),
        t if t.contains("/d5xx") => (502, "<html><body>Bad gateway</body></html>".to_string()),
        t if t.contains("/k-wall") && !js => (403, "<html><head><title>Just a moment...</title></head></html>".into()),
        t if t.contains("/k-") => (200, html_page("Decodo Page", &"Decodo fetched this page whole. ".repeat(20))),
        _ => return echo(401),
    };
    let r = serde_json::json!({"content": html, "status_code": status, "url": target, "headers": {"x-site": "1"}});
    Reply { status: 200, headers: vec![], body: serde_json::json!({"results": [r]}).to_string().into_bytes() }
}

const PDF: &[u8] = include_bytes!("fixtures/three-pages.pdf");
/// A sign-in gate's app shell, where `walled()`'s `/profile/all` redirects.
const LOGIN_SHELL: &str = include_str!("fixtures/login-shell.html");
/// A PDF whose own text holds `\x9c` (4 characters) before its cross-reference table.
const LITERAL: &[u8] = include_bytes!("fixtures/literal-escape.pdf");

/// The fixture cut inside page 2: pages 2 and 3 lost, no end-of-file line.
fn cut_pdf() -> &'static [u8] {
    &PDF[..PDF.windows(7).position(|w| w == b"5 0 obj").unwrap_or_default() + 40]
}

/// A file as Oxylabs sends it in `html`: decoded as UTF-8, every byte that does not
/// decode written as `\xNN` (Python's `backslashreplace`).
fn escaped(mut b: &[u8]) -> String {
    let mut s = String::new();
    loop {
        match std::str::from_utf8(b) {
            Ok(t) => return s + t,
            Err(e) => {
                let (ok, bad) = b.split_at(e.valid_up_to());
                s += std::str::from_utf8(ok).unwrap_or_default();
                let n = e.error_len().unwrap_or(bad.len());
                bad[..n].iter().for_each(|x| s += &format!("\\x{x:02x}"));
                b = &bad[n..];
            }
        }
    }
}

/// PDF URLs on the local server (`walled`): `paper.pdf` Oxylabs sends escaped; `page.pdf` (and `/pdf/page`, no
/// suffix) Oxylabs answers with a sign-in page, Bright Data with the file; `cut.pdf` both send the cut file. Bright
/// Data sends bytes only when asked for `format: raw` (its JSON string is a lossy copy).
fn pdf_reply(api: Api, req: &Value, target: &str) -> Option<Reply> {
    let page = "<html><head><title>Sign in</title></head><body>Sign in to read</body></html>";
    let file = match target.rsplit('/').next()? {
        "paper.pdf" | "page.pdf" | "page" => PDF,
        "cut.pdf" => cut_pdf(),
        "signin.pdf" => page.as_bytes(),
        "literal.pdf" => LITERAL,
        // the file is gone: Bright Data says so in `x-brd-status-code`, its outer status is 200
        "missing.pdf" if api == Api::BdRequest => {
            let headers = h(&[("x-brd-status-code", "404")]);
            return Some(Reply { status: 200, headers, body: b"<html><body>Not Found</body></html>".to_vec() });
        }
        _ => return None,
    };
    let body = match api {
        Api::OxyScrape if target.contains("/page") => serde_json::json!({"results": [{"html": page}]}).to_string(),
        Api::OxyScrape => serde_json::json!({"results": [{"html": escaped(file)}]}).to_string(),
        Api::BdRequest if req["format"] == "raw" => {
            return Some(Reply { status: 200, headers: vec![], body: file.into() });
        }
        _ => serde_json::json!({"status_code": 200, "body": String::from_utf8_lossy(file)}).to_string(),
    };
    Some(Reply { status: 200, headers: vec![], body: body.into_bytes() })
}

/// A page whose `h1` is its title, over one paragraph.
fn html_page(title: &str, text: &str) -> String {
    format!("<html><head><title>{title}</title></head><body><h1>{title}</h1><p>{text}</p></body></html>")
}

/// A local site behind a wall: every `.pdf` is a 403 sign-in page; `report.docx` is a zip, `locked.docx` a 403;
/// a page under `/pdf/` is a page (`/pdf/page` excepted: a 403); `/long…` a page (a search's prefetch).
/// Plain fetches it for real (loopback, `FATS_ALLOW_PRIVATE=1`); providers are the fake. Returns the base URL.
fn walled() -> String {
    static BASE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BASE.get_or_init(|| {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://127.0.0.1:{}/w", l.local_addr().expect("addr").port());
        std::thread::spawn(move || {
            for mut c in l.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let n = c.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]);
                let path = head.split(' ').nth(1).unwrap_or_default().to_string();
                HITS.lock().unwrap_or_else(|e| e.into_inner()).push(std::time::Instant::now());
                let guide = html_page("Merge PDF files", &"Pick the files, drop them here and merge them into one. ".repeat(12));
                // `/shell`: an app shell (200, a title the body never shows) to any client
                let shell = "<html><head><title>Loading workspace dashboard</title></head><body><div id=\"root\">\
                             </div><script src=/app.js></script></body></html>";
                let long = html_page("Plain Prefetch Works", &"A free copy fetched before anyone asked. ".repeat(30));
                // `/doc/<words>`: a served page titled by its own path's words
                let doc = path.split("/doc/").nth(1).map(|d| html_page(&d.replace('-', " "), &"Keep a session safe. ".repeat(40)));
                let page = og_md(&path, head.to_lowercase().contains("\r\naccept: text/markdown"));
                let (status, ctype, body): (&str, &str, &[u8]) = if let Some((ct, b)) = &page {
                    ("200 OK", *ct, b.as_bytes())
                } else if path.contains("/pdf/") && !path.ends_with("/page") {
                    ("200 OK", "text/html", guide.as_bytes())
                } else if path.contains("/long") {
                    ("200 OK", "text/html", long.as_bytes())
                } else if landing(&path).is_some() {
                    ("302 Found", "text/html", b"".as_slice())
                } else if let Some(d) = &doc {
                    ("200 OK", "text/html", d.as_bytes())
                } else if path.contains("/i/flow/onboarding") {
                    ("200 OK", "text/html", LOGIN_SHELL.as_bytes())
                } else if path.contains("/k-capslow") {
                    ("403 Forbidden", "text/html", b"<html><body><div id=px-captcha></div></body></html>".as_slice())
                } else if path.ends_with("shell") || path.contains("/land/") {
                    ("200 OK", "text/html", shell.as_bytes())
                } else if path.ends_with("/report.docx") {
                    (
                        "200 OK",
                        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                        b"PK\x03\x04 a docx",
                    )
                } else {
                    (
                        "403 Forbidden",
                        "text/html",
                        b"<html><head><title>Forbidden</title></head><body>Denied</body></html>",
                    )
                };
                let signal = if ctype == "text/markdown" { "content-signal: ai-train=yes, search=yes\r\n" } else { "" };
                let to = landing(&path).map(|l| format!("location: {l}\r\n")).unwrap_or_default();
                let h = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: {ctype}\r\n{signal}{to}content-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let (late, out) = (late_ms(&path), [h.as_bytes(), body].concat());
                std::thread::spawn(move || (std::thread::sleep(std::time::Duration::from_millis(late)), c.write_all(&out).ok()));
            }
        });
        base
    })
    .clone()
}

/// A loopback host whose https is refused for its certificate (self-signed, made at start: no key in the tree)
/// and whose http answers `301` back to https on the same port, the shape of a host with an expired certificate.
/// The first byte tells them apart (`0x16` = a TLS ClientHello). Returns the https base URL.
fn bad_cert() -> String {
    static BASE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BASE.get_or_init(|| {
        use btls::{asn1::Asn1Time, ec, hash::MessageDigest, nid::Nid, pkey::PKey, ssl, x509};
        use std::io::{Read, Write};
        let key = ec::EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).and_then(|g| ec::EcKey::generate(&g));
        let key = key.and_then(PKey::from_ec_key).expect("key");
        let mut name = x509::X509NameBuilder::new().expect("name");
        name.append_entry_by_text("CN", "fats-test.invalid").expect("cn");
        let name = name.build();
        let mut cert = x509::X509::builder().expect("cert");
        cert.set_version(2).expect("v3");
        cert.set_subject_name(&name).and_then(|()| cert.set_issuer_name(&name)).expect("names");
        cert.set_pubkey(&key).expect("pubkey");
        let (from, to) = (Asn1Time::days_from_now(0).expect("now"), Asn1Time::days_from_now(1).expect("end"));
        cert.set_not_before(&from).and_then(|()| cert.set_not_after(&to)).expect("dates");
        cert.sign(&key, MessageDigest::sha256()).expect("sign");
        let cert = cert.build();
        let mut tls = ssl::SslAcceptor::mozilla_intermediate(ssl::SslMethod::tls()).expect("acceptor");
        tls.set_private_key(&key).and_then(|()| tls.set_certificate(&cert)).expect("pair");
        let tls = tls.build();
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let host = format!("127.0.0.1:{}", l.local_addr().expect("addr").port());
        let base = format!("https://{host}");
        std::thread::spawn(move || {
            for mut c in l.incoming().flatten() {
                let mut first = [0u8; 1];
                if c.peek(&mut first).unwrap_or(0) == 1 && first[0] == 0x16 {
                    // the client rejects the certificate mid-handshake: the server side's error is expected
                    tls.accept(c).ok();
                    continue;
                }
                let mut buf = [0u8; 4096];
                let n = c.read(&mut buf).unwrap_or(0);
                let path = String::from_utf8_lossy(&buf[..n]).split(' ').nth(1).unwrap_or("/").to_string();
                let to = format!("location: https://{host}{path}");
                let head =
                    format!("HTTP/1.1 301 Moved Permanently\r\n{to}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
                c.write_all(head.as_bytes()).ok();
            }
        });
        base
    })
    .clone()
}

/// How long `walled()` holds a page: `/long/late…` and a `preq late…` search's results 1.5 s (the server free for
/// other requests meanwhile), the rest none.
fn late_ms(path: &str) -> u64 {
    u64::from(path.contains("/long") && path.contains("late")) * 1500
}

/// Where `walled()`'s landings send the asked path back: (landing, how plain reads it). `/go/<n>-…` redirects to
/// `/w/land/<GOES[n]>` + the asked path; every `/land/` page is a thin shell.
const GOES: [(&str, &str); 3] = [
    ("consent/login-free?continue=", "redirected"),
    ("signing-guide?from=", "redirected"),
    ("customer-login?redirect=", "login_wall"),
];

/// Where `walled()` redirects: `/profile/all` to a sign-in gate's onboarding shell that carries the asked path back
/// (`login-shell.html`), `/go/<n>-…` to `GOES[n]`.
fn landing(path: &str) -> Option<String> {
    if path.ends_with("/profile/all") {
        return Some(format!("/w/i/flow/onboarding?redirect_after_login={path}&mode=login"));
    }
    let n: usize = path.split("/go/").nth(1)?.split('-').next()?.parse().ok()?;
    Some(format!("/w/land/{}{}", GOES.get(n)?.0, path.replace('/', "%2F")))
}

/// `walled()`'s og and Markdown pages. `/og-own`: a JS shell (200) carrying the page's own og card; `/og-site`: one
/// carrying the site-wide card (og:title = og:site_name); `/og-one`: og:title alone; `/og-logo`: no og:title, a
/// description and a logo. `/md` and `/md-thin`: the
/// server's Markdown when asked for it (`accept: text/markdown`), else HTML; `/md-thin`'s Markdown is a stub;
/// `/md-only`: the same stub, and a 403 to an HTML ask; `/md-flat`: HTML to the first ask, then Markdown without
/// a heading. `/latest`: a bare page whose one move is a meta refresh to a `/doc/` page (`/latest-5s`: after 5 s;
/// `/latest-gone`: to a 403; `/moved/Release-Notes-20261020`: to the `/doc/` page of the same id).
fn og_md(path: &str, md: bool) -> Option<(&'static str, String)> {
    static FLAT_MD: AtomicUsize = AtomicUsize::new(0);
    let shell = |meta: &str| {
        format!(
            "<html><head><title>Loading</title>{meta}</head><body><div id=\"root\"></div><script src=/app.js></script></body></html>"
        )
    };
    let m = |k: &str, v: &str| format!("<meta property=\"og:{k}\" content=\"{v}\">");
    let desc = m("description", "Artemis II crew photographs the far side of the Moon from Orion.");
    let og = match path.rsplit('/').next().unwrap_or_default() {
        "og-own" => m("title", "NASA") + &desc + &m("image", "/moon.jpg") + &m("site_name", "Facebook"),
        "og-site" => m("title", "Maps") + &desc + &m("image", "/logo.png") + &m("site_name", "maps"),
        "og-one" => m("title", "NASA"),
        "og-logo" => desc.clone() + &m("image", "/logo.png"),
        _ => String::new(),
    };
    if !og.is_empty() {
        return Some(("text/html", shell(&og)));
    }
    // `/oops`: an error page under a 200 (no script, none of its own words)
    if path.ends_with("/oops") {
        return Some(("text/html", "<html><head><title>Oops</title></head><body>Gone wrong.</body></html>".into()));
    }
    // `/long…`: a plain page that passes the content check (what a prefetch stores)
    if path.contains("/long") {
        let p = "A free copy fetched before anyone asked. ".repeat(30);
        let title = "Plain Prefetch Works";
        return Some((
            "text/html",
            format!("<html><head><title>{title}</title></head><body><h1>{title}</h1><p>{p}</p></body></html>"),
        ));
    }
    let prose = "Workers run your code on the network, close to every user, in milliseconds. ".repeat(6);
    let html = format!(
        "<html><head><title>Workers</title></head><body><h1>Workers</h1><h2>Start</h2><p>{prose}</p><h2>Limits</h2>\
         <p>{prose}</p><h2>Pricing</h2><p>{prose}</p></body></html>"
    );
    let full = format!(
        "---\ntitle: Workers\ndescription: Run code at the edge\n---\n\n# Workers\n\n## Start\n\n{prose}\n\n\
         ![The Workers dashboard](/dash.png)\n\nSee [the limits](/limits) and [pricing](https://ex.com/pricing).\n\n\
         ## Limits\n\n```\n# not a heading\n```\n\n{prose}\n"
    );
    match path.rsplit('/').next().unwrap_or_default() {
        "md" if md => Some(("text/markdown", full)),
        "md-thin" | "md-only" if md => Some(("text/markdown", "# Workers\n\nLoading\n".to_string())),
        "md-flat" if md && FLAT_MD.fetch_add(1, Ordering::SeqCst) > 0 => Some(("text/markdown", prose.repeat(3))),
        "md-flat" => Some(("text/html", html)),
        "md" | "md-thin" => Some(("text/html", html)),
        last if matches!(last, "latest" | "latest-5s" | "latest-gone") || path.contains("/moved/") => {
            let delay = if path.ends_with("5s") { 5 } else { 0 };
            let to = match last {
                "latest-gone" => "/w/gone".into(),
                "latest" | "latest-5s" => "/w/doc/Announcing-The-Release".into(),
                id => format!("/w/doc/{id}"),
            };
            let tag = format!("<meta http-equiv=\"refresh\" content=\"{delay}; url={to}\">");
            Some(("text/html", format!("<html><head><title>Redirecting</title>{tag}</head><body></body></html>")))
        }
        _ => None,
    }
}

/// Every byte fats wrote: the whole cache.
fn written() -> String {
    fn walk(p: &std::path::Path, out: &mut String) {
        for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push_str(&String::from_utf8_lossy(&std::fs::read(&p).unwrap_or_default()));
            }
        }
    }
    let mut out = String::new();
    walk(&dir().join("cache"), &mut out);
    out
}

/// The variables a test under `lock` changes, each put back as it was when the guard drops.
#[derive(Default)]
struct Env(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl Env {
    /// Sets `k` to `v`, or removes it on None.
    fn set(&mut self, k: &'static str, v: Option<&str>) -> &mut Self {
        if !self.0.iter().any(|(n, _)| *n == k) {
            self.0.push((k, std::env::var_os(k)));
        }
        put(k, v.map(std::ffi::OsStr::new));
        self
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        for (k, v) in std::mem::take(&mut self.0) {
            put(k, v.as_deref());
        }
    }
}
/// The one environment write after `lock`'s first: `Env` only.
#[expect(unsafe_code, reason = "under `lock`: every other test in this process waits; nextest runs one per process")]
fn put(k: &str, v: Option<&std::ffi::OsStr>) {
    // SAFETY: under `lock`, so no other thread of this process reads or writes the environment meanwhile
    unsafe {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
}

#[expect(unsafe_code, reason = "set_var once, before the first settings read, while every other test waits")]
fn lock() -> (Scratch, MutexGuard<'static, ()>) {
    static ONCE: Once = Once::new();
    static TURN: Mutex<()> = Mutex::new(());
    let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    ONCE.call_once(|| {
        // SAFETY: set once, before the first settings read, while every other test in this process waits on
        // `TURN`; nothing else in the process reads the environment concurrently.
        unsafe {
            std::env::set_var("FATS_CACHE", dir().join("cache"));
            std::env::set_var("FATS_KEY_DIR", dir().join("keys"));
            std::env::set_var("FATS_SESSION", "t");
            std::env::set_var("OXYLABS_WEB_API_KEY", OXY);
            std::env::set_var("BRIGHTDATA_API_KEY", BD);
            std::env::set_var("BRIGHTDATA_UNLOCKER_ZONE", "my_unlocker");
            std::env::set_var("GH_TOKEN", GH);
            std::env::set_var("FATS_ALLOW_PRIVATE", "1"); // plain fetches the local `walled` site
            // counted (full only), never sent: `telemetry::tick` runs in main and the MCP loop, neither here
            std::env::set_var("FATS_TELEMETRY", "full");
            std::env::set_var("FATS_PLAIN_HOST_PACE_MS", "0"); // unbraked: one test names it (`plain_host_pace_ms:`)
            let quiet = ["CI", "BUILD_NUMBER", "TF_BUILD", "TEAMCITY_VERSION", "GITHUB_ACTIONS", "DO_NOT_TRACK"];
            for k in ["FATS_OFFLINE", "FATS_JSON", "FATS_NOCACHE", "FATS_CHUNK"].into_iter().chain(quiet) {
                std::env::remove_var(k);
            }
        }
        fats::provider::fake(provider);
    });
    // the dir goes before the turn does (a tuple drops in order)
    (Scratch::new("redact"), turn)
}

fn run(cmd: &str) -> String {
    run_rc(cmd).0
}

fn run_rc(cmd: &str) -> (String, i32) {
    let (out, rc, _) = fats::cli::run(&cmd.split_whitespace().map(String::from).collect::<Vec<_>>());
    (out, rc)
}

/// No `!` row's fix sends a tier the run already tried (the tier path: `oxy:not_pdf✗ bd✓`); `js:true` = oxyjs.
fn no_tried_tier_in_fixes(out: &str) {
    let tried: Vec<&str> = out
        .split_whitespace()
        .filter(|w| w.contains('✗') || w.ends_with('✓'))
        .filter_map(|w| w.split([':', '✓', '✗']).next())
        .collect();
    for row in out.lines().filter(|l| l.contains(" ! ") || l.starts_with('!')) {
        let fix = row.split(" → ").nth(1).unwrap_or_default();
        let js = fix.split(' ').any(|w| w == "js:true").then_some("oxyjs");
        if let Some(t) = js.or(fix.split(' ').filter_map(|w| w.strip_prefix("provider:")).next_back()) {
            assert!(!tried.contains(&t), "the fix sends {t} again: {out}");
        }
    }
}

/// Nothing a provider said and no key, in lines or JSON.
fn clean(out: &str) {
    for leak in [OXY, BD, GH, DECODO, "Invalid api key", "Traceback", "exploded", "<pre>"] {
        assert!(!out.contains(leak), "printed {leak:?}: {out}");
    }
    assert!(!out.contains(&OXY[3..15]), "a 12-char run of the key: {out}");
}

/// (1) A 401 that echoes the key: one `! bad_key <key name> → fats setup` row (+ the keys page when the
/// provider documents one), no key, no body; fetch (oxy,
/// bd) and search alike, JSON too.
#[test]
fn refused_key_names_the_key_not_the_answer() {
    let _t = lock();
    let b = fats::lang::bin();
    // Oxylabs: its docs' dashboard + the product the key must come from
    let oxy = " · oxy https://dashboard.oxylabs.io (a Web API instance's key)";
    let bd = " · bd https://brightdata.com/cp/setting/users";
    for (cmd, key, page) in [
        ("https://ex.com/401 provider:oxy", "oxy", oxy.to_string()),
        ("https://ex.com/401b provider:bd", "bd", bd.into()),
        ("cheapest proxies provider:oxy", "oxy", oxy.into()),
        ("cheapest proxies de", "oxy bd", format!("{oxy}{bd}")),
    ] {
        let out = run(cmd);
        clean(&out);
        assert!(out.starts_with("= ") && out.contains("! bad_key"), "{cmd}: {out}");
        let rows: Vec<&str> = out.lines().skip(1).filter(|l| l.contains(" ! ") || l.starts_with('!')).collect();
        assert_eq!(rows.len(), 1, "{cmd}: one ! row: {out}");
        assert!(rows[0].ends_with(&format!("! bad_key {key} → {b} setup{page}")), "{cmd}: {out}");
        let json = run(&format!("{cmd} fresh:true json:true"));
        clean(&json);
        let r: Value = serde_json::from_str(&json).expect("one record");
        assert_eq!(r["error"], "bad_key", "{json}");
    }
}

/// A key echoed in `\u00XX` escapes and upper-cased never reaches the kept answers: a JSON answer is masked as
/// decoded, and the mask matches ASCII case-folded.
#[test]
fn an_escaped_or_upper_key_is_masked_when_kept() {
    let _t = lock();
    let out = run("https://ex.com/500esc provider:oxy");
    let kept = written();
    clean(&out);
    let esc: String = OXY.chars().map(|c| format!("\\u{:04x}", c as u32)).collect();
    assert!(kept.contains("bad key *** ***"), "both echoes masked: {out}");
    assert!(!kept.contains(&esc) && !kept.to_lowercase().contains(&OXY.to_lowercase()), "no key kept");
}

/// (2) A 500 with HTML/JSON junk: one `!` row, the body never shown; kept (masked) in the snapshot's
/// error.txt, mode 0600, and `fats stats` names the file.
#[test]
fn error_body_is_kept_not_shown() {
    let _t = lock();
    let out = run("https://ex.com/500 provider:oxy");
    clean(&out);
    let rows: Vec<&str> = out.lines().skip(1).filter(|l| l.contains(" ! ")).collect();
    assert_eq!(rows.len(), 1, "{out}");
    assert!(out.starts_with("= ") && rows[0].contains("! provider_error oxyjs 500 → "), "{out}");
    let stats = run("stats");
    clean(&stats);
    let (path, kept) = stats
        .lines()
        .filter_map(|l| l.split(" · provider answers ").nth(1).and_then(|p| p.split(" → ").next()).map(PathBuf::from))
        .filter_map(|p| Some((p.clone(), std::fs::read_to_string(&p).ok()?)))
        .find(|(_, k)| k.contains("# oxylabs 500"))
        .unwrap_or_else(|| panic!("stats points at error.txt: {stats}"));
    assert!(kept.starts_with("# oxylabs 500 provider_error\n") && kept.contains("Traceback"), "{kept}");
    assert!(kept.contains("# oxylabs-js 500 provider_error\n"), "every hop's answer: {kept}");
    assert!(!kept.contains(OXY) && kept.contains("key=***"), "kept masked: {kept}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", path.display());
    }
    // a search keeps its answers next to its record
    clean(&run("rust news provider:oxy"));
    let stats = run("stats");
    let row = stats.lines().find(|l| l.contains("\"rust news\"")).unwrap_or_default();
    assert!(row.contains(" search ") && row.contains("error.txt"), "{stats}");
    clean(&stats);
}

/// (4) `fats` and `fats config` say where a key comes from, never what it is: not even before the printer's
/// redactor runs, and BD's key is an ordinary word.
#[test]
fn status_and_config_show_presence_only() {
    let _t = lock();
    let c = fats::cache::Cache::open();
    for (name, o) in [("status", fats::stats::status(&c)), ("config", fats::stats::config())] {
        let text = o.lines.join("\n");
        assert!(!text.contains(OXY) && !text.contains(BD), "{name} builds a key into its lines: {text}");
        assert!(text.contains("env"), "{name} says where the key is from: {text}");
    }
    let status = run("");
    assert!(status.contains("keys oxy:env bd:env"), "{status}");
    for cmd in ["", "config", "json:true", "config json:true"] {
        clean(&run(cmd));
    }
}

/// (5) The redactor runs on every printed byte: a local page that contains a key prints it masked, in lines and
/// in the JSON record.
#[test]
fn printed_page_text_is_masked_too() {
    let _t = lock();
    let work = dir().join("work");
    std::fs::create_dir_all(&work).expect("work dir");
    let page = work.join("leak.html");
    let html =
        format!("<html><head><title>Leak</title></head><body><h1>Leak</h1><p>token {OXY} and {BD}</p></body></html>");
    std::fs::write(&page, html).expect("page");
    let p = page.display().to_string();
    for cmd in [format!("{p} full"), format!("{p} full json:true")] {
        let out = run(&cmd);
        clean(&out);
        assert!(out.contains("token *** and ***"), "{cmd}: {out}");
    }
}

/// Bright Data's 200 + empty body is a failure: its error-code header becomes the row's token
/// (`! provider_refused bd proxy_error`), the header's message is never printed or stored; a bare empty 200 is
/// `provider_error … empty`. Fetch and search alike.
/// Bright Data's 400 naming the zone fats sent is our setup, never a retry; the body
/// stays masked.
#[test]
fn a_missing_zone_is_setup() {
    let _t = lock();
    let b = fats::lang::bin();
    for cmd in ["https://ex.com/nozone provider:bd", "nozone words provider:bd"] {
        let out = run(cmd);
        clean(&out);
        assert!(
            out.contains(&format!("! bad_zone bd my_unlocker → {b} setup\n")) && !out.contains("fresh:true"),
            "{out}"
        );
    }
    assert!(!written().contains(BD), "the body's key was stored");
    let out = run("https://ex.com/nozone provider:oxy");
    assert!(out.contains("! provider_error ") && !out.contains("bad_zone"), "a bad zone is a provider error: {out}");
}

#[test]
fn bright_data_refusal_names_its_code() {
    let _t = lock();
    let b = fats::lang::bin();
    for (cmd, row) in [
        ("https://ex.com/govblock provider:bd", "! provider_refused bd proxy_error → "),
        ("govblock rules provider:bd", "! provider_refused bd proxy_error → "),
        ("https://ex.com/empty provider:bd", "! provider_error bd empty → "),
        // inside a `format: json` answer: Bright Data's 403, not the site's; not billed (zone counter +0)
        ("https://ex.com:81/ provider:bd", "! provider_refused bd policy_20020 → "),
    ] {
        let out = run(cmd);
        clean(&out);
        assert!(
            !out.contains("Access denied") && !out.contains("Government") && !out.contains("Bad Port"),
            "{cmd}: {out}"
        );
        assert!(out.contains("spent $0.0000"), "a refusal is not billed: {out}");
        let rows: Vec<&str> = out.lines().skip(1).filter(|l| l.contains(" ! ") || l.starts_with('!')).collect();
        assert_eq!(rows.len(), 1, "{cmd}: {out}");
        assert!(rows[0].contains(row) && rows[0].contains(&format!("→ {b} ")), "{cmd}: {out}");
        let json = run(&format!("{cmd} fresh:true json:true"));
        assert!(!json.contains("Access denied"), "{json}");
    }
    // A site status bd relayed is the site's block, a rate limit a wait; both unbilled, said so
    let out = run("https://ex.com/relayed provider:bd");
    assert!(out.contains("bd:blocked_403✗$0 (not charged)") && out.contains("! blocked_403 bd http_status"), "{out}");
    let out = run("https://ex.com/bucket provider:bd");
    let rate = format!("! rate_limited bd bucket_rate_limit → {b} u");
    assert!(
        out.contains("bd:rate_limited✗$0 (not charged)")
            && out.lines().any(|l| l.contains(&rate) && l.ends_with(" fresh:true ≤$0.0040 · last try not charged"))
            && !out.contains("nothing left"),
        "{out}"
    );
    // beside a 5xx it is an unlock failure: the 502 it is, unbilled (before the policy rule: also http_502)
    let out = run("https://ex.com/rejectblock provider:bd");
    assert!(out.contains("bd:http_502✗$0 (not charged)") && out.contains("spent $0.0000"), "{out}");
    let files = written();
    assert!(!files.contains("Access denied") && !files.contains("x-brd-error"), "a header was stored");
    assert!(files.contains("\"why\": \"proxy_error\""), "the token is in the hop record");
}

/// Oxylabs' request budget is kept as two numbers per hop and shown in health; a `RATE_LIMIT_EXCEEDED` 429 says
/// when to retry (`retry-after`, else 5 s); a 429 without that title (quota?) does not. The account number in the
/// header names never reaches a printed line or a cache file.
#[test]
fn oxylabs_budget_is_two_numbers() {
    let _t = lock();
    let mut printed = String::new();
    let mut go = |cmd: &str| {
        let out = run(cmd);
        clean(&out);
        printed.push_str(&out);
        out
    };
    assert!(go("https://ex.com/ok provider:oxy").starts_with("= j"), "a page");
    assert!(go("okq provider:oxy fetch:0").contains("ex.com/r"), "a search");
    let status = go("");
    assert!(status.contains("\noxy limits 250 scrape · 10 search\n"), "{status}");
    for (cmd, row) in [
        ("burst now provider:oxy", "! rate_limited oxy retry in 7s → "),
        ("https://ex.com/burst provider:oxy js:true", "! rate_limited oxy retry in 5s → "),
    ] {
        let out = go(cmd);
        let rows: Vec<&str> = out.lines().skip(1).filter(|l| l.contains(" ! ") || l.starts_with('!')).collect();
        assert_eq!(rows.len(), 1, "{cmd}: {out}");
        assert!(rows[0].contains(row), "{cmd}: {out}");
    }
    let out = go("quota provider:oxy");
    assert!(out.contains("! rate_limited → ") && !out.contains("retry in"), "no retry time for a maybe-quota: {out}");
    go("stats");
    go("json:true");
    let files = written();
    assert!(files.contains("\"limit\": 250") && files.contains("\"remaining\": 3"), "the two numbers are kept");
    for (what, text) in [("printed", &printed), ("written", &files)] {
        assert!(!text.contains(ACCT) && !text.contains("x-ratelimit"), "{what}: the account number leaked");
    }
}

/// A page's country comes from its domain (`.fr` → FR) unless `country:` names another. Here, not in tests/cli.rs:
/// a domain needs the plan's host lookup, and the fake transport is the only resolver (no network in tests).
#[test]
fn dry_country_from_domain() {
    let _turn = lock();
    let out = run("https://www.lemonde.fr/ dry:true");
    assert!(out.contains("· country:FR (from the domain)"), "{out}");
    let out = run("https://www.lemonde.fr/ https://www.bbc.co.uk/ country:pl dry:true");
    assert!(out.matches("· country:PL").count() == 2 && !out.contains("country:FR"), "overrides the domain: {out}");
}

/// What the router does that is not a mod is shown: `config` rows, no route row (no site log);
/// `fats routes` is gone (a `?`).
fn router_shown() {
    let config = run("config");
    assert!(!config.lines().any(|l| l.starts_with("route ")) && !config.contains("routes.json"), "{config}");
    assert!(config.contains("FATS_ACCEPT_MD FATS_TELEMETRY FATS_TELEMETRY_DEBUG DO_NOT_TRACK · status"), "{config}");
    let b = fats::lang::bin();
    for want in [
        "\ntext text, json, xml bodies (by content type or start) served as they are, l<N> per line, 300 lines",
        "\npdf plain → oxy (no JS, 30s) → bd (raw bytes), never oxyjs · ",
        "\nlow_balance $1.00 · ",
        "\nmarkdown on · plain sends Accept: text/markdown;q=1.0, text/html;q=0.9, ",
    ] {
        assert!(config.contains(want), "{want}: {config}");
    }
    assert!(!run("").contains("routes"), "no routes line in the status");
    // `fats routes` went with the learned start: a `?`, never a web search
    let (gone, rc) = run_rc("routes");
    assert!(gone.starts_with(&format!("? routes: gone (no learned starts) · a page's route: {b} <url> dry:true")));
    assert_eq!(rc, 2, "{gone}");
}

/// A wall is no serve, even when it shows the page's own words (`outline::shown`): only
/// `meta.ok` serves; the row says what came back.
#[test]
fn a_wall_showing_the_page_serves_nothing() {
    let _l = lock();
    let out = run(&format!("{}/holdwall", walled()));
    assert!(out.starts_with("= j") && out.contains(" ! ") && out.contains("Holdwall garden"), "{out}");
    assert!(!fats::cache::Cache::open().root.join("routes.json").exists(), "no site log: {out}");
}

/// No learned start and no `js:true` site order:
/// every page walks the whole ladder, `provider:auto` the same, `js:true` the table order at a fixed `≈25s`, one
/// provider at a time (its worst case = `up to`); `config` says so.
#[test]
fn every_page_walks_the_table_order() {
    let _t = lock();
    for (mods, want) in [
        ("", "fetch plain → oxy (after a captcha) → bd ∥ oxyjs after 5s → new u"),
        ("provider:auto", "fetch plain → oxy (after a captcha) → bd ∥ oxyjs after 5s → new u"),
        ("js:true", "fetch bd → oxyjs → new u1 · up to $0.0030 · ≈25s · hedge:off, one provider at a time"),
        (
            "js:true provider:auto",
            "fetch bd → oxyjs → new u1 · up to $0.0030 · ≈25s · hedge:off, one provider at a time",
        ),
    ] {
        let dry = run(&format!("https://www.route.example.com/long {mods} dry:true"));
        assert!(dry.contains(want) && !dry.contains("route:") && !dry.contains("failed"), "{mods}: {dry}");
    }
    router_shown();
}

/// A refused key (the answer echoes it), every provider refused, a low balance: the rows, their pages, the rc,
/// the records; account.json keeps numbers only. `n` = the provider calls before the account test.
fn account_refusals(file: &std::path::Path, n: usize) {
    let b = fats::lang::bin();
    let calls = || CALLS.load(Ordering::SeqCst);
    // a refused key (the answer echoes it): the keys page; the next read does not trust the failed row
    ACCOUNT.store(1, Ordering::SeqCst);
    let (out, rc) = run_rc("account fresh:true");
    clean(&out);
    let refused =
        format!("\nbd ! bad_key replace BRIGHTDATA_API_KEY → {b} setup · bd https://brightdata.com/cp/setting/users\n");
    assert!(out.contains(&refused) && out.starts_with("= account · ") && rc == 0, "one provider read: {out}");
    assert_eq!(calls(), n + 12, "bd stops at the refused key");
    assert!(run("").contains(" · bd ! bad_key · "), "status names it");
    let json = run("account json:true");
    clean(&json);
    assert_eq!(calls(), n + 14, "a failed row is read again");
    let recs: Vec<Value> = json.lines().map(|l| serde_json::from_str(l).expect("record")).collect();
    assert_eq!(recs.len(), 2, "{json}");
    assert_eq!(
        (recs[0]["provider"].as_str(), recs[0]["code"].as_str(), recs[0]["cost"].as_f64()),
        (Some("oxylabs"), Some("ok"), Some(0.0))
    );
    assert_eq!((recs[1]["provider"].as_str(), recs[1]["error"].as_str()), (Some("brightdata"), Some("bad_key")));

    // every provider refused: the command failed, rc 1, `!` on the `=` line, one record each with its error
    ACCOUNT.store(3, Ordering::SeqCst);
    let (out, rc) = run_rc("account fresh:true");
    clean(&out);
    assert!(rc == 1 && out.starts_with("= ! bad_key account · 2 providers · read now"), "{out}");
    assert!(out.contains("\noxy ! bad_key replace OXYLABS_WEB_API_KEY → ") && out.contains(&refused), "{out}");
    let json = run("account fresh:true json:true");
    assert_eq!(json.lines().filter(|l| l.contains("\"error\":\"bad_key\"")).count(), 2, "{json}");

    // under $1: the billing page
    ACCOUNT.store(2, Ordering::SeqCst);
    let out = run("account fresh:true");
    ACCOUNT.store(0, Ordering::SeqCst);
    let low = format!(
        "\nbd ! low_balance $0.42 left < $1.00 → top up, then {b} account fresh:true · bd https://brightdata.com/cp/billing\n"
    );
    assert!(out.contains("\nbd  $0.42 left · ") && out.contains(&low), "{out}");
    assert!(run("").contains(" · bd ! low_balance · "));
    let kept = std::fs::read_to_string(file).expect("account.json");
    assert!(!kept.contains(OXY) && !kept.contains(BD) && !kept.contains("Invalid api key"), "numbers only: {kept}");

    // a key scoped without one read (403 there only): the key is ok and that read says so; a 403 on
    // every read is a refused key
    ACCOUNT.store(4, Ordering::SeqCst);
    let scoped = run("account fresh:true");
    let status = run("");
    ACCOUNT.store(5, Ordering::SeqCst);
    let all = run("account fresh:true");
    ACCOUNT.store(0, Ordering::SeqCst);
    assert!(scoped.contains("\nbd  key ok (scoped: no balance api) · balance no permission · $"), "{scoped}");
    assert!(!scoped.contains("bd !") && !status.contains("bd !"), "{scoped}{status}");
    assert!(all.contains(&refused), "{all}");
    // A zone the account lacks is the same setup row as a fetch's, at once (no retry)
    ACCOUNT.store(6, Ordering::SeqCst);
    let t = std::time::Instant::now();
    let out = run("account fresh:true");
    ACCOUNT.store(0, Ordering::SeqCst);
    assert!(out.contains(&format!("\n! bad_zone bd my_unlocker → {b} setup\n")) && t.elapsed().as_secs() < 2, "{out}");
    assert!(!out.contains("again:") && out.contains(&format!("\ncalls: {b} account dry:true")), "{out}");
}

/// `fats account`: the saved answers → the expected rows; the 15-minute cache (hit, expired, `fresh:true`);
/// `dry:true` and status never call a provider; a refused key → the keys page, a low balance → the billing page;
/// no key and no provider answer is printed or written.
#[test]
fn account_rows_cache_and_links() {
    let _t = lock();
    let b = fats::lang::bin();
    let calls = || CALLS.load(Ordering::SeqCst);
    let file = dir().join("cache").join(fats::stats::ACCOUNT_FILE);
    std::fs::remove_file(&file).ok();
    let n = calls();
    assert!(run("").contains(&format!("\naccount: not read yet · {b} account\n")));
    assert_eq!(calls(), n, "status never calls a provider");

    let out = run("account");
    clean(&out);
    assert_eq!(calls(), n + 5, "oxy 1 + bd 4: {out}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("= account · 2 providers · read now · 5 calls ") && lines[0].ends_with(" · $0"));
    assert!(lines[1].starts_with("oxy  key ok · 351 scrapers · no balance api"), "{out}");
    assert_eq!(
        lines[2],
        "bd  $1,234.56 left · $5.50 spent this month · zone $3.2100 mtd · 1,500 unblocker + 700 serp reqs this month \
         · zones: dc, serp, unblocker"
    );
    assert_eq!(
        lines[3],
        format!(
            "again: {b} account fresh:true · calls: {b} account dry:true · usage: bd https://brightdata.com/cp/billing/costs"
        )
    );

    // hit: no call; status reads the file only
    let hit = run("account");
    assert!(hit.lines().next().unwrap_or_default().contains("ago (cached, ttl 15m)") && hit.contains(lines[2]));
    let dry = run("account dry:true");
    assert!(dry.starts_with("= dry account · 0 calls") && dry.contains("→ no calls"), "{dry}");
    let dry = run("account fresh:true dry:true");
    assert!(dry.starts_with("= dry account · 5 calls · read-only"), "{dry}");
    assert!(
        dry.contains("\nbd GET https://api.brightdata.com/zone/cost?from=") && dry.contains("&zone=my_unlocker · "),
        "{dry}"
    );
    assert!(dry.contains("\noxy GET https://webapi.oxylabs.io/v1/scrapers · key check"), "{dry}");
    let status = run("");
    assert!(
        status.lines().any(|l| l.starts_with("account: 2 providers · ") && l.ends_with(&format!(" ago · {b} account")))
    );
    assert_eq!(calls(), n + 5, "cache hit, dry and status call nothing");

    // expired: read again
    let mut r: Value = serde_json::from_str(&std::fs::read_to_string(&file).expect("account.json")).expect("json");
    r["t"] = (r["t"].as_f64().unwrap_or_default() - 16.0 * 60.0).into();
    std::fs::write(&file, r.to_string()).expect("age it");
    assert!(run("account").contains("· read now · 5 calls"));
    assert_eq!(calls(), n + 10);

    account_refusals(&file, n);
}

/// A zip document (docx) never goes to a provider: read as `not_in_v1`, or plain's own failure.
fn zip_documents_never_call(w: &str) {
    let calls = || CALLS.load(Ordering::SeqCst);
    let n = calls();
    let out = run(&format!("{w}/report.docx"));
    assert!(out.starts_with("= ") && out.contains("! not_in_v1 zip (docx/xlsx)"), "{out}");
    let out = run(&format!("{w}/locked.docx"));
    assert!(out.contains("! blocked_403") && !out.contains("no_key") && !out.contains("budget_hit"), "{out}");
    assert_eq!(calls(), n, "no provider call for a zip document");
}

/// A PDF behind a wall: plain's 403 sends it on to Oxylabs without JS, whose escaped copy comes back byte for byte
/// (stored as raw.pdf, 3 pages read); Oxylabs' sign-in page is `not_pdf` and Bright Data (raw bytes) serves it;
/// a file both cut short is shown with `! pdf_truncated` and the pages read. A zip document never reaches a
/// provider, served or walled.
#[test]
fn pdf_behind_a_wall() {
    let _t = lock();
    let w = walled();
    let out = run(&format!("{w}/paper.pdf"));
    clean(&out);
    let head = out.lines().next().unwrap_or_default();
    let tried = "plain:blocked_403✗ oxy✓";
    assert!(head.contains(" · pdf · 3 pages · ") && head.contains(tried) && head.contains("$0.0010"), "{out}");
    assert!(out.contains("Page three: the conclusion, signed") && !out.contains(" ! "), "{out}");
    let files = |name: &str| {
        let mut found = Vec::new();
        let mut dirs = vec![dir().join("cache")];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                match e.path() {
                    p if p.is_dir() => dirs.push(p),
                    p if p.file_name().is_some_and(|f| f == name) => found.push(std::fs::read(p).unwrap_or_default()),
                    _ => {}
                }
            }
        }
        found
    };
    assert!(files("raw.pdf").iter().any(|f| f == PDF), "the file, byte for byte (its own `\\x48` kept)");

    // the file's own `\x9c` text decodes to one byte in Oxylabs' copy: the offsets moved, Bright Data's raw bytes
    let out = run(&format!("{w}/literal.pdf"));
    let head = out.lines().next().unwrap_or_default();
    let tried = "oxy:pdf_mangled✗$0.0010 bd✓";
    assert!(head.contains(" · 3 pages · ") && head.contains(tried) && !out.contains(" ! "), "{out}");
    assert!(files("raw.pdf").iter().any(|f| f == LITERAL), "bd's exact bytes stored");

    let out = run(&format!("{w}/page.pdf"));
    no_tried_tier_in_fixes(&out);
    let head = out.lines().next().unwrap_or_default();
    let tried = "plain:blocked_403✗ oxy:not_pdf✗$0.0010 bd✓";
    assert!(head.contains(" · pdf · 3 pages · ") && head.contains(tried), "{out}");

    let out = run(&format!("{w}/cut.pdf"));
    no_tried_tier_in_fixes(&out);
    let row = out.lines().skip(1).find(|l| l.contains(" ! ")).unwrap_or_default();
    let gave_up = format!("→ nothing left to try · open {w}/cut.pdf in a browser");
    assert!(row.contains(" ! pdf_truncated 1 of 3 pages read · ") && row.ends_with(&gave_up), "bd sent it cut: {out}");
    assert!(!out.contains("no_text"), "the lost pages are the cut, not scans: {out}");
    assert!(out.contains("Page one: the walled report starts here"), "the pages read are shown: {out}");

    zip_documents_never_call(&w);
    let dry = run(&format!("{w}/x.pdf dry:true"));
    assert!(dry.contains("fetch plain → oxy → bd → new u"), "{dry}");
    // both providers sent a sign-in page: the fix does not send bd again
    let out = run(&format!("{w}/signin.pdf"));
    no_tried_tier_in_fixes(&out);
    let tried = "oxy:not_pdf✗$0.0010 bd:not_pdf✗$0.0015";
    assert!(out.contains(tried) && out.contains("→ nothing left to try · open "), "{out}");
    let out = run(&format!("{w}/missing.pdf"));
    no_tried_tier_in_fixes(&out);
    let row = out.lines().skip(1).find(|l| l.contains(" ! ")).unwrap_or_default();
    assert!(out.contains(" bd:not_found✗") && row.contains("! not_found ") && !row.contains("provider:"), "{out}");
    let out = run(&format!("{w}/gone.pdf"));
    no_tried_tier_in_fixes(&out);
    let tried = "plain:blocked_403✗ oxy:bad_key✗ bd:bad_key✗";
    assert!(out.contains(tried), "a PDF run that failed: {out}");
}

/// og_only (`fetch::og_card`): a JS shell carrying the page's own og card is served from plain as that card: mode
/// og on the `=` line, in the record and in `stats`, 1 photo, $0, no provider called, and `! js_shell → … js:true`.
/// `uN js:true` then refetches through the render tiers, past the card.
#[test]
fn og_card_serves_a_js_shell() {
    let _t = lock();
    let (n, js) = (CALLS.load(Ordering::SeqCst), JS.load(Ordering::SeqCst));
    let out = run(&format!("{}/og-own", walled()));
    let head = out.lines().next().unwrap_or_default();
    assert!(head.contains(" NASA · 127.0.0.1 · 0 sections · 1 photo · ") && head.contains("tok · og plain"), "{out}");
    assert!(
        head.contains("$0.0000 · ! js_shell → ") && head.contains(" js:true · bd, then oxyjs ≤$0.0030 ≈25s"),
        "{out}"
    );
    assert!(out.lines().any(|l| l.starts_with("m1 photo jpg | (no alt) → http://127.0.0.1:")), "{out}");
    assert_eq!(CALLS.load(Ordering::SeqCst), n, "the ladder stops at the card: {out}");
    let id = head.split_whitespace().nth(1).unwrap_or_default().rsplit('.').next().unwrap_or_default().to_string();
    assert!(run("stats full").contains("(og)"), "stats names the mode");
    let rec: Value = serde_json::from_str(&run(&format!("{id} json"))).unwrap_or_default();
    assert_eq!(rec["mode"], "og", "the record names the mode");
    // a doc of another version is rebuilt from the stored body: the card again, never the shell's own text
    let doc = PathBuf::from(rec["path"].as_str().unwrap_or_default()).join("doc.json");
    let mut old: Value = serde_json::from_str(&std::fs::read_to_string(&doc).unwrap_or_default()).expect("doc");
    old["v"] = 0.into();
    std::fs::write(&doc, old.to_string()).expect("doc.json");
    let rebuilt = run(&id);
    let head = rebuilt.lines().next().unwrap_or_default();
    assert!(head.contains(" 1 photo · ") && head.contains("tok · og cache "), "{rebuilt}");
    assert!(head.contains(" · ! js_shell → ") && rebuilt.contains("(card only)"), "{rebuilt}");
    let again = run(&format!("{id} js:true"));
    assert!(JS.load(Ordering::SeqCst) > js, "js:true runs a render tier: {again}");
    assert!(!again.lines().next().unwrap_or_default().contains(" og "), "{again}");
}

/// The render arrow names the tier its `dry:true` would call: with only the Oxylabs key, `(oxyjs) ≤$0.0015`, never
/// the both-keys route's bd price.
#[test]
fn og_card_arrow_names_the_render_tier() {
    let _t = lock();
    let mut env = Env::default();
    env.set("BRIGHTDATA_API_KEY", None);
    let out = run(&format!("{}/og-own fresh:true", walled()));
    let head = out.lines().next().unwrap_or_default();
    assert!(head.contains(" · ! js_shell → ") && head.ends_with(" js:true · oxyjs ≤$0.0015 ≈25s"), "{out}");
}

/// Never a card from the render: its shell is the render's own answer, and with the render
/// spent a `! js_shell` would have no next step. The run fails as the shell it is.
#[test]
fn og_card_never_from_the_render() {
    let _t = lock();
    let js = JS.load(Ordering::SeqCst);
    let out = run(&format!("{}/og-js", walled()));
    let head = out.lines().next().unwrap_or_default();
    assert!(JS.load(Ordering::SeqCst) > js, "the render ran: {out}");
    assert!(!head.contains(" og ") && !out.contains("(card only)"), "{out}");
}

/// The card gate: a shell carrying the site-wide card (og:title = og:site_name) or og:title alone is not the page,
/// so it is never served as one; the ladder goes on.
#[test]
fn og_card_needs_the_pages_own_card() {
    let _t = lock();
    for path in ["og-site", "og-one"] {
        let n = CALLS.load(Ordering::SeqCst);
        let out = run(&format!("{}/{path}", walled()));
        assert!(!out.contains(" og plain") && CALLS.load(Ordering::SeqCst) > n, "{path}: {out}");
    }
    // no og:title, a description and a logo: the gate cannot tell a site-wide card from
    // the page's here, so it serves the card, but never as the whole page: the logo is not counted and the
    // render stays the next step
    let out = run(&format!("{}/og-logo", walled()));
    let head = out.lines().next().unwrap_or_default();
    assert!(head.contains(" 0 photos · ") && head.contains("tok · og plain"), "{out}");
    assert!(head.contains(" · ! js_shell → ") && head.contains(" js:true · bd, then oxyjs ≤$"), "{out}");
}

/// A server's Markdown (`Accept: text/markdown` on plain): cleaned as a page, `md` on the `=` line, headings as
/// sections (not the `#` inside a fence), `![alt](src)` as m1, `[text](url)` as links, the content-signal in `stats`.
/// Off (`FATS_ACCEPT_MD=0` reads at start; here `Ask::html`'s path): a stub Markdown is refused for the HTML
/// (`plain✓md:thin`), which is served unchanged.
#[test]
fn server_markdown_is_a_page() {
    let _t = lock();
    let out = run(&format!("{}/md", walled()));
    let head = out.lines().next().unwrap_or_default();
    assert!(
        head.contains(" Workers · 127.0.0.1 · 3 sections · 1 photo · ") && head.contains("tok · md plain"),
        "{out}"
    );
    assert!(out.lines().any(|l| l.starts_with("m1 photo png | The Workers dashboard → http://127.0.0.1:")), "{out}");
    assert!(
        out.contains("the limits [l1]") && out.contains("pricing [l2]") && !out.contains("not a heading ("),
        "{out}"
    );
    assert!(run("stats full").contains("(md)") && run("stats full").contains(" · signal ai-train=yes, search=yes · "));
    let thin = run(&format!("{}/md-thin", walled()));
    let head = thin.lines().next().unwrap_or_default();
    assert!(head.contains(" 4 sections · 0 photos · ") && head.contains(" plain✓md:thin "), "{thin}");
    assert!(!head.contains(" md "), "the HTML is served: {thin}");
    // the HTML ask failing keeps the Markdown copy: served, no paid tier
    let n = CALLS.load(Ordering::SeqCst);
    let only = run(&format!("{}/md-only", walled()));
    let head = only.lines().next().unwrap_or_default();
    assert_eq!(CALLS.load(Ordering::SeqCst), n, "{only}");
    assert!(head.contains(" Workers · ") && head.contains(" plain✓md:thin+html:blocked_403 "), "{only}");
    // the stored copy (3 sections) is the yardstick, read once the Markdown came: a flat answer is asked again
    let url = format!("{}/md-flat", walled());
    assert!(run(&url).lines().next().unwrap_or_default().contains(" 4 sections · "), "the HTML first");
    let again = run(&format!("{url} fresh:true"));
    assert!(again.lines().next().unwrap_or_default().contains(" plain✓md:flat "), "{again}");
}

/// A count is printed only for a page that was seen: a JS shell served nothing, so its `=`
/// line says no `0 photos`, on the fetch and on every read after it, and `stats` neither.
#[test]
fn shell_prints_no_photo_count() {
    let _t = lock();
    let n = CALLS.load(Ordering::SeqCst);
    let out = run(&format!("{}/shell provider:plain", walled()));
    assert_eq!(CALLS.load(Ordering::SeqCst), n, "provider:plain: {out}");
    let head = out.lines().next().unwrap_or_default();
    assert!(head.contains("! js_shell") && !head.contains("photo"), "{out}");
    let id = head.split_whitespace().nth(1).and_then(|j| j.rsplit('.').next()).unwrap_or_default();
    let stats = run("stats");
    let row = stats.lines().find(|l| l.starts_with(&format!("{id} "))).unwrap_or_default();
    assert!(!row.is_empty() && !row.contains("photo"), "stats: no count for an unseen page: {stats}");
    assert!(!run(id).lines().next().unwrap_or_default().contains("photo"), "nor on a read after it");
}

/// A `/pdf/` path is a hint, not a PDF run: a page there that passes the content check is served by plain (no
/// provider call, rc 0), with or without `.html`; a walled one goes on as a page whose Bright Data call asks for
/// raw bytes, and the file it gets back is read as a PDF.
#[test]
fn pdf_path_is_a_hint() {
    let _t = lock();
    let (w, calls) = (walled(), || CALLS.load(Ordering::SeqCst));
    let n = calls();
    for url in [&format!("{w}/docs/pdf/merge.html"), &format!("{w}/pdf/guide")] {
        let (out, rc) = run_rc(url);
        let head = out.lines().next().unwrap_or_default();
        assert!(rc == 0 && head.contains("Merge PDF files") && !out.contains(" ! "), "{url}: {out}");
        assert!(!out.contains("not_pdf"), "{url}: {out}");
    }
    assert_eq!(calls(), n, "plain served both: no provider call");
    let out = run(&format!("{w}/pdf/page"));
    let head = out.lines().next().unwrap_or_default();
    let tried = "plain:blocked_403✗ oxy:error_page✗$0.0010 bd✓";
    assert!(head.contains(" · pdf · 3 pages · ") && head.contains(tried), "{out}");
    let dry = run(&format!("{w}/pdf/other dry:true"));
    assert!(dry.contains("fetch plain → oxy (after a captcha) → bd ∥ oxyjs after 5s"), "the page ladder: {dry}");
}

/// Words that look like commands never become a paid search: `status`, `ls`, `--version` run as commands, a typo
/// one edit from one (`stat`, `acount`) asks, and so does a first word up to two edits from one before a URL or
/// id (`fetchh https://x` once ran a paid site search), a `file:` URL is a local path. A query that
/// holds a configured key is refused before any provider (or jobs.json) sees it.
#[test]
fn command_like_words_never_search() {
    let _t = lock();
    let calls = || CALLS.load(Ordering::SeqCst);
    let n = calls();
    let missing = dir().join("no-such-dir").join("passwd");
    let typos = ["stat", "acount", "confg", "stauts", "fetchh", "fetchh https://ex.com/ok", "fecth u1", "acont u1"];
    for cmd in ["status", "ls", "--version", "-V", "version"].into_iter().chain(typos) {
        let (out, rc) = run_rc(cmd);
        assert!(!out.contains(" search \""), "{cmd}: {out}");
        let w = cmd.split(' ').next().unwrap_or_default();
        assert!(rc == 0 || out.starts_with(&format!("? {w}: a command? ")), "{cmd}: {out}");
    }
    let (out, rc) = run_rc(&format!("file://{}", missing.display()));
    assert!(rc == 1 && out.contains("! no_file "), "a local path: {out}");
    assert!(run("version").starts_with(&format!("= {} {}", fats::lang::bin(), env!("CARGO_PKG_VERSION"))));
    assert!(run("ls").contains("\nhealth (last "), "ls = status");
    assert_eq!(calls(), n, "no provider call");
    let jobs = || std::fs::read_to_string(dir().join("cache").join("jobs.json")).unwrap_or_default();
    for cmd in [format!("rust news {OXY}"), format!("{BD} seeds dry:true")] {
        let (out, rc) = run_rc(&cmd);
        clean(&out);
        assert!(rc == 1 && out.contains("= ! key_in_query search · ") && out.contains("! key_in_query → "), "{out}");
    }
    assert_eq!(calls(), n, "the key was not sent");
    assert!(!jobs().contains(OXY) && !jobs().contains(&format!("{BD} seeds")), "nor stored");
}

/// A host no resolver knows ends the job at once: `! no_host`, $0, no provider call; `dry:true` says so too. A
/// name the system cannot resolve but the second resolver knows goes on up the ladder.
#[test]
fn dead_host_is_final() {
    let _l = lock();
    let (calls, t0) = (CALLS.load(Ordering::SeqCst), std::time::Instant::now());
    let (out, rc) = run_rc("https://dead.invalid/menu");
    assert!(t0.elapsed() < std::time::Duration::from_secs(1), "{:?}", t0.elapsed());
    assert_eq!(rc, 1, "{out}");
    let head = out.lines().next().unwrap_or_default();
    assert!(head.contains("! no_host") && head.contains("tried plain:no_host✗") && head.contains("$0.0000"), "{out}");
    assert!(out.contains("check the host's spelling"), "{out}");
    assert_eq!(CALLS.load(Ordering::SeqCst), calls + 1, "the second resolver only, no provider: {out}");
    let dry = run("https://dead.invalid/menu dry:true");
    assert!(dry.contains("! no_host") && !dry.contains("fetch plain"), "{dry}");
    let out = run("https://alive.invalid/long");
    assert!(out.starts_with("= j") && out.contains("Learned Route Works") && out.contains("oxyjs✓"), "{out}");
    // NODATA (the name exists, no A, no AAAA) is no address either: `no_host` at once, no ladder, no http arrow
    let (calls, t0) = (CALLS.load(Ordering::SeqCst), std::time::Instant::now());
    let (out, rc) = run_rc("https://void.ex.com/page wait:30");
    assert!(t0.elapsed() < std::time::Duration::from_secs(1) && rc == 1, "{:?} {out}", t0.elapsed());
    assert!(out.contains("! no_host") && out.contains("$0.0000") && !out.contains("http://"), "{out}");
    assert_eq!(CALLS.load(Ordering::SeqCst), calls + 2, "A, then AAAA, no provider: {out}");
    let out = run("https://six.ex.com/long");
    assert!(out.contains("Learned Route Works"), "an AAAA is an address: {out}");
}

/// A name no resolver can ask for (a 64-char or empty label, a hyphen at a label's end) is `no_host` by its shape on
/// a real run too: neither resolver asked (a `lag.` name's system lookup takes 2 s, the second resolver is a
/// call), $0, never `dns_failed … → check the network`.
#[test]
fn unaskable_host_is_never_looked_up() {
    let _l = lock();
    for host in [format!("lag.{}.com", "a".repeat(64)), "lag.a..com".into(), "lag.-a.com".into(), "lag.a-.com".into()] {
        let (calls, t0) = (CALLS.load(Ordering::SeqCst), std::time::Instant::now());
        let (out, rc) = run_rc(&format!("https://{host}/page fresh:true wait:30"));
        assert!(t0.elapsed() < std::time::Duration::from_secs(1) && rc == 1, "{:?} {out}", t0.elapsed());
        let head = out.lines().next().unwrap_or_default();
        assert!(head.contains("! no_host") && head.contains("$0.0000") && !out.contains("network"), "{out}");
        assert_eq!(CALLS.load(Ordering::SeqCst), calls, "no resolver, no provider: {out}");
    }
}

/// The spend ledger: a paid fetch and a paid search each add one `[t, [cost, unverified], "<record dir>"]` row next to
/// the keys (no URL, no query: the sweep takes those within the hour), and `stats` sums every row; once the page and
/// the search are swept, the session total still carries what they cost.
#[test]
fn spend_outlives_the_pages() {
    let _l = lock();
    let ledger = dir().join("keys").join(fats::cache::SPEND_FILE);
    let text = || std::fs::read_to_string(&ledger).unwrap_or_default();
    let before = text().lines().count();
    assert!(run("https://ex.com/ok-ledger provider:oxy").starts_with("= j"), "a paid page");
    assert!(run("okq provider:oxy fetch:0").contains("ex.com/r"), "a paid search");
    assert_eq!(text().lines().count(), before + 2, "{}", text());
    let new: Vec<String> = text().lines().skip(before).map(String::from).collect();
    let parse = |l: &String| serde_json::from_str::<(f64, [f64; 2], String)>(l).ok();
    let ok = |l: &String| parse(l).is_some() && !l.contains("ex.com");
    assert!(new.iter().all(ok) && !text().contains("okq"), "numbers and a dir only: {}", text());
    let stats = run("stats");
    let row = stats.lines().find(|l| l.starts_with("spend all sessions: 24h $")).unwrap_or_default();
    assert!(row.contains(&format!(" · {} paid records · ledger ", before + 2)), "{stats}");
    let total = |s: &str| s.lines().next().and_then(|l| l.split(" spent ").nth(1)).unwrap_or_default().to_string();
    let was = total(&stats);
    let dirs: Vec<String> = new.iter().filter_map(|l| Some(parse(l)?.2)).collect();
    // by path components: a stored dir may mix `\` and `/` on Windows (`cache\t/u1`)
    assert!(dirs.iter().all(|d| std::path::Path::new(d).starts_with(dir().join("cache").join("t"))), "{dirs:?}");
    dirs.iter().for_each(|d| std::fs::remove_dir_all(d).expect("swept"));
    let after = run("stats");
    assert_eq!(total(&after), was, "{after}");
    let name = |d: &str| d.rsplit('/').next().unwrap_or_default().to_string();
    let (page, search) = (name(&dirs[0]), name(&dirs[1]));
    assert!(after.lines().any(|l| l.starts_with(&format!("{page} ex.com · swept 0ms $0.0000"))), "{after}");
    assert!(!after.contains(&format!("{search} search")), "a swept search's money is in the total only: {after}");
}

/// Bright Data's final URL (`x-unblocker-redirected-to` in its JSON `headers`) reaches the `=` line like plain's
/// and Oxylabs': a wrong slug that lands on the listing says redirected. The listing lost the asked id
/// (`Charizard-V9-BS4`) and shows none of the card's words, so the copy is no page (`redirected`, best copy shown).
#[test]
fn bright_data_redirect_says_redirected() {
    let _l = lock();
    let out = run("https://ex.com/Singles/Base-Set/Charizard-V9-BS4 provider:bd");
    let head = out.lines().next().unwrap_or_default();
    assert!(head.starts_with("= j") && head.ends_with("redirected /Singles/Base-Set"), "{out}");
    assert!(out.contains("tried bd:redirected✗") && out.contains("showing the best copy"), "{out}");
}

/// Bright Data's `status_code` past u16 is a broken answer (`provider_error`), never wrapped into another status
/// (65940 read `as u16` was 404, `not_found`).
#[test]
fn bright_data_status_never_wraps() {
    let _l = lock();
    let (out, rc) = run_rc("https://ex.com/huge-status provider:bd");
    assert_eq!(rc, 1, "{out}");
    assert!(out.contains("tried bd:provider_error✗") && !out.contains("not_found"), "{out}");
}

/// A paid ask (`js:true`, even with `hedge:0`) calls ONE provider, bd first: its 404 at once is the only call
/// (Oxylabs' 3 s answer is never asked for, never abandoned and billed); oxyjs starts only after bd failed.
#[test]
fn paid_ask_calls_one_provider() {
    let _l = lock();
    let (calls, t0) = (CALLS.load(Ordering::SeqCst), std::time::Instant::now());
    let (out, rc) = run_rc("https://ex.com/vanished js:true hedge:0");
    assert_eq!(CALLS.load(Ordering::SeqCst), calls + 2, "the resolver and bd, no oxyjs: {out}");
    assert!(t0.elapsed() < std::time::Duration::from_secs(2), "{:?} {out}", t0.elapsed());
    assert!(rc == 1 && out.contains("! not_found") && out.contains("tried bd:not_found✗$0.0015 ·"), "{out}");
    let out = run("https://ex.com/slow js:true hedge:0");
    assert!(out.contains("bd:bad_key✗ oxyjs✓ 2") && out.contains(" · hedge:off"), "{out}");
    let stats = run("stats full"); // each paid call's request shape, bd with no country sent
    let unhedged = stats.contains("$0.0015 · hedge:off raw ") && stats.contains(" · bd/60s:bad_key✗ oxyjs@US/60s✓");
    assert!(unhedged, "stats shows the run unhedged: {stats}");
    let read = run("https://ex.com/slow"); // a paid run is read from the cache, and says so there too
    assert!(read.lines().next().is_some_and(|l| l.contains(" · cache 0s · hedge:off")), "{read}");
}

/// One billing rule for both providers: a call WE cut (`timeout:1`) prints `≤$X (unverified)`, never `$0 (not
/// charged)` (bd billed 15 s search cuts printed $0), in the run's row and `stats`, whose total adds it apart.
#[test]
fn timeouts_bill_by_provider() {
    let _l = lock();
    let out = run("https://ex.com/stall js:true timeout:1 wait:5");
    // `timeout:` is the renders' one wall: bd's cut used it, oxyjs never starts
    let tried = "tried bd:provider_timeout✗≤$0.0015 (unverified) · ";
    assert!(out.contains(tried) && out.contains("spent $0.0000"), "{out}");
    // `timeout:8` once said "nothing left to try": the agent's own cut → the default time
    let row = out.lines().find(|l| l.contains(" timeout:1s → ")).unwrap_or_default();
    assert!(row.ends_with(" u1 fresh:true ≤$0.0040") && !out.contains("nothing left"), "{out}");
    let stats = run("stats");
    // The request shape proves what was asked: bd with no country, cut at the agent's 1 s
    let shaped = " · bd/1s:provider_timeout✗≤$0.0015 (unverified)";
    assert!(stats.lines().any(|l| l.contains(" $0.0000 ") && l.contains(shaped)), "{stats}");
    assert!(stats.lines().next().is_some_and(|l| l.contains(" + ≤$") && l.ends_with(" unverified")), "{stats}");
}

/// A search answer without a result list bills nothing (bd's zone +0 for 2 search `provider_error`s).
#[test]
fn a_search_without_results_is_not_billed() {
    let _l = lock();
    let out = run("nolist rust wait:20");
    let tried = "oxy:provider_error✗$0 (not charged) bd:provider_error✗$0 (not charged)";
    assert!(out.contains(tried) && !out.contains("$0.0010") && !out.contains("$0.0015"), "{out}");
    // Its job id read later says the same failure, not `0 pages`; so does `stats`
    let j = out.split(' ').nth(3).unwrap_or_default();
    let again = run(j);
    assert!(again.starts_with(&format!("= ! provider_error {j} search \"nolist rust\" · tried {tried}")), "{again}");
    let stats = run("stats");
    let row = format!("{j} search \"nolist rust\" · ! provider_error · - ");
    assert!(stats.lines().any(|l| l.starts_with(&row)), "{stats}");
    // A search sends a country on both providers (Google's `gl`), each call's timeout beside it
    let shaped = "oxy@US/15s:provider_error✗$0 (not charged) bd@US/15s:provider_error✗$0 (not charged)";
    assert!(stats.contains(shaped), "{stats}");
}

/// A search that failed the same way on the same query earlier in the session is a count, no
/// arrow: the arrow would send the call that failed twice (a provider outage = an endless $0 loop).
#[test]
fn a_repeated_search_failure_is_a_count() {
    let _l = lock();
    let first = run("nolist twice wait:20");
    assert!(first.contains("! provider_error") && first.contains(" → "), "{first}");
    let again = run("nolist twice wait:20");
    let row = again.lines().find(|l| l.starts_with("! provider_error")).unwrap_or_default();
    assert!(row.ends_with(" · failed 2/2 here") && !row.contains('→'), "{again}");
    let j = first.split(' ').nth(3).unwrap_or_default();
    assert!(run(j).contains("failed 2/2 here"), "its record read later says the count too");
}

/// Airbnb, both keys: the first answer of a fetch (its detached run, served within `wait:`) and a
/// cache read of the same copy print the same next move, byte for byte (`! thin … → fats uN#5 · 3.3k tok · $0`):
/// one printer, `outline::flags`, on every `=` line (cli `page_cmd`).
#[test]
fn the_first_answer_and_a_cache_read_print_one_arrow() {
    let _l = lock();
    let first = run("https://ex.com/thin provider:oxy fresh:true");
    let id = first.split(['.', ' ']).nth(2).unwrap_or_default().to_string();
    let flags = |out: &str| out.lines().next().unwrap_or_default().split_once(" · ! thin ").map(|f| f.1.to_string());
    let tail = flags(&first).unwrap_or_default();
    assert!(tail.contains(&format!(" {id}#")) && tail.ends_with(" tok · $0"), "{first}");
    assert_eq!(flags(&run(&id)), Some(tail), "{id}");
}

/// A fetch slower than `wait:` answers `pending` at the cap and goes on detached: its id read with `wait:0` says
/// pending at once while it runs, status lists it, then the id read with the arrow's `wait:` is the page as the run
/// ends (one poll, not a turn per 5 s) and stats has its cost. `wait:0` answers at once (`starting` when its run has
/// not begun yet).
#[test]
fn slow_fetch_answers_pending() {
    use std::time::{Duration, Instant};
    let _l = lock();
    for (wait, cap) in [("wait:1", 1000), ("wait:0", 0)] {
        let t0 = Instant::now();
        let (out, rc) = run_rc(&format!("https://ex.com/slow provider:oxy fresh:true {wait}"));
        let ms = t0.elapsed().as_millis();
        // the promise is `wait:` + 1 s (scripts/dogfood.sh fails past it), not a tighter bound a loaded machine breaks
        assert!(ms >= cap && ms < cap + 1000, "{wait}: {ms} ms {out}");
        assert_eq!(rc, 0, "{out}");
        let id = out.split(['.', ' ']).nth(2).unwrap_or_default().to_string();
        assert!(
            out.starts_with("= j")
                && out.contains(" pending ")
                && (out.contains(" · oxy running → ") || wait == "wait:0" && out.contains(" · starting → "))
                && out.contains(&format!(" {id} wait:30 · free, ")),
            "{out}"
        );
        let t0 = Instant::now();
        let again = run(&format!("{id} wait:0"));
        assert!(again.contains(" pending ") && t0.elapsed() < Duration::from_secs(1), "{again}");
        // a run that has not taken its lock yet is `starting` in status too, never `! abandoned` (seen under load)
        let status = run("status");
        assert!(status.contains(&format!("running {id} pid ")) || status.contains(&format!("running {id} starting ")));
        assert!(!status.contains("abandoned"), "{status}");
        let page = run(&format!("{id} wait:30"));
        assert!(page.contains(" Slow Page · ") && t0.elapsed() < Duration::from_secs(5), "{page}");
        let stats = run("stats");
        // the country the call sent, US where nothing named one (`stats` shows what was sent)
        assert!(
            stats.lines().any(|l| l.starts_with(&format!("{id} ex.com · ")) && l.contains(" · oxylabs@US 2")),
            "{stats}"
        );
        assert!(!run("status").contains("running u"), "nothing left running");
    }
}

/// `wait:` is ONE deadline a command, never one per URL (two walled URLs waited 2 × 30 s, past an MCP
/// host's 60 s call), and the URLs run side by side under it (not one after another: the 3rd started at the
/// deadline): both 2 s pages are served, the 6 s one is `pending` when the 3 s are up, and the `=` line counts it
/// so (not `3 ok` over pending rows).
#[test]
fn wait_is_one_deadline_for_every_url() {
    let _l = lock();
    let t0 = std::time::Instant::now();
    let out =
        run("https://ex.com/slow/b2a https://ex.org/slow/b2b https://ex.net/slow6/b2c provider:oxy fresh:true wait:3");
    let ms = t0.elapsed().as_millis();
    assert!(out.contains(" 3 inputs · 2 ok · 1 pending · ") && out.matches(" Slow Page · ").count() == 2, "{out}");
    assert!((3000..3900).contains(&ms), "{ms} ms {out}");
}

/// A pending run's promise is this session's median call on its route (host + tier), never a constant:
/// no number before 3 such calls, then 1 s left of the 2 s three Oxylabs calls took, at `wait:1`.
#[test]
fn pending_promises_the_sessions_median() {
    let _l = lock();
    let ids: Vec<String> = (1..=4)
        .map(|i| run(&format!("https://ex.com/slow/m{i} provider:oxy fresh:true wait:0")))
        .inspect(|out| assert!(out.contains(" wait:30 · free, check again in 30 s"), "{out}"))
        .map(|out| out.split(['.', ' ']).nth(2).unwrap_or_default().to_string())
        .collect();
    for id in &ids {
        assert!(run(&format!("{id} wait:5")).contains(" Slow Page · "));
    }
    let out = run("https://ex.com/slow/m5 provider:oxy fresh:true wait:1");
    assert!(out.contains(" oxy running → ") && out.contains(" wait:30 · free, usually ready in ≈1s"), "{out}");
    // never past the running call's own deadline, read 0.3 s in: `timeout:` caps every call, the oxy no-JS
    // gate (it ran 20 s under `timeout:1`) and a render alike, in either spelling
    let later = |cmd: &str| {
        let id = run(cmd).split(['.', ' ']).nth(2).unwrap_or_default().to_string();
        std::thread::sleep(std::time::Duration::from_millis(300));
        run(&format!("{id} wait:0"))
    };
    let out = later("https://ex.com/slow/m6 provider:oxy fresh:true wait:0 timeout:1");
    assert!(out.contains(" wait:30 · free, usually ready in ≈1s"), "{out}");
    for i in 7..=9 {
        assert!(
            run(&format!("https://ex.com/slow/r{i} js:true provider:oxy fresh:true wait:5")).contains(" Slow Page ")
        );
    }
    for t in ["1", "1s"] {
        let out = later(&format!("https://ex.com/slow/r{t} js:true provider:oxy fresh:true wait:0 timeout:{t}"));
        assert!(out.contains(" oxyjs running → ") && out.contains(" wait:30 · free, usually ready in ≈1s"), "{out}");
    }
    // another host is another route: no number from ex.com's calls (any host's bd calls once promised 28 s, 60.5)
    let out = run("https://ex.org/slow/m7 provider:oxy fresh:true wait:1");
    assert!(out.contains(" oxy running → ") && out.contains(" wait:30 · free, check again in 30 s"), "{out}");
    // `timeout:` is the run's one wall: the no-JS call is cut at it, and no render follows
    let t0 = std::time::Instant::now();
    let out = run("https://ex.com/slow/m8 provider:oxy fresh:true wait:5 timeout:1");
    let ms = t0.elapsed().as_millis();
    assert!(ms < 1800 && out.contains(" oxy:provider_timeout✗") && !out.contains("oxyjs:"), "{ms} ms {out}");
}

/// The CLI's last line names this session's background runs: pending with its age while it runs, `done` once
/// it ended, then never again; never the id the answer itself is about.
#[test]
fn footer_names_background_runs() {
    let _l = lock();
    let out = run("https://ex.com/slow/footer provider:oxy fresh:true wait:0");
    let id = out.split(['.', ' ']).nth(2).unwrap_or_default().to_string();
    assert!(out.contains(" pending ") && !out.contains("\n~ "), "its own id: {out}");
    let (t0, mut tails) = (std::time::Instant::now(), Vec::new());
    while !tails.iter().any(|t: &String| t.ends_with(" done")) && t0.elapsed().as_secs() < 8 {
        let s = run("config");
        tails.extend(s.lines().last().filter(|l| l.starts_with("~ ")).map(String::from));
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let next = format!(" {id} wait:30 · free, check again in 30 s");
    let first = tails.first().filter(|t| t.starts_with(&format!("~ {id} pending ")) && t.contains(" · next: "));
    assert!(first.is_some_and(|t| t.contains(&next)), "{tails:?}");
    assert_eq!(tails.last(), Some(&format!("~ {id} done")), "{tails:?}");
    assert!(!run("config").contains("\n~ "), "told once");
}

/// A batch runs in lanes, one process each, at most `batch_parallel` (8; 4 here, the mod), dealt round-robin in
/// input order whatever the host: the rest queue behind them under the same `wait:`, `pending` at it (without lanes
/// 50 URLs started 50 processes at once). Every queued URL is fetched in the end; one host's URLs run side by side
/// (one lane a host made 3 walled pages of one site wait for each other).
#[test]
fn a_batch_runs_in_lanes() {
    let _l = lock();
    let urls: Vec<String> = (0..50).map(|i| format!("https://h{}.ex.com/slowq/{i}", i % 10)).collect();
    let out = run(&format!("{} provider:oxy fresh:true wait:1 cost:1 batch_parallel:4", urls.join(" ")));
    assert!(out.starts_with("= j") && out.contains(" 50 inputs · ") && out.contains(" pending · "), "{out}");
    assert!(out.contains(".ex.com · queued after u"), "{out}");
    let t0 = std::time::Instant::now();
    while run("status").contains("running u") && t0.elapsed().as_secs() < 20 {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let calls = LANES.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(calls.len(), 50, "each URL fetched once");
    // a call takes 150 ms: the calls started within 140 ms before one are in flight with it
    let host = |u: &str| u.split('/').nth(2).unwrap_or_default().to_string();
    let beside = |(u, t): &(String, std::time::Instant)| {
        let on = calls.iter().filter(|(_, s)| s <= t && t.duration_since(*s).as_millis() < 140);
        (on.clone().count(), on.filter(|(v, _)| host(v) == host(u)).count())
    };
    let most = calls.iter().map(beside).fold(0, |a, b| a.max(b.0));
    assert!(most <= 4, "{most} in flight");
    let again = run(&format!("{} provider:oxy wait:5 cost:1", urls.join(" ")));
    assert!(again.contains(" 50 inputs · 50 ok · "), "{again}");
    let one: Vec<String> = (0..4).map(|i| format!("https://hs.ex.com/slowq/s{i}")).collect();
    run(&format!("{} provider:oxy fresh:true wait:5", one.join(" ")));
    let calls = LANES.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let (first, last) = (calls.iter().find(|c| c.0.ends_with("/s0")), calls.iter().find(|c| c.0.ends_with("/s3")));
    let apart = first.zip(last).map(|(a, b)| b.1.duration_since(a.1).as_millis());
    assert!(apart.is_some_and(|ms| ms < 140), "one host's 4 URLs at once: {apart:?} ms apart");
    // a URL twice: one run; the second row names the first's id in one line
    let dup = run("https://hd.ex.com/slowq/dup https://hd.ex.com/slowq/dup provider:oxy fresh:true wait:5");
    let calls = LANES.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|c| c.0.ends_with("/dup")).count();
    assert!(dup.contains(" 2 inputs · 2 ok · ") && calls == 1, "{calls} calls: {dup}");
    let id = dup.lines().nth(1).and_then(|l| l.split(' ').nth(1)).unwrap_or_default();
    let rows: Vec<&str> = dup.lines().filter(|l| l.starts_with(&format!("= {id} "))).collect();
    assert_eq!(rows.get(1), Some(&format!("= {id} (same as above)").as_str()), "{dup}");
}

/// A scheduler slice, the pace gap bar's slack; Windows' default timer ticks every 15.6 ms on top (a sleep or a
/// wake lands on a tick: Windows CI measured one 165 ms gap at a 200 ms pace).
const SLICE_MS: u128 = if cfg!(windows) { 25 + 16 } else { 25 };

/// THE home-IP brake waits, it never buys: 12 plain pages of one host with every key = $0 and no `!` row (a call
/// that skipped its turn would buy Bright Data, $0.0015 for a free page); the wait shows apart from the fetch ms.
/// The gaps are timed where the local site reads each request, not where the brake lets the call start: connect,
/// accept and 8 lanes on a loaded machine's scheduler move each by a few ms, so the bar is the pace less
/// `SLICE_MS`; the brake's own starts are pinned in `a_new_host_is_paced_from_its_first_start`.
#[test]
fn a_paced_batch_waits_and_spends_nothing() {
    let _l = lock();
    let urls: Vec<String> = (0..12).map(|i| format!("{}/doc/brake-page-{i}", walled())).collect();
    let out = run(&format!("{} wait:30 plain_host_pace_ms:200", urls.join(" ")));
    assert!(out.contains(" 12 inputs · 12 ok · ") && out.contains(" $0.0000\n"), "{out}");
    assert!(!out.contains(" ! ") && !out.contains("\n!"), "{out}");
    let stats = run("stats");
    assert!(stats.contains(" · waited 0.") && !stats.contains("paced"), "the wait apart from the ms: {stats}");
    // a new host's first batch too: every lane opened the pace file before the first start, each still waits
    let mut at = HITS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    at.sort();
    let gaps: Vec<u128> = at.windows(2).map(|w| w[1].duration_since(w[0]).as_millis()).collect();
    assert!(gaps.len() == 11 && gaps.iter().all(|&g| g >= 200 - SLICE_MS), "requests ms apart: {gaps:?}");
}

/// The brake on a host's first starts: lanes that all opened its pace file before the first one started (a new
/// batch's processes start at once) still start one `plain_host_pace_ms` apart. A start is the one the turn wrote,
/// read back through the held turn (a lane's clock read after `pace` returns also counts the scheduler's delay).
#[test]
fn a_new_host_is_paced_from_its_first_start() {
    use std::io::Seek;
    let _l = lock();
    let go = std::sync::Arc::new(std::sync::Barrier::new(6));
    let lane = |go: std::sync::Arc<std::sync::Barrier>| {
        fats::settings::mods(&[("plain_host_pace_ms".into(), "200".into())]);
        go.wait();
        let mut turn = fats::provider::pace("https://first-batch.example/a").0?;
        turn.rewind().ok()?;
        std::io::read_to_string(&turn).ok()?.get(..20)?.parse::<f64>().ok()
    };
    let lanes: Vec<_> = (0..6).map(|_| go.clone()).map(|go| std::thread::spawn(move || lane(go))).collect();
    let mut at: Vec<f64> = lanes.into_iter().filter_map(|l| l.join().ok().flatten()).collect();
    at.sort_by(f64::total_cmp);
    let gaps: Vec<f64> = at.windows(2).map(|w| (w[1] - w[0]) * 1000.0).collect();
    assert!(gaps.len() == 5 && gaps.iter().all(|&g| g >= 195.0), "starts ms apart: {gaps:?}");
    // the last start is the one written, whatever the file's mtime says (a copy, a sync); a file that will not open
    // (a folder by the host's name) is no turn and no wait: the call runs unbraked
    fats::settings::mods(&[("plain_host_pace_ms".into(), "200".into())]);
    let pace = fats::settings::get().cache.value.join("pace");
    let file = std::fs::File::options().write(true).open(pace.join("first-batch.example"));
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    assert!(file.and_then(|f| f.set_modified(old)).is_ok());
    assert!(fats::provider::pace("https://first-batch.example/b").1 >= 150, "the written start, not the mtime");
    std::fs::create_dir_all(pace.join("folder.example").join("x")).ok();
    let t0 = std::time::Instant::now();
    assert!(fats::provider::pace("https://folder.example/").0.is_none() && t0.elapsed().as_millis() < 50);
    fats::settings::mods(&[]);
}

/// A batch's `cost:` is ONE pool (`fetch::pooled`): a paid call holds its price before it starts, so walled pages
/// whose every rung bills never sum past it (2 walled pages at `cost:0.002` billed $0.0030 when each page got its
/// own share); 8 pages that each need one $0.0015 render all get it at `cost:0.012`, also on 2 lanes with the
/// default hedge when the first two pages' Bright Data is slow: each page holds one paid try (its dearest rung) from
/// the launch, so a hedge never buys a queued page's (the two hedged, 2 pages behind them were `budget_hit`). A page
/// left without its rung fails as `budget_hit` on the `=` line, its row naming the typed cap and who spent it, its
/// one arrow.
#[test]
fn a_batch_cost_is_one_pool() {
    let _l = lock();
    let urls: Vec<String> =
        (0..8).map(|i| format!("{}/k-wall/{}r{i}", walled(), if i < 2 { "late" } else { "" })).collect();
    let out = run(&format!("{} fresh:true wait:30 cost:0.012 batch_parallel:2", urls.join(" ")));
    assert!(out.contains(" 8 inputs · 8 ok · ") && !out.contains("budget_hit"), "{out}");
    let billed = |out: &str| out.lines().next()?.rsplit("spent $").next()?.parse::<f64>().ok();
    for (n, cap) in [(2, 0.002), (2, 0.003), (3, 0.0045), (5, 0.0075), (5, 0.004)] {
        let urls: Vec<String> = (0..n).map(|i| format!("{}/holdwall/p{n}-{cap}-{i}", walled())).collect();
        let out = run(&format!("{} fresh:true wait:30 cost:{cap}", urls.join(" ")));
        assert!(billed(&out).is_some_and(|b| b > 0.0 && b <= cap + 1e-9), "{n} pages, cost:{cap}: {out}");
        assert!(out.contains(" ! budget_hit "), "{out}");
    }
    // a cap that stops a page alone: no one else spent it
    let out = run(&format!("{}/k-wall/r9 {}/k-wall/r10 fresh:true wait:30 cost:0.001", walled(), walled()));
    assert!(out.contains(" ! budget_hit bd oxyjs not tried · $0.0030 > cost:0.001 → "), "{out}");
}

/// A page short of the pool only by a page still on plain waits for it: of two at `cost:0.002` ($0.001 each), the
/// shell needing a $0.0015 render gets it once the other's plain serves (never budget_hit beside a $0 page).
/// A page queued behind it in its lane holds its floor until its turn: no wait for it, `budget_hit` at once.
#[test]
fn a_page_waits_for_a_floor_coming_back() {
    let _l = lock();
    let w = walled();
    let out = run(&format!("{w}/long/late-pool {w}/pool/shell fresh:true wait:30 cost:0.002"));
    assert!(out.contains(" 2 inputs · 2 ok · ") && out.contains(" · plain:js_shell✗ bd✓ "), "{out}");
    assert!(!out.contains("budget_hit") && out.lines().next().is_some_and(|l| l.ends_with("spent $0.0015")), "{out}");
    let out = run(&format!("{w}/pool/shell/q-shell {w}/long/late-q fresh:true wait:30 cost:0.002 batch_parallel:1"));
    assert!(out.contains(" 2 inputs · 1 ok · 1 ! budget_hit · "), "{out}");
}

/// The page the pool left without a paid try fails as `budget_hit`: its `=` line, the batch line's count, its
/// `stats` row and its row with the one arrow, with a copy or without one (a bare `! captcha_wall` with no way out
/// when plain kept no doc); a page that bought a rung keeps the code the rung met, its budget row the arrow. Who
/// spent the pool names every page, this one too, and sums to what was billed once the batch is done (a row
/// written as its page stopped named another's unbilled hold). Two pages over a cap that covers one: one pays (the
/// other gave its share back; which one is a race), never none. A batch never quotes past its cap in `dry:true`.
#[test]
fn a_page_the_pool_starved_says_budget_hit() {
    let _l = lock();
    let w = walled();
    let two = format!("{w}/holdwall/a {w}/holdwall/b fresh:true wait:30 cost:0.002");
    let dry = run(&format!("{w}/holdwall/a {w}/holdwall/b {w}/holdwall/c wait:30 cost:0.002 dry:true"));
    assert!(dry.contains("\nbatch up to $0.0020 · cap $0.0020\n"), "{dry}");
    let out = run(&two);
    let (b, a) = starved(&out);
    let head = out.lines().next().unwrap_or_default();
    assert!(
        head.contains(" 0 ok · ") && head.contains(" 1 ! cloudflare_wall · ") && head.contains(" 1 ! budget_hit · ")
    );
    let copy = format!("{a} ! cloudflare_wall · page content shown ");
    let spent = format!("{a} ! budget_hit oxyjs not tried · $0.0030 > cost:0.002 (spent $0.0015: {a} $0.0015) → ");
    let hungry = format!("{b} ! budget_hit bd oxyjs not tried · $0.0030 > cost:0.002 (spent $0.0015: {a} $0.0015) → ");
    assert!([copy, spent, hungry].iter().all(|r| out.contains(&format!("\n{r}"))), "{out}");
    // no doc: plain found nothing to keep
    let none = "http://127.0.0.1:9/holdwall/k-capslow-a http://127.0.0.1:9/holdwall/k-capslow-b fresh:true wait:30";
    let out = run(&format!("{none} cost:0.0015"));
    let b = starved(&out).0;
    // the other holds its $0.0015 render until it stops, then $0.0010 billed: either is what `b` met
    let row = format!("\n{b} ! budget_hit bd oxyjs not tried · $0.0030 > cost:0.0015 (spent $0.001");
    for read in [out.clone(), run(&b)] {
        assert!(read.contains(&format!(".{b} ! budget_hit · 127.0.0.1 · tried plain:connect_failed✗ · ")), "{read}");
        let arrows = read.lines().filter(|l| l.starts_with(&format!("{b} ")) && l.contains(" → ")).count();
        assert!(read.contains(&row) && arrows == 1 && !read.contains(" held"), "one row, one arrow: {read}");
    }
    assert!(run("stats").lines().any(|l| l.starts_with(&format!("{b} ")) && l.contains(" ! budget_hit")));
    // every page spent: the parts sum to the cap
    let three: Vec<String> = (0..3).map(|i| format!("{w}/holdwall/sum{i}")).collect();
    let out = run(&format!("{} fresh:true wait:30 cost:0.0045", three.join(" ")));
    let [a, b, c] = <[String; 3]>::try_from(ids(&out)).unwrap_or_default();
    let sum = format!(" (spent $0.0045: {a} $0.0015, {b} $0.0015, {c} $0.0015) → ");
    assert!(out.contains(&sum), "{out}");
}

/// A batch's page ids (`u1`), in its `=` lines' order.
fn ids(out: &str) -> Vec<String> {
    let id = |l: &str| Some(l.strip_prefix("= j")?.split(' ').next()?.split_once('.')?.1.to_string());
    out.lines().filter_map(id).collect()
}

/// Of a two-page batch: the page the pool starved (`budget_hit` before any paid rung), and the other.
fn starved(out: &str) -> (String, String) {
    let [a, b] = <[String; 2]>::try_from(ids(out)).unwrap_or_default();
    if out.contains(&format!("\n{b} ! budget_hit bd ")) { (b, a) } else { (a, b) }
}

/// A starved page's way out: with no key a raised cost buys nothing (`setup`); once a newer copy of it served
/// (its arrow followed), it and its `stats` row point there at $0, never the same `cost:` arrow again.
#[test]
fn a_starved_pages_newer_copy_is_its_move() {
    let _l = lock();
    // no doc: plain could not connect
    let out =
        run("http://127.0.0.1:9/k-wall/starve-a http://127.0.0.1:9/k-wall/starve-b fresh:true wait:30 cost:0.0015");
    let b = starved(&out).0;
    assert!(out.contains(&format!("\n{b} ! budget_hit ")), "{out}");
    let mut keys = Env::default();
    keys.set("OXYLABS_WEB_API_KEY", None).set("BRIGHTDATA_API_KEY", None);
    let none = run(&b);
    assert!(none.contains(&format!(" → {} setup", fats::lang::bin())) && !none.contains("fresh:"), "{none}");
    drop(keys);
    let fixed = run(&format!("{b} fresh:true wait:30 cost:0.003"));
    let c = fixed.lines().next().and_then(|l| l.strip_prefix("= ")?.split(' ').next()?.rsplit('.').next());
    let c = c.unwrap_or_default().to_string();
    let row = run("stats").lines().find(|l| l.starts_with(&format!("{b} "))).unwrap_or_default().to_string();
    for read in [run(&b), row] {
        assert!(read.contains(&format!(" → {} {c}", fats::lang::bin())) && !read.contains("cost:0.00"), "{read}");
    }
}

/// No paid call before a verdict on the host's address: a name no resolver answers for is final at $0. The system
/// resolver's error back at once (the fake's: no network) is `dns_failed` with its words, the arrow the same page
/// fresh, no `dns_wait:` (a longer wait cures nothing there); none within `dns_wait` is `dns_timeout`, its arrow
/// more time (at 1.5 s every provider ran: a name of 127.0.0.1 billed a render). A slow system resolver (the fake's
/// `lag.*`: 2 s) is waited for up to `dns_wait`, never asked twice.
#[test]
fn no_address_verdict_no_paid_call() {
    let _l = lock();
    // SERVFAIL (`mute.`) and an unreachable second resolver (`down.`) alike: no verdict, never a ladder
    for host in ["mute", "down"] {
        let calls = CALLS.load(Ordering::SeqCst);
        let (out, rc) = run_rc(&format!("https://{host}.ex.com/page fresh:true wait:30"));
        assert_eq!(CALLS.load(Ordering::SeqCst), calls + 1, "the second resolver only: {out}");
        let row = out.lines().find(|l| l.starts_with('u') && l.contains(" ! dns_failed ")).unwrap_or_default();
        assert!(rc == 1 && row.contains(" dns_failed plain nodename nor servname provided, or not known → "), "{out}");
        assert!(row.ends_with(" fresh:true") && !out.contains("dns_wait:") && !out.contains("dns_timeout"), "{out}");
        assert!(out.contains(" spent $0.0000"), "{out}");
    }
    // `dry:true` looks nothing up (no network at all, CI's sandbox has none): no resolver call, no wait on a
    // slow one, its route quoted; `dns_timeout` is the real run's verdict only
    let (calls, t0) = (CALLS.load(Ordering::SeqCst), std::time::Instant::now());
    for url in ["https://mute.ex.com/page", "https://lag.ex.com/page"] {
        let dry = run(&format!("{url} fresh:true dns_wait:1 dry:true"));
        assert!(dry.contains(" fetch plain") && !dry.contains("dns_timeout"), "{dry}");
    }
    assert!(t0.elapsed() < std::time::Duration::from_secs(1), "no lookup: {:?}", t0.elapsed());
    assert_eq!(CALLS.load(Ordering::SeqCst), calls, "no second resolver either");
    let (out, rc) = run_rc("https://lag.ex.com/page fresh:true wait:30 dns_wait:1");
    assert_eq!(CALLS.load(Ordering::SeqCst), calls, "no answer by 1 s, no call at all: {out}");
    assert!(rc == 1 && out.contains("dns_timeout") && out.contains(" spent $0.0000"), "{out}");
    assert!(out.contains(" fresh:true dns_wait:15"), "no answer in time, more time: {out}");
    let out = run("https://lag.ex.com/page fresh:true wait:30");
    assert!(!out.contains("dns_timeout"), "answered at 2 s, inside the 5 s wait: {out}");
}

/// A batch's ids are the input order's, so `dry:true` names the ids the run fills: new URLs and a cache hit
/// between them, two hosts, one URL twice.
#[test]
fn a_batch_dry_names_the_run_ids() {
    let _l = lock();
    run("https://hx.ex.com/slowq/cached provider:oxy wait:5");
    let cmd = "https://hx.ex.com/slowq/i1 https://hx.ex.com/slowq/cached https://hy.ex.com/slowq/i2 \
               https://hx.ex.com/slowq/i1 https://hx.ex.com/slowq/i3 provider:oxy wait:5";
    let dry = run(&format!("{cmd} dry:true"));
    let named: Vec<&str> = dry.split(" → new ").skip(1).filter_map(|r| r.split(' ').next()).collect();
    let out = run(cmd);
    let rows = out.lines().skip(1).filter(|l| l.starts_with("= j"));
    let ids: Vec<&str> = rows.filter_map(|l| l.split(['.', ' ']).nth(2)).collect();
    assert_eq!(named.len(), 4, "{dry}");
    assert_eq!([named[0], named[1], named[2], named[0], named[3]].to_vec(), ids, "{dry}\n{out}");
    assert!(!out.contains("\n~ "), "every run is named on its own `=` row, never again in the footer: {out}");
}

/// One URL in flight is fetched once: a second call for it (and `fresh:true` on its id) answers the first run's
/// `pending` at `wait:0`, its page as it ends within `wait:`; it starts no run of its own.
#[test]
fn one_fetch_per_url_in_flight() {
    let _l = lock();
    let first = run("https://ex.com/slow/once provider:oxy fresh:true wait:0");
    let id = first.split(['.', ' ']).nth(2).unwrap_or_default().to_string();
    assert!(first.contains(" pending "), "{first}");
    for again in ["https://ex.com/slow/once provider:oxy fresh:true wait:0", &format!("{id} fresh:true wait:0")] {
        let out = run(again);
        assert!(
            out.contains(" pending ") && out.contains(&format!(" {id} wait:30 · free, check again in 30 s")),
            "{again}: {out}"
        );
    }
    // without `wait:` the id waits for the run in flight: its page is the fresh copy, no second call
    let page = run(&format!("{id} fresh:true"));
    assert!(page.contains(" Slow Page · ") && !page.contains(" pending "), "{page}");
    assert_eq!(ONCE.load(Ordering::SeqCst), 1, "provider calls for one URL in flight");
}

/// `wait:0` answers at once, also for a `fresh:true` of a fetch still running (2 s before: the call waited for the
/// run to take its lock), and a pending `<url> <words>` keeps the words in its next move.
#[test]
fn wait_zero_is_instant_and_keeps_words() {
    use std::time::{Duration, Instant};
    let _l = lock();
    let out = run("https://ex.com/slow apples provider:oxy fresh:true wait:0");
    let id = out.split(['.', ' ']).nth(2).unwrap_or_default().to_string();
    assert!(out.contains(" pending ") && out.contains(&format!(" {id} apples wait:30 · free, check")), "{out}");
    let t0 = Instant::now();
    let again = run(&format!("{id} fresh:true wait:0"));
    let ms = t0.elapsed().as_millis();
    eprintln!("wait:0 on a running id: {ms} ms");
    assert!(again.contains(" pending ") && ms < 300, "{ms} ms {again}");
    let t0 = Instant::now();
    while run(&id).contains(" pending ") && t0.elapsed() < Duration::from_secs(8) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Oxylabs' answer for a missing page is the site's 404 (in `metadata.statuses`): `! not_found`, final, billed like
/// any answer (live 2026-10-13: Bright Data `billed=true`, Oxylabs counted it), no other tier asked.
#[test]
fn oxylabs_404_is_not_found() {
    let _l = lock();
    let calls = CALLS.load(Ordering::SeqCst);
    let (out, rc) = run_rc("https://ex.com/gone404 provider:oxy");
    assert_eq!(rc, 1, "{out}");
    assert!(out.starts_with("= j") && out.contains("spent $0.0010 · hedge:off · ! not_found\n"), "{out}");
    assert!(out.contains(" · tried oxy:not_found✗$0.0010 · showing the best copy"), "{out}");
    assert_eq!(CALLS.load(Ordering::SeqCst), calls + 2, "the resolver and oxy, not oxyjs: {out}");
}

/// A finished Oxylabs job that relays the site's own 503 is billed (its "Not charged" is only its own API
/// error envelope, developers.oxylabs.io/products/web-api/troubleshooting), printed `(assumed)` until one is
/// reconciled on the dashboard; Bright Data's inner 5xx stays `$0 (not charged)`
/// (`bright_data_refusal_names_its_code`).
#[test]
fn oxylabs_relayed_site_5xx_is_billed() {
    let _l = lock();
    let out = run("https://ex.com/site503 provider:oxy");
    assert!(out.contains("oxy:http_503✗$0.0010 (assumed)") && !out.contains("spent $0.0000"), "{out}");
}

/// `fats setup`: each key checked with its one free read, saved 0600 in the key dir, the zone picked from the
/// account's Web Unlocker zones; a refused key is not saved; `dry:true` calls nothing. No key ever prints. A key
/// dir that exists at 0755 (an installer wrote its receipt there first) is made 0700.
#[test]
fn setup_checks_and_saves_keys() {
    let _l = lock();
    let keys = dir().join("keys");
    std::fs::remove_dir_all(&keys).ok();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(std::fs::DirBuilder::new().recursive(true), 0o755)
        .create(&keys)
        .expect("key dir");
    let calls = CALLS.load(Ordering::SeqCst);
    let (out, rc) = run_rc("setup dry:true");
    assert!(rc == 0 && out.starts_with("= dry setup oxy not checked · bd not checked"), "{out}");
    assert!(
        out.contains("\nbd key from env · would read GET https://api.brightdata.com/zone/get_active_zones"),
        "{out}"
    );
    assert_eq!(CALLS.load(Ordering::SeqCst), calls, "dry: no call");
    let (out, rc) = run_rc("setup");
    assert_eq!(rc, 0, "{out}");
    assert!(out.starts_with("= setup oxy ok 351 scrapers · bd ok zone my_unlocker · decodo none · key dir "), "{out}");
    assert!(out.contains("\nbd ok zone my_unlocker · key from env · saved auth.json settings.json (600)\n"), "{out}");
    assert_eq!(CALLS.load(Ordering::SeqCst), calls + 2, "one read per provider: {out}");
    let saved = |f: &str| std::fs::read_to_string(keys.join(f)).unwrap_or_default();
    for (f, v) in [
        ("auth.json", format!("\"oxy\": \"{OXY}\"")),
        ("auth.json", format!("\"bd\": \"{BD}\"")),
        ("settings.json", "\"bd_zone\": \"my_unlocker\"".into()),
    ] {
        assert!(saved(f).contains(&v), "{f}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(keys.join(f)).map(|m| m.permissions().mode() & 0o777).unwrap_or(0);
            assert_eq!(mode, 0o600, "{f}");
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&keys).map(|m| m.permissions().mode() & 0o777).unwrap_or(0);
        assert_eq!(mode, 0o700, "key dir");
    }
    let (out, rc) = run_rc("setup zone:nope");
    assert_eq!(rc, 1, "{out}");
    let b = fats::lang::bin();
    let row = format!(
        "\nbd ! no_zone nope is not a Web Unlocker zone here · unblocker zones: zone_b, my_unlocker → {b} setup bd zone:zone_b\n"
    );
    assert!(out.contains(&row), "{out}");
    std::fs::remove_dir_all(&keys).ok();
    ACCOUNT.store(3, Ordering::SeqCst);
    let (out, rc) = run_rc("setup");
    ACCOUNT.store(0, Ordering::SeqCst);
    assert!(rc == 1 && out.starts_with("= setup oxy ! bad_key · bd ! bad_key"), "{out}");
    assert!(!keys.join("auth.json").exists(), "a refused key is not saved");
    for k in [OXY, BD] {
        assert!(!out.contains(k), "{out}");
    }
}

/// `fetch:N`: a search starts its top N results (default 1) fetching detached, within `cost:` (the search's cost
/// first); `fats u<N>` then reads the stored page; `fetch:0` starts none; `dry:true` says what it would start. A
/// bare id from before the search is flagged as older than the search's results.
#[test]
fn search_prefetches_top_results() {
    let _l = lock();
    let page = run("https://ex.com/long0 provider:oxy");
    let older = page.split(['.', ' ']).nth(2).unwrap_or_default().to_string();
    let calls = CALLS.load(Ordering::SeqCst);
    let out = run("preq a provider:oxy fetch:3");
    let job = out.split(' ').nth(1).unwrap_or_default();
    let stale = run(&older);
    assert!(stale.contains(&format!(" (1 job ago; newest {job})")), "a search's ids are newer too: {page} {stale}");
    let first: usize = out
        .lines()
        .find(|l| l.contains("/long1-preq-a"))
        .and_then(|l| l.split_whitespace().next()?.strip_prefix('u')?.parse().ok())
        .unwrap_or_default();
    let (a, c) = (format!("u{first}"), format!("u{}", first + 2));
    // three pages through plain only, whatever `provider:` the search named: free
    let row = format!("\nprefetching {a} u{} {c} free, plain only (detached; fetch:0 = none) · status: ", first + 1);
    assert!(out.contains(&row), "{out}");
    settle(&(first..first + 3).map(|n| format!("u{n}")).collect::<Vec<_>>());
    let page = run(&a);
    assert!(
        page.starts_with("= j") && page.contains(" Plain Prefetch Works · 127.0.0.1") && !page.contains("pending"),
        "{page}"
    );
    assert_eq!(CALLS.load(Ordering::SeqCst), calls + 1, "the search; a prefetch calls no provider");
    // free: `cost:` never stops one
    let capped = run("preq b provider:oxy cost:0.0015 fetch:3");
    let started = capped.split("prefetching ").nth(1).map(|r| r.split(" free").next().unwrap_or_default());
    assert_eq!(started.map(|r| r.split(' ').count()), Some(3), "{capped}");
    settle(&started.unwrap_or_default().split(' ').map(String::from).collect::<Vec<_>>());
    assert!(!run("preq c provider:oxy fetch:0").contains("prefetching"));
    // past `batch_parallel` (8; 4 by its mod) the rest queue in its lanes: 4 processes for 6 results
    let six = run("preq e provider:oxy fetch:6 batch_parallel:4");
    let ids: Vec<&str> =
        six.split("prefetching ").nth(1).and_then(|r| r.split(" free").next()).unwrap_or_default().split(' ').collect();
    let by = |id: &&str| {
        let run = std::fs::read_to_string(dir().join(format!("cache/t/run/{id}.json"))).unwrap_or_default();
        serde_json::from_str::<Value>(&run).ok().and_then(|r| r["by"].as_u64())
    };
    let heads = ids.iter().filter(|id| by(id) == Some(0)).count();
    assert_eq!((ids.len(), heads), (6, 4), "{six}");
    settle(&ids.iter().map(|id| id.to_string()).collect::<Vec<_>>());
    let dry = run("preq d provider:oxy dry:true");
    assert!(dry.contains("\nthen fetch:1 prefetches the top 1 detached · free, plain only\n"), "{dry}");
}

/// The first read right after a search waits for the result's prefetch like a fetch (default `wait:`), and
/// answers its page the moment the run ends; `wait:0` answers `pending` at once. One string, as `fats mcp` passes it.
#[test]
fn a_read_after_a_search_waits_for_its_prefetch() {
    use std::time::{Duration, Instant};
    let _l = lock();
    let mcp = |cmd: &str| fats::cli::run(&[cmd.to_string()]).0;
    let first = |out: &str| out.split("prefetching ").nth(1).and_then(|r| r.split(' ').next()).map(String::from);
    let out = mcp("preq late provider:oxy");
    let id = first(&out).unwrap_or_else(|| panic!("{out}"));
    let t0 = Instant::now();
    let page = mcp(&format!("{id} free copy"));
    let ms = t0.elapsed().as_millis();
    assert!(page.contains(" 1 hit ") && page.contains("A free copy") && !page.contains(" pending "), "{ms} ms {page}");
    assert!(t0.elapsed() < Duration::from_secs(5), "returns as the run ends, never at the cap: {ms} ms");
    let out = mcp("preq late0 provider:oxy");
    let id = first(&out).unwrap_or_else(|| panic!("{out}"));
    let t0 = Instant::now();
    let now = mcp(&format!("{id} wait:0"));
    assert!(now.contains(" pending ") && t0.elapsed() < Duration::from_secs(1), "{now}");
    settle(&[id]);
}

/// Waits (≤ 5 s) until none of `ids` is `pending`: every detached run ends before the next test counts calls,
/// and before the test's dir goes (a run thread still writing would make it again).
fn settle(ids: &[String]) {
    let t0 = std::time::Instant::now();
    while ids.iter().any(|id| run(id).contains(" pending ")) && t0.elapsed().as_secs() < 5 {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// `site:` is re-checked on every result (the engine may ignore it): results off the host get no id and are
/// never prefetched, a `!` row counts them; none left says so.
#[test]
fn search_drops_off_site_results() {
    let _l = lock();
    let out = run("site:ex.com mixed fetch:0");
    assert!(out.contains(" · 2 results · ") && out.contains("\n! dropped 2 off-site · not on ex.com"), "{out}");
    assert!(out.contains("ex.com/long-on") && out.contains("docs.ex.com/long-under"), "{out}");
    assert!(!out.contains("notex.com") && !out.contains("other.org"), "{out}");
    let job = out.split_whitespace().nth(1).unwrap_or_default();
    let view = run(job);
    assert!(view.starts_with(&format!("= {job} 2 results · ")), "results, never pages: {view}");
    let none = run("site:ex.com offsite fetch:0");
    assert!(none.contains(" · 0 results · ") && none.contains("\nno on-site results · try without site: → "), "{none}");
}

/// A prefetch runs plain only (speculative work is free) and learns nothing; the background line says why it
/// failed and that it was not charged; `fats u<N>` then fetches the page through the whole ladder; a bare `more` /
/// `up` on it never does.
#[test]
fn prefetch_never_renders() {
    let _l = lock();
    let js = JS.load(Ordering::SeqCst);
    let out = run("shellq a fetch:3");
    let first: usize = out
        .lines()
        .find(|l| l.contains("/shell1-shellq-a"))
        .and_then(|l| l.split_whitespace().next()?.strip_prefix('u')?.parse().ok())
        .unwrap_or_default();
    let row = format!("\nprefetching u{first} u{} free, plain only (detached;", first + 1);
    assert!(out.contains(&row), "{out}");
    let calls = CALLS.load(Ordering::SeqCst);
    // the run file is pretty JSON (`"done": {`): parse it, never match its spacing
    let done = |n: usize| {
        let r = std::fs::read_to_string(dir().join(format!("cache/t/run/u{n}.json"))).unwrap_or_default();
        serde_json::from_str::<Value>(&r).is_ok_and(|j| j["done"].is_object())
    };
    let t0 = std::time::Instant::now();
    while !(done(first) && done(first + 1)) && t0.elapsed().as_secs() < 10 {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(done(first) && done(first + 1), "both prefetches ended within 10 s");
    assert_eq!((JS.load(Ordering::SeqCst), CALLS.load(Ordering::SeqCst)), (js, calls), "a prefetch calls no provider");
    let foot = format!("\n~ prefetch u{first} failed · blocked_403 · not charged · prefetch u{} failed · ", first + 1);
    assert!(run("config").contains(&foot), "the reason, free");
    // a read word with no id reads what is there: `more` the search's list (its cursor
    // survived the prefetch's own state writes), `up` the failed page's best copy, never the paid ladder
    for (bare, says) in [("more", "search \"shellq a\""), ("up", " Forbidden · ")] {
        let o = run(bare);
        assert_eq!(CALLS.load(Ordering::SeqCst), calls, "`{bare}` never fetches: {o}");
        assert!(o.lines().next().is_some_and(|l| l.contains(says)), "{bare}: {o}");
    }
    let page = run(&format!("u{first}"));
    assert!(page.contains("Rendered Shell"), "the open escalates: {page}");
    assert!(JS.load(Ordering::SeqCst) > js);
}

/// `fats gh <words>`: GitHub repositories by stars as unfetched ids (`★12.4k owner/repo · description · pushed
/// date`), free, sent the token; a rate limit says when to retry; a refused token names its fix, never itself.
#[test]
fn gh_lists_repositories() {
    let _l = lock();
    let out = run("gh ghok fetch:0");
    clean(&out);
    assert!(out.contains("\" · 2 results · github · ") && out.contains(" · spent $0.0000\n"), "{out}");
    assert!(out.contains("  ★12.4k owner/repo · A repo · pushed 2026-10-01\n"), "{out}");
    assert!(out.contains("  ★7 owner/bare · pushed 2025-01-02\n"), "no description: {out}");
    assert!(run("stats").contains(" search \"") && run("stats full").contains(" · gh✓"));
    let dry = run("gh ghok dry:true");
    assert!(dry.contains("\nsearch github \"ghok\" by stars · limit:10 → new u"), "{dry}");
    for (q, row) in [("ghwait", "retry in 42s → "), ("ghspent", "retry in 6")] {
        let (out, rc) = run_rc(&format!("gh {q} fetch:0"));
        assert!(rc != 0 && out.contains(&format!("\n! rate_limited gh {row}")) && !out.contains('≤'), "{q}: {out}");
        // the job read again is the same answer, free (once `! rate_limited → … ≤$0.0025`, a web search's price)
        let j = out.split(' ').nth(3).unwrap_or_default();
        assert_eq!(run(j).lines().nth(1), out.lines().nth(1), "{q}: {out}");
    }
    // a login-shaped word also lists that account's repos, merged by stars (its own repos bear no word a
    // search matches); its failure is said on the `=` line, never a quiet search-only list; a phrase asks once
    let calls = CALLS.load(Ordering::SeqCst);
    let out = run("gh tokio-rs fetch:0");
    assert!(
        out.contains(" · 5 results · github · ")
            && out.lines().nth(1).is_some_and(|l| l.contains("  ★33.4k tokio-rs/tokio · A")),
        "{out}"
    );
    assert!(out.contains("tokio-rs/axum") && out.find("tokio-rs/mio") > out.find("owner/repo"), "by stars: {out}");
    assert_eq!(CALLS.load(Ordering::SeqCst), calls + 2, "the search and the account, at once");
    run("gh two words fetch:0");
    assert_eq!(CALLS.load(Ordering::SeqCst), calls + 3, "a phrase is no login");
    let out = run("gh ghnobody fetch:0 json:true");
    assert!(out.contains(r#""provider":"github""#), "the HOW note never leaks into the record: {out}");
    for (q, note) in
        [("ghnobody", " · no user ghnobody · "), ("ghcore", " · user lookup: rate_limited retry in 720s · ")]
    {
        let out = run(&format!("gh {q} fetch:0"));
        assert!(
            out.contains(&format!(" · 2 results · github{note}")) && out.contains("  ★12.4k owner/repo · A repo"),
            "{out}"
        );
    }
    // the account call failing beside an empty search is the answer's `!` row, never a quiet `0 results`
    let (out, rc) = run_rc("gh ghcore0 fetch:0");
    assert!(rc != 0 && out.contains("\n! rate_limited gh user ghcore0 retry in 720s → "), "{out}");
    let stats = run("stats full");
    assert!(
        stats.contains(" search \"tokio-rs\" · 5 results · github ") && stats.contains(" · gh✓ gh✓"),
        "both calls: {stats}"
    );
    let out = run("gh ghbad fetch:0");
    clean(&out);
    let fix = format!("\n! bad_key gh → {} setup · gh https://github.com/settings/tokens", fats::lang::bin());
    assert!(out.contains(&fix), "{out}");
    assert!(out.starts_with("= ! bad_key j") && out.contains(" search \"ghbad\" · tried gh:bad_key✗ · "), "{out}");
    assert!(run(out.split(' ').nth(3).unwrap_or_default()).contains(&fix), "{out}");
    // GitHub is a search of its own, never a tier: a page fetch cannot be sent to it
    for cmd in ["https://ex.com/a provider:gh dry:true", "https://ex.com/a provider:github"] {
        let (out, rc) = run_rc(cmd);
        assert!(rc == 2 && out.contains(" · use provider:auto|plain|bd|oxy|decodo"), "{cmd}: {out}");
    }
}

/// A login's repos come by stars from the search (`user:<login>`: the account list holds only its 100 last
/// pushed); an account the search leaves out (422) falls back to that list, its ★9999 repo kept.
#[test]
fn gh_lists_a_login_the_search_leaves_out() {
    let _l = lock();
    let calls = CALLS.load(Ordering::SeqCst);
    let out = run("gh ghhidden fetch:0");
    assert!(out.find(" ghhidden/oldstar").is_some_and(|i| out.find(" owner/bare").is_some_and(|j| i < j)), "{out}");
    assert_eq!(CALLS.load(Ordering::SeqCst), calls + 3, "the search, the account search, its list");
}

/// Without a token (no `GH_TOKEN`, no `gh` on the PATH) the list says GitHub's tighter limit on its `=` line, in
/// the HOW part; the record's `provider` stays `github` (the row layout and the JSON contract read it).
#[test]
fn gh_without_a_token_says_its_limit() {
    let _l = lock();
    let mut env = Env::default();
    env.set("GH_TOKEN", None).set("PATH", Some(""));
    let out = run("gh ghok fetch:0");
    assert!(out.contains(" · 2 results · github · no token: 10 searches/min · "), "{out}");
    assert!(run("gh ghok fetch:0 json:true").contains(r#""provider":"github""#));
}

/// A sign-in gate (plain redirected to an onboarding shell that carries the asked path back) gets ONE paid render,
/// not two: the gate answers every render alike (a profile's sign-in shell: bd and oxyjs billed for the same shell).
#[test]
fn a_login_wall_gets_one_render() {
    let _t = lock();
    let js = JS.load(Ordering::SeqCst);
    let out = run(&format!("{}/profile/all fresh:true", walled()));
    assert_eq!(JS.load(Ordering::SeqCst), js + 1, "one render: {out}");
    let row = out.lines().find(|l| l.contains(" ! login_wall ")).unwrap_or_default();
    assert!(row.contains(" · tried plain:login_wall✗ bd:js_shell✗$0.0015 · "), "{out}");
    // no paid move: another render meets the same gate; the money is the tried path's billed `$`, no quote glyph
    assert!(
        row.contains("$0.0015 · showing the best copy → nothing left to try · open http") && !row.contains("≤$"),
        "{out}"
    );
}

/// A moved landing names a sign-in by a whole word, a compound by its last: a consent page's `login-free` mode
/// and a signing guide are bounces (`redirected`), a customer login is a gate; a served page whose own path holds a
/// sign-in word is the page, free, with no render.
#[test]
fn a_sign_in_is_named_by_the_landing() {
    let _t = lock();
    for (n, (_, code)) in GOES.iter().enumerate() {
        let out = run(&format!("{}/go/{n}-abcdefghij fresh:true cost:0", walled()));
        assert!(out.contains(&format!(" plain:{code}✗")), "{n}: {out}");
    }
    for doc in ["login-best-practices", "signing-contracts-guide"] {
        let calls = CALLS.load(Ordering::SeqCst);
        let out = run(&format!("{}/doc/{doc} fresh:true", walled()));
        let head = out.lines().next().unwrap_or_default();
        assert!(head.contains(" plain ") && head.ends_with("spent $0.0000") && !head.contains(" ! "), "{out}");
        assert_eq!(CALLS.load(Ordering::SeqCst), calls, "no provider call: {out}");
    }
}

/// Plain, Bright Data and the Oxylabs render all bring the app's JS shell (no card): shells
/// from two providers are an app page, `! no_copy`; with every keyed render tried the move is the key whose
/// render is untried (`! no_key decodo`), never "nothing left".
/// One provider's shell is that provider's failure: the other provider still renders (airbnb).
#[test]
fn two_providers_shells_are_an_app_page() {
    let _t = lock();
    let (js, calls) = (JS.load(Ordering::SeqCst), CALLS.load(Ordering::SeqCst));
    let out = run(&format!("{}/deadshell fresh:true", walled()));
    let head = out.lines().next().unwrap_or_default();
    assert_eq!(JS.load(Ordering::SeqCst), js + 2, "bd shell, then oxyjs: {out}");
    assert_eq!(CALLS.load(Ordering::SeqCst), calls + 2, "bd and oxyjs, no oxy no-JS after a shell: {out}");
    assert!(
        head.contains(" · ! no_copy")
            && out.contains("tried plain:js_shell✗ bd:js_shell✗$0.0015 oxyjs:js_shell✗$0.0015"),
        "{out}"
    );
    let row = out.lines().find(|l| l.contains("! no_copy ")).unwrap_or_default();
    assert!(row.ends_with(" setup") && !out.contains("nothing left"), "every keyed render was tried: {out}");
    assert!(out.contains("\nu1 ! no_key decodo · would try JS render ≤$0.0015 · set DECODO_AUTH_TOKEN"), "{out}");
    let id = head.split(['.', ' ']).nth(2).unwrap_or_default();
    // `dry:true` on that id with `js:true` quotes the refetch, never `local read · $0`
    let dry = run(&format!("{id} js:true dry:true"));
    assert!(dry.contains("/w/deadshell fetch bd → oxyjs → new u") && dry.contains(" · up to $0.0030 · ≈"), "{dry}");
    let rendered = run(&format!("{}/shell fresh:true js:true", walled()));
    assert!(rendered.contains("Rendered Shell") && rendered.contains(" · hedge:off"), "{rendered}");
}

/// One key or two, end to end: the same pages with only an Oxylabs key, only a Bright Data key, and
/// both. A wall and a shell are served by whichever provider can; a 500 gets one retry with one key; a search
/// runs on either key; no hint names the missing provider.
#[test]
fn any_one_key_gets_the_page() {
    let _t = lock();
    let mut env = Env::default();
    let w = walled();
    // (keys, page, the tier path it must print)
    let rows: &[(&str, &str, &str)] = &[
        ("oxy", "k-wall", "plain:blocked_403✗ oxy:cloudflare_wall✗$0.0010 oxyjs✓"),
        ("bd", "k-wall", "plain:blocked_403✗ bd✓"),
        ("both", "k-wall", "plain:blocked_403✗ bd✓"),
        ("oxy", "k-shell", "plain:blocked_403✗ oxy:js_shell✗$0.0010 oxyjs✓"),
        ("bd", "k-shell", "plain:blocked_403✗ bd:js_shell✗$0.0015 "),
        ("both", "k-shell", "plain:blocked_403✗ bd:js_shell✗$0.0015 oxyjs✓"),
        // a provider's refusal is its answer: no retry, and not billed
        ("oxy", "k-flaky", "plain:blocked_403✗ oxy:provider_error✗$0 (not charged) oxyjs:provider_error✗$0 (not"),
        ("bd", "k-flaky", "plain:blocked_403✗ bd:provider_error✗$0 (not charged) "),
        // The render that erred is the one move left, by name: never the ladder that buys bd's shell again
        ("both", "k-sherr", "plain:blocked_403✗ bd:js_shell✗$0.0015 oxyjs:provider_error✗$0 (not charged)"),
    ];
    for (keys, page, path) in rows {
        env.set("OXYLABS_WEB_API_KEY", (*keys != "bd").then_some(OXY));
        env.set("BRIGHTDATA_API_KEY", (*keys != "oxy").then_some(BD));
        let out = run(&format!("{w}/{page}-{keys} provider:auto fresh:true wait:30"));
        assert!(out.lines().next().unwrap_or_default().contains(path), "{keys} {page}: {out}");
        assert!(*page != "k-sherr" || out.contains(" fresh:true provider:oxy js:true ≤$0.0015"), "{out}");
        let missing = [("oxy", "provider:bd"), ("bd", "provider:oxy")].iter().find(|k| k.0 == *keys);
        let missing = missing.map_or("no hint is missing", |k| k.1);
        assert!(!out.contains(missing), "a hint names the missing key's provider: {keys} {page}: {out}");
        if (*keys, *page) == ("bd", "k-shell") {
            // a failed run names the key that could still render it, never "nothing left"
            let oxy = "! no_key oxy · would try JS render ≤$0.0015";
            assert!(out.contains(oxy) && !out.contains("nothing left"), "what the missing key buys: {out}");
            // The row is the run's record: once in the answer, and on every later read of the id
            let again = run(out.split(['.', ' ']).nth(2).unwrap_or_default());
            let once = |o: &str| o.matches("! no_key oxy · would try").count() == 1;
            assert!(once(&out) && once(&again), "{out}\n{again}");
        }
        let search = run("kq fetch:0 provider:auto wait:5");
        let (from, spent) = if *keys == "bd" { ("From BD", "$0.0015") } else { ("From Oxy", "$0.0010") };
        // a billed bd search is its price, never `≤$` (the zone counted it at $0.0015, every run)
        assert!(search.contains(from) && search.contains(&format!(" · spent {spent}\n")), "{keys} search: {search}");
        if *keys == "bd" && *page == "k-wall" {
            // A bd search slower than `wait:` is `pending` by `wait:`, then its job lists it for $0 more
            let t0 = std::time::Instant::now();
            let pending = run("kslow fetch:0 wait:1");
            let job = pending.split(' ').nth(1).unwrap_or_default().to_string();
            assert!(t0.elapsed().as_millis() < 1300 && pending.contains(" pending 1s search \"kslow\""), "{pending}");
            // its move waits, as a page's: `fats j2 wait:30` answers with the list once stored
            let t0 = std::time::Instant::now();
            let got = run(&format!("{job} wait:30"));
            assert!(got.contains(" ex.com/bdk · not fetched → ") && t0.elapsed().as_secs() < 5, "{got}");
        }
    }
}

/// The provider's own reason on the `!` row and in `stats`; a render it refused (a 400) or gave up on (`faulted`)
/// is settled for the URL: never asked again, by name or by a ladder, so a tier left is the move, else the end.
#[test]
fn a_settled_render_is_never_asked_again() {
    let _t = lock();
    let w = walled();
    for (page, why) in [("k-sh400", "oxyjs 400 url: value not supported"), ("k-shfault", "oxyjs 500 faulted: request")]
    {
        let out = run(&format!("{w}/{page} provider:auto fresh:true wait:30"));
        clean(&out);
        let row = out.lines().find(|l| l.starts_with("u") && l.contains(" ! provider_error ")).unwrap_or_default();
        // no provider move left; decodo's missing key could serve: `→ fats setup`
        let setup = format!(" · failed 1/1 here → {} setup", fats::lang::bin());
        assert!(row.contains(why) && row.ends_with(&setup), "{page}: {out}");
        assert!(!out.contains("provider:oxy"), "{page}: no arrow names oxy, oxyjs and bd spent: {out}");
        let stats = run("stats full");
        assert!(stats.contains(&format!("(not charged, {}", &why[6..])), "{page}: {stats}");
    }
    // faulted twice: the second lap counts, no arrow names oxy
    let again = run(&format!("{w}/k-shfault provider:auto fresh:true wait:30"));
    let row = again.lines().find(|l| l.starts_with("u") && l.contains(" ! provider_error ")).unwrap_or_default();
    assert!(row.contains(" · failed 2/2 here") && !again.contains("provider:oxy"), "{again}");
    clean(&again);
}

/// Mastodon: no `js:true` site order (no follow-up `js:true` run ever validated one): `js:true` runs bd first
/// every time, a shell on the site's last 2 runs moves nothing; what it bills stays within dry's `up to` (the arrow's
/// `≤$X`, no second price).
#[test]
fn js_true_keeps_the_table_order() {
    let _t = lock();
    let w = walled();
    let dry = |n: u32| run(&format!("{w}/k-shell-{n} js:true dry:true"));
    let quote = |d: &str| d.split(" · up to $").nth(1).map(|q| q[..6].to_string()).unwrap_or_else(|| panic!("{d}"));
    let mut bills = Vec::new();
    for n in 1..=3 {
        let before = dry(n);
        let out = run(&format!("{w}/k-shell-{n} js:true"));
        let head = out.lines().next().unwrap_or_default().to_string();
        let spent = head.split("spent $").nth(1).map(|v| v[..6].to_string()).unwrap_or_default();
        bills.push((quote(&before), spent, before.contains("fetch oxyjs → bd"), head.contains(" bd:js_shell✗$")));
    }
    let want = [
        ("0.0030".into(), "0.0030".into(), false, true), // no history: bd shell then oxyjs bills both
        ("0.0030".into(), "0.0030".into(), false, true), // bd failed last time
        ("0.0030".into(), "0.0030".into(), false, true), // bd failed twice: still bd first
    ];
    assert_eq!(bills, want);
    assert!(dry(4).contains("fetch bd → oxyjs → new u") && !dry(4).contains("route:"));
}

/// A barred fix whose tier is no-JS (`provider_refused` → `provider:oxy`, after the
/// session's oxy copy) offers the keyed tier left (oxyjs: the oxy copy's cap stopped it), never "nothing left".
#[test]
fn a_barred_fix_offers_the_tier_left() {
    let _t = lock();
    let b = fats::lang::bin();
    let first = run("https://ex.com/site503/govblock provider:oxy cost:0.001");
    assert!(first.contains("tried oxy:http_503✗") && first.contains("! budget_hit oxyjs not tried"), "{first}");
    let out = run("https://ex.com/site503/govblock fresh:true provider:bd");
    let row = format!("! provider_refused bd proxy_error → {b} u2 fresh:true provider:oxy js:true ≤$0.0015\n");
    assert!(out.contains(&row) && !out.contains("nothing left"), "{out}");
}

/// Through the binary: a Bright Data copy (no country sent) in the site's own language, its canonical link the
/// URL asked, gets no `! lang:es` and no paid fix, only the `own:es` trace in its tried path (`stats`); a geo copy
/// of a page is still flagged, with its fix.
#[test]
fn a_sites_own_language_is_no_geo_copy() {
    let _l = lock();
    let own = run("https://www.pisos.example/alquiler/madrid/?orden=precio provider:bd fresh:true");
    let geo = run("https://www.hotels.example/hotel/fr/example-hotel.html provider:bd fresh:true");
    assert!(own.starts_with("= j") && !own.contains("lang:es") && !own.contains("country:"), "{own}");
    assert!(geo.contains(" · ! lang:es\n") && geo.contains(" ! lang:es · served in es, sent none · "), "{geo}");
    let stats = run("stats full");
    let row = |p: &str| stats.lines().find(|l| l.contains(p) && l.contains(" bd/60s✓")).map(|l| l.contains("✓own:es"));
    assert_eq!((row(" raw 1.0kB "), row(" raw 908B ")), (Some(true), Some(false)), "{stats}");
}

/// `fats search login` is a search, and every command it prints or stores replays one: the job view, the
/// dry line, the run file a prefetch reads (`login` alone would run setup).
#[test]
fn a_searched_command_word_replays_as_a_search() {
    let _l = lock();
    let out = run("search usage");
    let job = out.split(' ').nth(1).unwrap_or_default().to_string();
    assert!(out.starts_with(&format!("= {job} search \"usage\" · 6 results · ")), "{out}");
    let n = out.split("prefetching u").nth(1).and_then(|r| r.split(' ').next()).unwrap_or_default().to_string();
    // the prefetch reads its run file's command: a search, so the next answer's footer says `prefetch`
    let foot = run("config");
    assert!(foot.contains(&format!("\n~ prefetch u{n} ")), "{foot}");
    let t0 = std::time::Instant::now();
    while run(&format!("u{n}")).contains(" pending ") && t0.elapsed().as_secs() < 5 {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let view = run(&job);
    let replay = format!(" · {} search usage", fats::lang::bin());
    assert!(view.lines().next().is_some_and(|l| l.ends_with(&replay)), "{view}");
    let dry = run("search login dry:true");
    assert!(dry.starts_with("= dry search login dry:true · ") && dry.contains("\nrun it: "), "{dry}");
    assert!(dry.ends_with(" search login\n") && dry.contains("search \"login\" via "), "{dry}");
}

/// `limit:` past the provider's cap is named and points nowhere: `limit:20` would only buy the rows shown.
#[test]
fn a_limit_past_the_cap_buys_nothing_more() {
    let _l = lock();
    let over = run("preq limit limit:30 fetch:0");
    assert!(over.contains("\n! ignored limit · a search returns 20 at most\n"), "{over}");
}

/// Decodo, the third provider: its key alone serves a page (no-JS, then its render), a wrong key is `bad_key`
/// naming the key, never the 401 it echoed; a site 5xx it relayed is billed `(assumed)`, its own 500 is not; with
/// three keys `dry:true` prints the ladder (plain → no-JS rungs → renders by price, then table order); `setup`
/// checks the key for $0 on the dead host (400 = good).
#[test]
fn decodo_is_a_third_tier() {
    let _t = lock();
    let mut env = Env::default();
    let w = walled();
    env.set("DECODO_AUTH_TOKEN", Some(DECODO));
    let dry = run(&format!("{w}/k-three dry:true"));
    let ladder = "fetch plain → oxy (after a captcha) → decodo (after a captcha) → bd ∥ oxyjs after 5s → decodojs";
    assert!(dry.contains(ladder) && dry.contains(" · up to $0.0065"), "{dry}");
    // oxy without JS cut at 10 s: the renders next, never decodo's no-JS rung (the same slow page)
    let out = run(&format!("{w}/k-capslow fresh:true wait:60"));
    let eq = out.lines().next().unwrap_or_default();
    assert!(eq.contains("plain:captcha_wall✗ oxy:provider_timeout✗") && !eq.contains("decodo:"), "{out}");
    // the key check: one $0 scrape of the dead host, a 400 = a good key (the other keys stay set: no key is
    // looked up outside the test)
    let (out, rc) = run_rc("setup");
    assert!(rc == 0 && out.contains(" · decodo ok key ok · key dir "), "{out}");
    assert!(out.contains("\ndecodo ok key ok · key from env · saved auth.json (600)\n"), "{out}");
    // the saved keys would serve the one-key runs below
    std::fs::remove_dir_all(dir().join("keys")).ok();
    env.set("DECODO_AUTH_TOKEN", Some("wrongDecodoToken42"));
    let (out, rc) = run_rc("setup");
    assert!(rc == 1 && out.contains(" · decodo ! bad_key") && !out.contains("wrongDecodoToken42"), "{out}");
    let auth = std::fs::read_to_string(dir().join("keys").join("auth.json")).unwrap_or_default();
    assert!(!auth.contains("decodo"), "a refused key is not saved: {auth}");
    clean(&out);
    std::fs::remove_dir_all(dir().join("keys")).ok();
    env.set("OXYLABS_WEB_API_KEY", None).set("BRIGHTDATA_API_KEY", None);
    let out = run("https://ex.com/401 provider:decodo");
    let row = format!(
        "! bad_key decodo → {} setup · decodo https://dashboard.decodo.com/web-data/playground",
        fats::lang::bin()
    );
    assert!(out.contains(&row) && !out.contains("wrongDecodoToken42"), "{out}");
    clean(&out);
    env.set("DECODO_AUTH_TOKEN", Some(DECODO));
    let out = run(&format!("{w}/k-wall-decodo fresh:true wait:30"));
    let path = "plain:blocked_403✗ decodo:cloudflare_wall✗$0.0010 decodojs✓";
    assert!(out.lines().next().unwrap_or_default().contains(path) && out.contains("Decodo Page"), "{out}");
    let out = run(&format!("{w}/k-plain-decodo fresh:true wait:30"));
    assert!(out.lines().next().unwrap_or_default().contains("plain:blocked_403✗ decodo✓"), "{out}");
    let out = run(&format!("{w}/d5xx fresh:true wait:30"));
    let path = "decodo:http_502✗$0.0010 (assumed) decodojs:provider_error✗$0 (not charged)";
    assert!(out.contains(path), "{out}");
    clean(&out);
    let stats = run("stats");
    assert!(stats.contains(" · decodo@US ") && stats.contains(" · decodo-js@US "), "{stats}");
    assert!(stats.contains(" spent $0.0045"), "the wall + render, a page, the relayed 502; the 500 free: {stats}");
}

/// One raw HTTP/1.1 request to the key form; "" = nothing listens there any more.
fn http(host: &str, req: &str) -> String {
    use std::io::{Read, Write};
    let Ok(mut s) = std::net::TcpStream::connect(host) else { return String::new() };
    s.write_all(req.as_bytes()).ok();
    let mut out = String::new();
    s.read_to_string(&mut out).ok();
    out
}

/// `fats setup oxy` without a terminal (an agent): a one-shot form on 127.0.0.1 behind a 128-bit path token. Only
/// `Host: 127.0.0.1:<port>` and no foreign `Origin` (else 400), then GET the page and POST its save on the token's
/// path, nothing else (404); a short key = the page again with its `!` row; one good key is checked ($0), saved in
/// auth.json, and the form is gone, so a second POST finds nothing. The key is never in argv (the run is
/// `--continue setup-oxy`), the URL, the run file, the log or any printed line; `fats setup oxy` shows the waiting
/// form again while it waits.
#[test]
fn setup_form_takes_one_key() {
    let _l = lock();
    let keys = dir().join("keys");
    std::fs::remove_dir_all(&keys).ok();
    let (out, rc) = run_rc("setup oxy");
    let url = out.split_whitespace().find(|w| w.starts_with("http://127.0.0.1:")).unwrap_or_default().to_string();
    assert!(
        rc == 0 && out.starts_with(&format!("= setup oxy · waiting for the key at {url} · 10 min · $0\n")),
        "{out}"
    );
    let (host, path) = url.trim_start_matches("http://").split_once('/').unwrap_or_default();
    let token = path.split('/').nth(1).unwrap_or_default();
    assert!(token.len() == 32 && token.bytes().all(|b| b.is_ascii_hexdigit()), "{url}");
    assert_eq!(run("setup oxy"), out, "one form a provider: the waiting one again");
    let get = |p: &str, h: &str| http(host, &format!("GET /{p} HTTP/1.1\r\nHost: {h}\r\n\r\n"));
    let post = |p: &str, body: &str| {
        let head = format!("POST /{p} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n", body.len());
        let head = if body.contains("evil") { format!("{head}Origin: http://evil.com\r\n") } else { head };
        http(host, &format!("{head}Content-Type: application/x-www-form-urlencoded\r\n\r\n{body}"))
    };
    assert!(get(path, "localhost").starts_with("HTTP/1.1 400 "), "a rebinding name is refused");
    assert!(get(&path.replace(token, &"0".repeat(32)), host).starts_with("HTTP/1.1 404 "));
    assert!(get("", host).starts_with("HTTP/1.1 404 "));
    assert!(get(&format!("{path}/save"), host).starts_with("HTTP/1.1 404 "), "the save route takes POST only");
    assert!(post(path, "key=x").starts_with("HTTP/1.1 404 "), "the page takes GET only");
    let page = get(path, host);
    let csp = "\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'\r\n";
    assert!(
        page.starts_with("HTTP/1.1 200 ") && page.contains(csp) && page.contains("\r\nCache-Control: no-store\r\n")
    );
    assert!(!page.contains("Access-Control") && !page.contains("<script") && page.contains("type=password"), "{page}");
    let key = "oxyF0rmK3y-9Qz2mWb5Lp8Tn";
    assert!(post(&format!("{path}/save"), "key=evil-page-K3y-9Qz2").starts_with("HTTP/1.1 400 "), "a foreign Origin");
    assert!(
        post(&format!("{path}/save"), "key=short").contains("! too short for a key: copy the whole key · try again")
    );
    let done = post(&format!("{path}/save"), &format!("key={key}"));
    assert!(done.contains("Oxylabs Web API works</b> · 351 scrapers") && done.contains("say <b>done</b>"), "{done}");
    let auth: Value =
        serde_json::from_str(&std::fs::read_to_string(keys.join("auth.json")).unwrap_or_default()).unwrap_or_default();
    assert_eq!(auth["keys"]["oxy"], key);
    let gone = std::time::Instant::now();
    while !post(&format!("{path}/save"), &format!("key={key}")).is_empty() && gone.elapsed().as_secs() < 3 {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(post(&format!("{path}/save"), "key=another-key-here").is_empty(), "single use: the form is gone");
    let run_dir = dir().join("cache/t/run");
    assert!(!run_dir.join("setup-oxy.json").exists(), "no form waits now");
    for f in ["setup-oxy.json", "setup-oxy.log"] {
        let text = std::fs::read_to_string(run_dir.join(f)).unwrap_or_default();
        assert!(!text.contains(key), "{f}");
    }
    assert!(!out.contains(key) && !url.contains(key));
    std::fs::remove_dir_all(&keys).ok();
}

/// `fats setup <name>` beside its switches: `free` needs no key (its off switch named), `dry:true` starts no
/// form.
#[test]
fn setup_names_without_a_key_form() {
    let _l = lock();
    let (out, rc) = run_rc("setup free");
    assert!(rc == 0 && out.starts_with("= setup free · built in, no key to set · $0 · off: "), "{out}");
    let dry = run("setup oxy dry:true");
    assert_eq!(dry, "= dry setup oxy · would start a one-shot key form on 127.0.0.1 · $0\n");
    assert!(!dir().join("cache/t/run/setup-oxy.json").exists(), "dry starts nothing");
}

/// Bright Data's form saves the key with the configured zone when the account has it (`setup::landed`: else the
/// only one, else the zones named and `fats setup bd zone:<name>`).
#[test]
fn setup_form_names_the_zones() {
    let _l = lock();
    let keys = dir().join("keys");
    std::fs::remove_dir_all(&keys).ok();
    let out = run("setup bd");
    let url = out.split_whitespace().find(|w| w.starts_with("http://127.0.0.1:")).unwrap_or_default().to_string();
    let (host, path) = url.trim_start_matches("http://").split_once('/').unwrap_or_default();
    let body = "key=sunflower-bd-key";
    let head = format!("POST /{path}/save HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n\r\n", body.len());
    let done = http(host, &format!("{head}{body}"));
    assert!(done.contains("Bright Data works</b> · zone my_unlocker · go back") && !done.contains("sunflower"));
    let auth = std::fs::read_to_string(keys.join("auth.json")).unwrap_or_default();
    assert!(auth.contains("sunflower-bd-key"), "{auth}");
    std::fs::remove_dir_all(&keys).ok();
}

/// https refused at the host (here a listener that speaks only http: the TLS handshake fails) is no wall: every
/// provider meets the same listener, so the same run asks the http URL on plain at once, $0, and serves it
/// (`landed http://…`); when http fails too the paid ladder runs on the https URL and the row offers no http. The
/// hop's why keeps the https cause first (`tls:wrong-version-number+http:200`); an http copy that is an error page
/// after a TLS failure is the certificate's answer: `tls_failed`, $0, no paid call, the keyed no-JS tier the move.
#[test]
fn https_refused_moves_to_http_once() {
    let _l = lock();
    let page = format!("{}/500/doc/Plain-Only", walled());
    let tls = page.replacen("http://", "https://", 1);
    let (calls, t0) = (CALLS.load(Ordering::SeqCst), std::time::Instant::now());
    let out = run(&format!("{tls} wait:30"));
    let head = out.lines().next().unwrap_or_default();
    assert!(
        head.contains(" · landed http://127.0.0.1 · ") && head.contains(" plain ") && !out.contains(" ! "),
        "{out}"
    );
    assert!(head.contains("spent $0.0000") && t0.elapsed().as_secs() < 2, "{out}");
    assert_eq!(CALLS.load(Ordering::SeqCst), calls, "no provider call");
    assert!(written().contains("\"why\": \"tls:wrong-version-number+http:200\""), "the https cause first");
    let oops = format!("{}/oops", walled());
    let cert = run(&format!("{} wait:30", oops.replacen("http://", "https://", 1)));
    let row = cert.lines().find(|l| l.starts_with("u") && l.contains(" ! tls_failed ")).unwrap_or_default();
    let id = row.split(' ').next().unwrap_or_default();
    let arrow = format!(
        "wrong-version-number · $0 → check the certificate · {} {id} fresh:true provider:oxy ≤$",
        fats::lang::bin()
    );
    assert!(row.contains(&arrow), "{cert}");
    assert_eq!(CALLS.load(Ordering::SeqCst), calls, "no paid call after the certificate's answer: {cert}");
    clean(&cert);
    // http refused as well (a 403 there): the ladder on the https URL as before, no http arrow
    let both = run(&format!("{} wait:30", walled().replacen("http://", "https://", 1) + "/k-sh400"));
    assert!(both.contains("plain:connect_failed✗") && both.contains("oxyjs:provider_error✗"), "{both}");
    assert!(!both.contains("→ fats http:") && !both.contains(" http://127"), "the http move was tried: {both}");
    assert!(written().contains("\"why\": \"tls:wrong-version-number+http:403\""), "what http answered: {both}");
    clean(&out);
}

/// https refused for its certificate (verify failed: self-signed here, as expired, wrong-host or untrusted) is the
/// site's answer whatever http says (here a 301 back to https): `plain:tls_failed✗`, $0, no paid call, the reason
/// on the `!` row and in `stats full`, the move the keyed no-JS tier by name, priced as its dry (`provider:oxy` =
/// oxy, then oxyjs: ≤$0.0025). A `provider:` or `js:true` ask runs as any ask.
#[test]
fn a_certificate_failure_ends_the_run_at_zero() {
    let _l = lock();
    let url = format!("{}/expired/page", bad_cert());
    let (calls, t0) = (CALLS.load(Ordering::SeqCst), std::time::Instant::now());
    let (out, rc) = run_rc(&format!("{url} wait:30"));
    let head = out.lines().next().unwrap_or_default();
    assert!(rc == 1 && head.contains("plain:tls_failed✗") && head.contains("spent $0.0000"), "{out}");
    assert!(t0.elapsed().as_secs() < 5, "{out}");
    assert_eq!(CALLS.load(Ordering::SeqCst), calls, "no provider call: {out}");
    let row = out.lines().find(|l| l.starts_with('u') && l.contains(" ! tls_failed ")).unwrap_or_default();
    let id = row.split(' ').next().unwrap_or_default();
    let want = format!(
        " ! tls_failed certificate-verify-failed · $0 → check the certificate · {} {id} fresh:true provider:oxy ≤$0.0025",
        fats::lang::bin()
    );
    assert!(row.contains(&want), "{out}");
    let stats = run("stats full");
    assert!(stats.contains("plain:tls_failed✗") && stats.contains("certificate-verify-failed"), "{stats}");
    for ask in [format!("{id} fresh:true provider:oxy"), format!("{url} js:true")] {
        let before = CALLS.load(Ordering::SeqCst);
        let asked = run(&format!("{ask} wait:30"));
        assert!(CALLS.load(Ordering::SeqCst) > before, "the agent's ask runs: {asked}");
    }
    clean(&out);
}

/// A bare page whose one move is a meta refresh to its own host is the site's redirect: plain follows it once, $0, and
/// serves the page it lands on, with no `redirected` flag (`latest` names no page: `outline::lost_page`, under 8
/// letters; the target keeps a longer asked id); one with a delay is no redirect: plain's copy is a shell, and the
/// ladder walks on.
#[test]
fn a_meta_refresh_is_followed_on_plain() {
    let _l = lock();
    let out = run(&format!("{}/latest wait:30", walled()));
    let head = out.lines().next().unwrap_or_default();
    assert!(head.contains(" Announcing The Release · ") && head.contains(" plain ") && head.contains("$0"), "{out}");
    // the target is judged as any landing: `latest` names no page, so no flag
    assert!(!out.contains("redirected") && !out.contains("oxy") && !out.contains(" ! "), "{out}");
    let meta = written();
    let from = meta.split("\"refreshed_from\":").nth(1).unwrap_or_default().trim_start();
    assert!(from.starts_with("\"http://127.0.0.1:") && from.contains("/w/latest\""), "the origin, on its own");
    let row = out.lines().find(|l| l.contains("#0 info | refreshed from: ")).unwrap_or_default();
    assert!(row.starts_with('u') && row.ends_with("/w/latest"), "the origin, printed: {out}");
    // a target that keeps the asked id is the page, no flag
    let kept = run(&format!("{}/moved/Release-Notes-20261020 wait:30", walled()));
    assert!(kept.contains(" Release Notes 20261020 · ") && !kept.contains(" ! "), "{kept}");
    let slow = run(&format!("{}/latest-5s wait:30", walled()));
    assert!(slow.contains("plain:js_shell✗") && !slow.contains(" Announcing "), "{slow}");
    // a target that answers no 2xx leaves the refreshing page the page: judged as it is, never connect_failed
    let gone = run(&format!("{}/latest-gone wait:30", walled()));
    assert!(gone.contains("plain:js_shell✗") && !gone.contains("connect_failed") && !gone.contains("403"), "{gone}");
    clean(&out);
}

/// The weekly usage summary of a session with a search, a page, a filter, a part read and a failed fetch whose
/// provider echoed the key: counts and codes only. No URL, host, query, filter word, path, id, key or provider
/// answer is in it; what ran is counted.
#[test]
fn weekly_summary_holds_no_input() {
    let _t = lock();
    for cmd in [
        "preq secretword provider:oxy fetch:0",
        "https://ex.com/long/secretpath provider:oxy",
        "u2 secretfilter",
        "u2#1",
        "https://ex.com/500/secretfail provider:oxy",
    ] {
        clean(&run(cmd));
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
    let w = fats::stats::weekly_of(&dir().join("cache"), &fats::cache::week_of(now));
    let text = w.to_string();
    let root = dir().to_string_lossy().into_owned();
    let leaks = [OXY, BD, GH, "preq", "secret", "ex.com", "127.0.0.1", "http", "Traceback", "exploded", "u1", "j1"];
    for leak in leaks.iter().copied().chain([root.as_str(), "/w/"]) {
        assert!(!text.contains(leak), "{leak} in {text}");
    }
    let runs: u64 = w["host"].as_object().map_or(0, |h| h.values().filter_map(|v| v.as_u64()).sum());
    assert_eq!((runs, w["cmd"]["search"].as_u64(), w["cmd"]["filter"].as_u64()), (5, Some(1), Some(1)), "{text}");
    assert_eq!(
        (w["provider"]["oxy"]["search"].as_u64(), w["keys"].as_array().map(Vec::len)),
        (Some(1), Some(2)),
        "{text}"
    );
    assert!(w["codes"]["provider_error"].as_u64() >= Some(1) && w["tier"]["oxy"]["ok"].as_u64() >= Some(1), "{text}");
}
