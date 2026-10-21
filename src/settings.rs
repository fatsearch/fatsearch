//! SETTINGS: every knob read once, with where it came from and any input that was refused.
//! One reader per knob, so `fats config` shows the value in effect: a second reader that parses the same env var
//! its own way can disagree with it. A refused value is never silent: it prints a `!` row and shows in `config`.

use crate::telemetry::Level;
use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf, absolute};
use std::sync::{LazyLock, OnceLock};

/// A value and its source (`env FATS_X`, `default`).
pub struct Set<T> {
    pub value: T,
    pub src: &'static str,
}

pub struct Settings {
    pub session: Set<String>,
    pub cache: Set<PathBuf>,
    pub key_dir: Set<PathBuf>,
    pub json: Set<bool>,
    pub nocache: Set<bool>,
    /// No network calls: URLs come from the cache only (`FATS_OFFLINE=1`; the test suite runs this way).
    pub offline: Set<bool>,
    /// Fetch loopback, private and link-local addresses too (`FATS_ALLOW_PRIVATE=1`); off, a URL that is or
    /// resolves to one is refused at every hop (a prompt-injected `http://169.254.169.254/` reads nothing).
    pub allow_private: Set<bool>,
    /// Plain asks for Markdown first (`provider::MD_ACCEPT`; `FATS_ACCEPT_MD=0`: Safari's own `Accept`). On by default.
    pub accept_md: Set<bool>,
    /// What telemetry sends (`telemetry::Level`): off under a CI marker (telemetry.json `ci_env`), `DO_NOT_TRACK` or
    /// `FATS_OFFLINE`, whatever is set; else `FATS_TELEMETRY`, then `settings.json` (`fats setup telemetry <level>`),
    /// then full.
    pub telemetry: Set<Level>,
    /// `FATS_TELEMETRY_DEBUG=1`: every payload printed on stderr, nothing sent.
    pub telemetry_debug: Set<bool>,
    /// The exit country of a paid call that names none (`provider::exit`): `FATS_COUNTRY`, else `us`, the
    /// country of the Accept-Language plain sends (`en-US,en;q=0.9`, wreq-util's Safari profile `header.rs`).
    pub country: Set<String>,
    /// Heading-less text is cut into blocks of this many tokens (default 300).
    pub chunk: Set<usize>,
    /// Minutes a `fats account` read stands in for a new one (`FATS_ACCOUNT_TTL`, default 15; 0 = always read).
    pub account_ttl: Set<u64>,
    /// Bright Data Web Unlocker zone (the BD CLI's `BRIGHTDATA_UNLOCKER_ZONE`, else `settings.json` `bd_zone`
    /// that `fats setup` writes); no default, zones are per account.
    pub bd_zone: Option<Set<String>>,
    /// Thresholds set outside limits.json (env `FATS_<NAME>`, `settings.json`), checked against its kinds (`limit_of`).
    pub limits: std::collections::HashMap<&'static str, Set<serde_json::Value>>,
    /// `settings.json` as read at start: providers switched off, the zone, `set`.
    pub saved: Saved,
    /// One `! bad_setting …` row per refused value.
    pub rejects: Vec<String>,
}

pub const CHUNK_RANGE: RangeInclusive<usize> = 50..=10_000;
pub const ACCOUNT_TTL: u64 = 15;

pub fn get() -> &'static Settings {
    static S: OnceLock<Settings> = OnceLock::new();
    // unit tests read no env (`FATS_CACHE`, keys) and no home dirs (`load`): never the developer's cache or keys
    S.get_or_init(|| load(&env))
}

/// Env `k` as settings read it: empty = unset; unit tests read none.
fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty() && !cfg!(test))
}

/// THE Bright Data zone read: the configured Web Unlocker zone (`BRIGHTDATA_UNLOCKER_ZONE`), None = unset.
pub fn zone() -> Option<&'static str> {
    get().bd_zone.as_ref().map(|z| z.value.as_str())
}

/// The key-dir files: `auth.json` = secrets only (0600, `keys` by provider: `oxy`, `bd`, `decodo`, `gh`);
/// `settings.json` = the rest `fats setup` and `fats config set` keep (providers switched `off`, the Bright Data
/// zone, settings in `set`). Both `{"version": 1, …}`, written whole (`save`).
pub const AUTH: &str = "auth.json";
pub const PREFS: &str = "settings.json";
/// The files 0.1 kept keys and the zone in, moved into those two once (`migrate`).
const LEGACY: [(&str, &str); 3] = [("oxy", "oxylabs.key"), ("bd", "brightdata.key"), ("bd_zone", "brightdata.zone")];

/// One of the two key-dir files; the fields the other one keeps stay empty and unwritten.
#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Saved {
    pub version: u32,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub off: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bd_zone: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub set: BTreeMap<String, serde_json::Value>,
}

/// Key-dir file `name` as saved; a missing or unreadable one is empty.
pub fn saved(name: &str) -> Saved {
    saved_in(&get().key_dir.value, name)
}

fn saved_in(dir: &Path, name: &str) -> Saved {
    std::fs::read(dir.join(name)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Change key-dir file `name` with `edit` and write it whole: a temp file (0600, synced), then a rename, so a
/// crash leaves the old file or the new one. A file a newer fats wrote (`version` > 1) is never overwritten.
/// Err = `<name>: <why>`.
fn save(name: &str, edit: impl FnOnce(&mut Saved)) -> Result<(), String> {
    save_to(&get().key_dir.value, name, edit).map_err(|e| format!("{name}: {e}"))
}

fn save_to(dir: &Path, name: &str, edit: impl FnOnce(&mut Saved)) -> std::io::Result<()> {
    let mut s = saved_in(dir, name);
    if s.version > 1 {
        return Err(std::io::Error::other(format!("{name} is version {}: written by a newer fats", s.version)));
    }
    edit(&mut s);
    s.version = 1;
    let tmp = format!(".{name}.tmp");
    save_in(dir, &tmp, &serde_json::to_vec_pretty(&s).map_err(std::io::Error::other)?, false)?;
    std::fs::rename(dir.join(&tmp), dir.join(name))
}

/// The 0.1 key files into `auth.json` / `settings.json`, once: copied, read back, then deleted (a file still
/// there wins: a move cut short is done again).
fn migrate(dir: &Path) {
    for (k, f) in LEGACY {
        let Ok(v) = std::fs::read_to_string(dir.join(f)).map(|v| v.trim().to_string()) else { continue };
        let (zone, v) = (k == "bd_zone", Some(v).filter(|v| !v.is_empty()));
        let name = if zone { PREFS } else { AUTH };
        let put = |s: &mut Saved| {
            if zone { s.bd_zone.clone_from(&v) } else { s.keys.extend(v.clone().map(|v| (k.into(), v))) }
        };
        let back = |s: Saved| if zone { s.bd_zone } else { s.keys.get(k).cloned() };
        if v.is_none() || save_to(dir, name, put).is_ok() && back(saved_in(dir, name)) == v {
            std::fs::remove_file(dir.join(f)).ok();
        }
    }
}

/// THE names `fats setup` keys, switches and saves under (`auth.json`): `free` = the plain fetch.
pub const SWITCHES: [&str; 5] = ["free", "oxy", "bd", "decodo", "gh"];

/// The env var of `name`: its providers.json row's (by `short`), `GH_TOKEN` for gh; none without a row.
pub fn env_of(name: &str) -> &'static str {
    let spec = crate::provider::PROVIDERS.specs.iter().find(|s| s.short == name);
    spec.map_or(if name == "gh" { "GH_TOKEN" } else { "" }, |s| s.env)
}

/// THE key read (fetches, setup, config): env first, then `auth.json`, read at each call (a long `fats mcp` sees a
/// key `fats setup` saved). (value, source `env` | `auth.json`); never printed or logged.
pub fn key(name: &str) -> Option<(String, &'static str)> {
    let from_env = Some(env_of(name)).filter(|e| !e.is_empty()).and_then(env).map(|v| (v, "env"));
    let file = || saved(AUTH).keys.get(name).map(|v| (v.clone(), AUTH));
    from_env.into_iter().chain(file()).map(|(v, at)| (v.trim().to_string(), at)).find(|k| !k.0.is_empty())
}

/// `name`'s key into `auth.json`, a Bright Data zone into `settings.json`. Err = the reason, never the key.
pub fn save_key(name: &str, key: &str, zone: Option<&str>) -> Result<(), String> {
    save(AUTH, |f| drop(f.keys.insert(name.into(), key.trim().into())))?;
    zone.map_or(Ok(()), |z| save(PREFS, |f| f.bd_zone = Some(z.into())))
}

/// `name`'s key out of `auth.json` (and bd's zone out of `settings.json`).
pub fn remove_key(name: &str) -> Result<(), String> {
    save(AUTH, |f| drop(f.keys.remove(name)))?;
    save(PREFS, |f| f.bd_zone = f.bd_zone.take().filter(|_| name != "bd"))
}

/// `name` is not switched off (`settings.json` `off`, read at start).
pub fn on(name: &str) -> bool {
    !get().saved.off.iter().any(|o| o == name)
}

/// Switch `name` on or off, never leaving nothing that can fetch: after it, the free fetch or a keyed provider
/// (not gh) stays on. Err = the reason (`! refused · <it>`).
pub fn set_on(name: &str, on: bool) -> Result<(), String> {
    if !SWITCHES.contains(&name) {
        return Err(format!("unknown {name} · one of {}", SWITCHES.join(" ")));
    }
    let mut off = saved(PREFS).off;
    off.retain(|o| o != name);
    off.extend((!on).then(|| name.to_string()));
    let fetches = |n: &&str| *n != "gh" && !off.iter().any(|o| o == n) && (*n == "free" || key(n).is_some());
    if !SWITCHES.iter().any(fetches) {
        return Err("nothing left that can fetch: turn one on or add a key".into());
    }
    save(PREFS, |f| f.off = off)
}

/// The settings `config set` takes besides the thresholds: the env readers', by their env name less `FATS_`.
const READERS: [&str; 12] = [
    "json", "nocache", "offline", "allow_private", "accept_md", "country", "chunk", "account_ttl", "session", "cache",
    "telemetry", "telemetry_debug",
];
/// The env readers `config set` never takes: the key folder (where `settings.json` lives) and the human switch.
const ENV_ONLY: [&str; 2] = ["key_dir", "human"];

/// A name in the env (`FATS_<NAME>`) or `settings.json` that no setting reads (a removed or mistyped one): one
/// `! unknown_setting` row each in `fats config`, never on every answer. Unit tests read no env.
pub fn unknown(s: &Settings) -> Vec<String> {
    let known = |n: &str| LIMITS.contains_key(n) || READERS.contains(&n) || ENV_ONLY.contains(&n);
    let vars = std::env::vars().filter(|_| !cfg!(test));
    let env = vars.filter_map(|(k, _)| Some((k.strip_prefix("FATS_")?.to_lowercase(), format!("env {k}"))));
    let file = s.saved.set.keys().map(|n| (n.clone(), format!("{PREFS} {n}")));
    let fix = |at: &str| match at.strip_prefix("env ") {
        Some(k) => format!("unset {k}"),
        None => format!("{} config unset {}", crate::lang::bin(), at.rsplit(' ').next().unwrap_or_default()),
    };
    let rows = env.chain(file).filter(|(n, _)| !known(n));
    rows.map(|(_, at)| format!("! unknown_setting {at} · read by nothing → {}", fix(&at))).collect()
}

/// `fats config set <name> <value>` into `settings.json`: a threshold of limits.json or a reader, checked by THE
/// reader (`load`, `fits`) as if the env said it: its `! bad_setting` row is the refusal. Used from the next command.
/// A mod's own `config` row (`cost_cap` of `cost:`, `wait`) is set per call only.
pub fn set(name: &str, value: &str) -> Result<(), String> {
    let m = name.strip_suffix("_cap").unwrap_or(name);
    if !LIMITS.contains_key(name) && !READERS.contains(&name) {
        let per = crate::lang::WORDS.keys.contains(&m).then(|| format!("{name} is per call: the {m}: mod"));
        return Err(per.unwrap_or(format!("unknown {name} · the names: {} config", crate::lang::bin())));
    }
    let k = format!("FATS_{}", name.to_uppercase());
    let read = load(&|e| if e == k { Some(value.into()) } else { env(e) });
    if let Some(bad) = read.rejects.iter().find(|r| r.starts_with(&format!("! bad_setting {k}="))) {
        let want = bad.split_once(" · want ").and_then(|w| w.1.split_once(" · using ")).map_or("", |w| w.0);
        return Err(format!("{name} {value} · want {want}"));
    }
    let v = fits(name, value).unwrap_or_else(|| value.into());
    save(PREFS, |f| drop(f.set.insert(name.into(), v)))
}

pub fn unset(name: &str) -> Result<(), String> {
    save(PREFS, |f| drop(f.set.remove(name)))
}

/// THE owner-only writer (keys in the key dir, a provider's kept error answer, the spend ledger's rows, `append`):
/// `<dir>/<file>` at 0600, an old file's wider mode too, before the bytes go in; the dir 0700, also one that
/// exists already (an installer's receipt can make the key dir 0755 before `fats` first runs). A whole write is
/// synced before it returns (`save` renames it next).
pub fn save_in(dir: &Path, file: &str, bytes: &[u8], append: bool) -> std::io::Result<()> {
    let mut open = std::fs::OpenOptions::new();
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        open.mode(0o600);
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut f = open.write(true).create(true).append(append).truncate(!append).open(dir.join(file))?;
    #[cfg(unix)]
    f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    std::io::Write::write_all(&mut f, bytes)?;
    if append { Ok(()) } else { f.sync_all() }
}

/// THE secret register: every credential this binary can send (each provider's env var, `GH_TOKEN`, every key in
/// `auth.json`). `lang::emit` masks them in everything it prints, a provider's error body is masked before it is
/// kept. A new credential goes in here.
pub fn secrets() -> Vec<String> {
    let envs = crate::provider::PROVIDERS.specs.iter().map(|s| s.env).chain(["GH_TOKEN"]).filter(|e| !e.is_empty());
    let all = envs.filter_map(|e| std::env::var(e).ok()).chain(saved(AUTH).keys.into_values());
    let all = all.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    all.collect::<std::collections::BTreeSet<_>>().into_iter().collect()
}

/// The built-in tables: data, not logic, as JSON under `src/data/`, each parsed once (`table`) into its owner's
/// typed struct. `fats config` counts them (`table_rows`); `tables_load` fails `cargo test` on a typo.
pub const TABLES: [(&str, &str); 8] =
    embed!("", ".json": "providers", "errors", "check", "sites", "clean", "words", "limits", "telemetry");

/// `[(name, text)]` of the files `src/data/<dir><name><ext>`, built in.
macro_rules! embed {
    ($dir:literal, $ext:literal: $($n:literal),+) => { [$(($n, include_str!(concat!("data/", $dir, $n, $ext)))),+] };
}
pub(crate) use embed;

/// Built-in table `name` as its owner's type. One that does not parse is a build defect, never an input:
/// `tables_load` fails `cargo test` before such a binary exists.
pub fn table<T: serde::Deserialize<'static>>(name: &str) -> T {
    let json = TABLES.iter().find(|t| t.0 == name).map_or("", |t| t.1);
    serde_json::from_str(json).unwrap_or_else(|e| panic!("built-in table {name}: {e}"))
}

/// The behaviour thresholds (`src/data/limits.json`: name with its unit → value), each a `fats config` row.
pub static LIMITS: LazyLock<indexmap::IndexMap<&str, serde_json::Value>> = LazyLock::new(|| table("limits"));

thread_local! {
    /// The running command's mods (`cli::run`): a threshold named as a mod (`batch_parallel:8`) wins for it.
    static MODS: std::cell::RefCell<Vec<(String, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The mods of the command this thread runs (`cli::run`, once per command; `fats mcp` runs each on its own).
pub fn mods(m: &[(String, String)]) {
    MODS.set(m.to_vec());
}

/// THE value of threshold `name`: its mod, else env `FATS_<NAME>`, else `settings.json` (`fats config set`), else
/// limits.json (`limit_of`). A missing or mistyped built-in is a build defect, never an input: `limits_used`
/// fails `cargo test` on a name, the owner's tests on a type; a user value of the wrong kind never gets here.
pub fn limit<T: serde::de::DeserializeOwned>(name: &str) -> T {
    serde_json::from_value(limit_of(name).0).unwrap_or_else(|e| panic!("limits.json {name}: {e}"))
}

/// `limit`'s value and where it came from: `mod`, `env`, `settings.json`, `build`.
pub fn limit_of(name: &str) -> (serde_json::Value, &'static str) {
    let built = LIMITS.get(name).cloned().unwrap_or_default();
    let m = MODS.with_borrow(|m| m.iter().rev().find(|x| x.0 == name).and_then(|x| fits(name, &x.1)));
    m.map_or_else(|| get().limits.get(name).map_or((built, "build"), |s| (s.value.clone(), s.src)), |v| (v, "mod"))
}

/// THE check of a threshold's value (a mod, env, `settings.json`, `config set`): `v` as threshold `name`'s, a whole
/// number for a whole row, any number for a fraction, never under its floor (`least`); `reason_chars` at most 200
/// (a reason is a few words, never the provider's answer: `provider::said_short`).
pub fn fits(name: &str, v: &str) -> Option<serde_json::Value> {
    let n: serde_json::Value = serde_json::from_str(v.trim()).ok().filter(serde_json::Value::is_number)?;
    let kind = LIMITS.get(name)?.is_f64() || n.is_u64();
    let most = if name == "reason_chars" { 200.0 } else { f64::MAX };
    (kind && n.as_f64().is_some_and(|f| f >= least(name) && f <= most)).then_some(n)
}

/// A threshold's floor: 0 for a fraction and where 0 turns it off (no host pace, no grace, no error body, a sweep
/// every run); 100 for `read_cap` (a chunk keeps 50 tokens back); else 1 (a count, cap or lane of 0 does nothing).
fn least(name: &str) -> f64 {
    match name {
        "plain_host_pace_ms" | "grace_s" | "err_cap_bytes" | "sweep_every_s" => 0.0,
        "read_cap" => 100.0,
        _ => f64::from(u8::from(!LIMITS.get(name).is_some_and(serde_json::Value::is_f64))),
    }
}

/// A table's rows: every array element and object entry that holds no list or map itself.
pub fn table_rows(v: &serde_json::Value) -> usize {
    match v {
        serde_json::Value::Array(a) => a.len(),
        serde_json::Value::Object(o) => o.values().map(table_rows).sum(),
        _ => 1,
    }
}

/// `1 true yes on` / `0 false no off`, any case.
pub fn boolean(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// A system folder: on Windows the variable the install guide names (`%APPDATA%`, `%LOCALAPPDATA%`) when it holds
/// an absolute path, else the OS's own folder (`dirs`: Windows' known folder, which ignores the variable); in a unit
/// test a scratch home's, never the developer's.
pub fn system_dir(env: &dyn Fn(&str) -> Option<String>, var: &str, os: fn() -> Option<PathBuf>) -> Option<PathBuf> {
    let os = if cfg!(test) { || Some(std::env::temp_dir().join("fats-unit")) } else { os };
    env(var).map(PathBuf::from).filter(|p| cfg!(windows) && p.is_absolute()).or_else(os)
}

fn load(env: &dyn Fn(&str) -> Option<String>) -> Settings {
    let mut r = Reader { env, rejects: Vec::new(), file: BTreeMap::new() };
    let key_dir = system_dir(env, "APPDATA", dirs::config_dir).unwrap_or_default().join("fatsearch");
    let key_dir = r.read("env FATS_KEY_DIR", key_dir, "", |v| Some(v.into()));
    migrate(&key_dir.value);
    let saved = saved_in(&key_dir.value, PREFS);
    r.file.clone_from(&saved.set);
    let session = r.session();
    let cache = system_dir(env, "LOCALAPPDATA", dirs::cache_dir).unwrap_or_else(std::env::temp_dir).join("fatsearch");
    // made absolute once (a relative dir is the cwd's at start); `~/…` no shell expanded, a dir fats can't make
    // (a host started in `/`) and one holding the keys (a sweep deletes in it) are refused for the default
    let safe = |v: &str, p: &PathBuf| !v.starts_with('~') && p.parent().is_some() && dirs::home_dir() != Some(p.into());
    let safe = |v: &str, p: &PathBuf| safe(v, p) && !key_dir.value.starts_with(p) && !p.starts_with(&key_dir.value);
    let ok = |v: &str| absolute(v).ok().filter(|p| safe(v, p) && crate::cache::private_root(p, false));
    let cache =
        r.read("env FATS_CACHE", cache, "a dir fats can make, not /, $HOME, in or over the key dir, never ~/…", ok);
    let mut limits = std::collections::HashMap::new();
    for (k, built) in LIMITS.iter() {
        let name = format!("FATS_{}", k.to_uppercase());
        let file = || saved.set.get(*k).map(|v| (v.to_string(), "settings.json"));
        let Some((v, src)) = env(&name).map(|v| (v, "env")).or_else(file) else { continue };
        match fits(k, &v) {
            Some(value) => drop(limits.insert(*k, Set { value, src })),
            None => r.refuse(&name, src, &v, &format!("a number ≥ {} like {built}", least(k)), &built.to_string()),
        }
    }
    let zone_env = env("BRIGHTDATA_UNLOCKER_ZONE").map(|value| Set { value, src: "env BRIGHTDATA_UNLOCKER_ZONE" });
    let zone_file = saved.bd_zone.clone().filter(|v| !v.is_empty()).map(|value| Set { value, src: "settings.json" });
    let want = format!("an integer {}-{}", CHUNK_RANGE.start(), CHUNK_RANGE.end());
    // fields run in order: the `! bad_setting` rows come out as listed
    let mut s = Settings {
        session,
        cache,
        key_dir,
        json: r.read("env FATS_JSON", false, BOOLS, boolean),
        nocache: r.read("env FATS_NOCACHE", false, BOOLS, boolean),
        offline: r.read("env FATS_OFFLINE", false, BOOLS, boolean),
        allow_private: r.read("env FATS_ALLOW_PRIVATE", false, BOOLS, boolean),
        accept_md: r.read("env FATS_ACCEPT_MD", true, BOOLS, boolean),
        telemetry: r.read("env FATS_TELEMETRY", Level::Full, "full|minimal|off", Level::parse),
        telemetry_debug: r.read("env FATS_TELEMETRY_DEBUG", false, BOOLS, boolean),
        country: r.read("env FATS_COUNTRY", "us".into(), "two letters: us · de", |v| {
            (v.len() == 2 && v.bytes().all(|b| b.is_ascii_alphabetic())).then(|| v.to_ascii_lowercase())
        }),
        chunk: r.read("env FATS_CHUNK", 300, &want, |v| v.parse().ok().filter(|n| CHUNK_RANGE.contains(n))),
        account_ttl: r.read("env FATS_ACCOUNT_TTL", ACCOUNT_TTL, "minutes 0-1440", |v| {
            v.parse().ok().filter(|n| (0..=1440).contains(n))
        }),
        bd_zone: zone_env.or(zone_file),
        limits,
        saved,
        rejects: r.rejects,
    };
    s.telemetry = crate::telemetry::quiet(env, s.offline.value).unwrap_or(s.telemetry);
    s
}

/// What a boolean setting takes (`boolean`).
const BOOLS: &str = "1|true|yes|on or 0|false|no|off";

/// The environment being read, and a `! bad_setting` row for each value it refused.
struct Reader<'a> {
    env: &'a dyn Fn(&str) -> Option<String>,
    rejects: Vec<String>,
    /// `settings.json`'s `set`: read when the env has no value (`FATS_JSON` → `json`).
    file: BTreeMap<String, serde_json::Value>,
}

impl Reader<'_> {
    /// The `! bad_setting` row of env `k`'s value, or of its `settings.json` name when `src` is the file.
    fn refuse(&mut self, k: &str, src: &str, v: &str, want: &str, using: &str) {
        let n = k.trim_start_matches("FATS_").to_lowercase();
        let (at, fix) = match src {
            PREFS => (format!("{PREFS} {n}"), format!("{} config unset {n} or set it again", crate::lang::bin())),
            _ => (k.into(), format!("unset {k} or fix it")),
        };
        self.rejects.push(format!("! bad_setting {at}={v} · want {want} · using {using} → {fix}"));
    }

    /// `FATS_SESSION` when it is a safe name (a refused name hashes to its own session, so parallel callers that
    /// each set one never merge); else one session per user, `default` under the cache root: every shell, tty and
    /// directory of that user reads the same ids (a cwd hash gave each directory its own `u1`).
    fn session(&mut self) -> Set<String> {
        match self.raw("env FATS_SESSION") {
            Some((v, src)) if v.len() <= 64 && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') => {
                Set { value: v, src }
            }
            Some((v, src)) => {
                let value = blake3::hash(v.as_bytes()).to_hex()[..8].to_string();
                self.refuse("FATS_SESSION", src, &v, "letters, digits, - and _ (≤ 64)", &value);
                Set { value, src: "hash of FATS_SESSION" }
            }
            None => Set { value: "default".into(), src: "default" },
        }
    }

    /// Env `src` (`env FATS_JSON`), else its `settings.json` value (`json`): (value as text, source).
    fn raw(&self, src: &'static str) -> Option<(String, &'static str)> {
        let k = src.trim_start_matches("env ");
        let v = self.file.get(&k.trim_start_matches("FATS_").to_lowercase());
        let file = v.map(|v| (v.as_str().map_or_else(|| v.to_string(), String::from), "settings.json"));
        (self.env)(k).map(|v| (v, src)).or(file)
    }

    /// Env `k` (`src` = `env k`), else `settings.json`, as `parse` reads it, else `default`; a value it refuses
    /// gets a `! bad_setting` row naming `want`.
    fn read<T: std::fmt::Debug>(
        &mut self,
        src: &'static str,
        default: T,
        want: &str,
        parse: impl Fn(&str) -> Option<T>,
    ) -> Set<T> {
        let Some((v, from)) = self.raw(src) else { return Set { value: default, src: "default" } };
        if let Some(value) = parse(&v) {
            return Set { value, src: from };
        }
        self.refuse(src.trim_start_matches("env "), from, &v, want, &format!("{default:?}"));
        Set { value: default, src: "default" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 0.1.7: a unit test never reads the developer's cache, keys or zone (`worst` read its stored copies):
    /// the settings of a test run sit under a scratch home and take nothing from the env.
    #[test]
    fn unit_tests_use_a_scratch_home() {
        let (s, home) = (get(), std::env::temp_dir().join("fats-unit"));
        assert!(s.cache.value.starts_with(&home) && s.key_dir.value.starts_with(&home), "{:?}", s.cache.value);
        assert_eq!((s.cache.src, s.bd_zone.as_ref().map(|z| z.src)), ("default", None));
    }

    /// A user's threshold has the built-in's kind: a whole number for a whole row, any number for a fraction;
    /// text, a negative or a fraction for a whole row is refused (never a panic in `limit`).
    #[test]
    fn a_threshold_keeps_its_kind() {
        let (whole, frac) = ("batch_parallel", "loss_max");
        assert_eq!(
            (fits(whole, "8"), fits(frac, "2"), fits("grace_s", "0")),
            (Some(8.into()), Some(2.into()), Some(0.into()))
        );
        let refused =
            [fits(whole, "2.5"), fits(whole, "-1"), fits(whole, "x"), fits(frac, "\"1\""), fits(frac, "-0.1")];
        assert!(refused.iter().all(Option::is_none));
        // a count of 0 does nothing (0 lanes, 0 rows): each row has its floor
        assert_eq!((fits(whole, "0"), fits("read_cap", "60"), fits("nope", "1")), (None, None, None));
        assert_eq!((fits("reason_chars", "200"), fits("reason_chars", "201")), (Some(200.into()), None), "a ceiling");
    }

    /// Every threshold in limits.json is read by name somewhere in src/ and every name read is in the file: a typo
    /// or a dead row fails here, and `fats config` prints exactly what the code uses.
    #[test]
    fn limits_used() {
        let src: String = std::fs::read_dir("src")
            .expect("src")
            .filter_map(|e| std::fs::read_to_string(e.ok()?.path()).ok())
            .collect();
        let read: std::collections::BTreeSet<&str> = src
            .split("limit(\"")
            .skip(1)
            .chain(src.split("limit::<u64>(\"").skip(1))
            .chain(src.split("limit::<usize>(\"").skip(1))
            .filter_map(|s| s.split('"').next())
            .collect();
        let have: std::collections::BTreeSet<&str> = LIMITS.keys().copied().collect();
        assert_eq!(read, have);
        assert_eq!((limit::<usize>("max_depth"), limit::<f64>("loss_max")), (512, 0.30));
    }

    /// `%APPDATA%` and `%LOCALAPPDATA%` count on Windows when absolute; elsewhere, and relative, the OS's folder
    /// (a unit test's: the scratch home).
    #[test]
    fn a_system_dir_takes_an_absolute_variable_on_windows() {
        let os = Some(std::env::temp_dir().join("fats-unit"));
        let abs = if cfg!(windows) { r"D:\Users\a\AppData\Local" } else { "/home/a/local" };
        let want = if cfg!(windows) { Some(abs.into()) } else { os.clone() };
        assert_eq!(system_dir(&|_| Some(abs.into()), "LOCALAPPDATA", dirs::cache_dir), want);
        assert_eq!(system_dir(&|_| Some("AppData".into()), "APPDATA", dirs::config_dir), os, "relative: the OS's");
        assert_eq!(system_dir(&|_| None, "APPDATA", dirs::config_dir), os);
    }

    /// Every built-in table parses into its owner's type, with its row count and a sample row (a JSON typo, an
    /// unknown field or a lost row fails here, not in front of an agent).
    #[test]
    fn tables_load() {
        let rows: Vec<(&str, usize)> =
            TABLES.iter().map(|(n, j)| (*n, table_rows(&serde_json::from_str(j).expect("json")))).collect();
        assert_eq!(
            rows,
            [
                ("providers", 49),
                ("errors", 53),
                ("check", 86),
                ("sites", 69),
                ("clean", 167),
                ("words", 1605),
                ("limits", 52),
                ("telemetry", 10)
            ]
        );
        let words = &crate::lang::WORDS;
        assert_eq!((words.stop[0], words.stop.len(), words.commands[0]), ("the", 30, "stats"));
        let clean = &crate::clean::LISTS;
        assert_eq!((crate::clean::sniff(b"PK\x03\x04x"), clean.text[2]), (Some("zip"), "xml"));
        let sites = &crate::fetch::SITES;
        assert_eq!((sites.gov[1], sites.gov.len()), ("mil", 7));
        let check = &crate::fetch::CHECK;
        assert_eq!((check.rules.len(), check.rules[3].code, check.rules[4].code), (6, "captcha_wall", ""));
        let fix = &crate::lang::FIX;
        assert_eq!(
            (fix[0], fix[48]),
            (("provider_timeout", "{b} {id} fresh:true {arg}"), ("fetch_failed", "{b} {id} fresh:true"))
        );
        let p = &crate::provider::PROVIDERS;
        assert_eq!((p.specs.len(), p.apis.len(), p.links.len(), p.cctld.len()), (6, 14, 4, 25));
        let bd = crate::provider::Tier::Bd.spec();
        assert_eq!((bd.short, bd.env, bd.timeout.as_secs(), bd.price), ("bd", "BRIGHTDATA_API_KEY", 60, 0.0015));
        let costs = crate::provider::Api::BdCosts.row();
        assert_eq!((costs.method, costs.url), ("POST", "https://api.brightdata.com/costs/export/json"));
        assert_eq!((p.links[2].billing, p.cctld["uk"]), (Some("https://brightdata.com/cp/billing"), "gb en"));
    }

    fn with(pairs: &[(&str, &str)]) -> Settings {
        let m: std::collections::HashMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        load(&|k| m.get(k).cloned())
    }

    /// What `config` shows is what consumers use; a refused value is a `!` row, never a silent default.
    #[test]
    fn values_sources_and_refusals() {
        let s = with(&[("FATS_JSON", "true"), ("FATS_NOCACHE", "on"), ("FATS_CHUNK", "400"), ("FATS_SESSION", "a-1")]);
        assert_eq!((s.json.value, s.nocache.value, s.chunk.value, s.session.value.as_str()), (true, true, 400, "a-1"));
        assert_eq!((s.json.src, s.chunk.src), ("env FATS_JSON", "env FATS_CHUNK"));
        assert!(s.rejects.is_empty());
        let s = with(&[("FATS_JSON", "maybe"), ("FATS_CHUNK", "abc"), ("FATS_SESSION", "../../etc")]);
        assert_eq!((s.json.value, s.chunk.value, s.session.src), (false, 300, "hash of FATS_SESSION"));
        assert_eq!(s.session.value.len(), 8, "hex hash, printed as s:<hash> (not s:s<hash>)");
        let (a, b) = (with(&[("FATS_SESSION", "p-a.x-1")]), with(&[("FATS_SESSION", "p-b.x-1")]));
        assert_ne!(a.session.value, b.session.value, "two refused names never share a session");
        assert!(a.rejects[0].contains("FATS_SESSION=p-a.x-1 · want letters, digits, - and _"));
        assert_eq!((with(&[]).session.value.as_str(), with(&[]).session.src), ("default", "default"));
        assert_eq!(s.rejects.len(), 3);
        assert!(s.rejects.iter().all(|r| r.starts_with("! bad_setting ") && r.contains('→')));
        assert!(with(&[("FATS_CHUNK", "0")]).rejects[0].contains("FATS_CHUNK=0"));
        // a default-on switch: on unless switched off; a refused value keeps it on and says so
        assert_eq!((with(&[]).accept_md.value, with(&[]).accept_md.src), (true, "default"));
        let off = with(&[("FATS_ACCEPT_MD", "off")]);
        assert_eq!((off.accept_md.value, off.accept_md.src), (false, "env FATS_ACCEPT_MD"));
        let bad = with(&[("FATS_ACCEPT_MD", "sometimes")]);
        let refused = &bad.rejects[0];
        assert!(bad.accept_md.value && refused.contains("FATS_ACCEPT_MD=sometimes") && refused.contains("using true"));
        // account TTL: minutes, 0 = always read; a refused value keeps the default and says so
        assert_eq!((with(&[]).account_ttl.value, with(&[("FATS_ACCOUNT_TTL", "0")]).account_ttl.value), (15, 0));
        let bad = with(&[("FATS_ACCOUNT_TTL", "15m")]);
        assert!(bad.account_ttl.value == 15 && bad.rejects[0].contains("FATS_ACCOUNT_TTL=15m · want minutes 0-1440"));
    }
}
