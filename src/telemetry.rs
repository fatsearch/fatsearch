//! TELEMETRY: the one-time notice, the daily update check (L1) and the weekly counts (L2); what each sends is
//! printed by `fats telemetry show` and listed in docs/telemetry.md. The foreground run only makes small files, one
//! `create_new` each, so ten agents that start at once still ping once a day; the one process that made today's
//! stamp goes on detached (`fats --continue telemetry`, `cli::detach`) and makes the call there. A process that
//! started before the notice was shown sends nothing. Off under a CI marker (telemetry.json `ci_env`),
//! `DO_NOT_TRACK`, `FATS_TELEMETRY=0` or `FATS_OFFLINE` (`settings::Settings::telemetry`): nothing written, nothing
//! sent. Never sent: an id, a URL, a search, a path, a key.

use crate::lang::{Out, bin, draw, screen};
use crate::provider::{Api, block, ours};
use crate::settings::{self, limit};
use serde_json::{Value, json};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What is sent: `full` = L1 + L2, `minimal` = L1 only, `off` = nothing (no update hint either).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Off,
    Minimal,
    Full,
}

impl Level {
    /// `full` · `minimal` · `off`, or a boolean (`FATS_TELEMETRY=0` = off, `1` = full).
    pub fn parse(v: &str) -> Option<Level> {
        match v.to_ascii_lowercase().as_str() {
            "full" => Some(Level::Full),
            "minimal" => Some(Level::Minimal),
            v => settings::boolean(v).map(|on| if on { Level::Full } else { Level::Off }),
        }
    }

    pub fn name(self) -> &'static str {
        ["off", "minimal", "full"][self as usize]
    }
}

impl std::fmt::Debug for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// `env CI` … `env DO_NOT_TRACK`: the switches that turn it off whatever is set, as `config` names them.
static OFF: LazyLock<Vec<String>> =
    LazyLock::new(|| TABLE.ci_env.iter().chain(&["DO_NOT_TRACK".into()]).map(|k| format!("env {k}")).collect());

/// Off whatever is set, and why: a robot is no user (a CI marker), the user said no to every tool (`DO_NOT_TRACK`),
/// no network is allowed (`FATS_OFFLINE`). A switch is set by any value but an empty one or a no (`0`, `false`,
/// `no`, `off`). `env` = the settings reader's (empty = unset).
pub fn quiet(env: &dyn Fn(&str) -> Option<String>, offline: bool) -> Option<settings::Set<Level>> {
    let on = |k: &str| env(k).is_some_and(|v| settings::boolean(&v) != Some(false));
    let robot = OFF.iter().find(|k| on(k.trim_start_matches("env "))).map(String::as_str);
    robot.or(offline.then_some("offline (FATS_OFFLINE)")).map(|src| settings::Set { value: Level::Off, src })
}

/// The level in effect (`settings::Settings::telemetry`).
pub fn level() -> Level {
    settings::get().telemetry.value
}

/// `src/data/telemetry.json`: the upgrade command of each install package (`via`), the env markers of a CI run.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    upgrade: std::collections::BTreeMap<String, String>,
    ci_env: Vec<String>,
}
static TABLE: LazyLock<Table> = LazyLock::new(|| settings::table("telemetry"));

/// `<cache>/telemetry/<name>`: `ping/` the day and week stamps, `days/` the day marks (`d7`), `week/` the weekly
/// tally (`cache::bump`), `latest.json` the last reply, `sent.jsonl`, notes to tell, the detached run's log.
pub fn at(name: &str) -> PathBuf {
    settings::get().cache.value.join("telemetry").join(name)
}

/// When this process started, in whole seconds (forced by `stats::hook`, the binary's first step): a notice made in
/// an earlier second was shown by an earlier process.
pub static STARTED: LazyLock<u64> = LazyLock::new(|| secs(SystemTime::now()));

fn secs(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Tries of a day's check (or a week's counts) that may go unanswered, and the wait after each.
const TRIES: u32 = 3;
const RETRY_S: u64 = 3600;

/// `<key dir>/telemetry-notice`: made when the notice was shown, holding the month of the first run (`first`);
/// beside the keys, so a wiped cache neither shows it again nor starts a new cohort.
fn notice_file() -> PathBuf {
    settings::get().key_dir.value.join("telemetry-notice")
}

/// This month (`2026-10`): the first run's, once this run's notice is claimed.
fn month() -> String {
    date(today()).format("%Y-%m").to_string()
}

fn today() -> i64 {
    (crate::cache::now() / 86_400.0).floor() as i64
}

/// Days since 1970-01-01 → the UTC date.
fn date(days: i64) -> chrono::NaiveDate {
    chrono::DateTime::from_timestamp(days * 86_400, 0).unwrap_or_default().date_naive()
}

/// Today's update check stamp, per version: the first run after an upgrade checks at once (its notes).
fn stamp() -> String {
    format!("{}-{VERSION}", date(today()))
}

fn week_stamp() -> String {
    format!("l2-{}", crate::cache::week_of(crate::cache::now()))
}

/// The CPU as the endpoints take it: `arm64`, `x86_64` (Rust says `aarch64`).
pub fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        a => a,
    }
}

/// Make `path` holding `text`: true for the one process that made it (`create_new`), its folder made first. A text
/// that fails to write reads as none (`first` = this month, a stamp = no retry).
fn claim(path: &Path, text: &str) -> bool {
    let make = || std::fs::File::options().write(true).create_new(true).open(path);
    let made = make().or_else(|e| match (e.kind(), path.parent()) {
        (std::io::ErrorKind::NotFound, Some(d)) => std::fs::create_dir_all(d).and_then(|()| make()),
        _ => Err(e),
    });
    made.map(|mut f| f.write_all(text.as_bytes()).is_ok()).is_ok()
}

/// Whether this process makes the call `stamp` stands for. The stamp holds the tries not answered yet (a running
/// one counts), `0` once one went through: the process that makes it tries first; after a failure, the one that
/// takes its lock (never a wait) and finds it ≥ `RETRY_S` old with fewer than `TRIES` tries, its write the new
/// mtime.
fn due(stamp: &Path) -> bool {
    if claim(stamp, "1") {
        return true;
    }
    let Ok(mut f) = std::fs::File::options().read(true).write(true).open(stamp) else { return false };
    let mut tries = String::new();
    let held = f.try_lock().is_ok() && f.read_to_string(&mut tries).is_ok();
    let age = f.metadata().and_then(|m| m.modified()).map_or(0, |t| secs(SystemTime::now()).saturating_sub(secs(t)));
    let Some(n) = tries.trim().parse::<u32>().ok().filter(|n| held && age >= RETRY_S && (1..TRIES).contains(n)) else {
        return false;
    };
    f.set_len(0).and_then(|()| f.rewind()).and_then(|()| write!(f, "{}", n + 1)).is_ok()
}

/// One call to our endpoint: the 200's body, or what failed (the HTTP status, else the code). A test build answers
/// `FATS_FAKE_TELEMETRY=<status>` instead, without the network.
fn call(to: Api, req: Value) -> Result<Vec<u8>, String> {
    #[cfg(feature = "fake")]
    if let Ok(status) = std::env::var("FATS_FAKE_TELEMETRY") {
        return Err(status);
    }
    block(ours(to, req, Duration::from_secs(limit("telemetry_wait_s")))).unwrap_or_else(|| Err("net_error".into()))
}

/// After every command `cmd` (`main`, each MCP call), after its answer: the cache sweep (`cache::sweep`); then, but
/// for a command about telemetry itself (`fats telemetry`, `fats setup telemetry …`: it may change the level now),
/// the notice once (`shown`), today's day mark, notes the last check left (`tell`), and the update check: detached,
/// by the one process that takes today's stamp (`due`); `FATS_TELEMETRY_DEBUG=1` prints it on stderr instead.
pub fn tick(cmd: &[String]) {
    crate::cache::sweep();
    let s = settings::get();
    let words: Vec<&str> = cmd.iter().flat_map(|a| a.split_whitespace()).collect();
    let own = matches!(words[..], ["telemetry", ..] | ["setup", "telemetry", ..]);
    if s.telemetry.value == Level::Off || own || !shown() {
        return;
    }
    claim(&at(&format!("days/{}", date(today()))), "");
    let mine = at(&format!("tell-{}", std::process::id()));
    if std::fs::rename(at("tell"), &mine).is_ok() {
        eprint!("{}", std::fs::read_to_string(&mine).unwrap_or_default());
        std::fs::remove_file(&mine).ok();
    }
    if s.telemetry_debug.value {
        eprintln!("[telemetry] GET {}", Api::Telemetry.url(&query()));
        send_l2();
    } else if due(&at(&format!("ping/{}", stamp()))) {
        crate::cli::detach("telemetry".into(), &at("ping.log"), None).ok();
    }
}

/// Whether this process may send: the notice was made in a second before this process started (by an earlier
/// one). Else nothing goes, and the process that makes the notice's file shows it (stderr); one that cannot make it
/// sends nothing either.
fn shown() -> bool {
    let file = notice_file();
    if let Ok(t) = std::fs::metadata(&file).and_then(|m| m.modified()) {
        return secs(t) < *STARTED;
    }
    if claim(&file, &month()) {
        eprintln!("{}", page("notice").trim_end());
    }
    false
}

/// The update check's fields (docs/telemetry.md): schema, version, the version before an upgrade (`from`, until a
/// check went through), OS, CPU, install package, month of the first run, days used of the last 7 (today too), level.
fn query() -> Value {
    let day = today();
    let d7 = 1 + (1..7).filter(|k| at(&format!("days/{}", date(day - k))).exists()).count(); // today: this run
    // before the first run's notice is claimed (after its command: `tick`), the month it will claim; a value not
    // shaped YYYY-MM is never sent
    let first = std::fs::read_to_string(notice_file()).ok().filter(|f| !f.trim().is_empty()).unwrap_or_else(month);
    let digits: String = first.trim().chars().map(|c| if c.is_ascii_digit() { '9' } else { c }).collect();
    let mut q = json!({"s": "1", "v": VERSION, "os": std::env::consts::OS, "arch": arch(), "via": via(),
        "first": first.trim(), "d7": d7.to_string(), "t": level().name()});
    if digits != "9999-99" {
        q.as_object_mut().into_iter().for_each(|o| drop(o.remove("first")));
    }
    if let Some(from) = reply()["v"].as_str().filter(|v| *v != VERSION) {
        q["from"] = from.into();
    }
    q
}

/// The last update check's reply (`latest`, `notes`, `skill`, the `l2` and `warn` switches) and the version that
/// sent it (`v`), `checked`; Null before the first.
fn reply() -> Value {
    let mut r = crate::cache::jload(&at("latest.json")).unwrap_or_default();
    checked(&mut r);
    r
}

/// A reply as kept and told: `latest` only as `1.2.3`; `notes` and `warn` as their first line, control characters
/// dropped, at most 200 chars (the user's terminal prints them).
fn checked(r: &mut Value) {
    let Some(o) = r.as_object_mut() else { return };
    let digits = |n: &str| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
    o.retain(|k, v| k != "latest" || v.as_str().is_some_and(|v| v.split('.').map(digits).eq([true; 3])));
    for k in ["notes", "warn"] {
        let line = o.get(k).and_then(Value::as_str).and_then(|t| t.lines().next()).unwrap_or_default();
        let line: String = line.chars().filter(|c| !c.is_control()).take(200).collect();
        o.insert(k.into(), Some(line).filter(|l| !l.trim().is_empty()).into());
    }
}

/// The install package, by where this binary is: npm (its package's `node_modules`), Homebrew (`Cellar`), the
/// shell installer (`$CARGO_HOME/bin` with the installer's receipt), cargo (`$CARGO_HOME/bin` without it), else
/// unknown. Every answer has an upgrade command in the table.
fn via() -> &'static str {
    let exe = std::env::current_exe().and_then(std::fs::canonicalize).unwrap_or_default();
    let has = |part: &str| exe.components().any(|c| c.as_os_str() == part);
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let cargo = var("CARGO_HOME").or_else(|| dirs::home_dir().map(|h| h.join(".cargo"))).unwrap_or_default();
    let in_cargo = std::fs::canonicalize(cargo.join("bin")).is_ok_and(|b| exe.parent() == Some(b.as_path()));
    // where the shell installer writes it: `$XDG_CONFIG_HOME`, else `~/.config`
    let conf = var("XDG_CONFIG_HOME").or_else(|| dirs::home_dir().map(|h| h.join(".config"))).unwrap_or_default();
    let receipt = conf.join("fatsearch").join("fatsearch-receipt.json").is_file();
    match () {
        _ if has("node_modules") => "npm",
        _ if has("Cellar") => "brew",
        _ if in_cargo && receipt => "sh",
        _ if in_cargo => "cargo",
        _ => "unknown",
    }
}

/// The detached half (`fats --continue telemetry`): today's update check; its reply kept, what to tell the user
/// left for the next run (`tick`), what was sent logged, a failed one too. A failed check keeps its stamp's count
/// (`due`: a later run retries after an hour, three tries a day); never an error row, never a retry here. Exit code.
pub fn send() -> i32 {
    let (q, old) = (query(), reply());
    if level() == Level::Off {
        return 1;
    }
    let url = Api::Telemetry.url(&q);
    let json = |b: Vec<u8>| serde_json::from_slice(&b).ok().filter(Value::is_object).ok_or_else(|| "not_json".into());
    let got = call(Api::Telemetry, q.clone()).and_then(json);
    log("l1", url.into(), got.as_ref().err());
    let Ok(mut r) = got else { return 1 };
    std::fs::write(at(&format!("ping/{}", stamp())), "0").ok();
    checked(&mut r);
    let mut tell = Vec::new();
    if q.get("from").is_some() {
        let notes = r["notes"].as_str().filter(|n| r["latest"] == VERSION && !n.is_empty());
        tell.push(format!("~ {} updated to {VERSION}{}", bin(), notes.map(|n| format!(": {n}")).unwrap_or_default()));
        tell.extend((r["skill"] == true).then(|| "~ new skill tips too: npx skills update fatsearch".to_string()));
    }
    tell.extend(r["warn"].as_str().filter(|w| old["warn"] != *w).map(|w| format!("~ {w}")));
    let mut file = std::fs::File::options().append(true).create(true).open(at("tell"));
    tell.iter().for_each(|t| drop(file.as_mut().map(|f| writeln!(f, "{t}"))));
    r["v"] = VERSION.into();
    std::fs::write(at("latest.json"), r.to_string()).ok();
    prune();
    send_l2();
    0
}

/// Stamps of past days, weeks and versions, and day marks older than 7 days (`d7` counts the last 7).
fn prune() {
    let names =
        |d: &str| std::fs::read_dir(at(d)).into_iter().flatten().flatten().map(|e| e.path()).collect::<Vec<_>>();
    let (keep, oldest) = ([stamp(), week_stamp()], date(today() - 6).to_string());
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let gone = names("ping").into_iter().filter(|p| !keep.contains(&name(p)));
    for p in gone.chain(names("days").into_iter().filter(|p| name(p) < oldest)) {
        std::fs::remove_file(p).ok();
    }
}

/// The weekly counts (`stats::weekly`, last week's): level full only, by the one process that takes this week's
/// stamp (`ping/l2-<week>`, `due`: three tries an hour apart), never while the last reply paused them (`"l2":
/// false`), over `telemetry_l2_bytes` (the endpoint takes no more) or for a week with no command;
/// `FATS_TELEMETRY_DEBUG=1` prints them instead.
pub fn send_l2() {
    let payload = weekly();
    let Some(body) = l2_body(&payload) else { return };
    if settings::get().telemetry_debug.value {
        return eprintln!("[telemetry] POST {} {body}", Api::TelemetryL2.row().url);
    }
    let stamp = at(&format!("ping/{}", week_stamp()));
    if !due(&stamp) {
        return; // sent this week already, or tried within the hour, or another process sends it now
    }
    let sent = call(Api::TelemetryL2, payload.clone()).inspect(|_| drop(std::fs::write(&stamp, "0")));
    log("l2", payload, sent.as_ref().err());
}

/// Last week's usage summary (`stats::weekly_of`): the payload the weekly send posts and `fats telemetry show` prints.
fn weekly() -> Value {
    crate::stats::weekly_of(
        &settings::get().cache.value,
        &crate::cache::week_of(crate::cache::now() - crate::cache::WEEK_S),
    )
}

/// The weekly counts as they would go, or None: not full, paused, too big or empty.
fn l2_body(payload: &Value) -> Option<String> {
    let body = payload.to_string();
    let used = payload["cmd"].as_object().is_some_and(|c| !c.is_empty());
    let paused = reply()["l2"] == false || body.len() > limit::<usize>("telemetry_l2_bytes");
    (level() == Level::Full && used && !paused).then_some(body)
}

/// One payload sent as `key` (`l1` its URL, `l2` its body) into `sent.jsonl` with its time and what failed; the
/// last `telemetry_sent_keep` kept.
fn log(key: &str, sent: Value, failed: Option<&String>) {
    let p = at("sent.jsonl");
    let at = chrono::DateTime::from_timestamp(crate::cache::now() as i64, 0).map(|t| t.format("%Y-%m-%dT%H:%M:%SZ"));
    let mut row = json!({ "at": at.map(|t| t.to_string()) });
    row[key] = sent;
    failed.into_iter().for_each(|f| row["failed"] = f.as_str().into());
    let mut rows: Vec<String> = std::fs::read_to_string(&p).unwrap_or_default().lines().map(String::from).collect();
    rows.push(row.to_string());
    let skip = rows.len().saturating_sub(limit("telemetry_sent_keep"));
    std::fs::write(&p, rows[skip..].join("\n") + "\n").ok();
}

/// Part `name` of `src/data/screens/telemetry.txt` (the notice, the setup row).
fn page(name: &str) -> &'static str {
    screen("telemetry").split("§ ").find_map(|s| s.strip_prefix(name)?.strip_prefix('\n')).unwrap_or_default()
}

/// The `telemetry` row of `fats setup` (what is sent and never sent, how to change it).
pub fn row() -> Vec<String> {
    let level = format!("{:<6}", level().name());
    draw(page("row"), &[("level", level.as_str()), ("b", bin())]).lines().map(String::from).collect()
}

/// The `telemetry` line of `fats status` and `fats telemetry`: the level and its source, and a newer version
/// with its upgrade command (the last check's reply; none when off).
pub fn status() -> String {
    let (set, b) = (&settings::get().telemetry, bin());
    let semver = |v: &str| v.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let latest = reply()["latest"].as_str().map(String::from);
    let newer = latest.filter(|l| set.value != Level::Off && semver(l) > semver(VERSION));
    let how = TABLE.upgrade.get(via()).map_or("", String::as_str);
    let hint = newer.map(|l| format!(" · {b} {l} available → {how}")).unwrap_or_default();
    let (level, src) = (set.value.name(), set.src);
    format!("telemetry {level} ({src}){hint} · what is sent: {b} telemetry show · {b} setup telemetry full|minimal|off")
}

/// `fats telemetry show`: the next update check exactly as it would go, the weekly counts, then the last ones sent
/// (a failed one with its status) and, after a failed check, when the next may go.
pub fn show() -> Out {
    let (lvl, l2) = (level(), Api::TelemetryL2.row().url);
    let mut lines = vec![format!("= {} · nothing sent now", status())];
    lines.push(match lvl {
        Level::Off => "next: nothing (off)".into(),
        _ => format!("next daily: GET {}", Api::Telemetry.url(&query())),
    });
    lines.push(match (lvl, l2_body(&weekly())) {
        (Level::Full, Some(body)) => format!("next weekly: POST {l2} {body}"),
        (Level::Full, None) => "next weekly: nothing (no command last week, or paused by the server)".into(),
        _ => "next weekly: nothing (full only)".into(),
    });
    let (p, keep) = (at("sent.jsonl"), |l: &str| serde_json::from_str::<Value>(l).ok());
    let rows: Vec<Value> = std::fs::read_to_string(&p).unwrap_or_default().lines().filter_map(keep).collect();
    let last = rows.iter().rev().find(|r| r["l1"].is_string()).filter(|r| r["failed"].is_string());
    let tries = std::fs::read_to_string(at(&format!("ping/{}", stamp()))).unwrap_or_default();
    let next = if tries.trim().parse().is_ok_and(|n: u32| n >= TRIES) { "tomorrow" } else { "after 1 h" };
    lines.extend(last.map(|r| {
        let day = r["at"].as_str().unwrap_or_default().split('T').next().unwrap_or_default();
        format!("last check: {day} failed {} · next {next}", r["failed"].as_str().unwrap_or_default())
    }));
    lines.push(format!("sent: {} kept in {}", rows.len(), p.display()));
    lines.extend(rows.iter().rev().map(|r| {
        let at = r["at"].as_str().unwrap_or_default();
        let failed = r["failed"].as_str().map(|f| format!(" · failed {f}")).unwrap_or_default();
        match r["l1"].as_str() {
            Some(u) => format!("{at} GET {u}{failed}"),
            None => format!("{at} POST {l2} {}{failed}", r["l2"]),
        }
    }));
    Out::new(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_parse() {
        let all = ["full", "MINIMAL", "off", "0", "1", "no", "maybe"].map(Level::parse);
        let want = [Some(Level::Full), Some(Level::Minimal), Some(Level::Off), Some(Level::Off), Some(Level::Full)];
        assert_eq!(all[..5], want);
        assert_eq!((all[5], all[6]), (Some(Level::Off), None));
        assert_eq!(format!("{:?}", Level::Minimal), "minimal", "a refused value's `using` names the level");
        assert_eq!(page("notice"), "~ anonymous usage stats on · what's sent: fats telemetry show · off: fats setup\n");
        assert_eq!(row().len(), 5, "the setup row: the level line and four more");
        let via = ["npm", "brew", "sh", "cargo", "unknown"];
        assert!(
            via.iter().all(|v| TABLE.upgrade.contains_key(*v)) && TABLE.upgrade.len() == via.len(),
            "what via names"
        );
    }

    /// A reply as kept: `latest` only as `1.2.3`; `notes` and `warn` one printable line of at most 200 chars.
    #[test]
    fn replies_are_checked() {
        let kept = |latest: &str, warn: &str| {
            let mut r = json!({"latest": latest, "notes": "fixed\nsecond line", "warn": warn, "l2": false});
            checked(&mut r);
            r
        };
        assert_eq!(kept("1.2.3", "ok")["latest"], "1.2.3");
        for bad in ["1.2", "1.2.3.4", "1.2.x", "1..3", "v1.2.3", "1.2.3-rc1", "1.2.3\u{1b}[2J"] {
            assert_eq!(kept(bad, "")["latest"], Value::Null, "{bad}");
        }
        let r = kept("1.2.3", &format!("\u{1b}[31mred\u{7}{}", "é".repeat(300)));
        assert_eq!((r["notes"].as_str(), r["l2"].as_bool()), (Some("fixed"), Some(false)));
        let warn = r["warn"].as_str().unwrap_or_default();
        assert!(warn.starts_with("[31mred") && warn.chars().count() == 200 && !warn.contains(char::is_control));
        assert_eq!(kept("1.2.3", " \t\n")["warn"], Value::Null, "nothing printable: no note");
        let mut odd = json!([1]);
        checked(&mut odd);
        assert_eq!(odd, json!([1]), "a reply that is no object stays what it is: read as none");
    }
}
