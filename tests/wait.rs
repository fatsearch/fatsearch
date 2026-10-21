//! Every call answers within `wait:`. A fake transport where every provider takes 30 s; each
//! command that calls a provider in the call (search, gh, account, setup) must answer by `wait:` + 1 s and
//! name the provider it gave up on. Its own binary: the fake transport is set once per process. One test per
//! command: nextest runs each in its own process, all at once (the sleeps are real: one test = their sum).

#[path = "../src/scratch.rs"]
mod scratch;

use fats::provider::{Api, Reply};
use scratch::Scratch;
use serde_json::Value;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Every provider answers, 30 s late; Bright Data's search for `oxyslow` answers at once (the search hedge).
fn slow(api: Api, _: &str, req: &Value) -> Reply {
    if api == Api::BdRequest && req["url"].as_str().is_some_and(|u| u.contains("q=oxyslow")) {
        let body = r#"{"organic":[{"link":"https://ex.com/bd","title":"From BD","description":"d"}]}"#;
        return Reply { status: 200, headers: vec![], body: body.as_bytes().to_vec() };
    }
    let headers = vec![("x-fake-delay-ms".to_string(), "30000".to_string())];
    Reply { status: 200, headers, body: b"{}".to_vec() }
}

/// The environment, set once per process, and this test's turn in a fresh dir, removed after (`cargo test`
/// runs the tests of one process in turns; nextest gives each its own process).
#[expect(unsafe_code, reason = "set_var once per process, before the first settings read")]
fn setup_env() -> (Scratch, MutexGuard<'static, ()>) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    static TURN: Mutex<()> = Mutex::new(());
    let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    ONCE.call_once(|| {
        let dir = Scratch::path("wait");
        // SAFETY: inside `call_once`, before any settings read; a test that runs beside it waits on the `Once`.
        unsafe {
            std::env::set_var("FATS_CACHE", dir.join("cache"));
            std::env::set_var("FATS_KEY_DIR", dir.join("keys"));
            std::env::set_var("FATS_SESSION", "t");
            std::env::set_var("OXYLABS_WEB_API_KEY", "oxyK3y-wait");
            std::env::set_var("BRIGHTDATA_API_KEY", "bdK3y-wait");
            std::env::set_var("BRIGHTDATA_UNLOCKER_ZONE", "my_unlocker");
            std::env::set_var("GH_TOKEN", "ghp_wait");
            std::env::remove_var("FATS_OFFLINE");
            std::env::set_var("FATS_TELEMETRY", "0");
            std::env::set_var("FATS_ALLOW_PRIVATE", "1");
        }
        fats::provider::fake(slow);
    });
    (Scratch::new("wait"), turn)
}

/// A command against the 30 s fake at `wait:1`: its wall time, and the row naming the provider cut at `wait:`.
fn answers_by_wait(cmd: &str, row: &str) {
    let _dir = setup_env();
    let t0 = Instant::now();
    let (out, ..) = fats::cli::run(&cmd.split_whitespace().map(String::from).collect::<Vec<_>>());
    let took = t0.elapsed();
    eprintln!("{cmd}: {} ms", took.as_millis());
    assert!(took <= Duration::from_millis(2000), "{cmd} took {took:?}: {out}");
    // a search slower than `wait:` goes on detached: `pending`, the list on `fats j1`, the move a page's
    let more = if row.contains("pending") { " j1 wait:30 · free, check again in 30 s" } else { " wait:30" };
    assert!(out.contains(row) && out.contains(more), "the fix gives more time: {cmd}: {out}");
    assert!(!out.contains("budget_hit"), "a cut tier is not a budget stop: {out}");
}

#[test]
fn search_answers_by_wait() {
    answers_by_wait("slow words fetch:0 wait:1", "= j1 pending 1s search \"slow words\" → ");
}

#[test]
fn search_hedge_zero_answers_by_wait() {
    answers_by_wait("slow words fetch:0 wait:1 hedge:0", "= j1 pending 1s search \"slow words\" → ");
}

#[test]
fn gh_answers_by_wait() {
    answers_by_wait("gh slow fetch:0 wait:1", "! provider_timeout gh wait:1s");
}

#[test]
fn account_answers_by_wait() {
    answers_by_wait("account fresh:true wait:1", "bd ! provider_timeout wait:1s → ");
}

#[test]
fn setup_answers_by_wait() {
    answers_by_wait("setup wait:1", "bd ! provider_timeout wait:1s");
}

/// Oxylabs 30 s late, Bright Data quick: the 2.5 s search hedge starts bd beside oxy, so `wait:5` gets bd's
/// results (before the hedge was 5 s = the default wait, so bd never started and the search timed out). Bounds =
/// the promise: not before the hedge, before `wait:` (past it the answer would be `pending`).
#[test]
fn search_hedge_answers_by_wait() {
    let _dir = setup_env();
    let t0 = Instant::now();
    let (out, ..) = fats::cli::run(&["oxyslow", "words", "fetch:0", "wait:5"].map(String::from));
    let took = t0.elapsed();
    eprintln!("oxyslow words wait:5: {} ms", took.as_millis());
    assert!(took >= Duration::from_millis(2500) && took < Duration::from_millis(5000), "{took:?}: {out}");
    assert!(out.contains("From BD") && out.contains("bd "), "bd answered: {out}");
}

/// Both keys, `timeout:8` (bd cut at 8 s, oxyjs hedged at 5 s and cut 8 s later: 13.3 s): the
/// agent's `timeout:` is the renders' ONE wall: one cut call, done by `timeout:` + 1 s. Plain fails at once: a
/// local site's 403 (a closed port is no instant failure everywhere: Windows retries the connect for ~2 s, plain's
/// own time, which the paid wall never counts).
#[test]
fn timeout_is_one_wall_with_both_keys() {
    use std::io::{Read, Write};
    let _dir = setup_env();
    let site = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/cut", site.local_addr().expect("addr"));
    std::thread::spawn(move || {
        for mut c in site.incoming().flatten() {
            // a request (one small packet) gets the 403
            if c.read(&mut [0; 4096]).is_ok_and(|n| n > 0) {
                c.write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").ok();
            }
        }
    });
    let t0 = Instant::now();
    let (out, ..) = fats::cli::run(&[&url, "timeout:2", "hedge:1", "wait:30"].map(String::from));
    let took = t0.elapsed();
    eprintln!("timeout:2: {} ms\n{out}", took.as_millis());
    let eq = out.lines().find(|l| l.starts_with("= ")).unwrap_or_default();
    assert_eq!(eq.matches("provider_timeout✗").count(), 1, "one cut call: {out}");
    assert!(took <= Duration::from_secs(3), "{took:?}: {out}");
}
