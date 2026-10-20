//! Telemetry end to end: the notice, the update check once a day however many processes start at once, the
//! weekly counts, `telemetry show`, and silence under every off switch. Nothing here reaches the network: a child
//! process runs with `FATS_TELEMETRY_DEBUG=1` (prints, never sends) or an off switch, and the in-process test answers
//! through the fake transport (`provider::fake`).

use fats::provider::{Api, Reply};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const NOTICE: &str = "~ anonymous usage stats on · what's sent: fats telemetry show · off: fats setup";

/// What the fake endpoint received: the update checks' URLs, the weekly counts' bodies.
static L1: Mutex<Vec<String>> = Mutex::new(Vec::new());
static L2: Mutex<Vec<Value>> = Mutex::new(Vec::new());

fn endpoint(api: Api, _: &str, req: &Value) -> Reply {
    let ok = |b: &str| Reply { status: 200, headers: vec![], body: b.as_bytes().to_vec() };
    match api {
        Api::Telemetry => L1.lock().unwrap_or_else(|e| e.into_inner()).push(api.url(req)),
        Api::TelemetryL2 => L2.lock().unwrap_or_else(|e| e.into_inner()).push(req.clone()),
        _ => return Reply { status: 500, headers: vec![], body: vec![] },
    }
    ok(r#"{"latest":"99.0.0","notes":"","skill":false}"#)
}

/// Wait (≤ 10 s) for `n` sends on `at`, then 300 ms more for any extra one.
fn landed<T>(at: &Mutex<Vec<T>>, n: usize) {
    let t0 = Instant::now();
    while at.lock().unwrap_or_else(|e| e.into_inner()).len() < n && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300));
}

/// `path`'s mtime set `secs` back: a notice an earlier process showed, a stamp an hour old.
fn age(path: &Path, secs: u64) {
    let f = std::fs::File::options().write(true).open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    f.set_modified(std::time::SystemTime::now() - Duration::from_secs(secs)).expect("mtime");
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fats-tele-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&d).ok();
    std::fs::create_dir_all(d.join("home")).expect("scratch");
    d
}

/// `fats <args>` in a fresh environment under `dir`, never offline unless `env` says so, and printing instead of
/// sending (`FATS_TELEMETRY_DEBUG=1`): (stdout, stderr).
fn fats(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (String, String) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_fats"));
    c.env_clear().env("PATH", "/usr/bin:/bin").env("HOME", dir.join("home"));
    c.env("FATS_CACHE", dir.join("cache")).env("FATS_KEY_DIR", dir.join("keys")).env("FATS_SESSION", "t");
    c.env("FATS_TELEMETRY_DEBUG", "1").envs(env.iter().copied()).args(args);
    let o = c.output().expect("fats runs");
    (String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into())
}

/// The first run shows the notice (stderr only) and sends nothing; the next prints the update check it would send,
/// once per run, with the fields of docs/telemetry.md and no id.
#[test]
fn notice_first_then_the_check() {
    let d = scratch("notice");
    let (out, err) = fats(&d, &["version"], &[]);
    assert!(err.contains(NOTICE) && !out.contains("usage stats"), "notice on stderr only: {out} | {err}");
    assert!(!err.contains("[telemetry]"), "the notice run sends nothing: {err}");
    let (_, err) = fats(&d, &["version"], &[]);
    assert!(!err.contains(NOTICE) && !err.contains("[telemetry]"), "nor a run of the notice's second: {err}");
    age(&d.join("keys/telemetry-notice"), 2);
    let (_, err) = fats(&d, &["version"], &[]);
    assert!(!err.contains(NOTICE), "once: {err}");
    let get = err.lines().find(|l| l.starts_with("[telemetry] GET https://fatsearch.com/v1/latest?s=1&v="));
    let get = get.unwrap_or_else(|| panic!("the check, printed: {err}"));
    for f in ["&os=", "&via=", "&first=", "&d7=1", "&t=full"] {
        assert!(get.contains(f), "{f}: {get}");
    }
    assert!(!get.contains("aarch64") && !get.contains("&from="), "arm64 on the wire, no upgrade: {get}");
    assert!(!d.join("cache/telemetry/ping").exists(), "debug claims no stamp");
    // a notice file that holds no month (edited, written by something else) sends no `first`
    std::fs::write(d.join("keys/telemetry-notice"), "x<script>").expect("notice");
    age(&d.join("keys/telemetry-notice"), 2);
    let (_, err) = fats(&d, &["version"], &[]);
    assert!(err.contains("[telemetry] GET ") && !err.contains("&first="), "{err}");
    let (out, _) = fats(&d, &["config"], &[]);
    assert!(out.contains("\ntelemetry full · default\n") && out.contains("\ntelemetry_debug true · env "), "{out}");
}

/// Every off switch: no notice, no check printed, no stamp, no day mark; `config` names the switch.
#[test]
fn off_switches_are_silent() {
    for (k, v, src) in [
        ("DO_NOT_TRACK", "1", "env DO_NOT_TRACK"),
        ("FATS_TELEMETRY", "0", "env FATS_TELEMETRY"),
        ("FATS_OFFLINE", "1", "offline (FATS_OFFLINE)"),
        ("CI", "true", "env CI"),
        ("GITHUB_ACTIONS", "true", "env GITHUB_ACTIONS"),
    ] {
        let d = scratch(&format!("off-{k}"));
        for _ in 0..2 {
            let (_, err) = fats(&d, &["version"], &[(k, v)]);
            assert!(!err.contains("usage stats") && !err.contains("[telemetry]"), "{k}: {err}");
        }
        for f in ["cache/telemetry", "keys/telemetry-notice"] {
            assert!(!d.join(f).exists(), "{k}: {f}");
        }
        let (out, _) = fats(&d, &["config"], &[(k, v)]);
        assert!(out.contains(&format!("\ntelemetry off · {src}\n")), "{k}: {out}");
    }
}

/// `fats setup telemetry <level>` is kept in settings.json and shown with its source; a wrong level is refused;
/// bare `setup telemetry` and `fats telemetry` print what is sent and how to change it.
#[test]
fn setup_sets_the_level() {
    let d = scratch("setup");
    let (out, err) = fats(&d, &["setup", "telemetry", "minimal"], &[]);
    assert!(!out.starts_with('!') && err.is_empty(), "no notice, no check from the switch itself: {out} | {err}");
    assert!(
        !d.join("keys/telemetry-notice").exists() && !d.join("cache/telemetry/ping").exists(),
        "no notice, no stamp"
    );
    std::fs::remove_dir_all(d.join("cache/telemetry/week")).ok(); // counted at full, the switch's own run
    let (out, _) = fats(&d, &["config"], &[]);
    assert!(out.contains("\ntelemetry minimal · settings.json\n"), "{out}");
    assert!(!d.join("cache/telemetry/week").exists(), "minimal counts nothing: the weekly counts are full's");
    let (out, _) = fats(&d, &["setup", "telemetry", "loud"], &[]);
    assert!(out.contains("telemetry loud · want full|minimal|off"), "{out}");
    let (out, _) = fats(&d, &["setup", "telemetry"], &[]);
    assert!(out.starts_with("telemetry  minimal anonymous usage stats") && out.contains("never sent: URLs"), "{out}");
    let (out, _) = fats(&d, &["telemetry"], &[("DO_NOT_TRACK", "1")]);
    assert!(out.starts_with("= telemetry off (env DO_NOT_TRACK)") && out.contains("telemetry show"), "{out}");
    let (out, err) = fats(&d, &["telemetry", "show"], &[("FATS_TELEMETRY", "full")]);
    assert!(err.is_empty(), "fats telemetry: {err}");
    assert!(out.contains("next daily: GET https://fatsearch.com/v1/latest?") && out.contains("sent: 0 kept"), "{out}");
    // the very first run shows the check as it will go: the month this run's notice claims
    let (out, _) = fats(&scratch("first"), &["telemetry", "show"], &[]);
    let first = out.split("&first=").nth(1).and_then(|f| f.get(..8)).unwrap_or_default();
    let shape: String = first.chars().map(|c| if c.is_ascii_digit() { '9' } else { c }).collect();
    assert_eq!(shape, "9999-99&", "{out}");
}

/// This process's environment for the in-process test: its own dirs, and nothing that silences telemetry: every
/// set variable the off rule reads (a CI marker of telemetry.json `ci_env`, `DO_NOT_TRACK`), asked of `quiet`
/// itself so a marker added to the table is cleared too, then the level's own switches.
fn in_process_env(cache: &Path, keys: &Path) {
    let silencer = |k: &str| fats::telemetry::quiet(&|n| (n == k).then(|| "1".into()), false).is_some();
    let set: Vec<String> = std::env::vars_os().filter_map(|(k, _)| k.into_string().ok()).collect();
    #[expect(
        unsafe_code,
        reason = "the env is set once, before the first settings read, by this file's only in-process test"
    )]
    // SAFETY: the other tests here spawn children with a cleared environment and read nothing of this one.
    unsafe {
        std::env::set_var("FATS_CACHE", cache);
        std::env::set_var("FATS_KEY_DIR", keys);
        std::env::set_var("FATS_SESSION", "t");
        for k in set.iter().map(String::as_str).filter(|k| silencer(k)) {
            std::env::remove_var(k);
        }
        for k in ["FATS_OFFLINE", "FATS_TELEMETRY", "FATS_TELEMETRY_DEBUG"] {
            std::env::remove_var(k);
        }
    }
}

/// N processes' worth of ticks at once → ONE update check (the `create_new` stamp), then the weekly counts once;
/// `telemetry show` prints exactly what the endpoint received. In process, through the fake endpoint.
#[test]
fn racers_ping_once_and_show_tells_what_was_sent() {
    let d = scratch("race");
    let (cache, keys) = (d.join("cache"), d.join("keys"));
    in_process_env(&cache, &keys);
    fats::provider::fake(endpoint);
    let tick = || fats::telemetry::tick(&["version".into()]);
    fats::stats::hook();
    tick();
    assert!(keys.join("telemetry-notice").is_file(), "the notice run marks the notice");
    std::thread::sleep(Duration::from_millis(300));
    let tele = cache.join("telemetry");
    assert!(L1.lock().expect("l1").is_empty() && !tele.join("ping").exists(), "the notice run sends nothing");
    tick();
    assert!(L1.lock().expect("l1").is_empty(), "nor this process later: it started before the notice");
    age(&keys.join("telemetry-notice"), 5); // shown by an earlier process
    // last week used: the weekly counts have something to say
    let last = fats::cache::week_of(fats::cache::now() - fats::cache::WEEK_S);
    std::fs::create_dir_all(tele.join("week")).expect("week dir");
    std::fs::write(tele.join(format!("week/{last}.json")), r#"{"n":{"cmd\tread":3}}"#).expect("week");
    let racers: Vec<_> = (0..16).map(|_| std::thread::spawn(tick)).collect();
    racers.into_iter().for_each(|r| r.join().expect("tick"));
    landed(&L2, 1); // a second check, were there one, lands by now too
    let l1 = L1.lock().expect("l1").clone();
    let l2 = L2.lock().expect("l2").clone();
    assert_eq!((l1.len(), l2.len()), (1, 1), "{l1:?} {l2:?}");
    assert_eq!((l2[0]["week"].as_str(), l2[0]["cmd"]["read"].as_u64()), (Some(last.as_str()), Some(3)));
    assert!(l2[0]["arch"] != "aarch64" && l2[0].to_string().len() <= 8192, "{}", l2[0]);
    tick(); // stamped today: nothing more
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!((L1.lock().expect("l1").len(), L2.lock().expect("l2").len()), (1, 1));
    let show = fats::telemetry::show().lines.join("\n");
    assert!(show.contains(&format!(" GET {}\n", l1[0])), "show = what was sent: {show}\n{}", l1[0]);
    assert!(show.contains(&format!(" POST https://fatsearch.com/v1/l2 {}", l2[0])), "{show}");
    assert!(show.contains("sent: 2 kept"), "{show}");
    let status = fats::telemetry::status();
    assert!(status.contains(&format!(" · {} 99.0.0 available → ", fats::lang::bin())), "the update hint: {status}");
    // after an upgrade (the last check went from another version) the check goes at once, with `from`, and the
    // next run is told
    for e in std::fs::read_dir(tele.join("ping")).expect("ping").flatten() {
        let ours = !e.file_name().to_string_lossy().starts_with("l2-");
        ours.then(|| std::fs::remove_file(e.path()).expect("unstamp"));
    }
    std::fs::write(tele.join("latest.json"), r#"{"v":"0.0.1","latest":"99.0.0"}"#).expect("latest");
    tick();
    landed(&L1, 2);
    assert!(L1.lock().expect("l1")[1].contains("&from=0.0.1"), "{:?}", L1.lock().expect("l1"));
    let tell = std::fs::read_to_string(tele.join("tell")).unwrap_or_default();
    assert_eq!(
        tell,
        format!("~ {} updated to {}\n", fats::lang::bin(), env!("CARGO_PKG_VERSION")),
        "told on the next run"
    );
}

/// The rows of `sent.jsonl` once `n` landed (≤ 10 s: the sender is detached), 300 ms more for an extra one.
fn sent(dir: &Path, n: usize) -> Vec<Value> {
    let read = || {
        let rows = std::fs::read_to_string(dir.join("cache/telemetry/sent.jsonl")).unwrap_or_default();
        rows.lines().filter_map(|l| serde_json::from_str(l).ok()).collect::<Vec<Value>>()
    };
    let t0 = Instant::now();
    while read().len() < n && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300));
    read()
}

/// An endpoint that fails (`FATS_FAKE_TELEMETRY=503`: the test build's answer, no network): ten commands make ONE
/// try, its stamp kept; a later command retries only once the stamp is an hour old, three tries a day in all;
/// `telemetry show` tells when the next may go.
#[test]
fn a_failed_check_retries_hourly_three_times() {
    let d = scratch("retry");
    let run = |args: &[&str]| fats(&d, args, &[("FATS_FAKE_TELEMETRY", "503"), ("FATS_TELEMETRY_DEBUG", "")]);
    let (_, err) = run(&["version"]);
    assert!(err.contains(NOTICE), "{err}");
    age(&d.join("keys/telemetry-notice"), 2);
    (0..10).for_each(|_| drop(run(&["version"])));
    let rows = sent(&d, 1);
    assert_eq!(rows.len(), 1, "one try for ten commands: {rows:?}");
    assert!(rows[0]["l1"].as_str().is_some_and(|u| u.contains("?s=1&v=")) && rows[0]["failed"] == "503", "{rows:?}");
    let ping = d.join("cache/telemetry/ping");
    let stamp = std::fs::read_dir(&ping).expect("ping").flatten().map(|e| e.path()).next().expect("the stamp kept");
    let day = rows[0]["at"].as_str().and_then(|a| a.split('T').next()).unwrap_or_default().to_string();
    let (show, _) = run(&["telemetry", "show"]);
    assert!(show.contains(&format!("\nlast check: {day} failed 503 · next after 1 h\n")), "{show}");
    for tries in [2, 3, 3] {
        age(&stamp, 3601);
        (0..3).for_each(|_| drop(run(&["version"])));
        assert_eq!(sent(&d, tries).len(), tries, "try {tries}");
    }
    assert_eq!(std::fs::read_to_string(&stamp).ok().as_deref(), Some("3"));
    let (show, _) = run(&["telemetry", "show"]);
    assert!(show.contains("failed 503 · next tomorrow\n"), "{show}");
}

/// 16 processes that started before any notice existed (held on stdin until all run): one shows the notice, none
/// sends, none makes a stamp.
#[test]
fn processes_older_than_the_notice_send_nothing() {
    let d = scratch("old");
    let mut kids: Vec<_> = (0..16)
        .map(|_| {
            let mut c = Command::new(env!("CARGO_BIN_EXE_fats"));
            c.env_clear().env("PATH", "/usr/bin:/bin").env("HOME", d.join("home")).env("FATS_SESSION", "t");
            c.env("FATS_CACHE", d.join("cache")).env("FATS_KEY_DIR", d.join("keys")).env("FATS_TELEMETRY_DEBUG", "1");
            c.arg("-").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null());
            c.stderr(std::process::Stdio::piped()).spawn().expect("fats runs")
        })
        .collect();
    std::thread::sleep(Duration::from_millis(1500)); // each one past its start, waiting on stdin
    kids.iter_mut().for_each(|k| drop(k.stdin.take()));
    let errs: Vec<String> = kids
        .into_iter()
        .map(|k| String::from_utf8_lossy(&k.wait_with_output().expect("ends").stderr).into_owned())
        .collect();
    assert_eq!(errs.iter().filter(|e| e.contains(NOTICE)).count(), 1, "{errs:?}");
    assert!(errs.iter().all(|e| !e.contains("[telemetry]")), "{errs:?}");
    assert!(!d.join("cache/telemetry/ping").exists() && !d.join("cache/telemetry/days").exists());
}
