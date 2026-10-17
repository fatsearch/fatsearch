//! SETTINGS: every knob read once, with where it came from and any input that was refused.
//! Replaces the six env readers the first version had (cache `root`/`session`, `FATS_NOCACHE` in the URL lookup, `FATS_JSON` in
//! the dispatcher, `outline::chunk`, `stats::key_dir`). Each parsed differently, so `fats config` could show a
//! value that was not in effect. A refused value is never silent: it prints a `!` row and shows in `config`.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf, absolute};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;

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
    /// The exit country of a paid call that names none (`provider::exit`): `FATS_COUNTRY`, else `us`, the
    /// country of the Accept-Language plain sends (`en-US,en;q=0.9`, wreq-util's Safari profile `header.rs`).
    pub country: Set<String>,
    /// Heading-less text is cut into blocks of this many tokens (default 300).
    pub chunk: Set<usize>,
    /// Minutes a `fats account` read stands in for a new one (`FATS_ACCOUNT_TTL`, default 15; 0 = always read).
    pub account_ttl: Set<u64>,
    /// Bright Data Web Unlocker zone (the BD CLI's `BRIGHTDATA_UNLOCKER_ZONE`, else the key dir's `brightdata.zone`
    /// that `fats setup` writes); no default, zones are per account.
    pub bd_zone: Option<Set<String>>,
    /// One `! bad_setting …` row per refused value.
    pub rejects: Vec<String>,
}

/// The key-dir file `fats setup` keeps the Bright Data zone in.
pub const ZONE_FILE: &str = "brightdata.zone";
pub const CHUNK_RANGE: RangeInclusive<usize> = 50..=10_000;
pub const ACCOUNT_TTL: u64 = 15;

pub fn get() -> &'static Settings {
    static S: OnceLock<Settings> = OnceLock::new();
    // unit tests read no env (`FATS_CACHE`, keys) and no home dirs (`load`): never the developer's cache or keys
    S.get_or_init(|| load(&|k| std::env::var(k).ok().filter(|v| !v.is_empty() && !cfg!(test))))
}

/// THE Bright Data zone read: the configured Web Unlocker zone (`BRIGHTDATA_UNLOCKER_ZONE`), None = unset.
pub fn zone() -> Option<&'static str> {
    get().bd_zone.as_ref().map(|z| z.value.as_str())
}

/// A provider key as fetches use it: env first, then `<key_dir>/<file>`. (value, source); never printed or logged.
pub fn key(env: &str, file: &str) -> Option<(String, &'static str)> {
    find(env, file, &["env", "file"])
}

/// Where `fats setup` finds a value, first hit wins: the environment, `./.env`, the Bright Data CLI's own files
/// (read only: `<config dir>/brightdata-cli/credentials.json` `api_key` and `config.json` `default_zone_unlocker`;
/// config dir = macOS `~/Library/Application Support`, Linux `~/.config`, Windows `%APPDATA%`, as
/// @brightdata/cli 0.3.6 `dist/utils/credentials.js` writes them), then the key dir; `only` = those places.
pub fn find(env: &str, file: &str, only: &[&str]) -> Option<(String, &'static str)> {
    let dotenv = || {
        let text = std::fs::read_to_string(".env").ok()?;
        text.lines().find_map(|l| {
            let (k, v) = l.trim().trim_start_matches("export ").split_once('=')?;
            (k.trim() == env).then(|| v.trim().trim_matches(['"', '\'']).to_string())
        })
    };
    let bdata = || {
        let (f, field) = match env {
            "BRIGHTDATA_API_KEY" => ("credentials.json", "api_key"),
            "BRIGHTDATA_UNLOCKER_ZONE" => ("config.json", "default_zone_unlocker"),
            _ => return None,
        };
        let text = std::fs::read_to_string(dirs::config_dir()?.join("brightdata-cli").join(f)).ok()?;
        serde_json::from_str::<serde_json::Value>(&text).ok()?[field].as_str().map(String::from)
    };
    let saved = || std::fs::read_to_string(get().key_dir.value.join(file)).ok();
    let places: [(&'static str, &dyn Fn() -> Option<String>); 4] =
        [("env", &|| std::env::var(env).ok()), ("./.env", &dotenv), ("bdata CLI", &bdata), ("file", &saved)];
    let places = places.iter().filter(|p| only.is_empty() || only.contains(&p.0));
    places.into_iter().find_map(|(at, f)| Some((f()?.trim().to_string(), *at)).filter(|v| !v.0.is_empty()))
}

/// THE owner-only writer (keys in the key dir, a provider's kept error answer, the spend ledger's rows, `append`):
/// `<dir>/<file>` at 0600, an old file's wider mode too, before the bytes go in; the dir 0700, also one that
/// exists already (an installer's receipt can make the key dir 0755 before `fats` first runs).
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
    std::io::Write::write_all(&mut f, bytes)
}

/// THE secret register: every credential this binary can send or `fats setup` can find (each provider key in
/// `Providers::specs`, from any place `find` reads).
/// `lang::emit` masks them in everything it prints, a provider's error body is masked before it is kept. A new
/// credential (a dashboard key, a proxy password, a Basic `user:pass` and its base64) goes in here.
pub fn secrets() -> Vec<String> {
    let keyed = crate::provider::PROVIDERS.specs.iter().filter(|s| !s.env.is_empty());
    let found = keyed.filter_map(|s| find(s.env, s.file, &[]).map(|k| k.0));
    let gh = std::env::var("GH_TOKEN").ok().into_iter().chain(GH.get().cloned()).filter(|t| !t.trim().is_empty());
    found.chain(gh).collect::<std::collections::BTreeSet<_>>().into_iter().collect()
}

static GH: OnceLock<String> = OnceLock::new();

/// The GitHub token `fats gh` sends: `GH_TOKEN`, else what `gh auth token` prints (the GitHub CLI's login), else
/// none (unauthenticated: 10 searches a minute). `GH_TOKEN` is read at each call, `gh`'s answer once per process
/// (it can wait on a Keychain prompt); `secrets` masks both.
pub fn gh_token() -> String {
    let cli = || output_within(std::process::Command::new("gh").args(["auth", "token"]), GH_WAIT).unwrap_or_default();
    std::env::var("GH_TOKEN").ok().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| GH.get_or_init(cli).clone())
}

/// How long `gh auth token` gets: on macOS it can wait on a Keychain prompt, which must never hold a call.
pub const GH_WAIT: Duration = Duration::from_secs(2);

/// `cmd`'s trimmed stdout when it succeeds within `wait`; else None (a slower one is killed).
fn output_within(cmd: &mut Command, wait: Duration) -> Option<String> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let t0 = std::time::Instant::now();
    while child.try_wait().ok()?.is_none() {
        if t0.elapsed() >= wait {
            // never read its pipe: a grandchild (`sh -c`, a Keychain helper) can keep it open
            child.kill().ok();
            child.wait().ok();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child.wait_with_output().ok().filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The built-in tables: data, not logic, as JSON under `src/data/`, each parsed once (`table`) into its owner's
/// typed struct. `fats config` counts them (`table_rows`); `tables_load` fails `cargo test` on a typo.
pub const TABLES: [(&str, &str); 7] = [
    ("providers", include_str!("data/providers.json")),
    ("errors", include_str!("data/errors.json")),
    ("check", include_str!("data/check.json")),
    ("sites", include_str!("data/sites.json")),
    ("clean", include_str!("data/clean.json")),
    ("words", include_str!("data/words.json")),
    ("limits", include_str!("data/limits.json")),
];

/// Built-in table `name` as its owner's type. One that does not parse is a build defect, never an input:
/// `tables_load` fails `cargo test` before such a binary exists.
pub fn table<T: serde::Deserialize<'static>>(name: &str) -> T {
    let json = TABLES.iter().find(|t| t.0 == name).map_or("", |t| t.1);
    serde_json::from_str(json).unwrap_or_else(|e| panic!("built-in table {name}: {e}"))
}

/// The behaviour thresholds (`src/data/limits.json`: name with its unit → value), each a `fats config` row.
pub static LIMITS: LazyLock<indexmap::IndexMap<&str, serde_json::Value>> = LazyLock::new(|| table("limits"));

/// Threshold `name` as its user's type. A missing or mistyped one is a build defect, never an input:
/// `limits_used` fails `cargo test` on a name, the owner's tests on a type.
pub fn limit<T: serde::de::DeserializeOwned>(name: &str) -> T {
    let v = LIMITS.get(name).cloned().unwrap_or_default();
    serde_json::from_value(v).unwrap_or_else(|e| panic!("limits.json {name}: {e}"))
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

fn load(env: &dyn Fn(&str) -> Option<String>) -> Settings {
    let mut r = Reader { env, rejects: Vec::new() };
    let session = r.session();
    let home = |d: Option<PathBuf>| if cfg!(test) { Some(std::env::temp_dir().join("fats-unit")) } else { d };
    let cache = home(dirs::cache_dir()).unwrap_or_else(std::env::temp_dir).join("fatsearch");
    // made absolute once (a relative dir is the cwd's at start); `~/…` no shell expanded and a dir fats can't make
    // (a host started in `/`) are refused for the default
    let safe = |v: &str, p: &PathBuf| !v.starts_with('~') && p.parent().is_some() && dirs::home_dir() != Some(p.into());
    let ok = |v: &str| absolute(v).ok().filter(|p| safe(v, p) && crate::cache::private_root(p, false));
    let cache = r.read("env FATS_CACHE", cache, "a dir fats can make, not / or $HOME, never ~/…", ok);
    let key_dir = home(dirs::config_dir()).unwrap_or_default().join("fatsearch");
    let key_dir = r.read("env FATS_KEY_DIR", key_dir, "", |v| Some(v.into()));
    let zone_env = env("BRIGHTDATA_UNLOCKER_ZONE").map(|value| Set { value, src: "env BRIGHTDATA_UNLOCKER_ZONE" });
    let zone_file = std::fs::read_to_string(key_dir.value.join(ZONE_FILE)).ok().map(|v| v.trim().to_string());
    let zone_file = zone_file.filter(|v| !v.is_empty()).map(|value| Set { value, src: "file brightdata.zone" });
    let want = format!("an integer {}-{}", CHUNK_RANGE.start(), CHUNK_RANGE.end());
    // fields run in order: the `! bad_setting` rows come out as listed
    Settings {
        session,
        cache,
        key_dir,
        json: r.read("env FATS_JSON", false, BOOLS, boolean),
        nocache: r.read("env FATS_NOCACHE", false, BOOLS, boolean),
        offline: r.read("env FATS_OFFLINE", false, BOOLS, boolean),
        allow_private: r.read("env FATS_ALLOW_PRIVATE", false, BOOLS, boolean),
        accept_md: r.read("env FATS_ACCEPT_MD", true, BOOLS, boolean),
        country: r.read("env FATS_COUNTRY", "us".into(), "two letters: us · de", |v| {
            (v.len() == 2 && v.bytes().all(|b| b.is_ascii_alphabetic())).then(|| v.to_ascii_lowercase())
        }),
        chunk: r.read("env FATS_CHUNK", 300, &want, |v| v.parse().ok().filter(|n| CHUNK_RANGE.contains(n))),
        account_ttl: r.read("env FATS_ACCOUNT_TTL", ACCOUNT_TTL, "minutes 0-1440", |v| {
            v.parse().ok().filter(|n| (0..=1440).contains(n))
        }),
        bd_zone: zone_env.or(zone_file),
        rejects: r.rejects,
    }
}

/// What a boolean setting takes (`boolean`).
const BOOLS: &str = "1|true|yes|on or 0|false|no|off";

/// The environment being read, and a `! bad_setting` row for each value it refused.
struct Reader<'a> {
    env: &'a dyn Fn(&str) -> Option<String>,
    rejects: Vec<String>,
}

impl Reader<'_> {
    fn refuse(&mut self, k: &str, v: &str, want: &str, using: &str) {
        self.rejects.push(format!("! bad_setting {k}={v} · want {want} · using {using} → unset {k} or fix it"));
    }

    /// `FATS_SESSION` when it is a safe name (a refused name hashes to its own session, so parallel callers that
    /// each set one never merge); else one session per user, `default` under the cache root: every shell, tty and
    /// directory of that user reads the same ids (a cwd hash gave each directory its own `u1`).
    fn session(&mut self) -> Set<String> {
        match (self.env)("FATS_SESSION") {
            Some(v) if v.len() <= 64 && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') => {
                Set { value: v, src: "env FATS_SESSION" }
            }
            Some(v) => {
                let value = blake3::hash(v.as_bytes()).to_hex()[..8].to_string();
                self.refuse("FATS_SESSION", &v, "letters, digits, - and _ (≤ 64)", &value);
                Set { value, src: "hash of FATS_SESSION" }
            }
            None => Set { value: "default".into(), src: "default" },
        }
    }

    /// Env `k` (`src` = `env k`) as `parse` reads it, else `default`; a value it refuses gets a `! bad_setting`
    /// row naming `want`.
    fn read<T: std::fmt::Debug>(
        &mut self,
        src: &'static str,
        default: T,
        want: &str,
        parse: impl Fn(&str) -> Option<T>,
    ) -> Set<T> {
        let k = src.trim_start_matches("env ");
        let Some(v) = (self.env)(k) else { return Set { value: default, src: "default" } };
        if let Some(value) = parse(&v) {
            return Set { value, src };
        }
        self.refuse(k, &v, want, &format!("{default:?}"));
        Set { value: default, src: "default" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `gh auth token` behind a Keychain prompt: a command past its wait is killed and gives no token, in time.
    #[test]
    fn output_within_kills_a_hung_command() {
        let t0 = std::time::Instant::now();
        let wait = Duration::from_millis(300);
        assert_eq!(output_within(Command::new("sh").args(["-c", "sleep 5; echo late"]), wait), None);
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
        assert_eq!(output_within(Command::new("sh").args(["-c", "echo tok"]), wait).as_deref(), Some("tok"));
        assert_eq!(output_within(Command::new("sh").args(["-c", "echo tok; exit 1"]), wait), None);
    }

    /// Review 0.1.7: a unit test never reads the developer's cache, keys or zone (`worst` read its stored copies):
    /// the settings of a test run sit under a scratch home and take nothing from the env.
    #[test]
    fn unit_tests_use_a_scratch_home() {
        let (s, home) = (get(), std::env::temp_dir().join("fats-unit"));
        assert!(s.cache.value.starts_with(&home) && s.key_dir.value.starts_with(&home), "{:?}", s.cache.value);
        assert_eq!((s.cache.src, s.bd_zone.as_ref().map(|z| z.src)), ("default", None));
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

    /// Every built-in table parses into its owner's type, with its row count and a sample row (a JSON typo, an
    /// unknown field or a lost row fails here, not in front of an agent).
    #[test]
    fn tables_load() {
        let rows: Vec<(&str, usize)> =
            TABLES.iter().map(|(n, j)| (*n, table_rows(&serde_json::from_str(j).expect("json")))).collect();
        assert_eq!(
            rows,
            [
                ("providers", 43),
                ("errors", 49),
                ("check", 86),
                ("sites", 42),
                ("clean", 167),
                ("words", 134),
                ("limits", 38)
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
            (fix[0], fix[45]),
            (("provider_timeout", "{b} {id} fresh:true {arg}"), ("fetch_failed", "{b} {id} fresh:true"))
        );
        let p = &crate::provider::PROVIDERS;
        assert_eq!((p.specs.len(), p.apis.len(), p.links.len(), p.cctld.len()), (4, 11, 3, 25));
        let bd = crate::provider::Tier::Bd.spec();
        assert_eq!((bd.short, bd.env, bd.timeout.as_secs(), bd.price), ("bd", "BRIGHTDATA_API_KEY", 60, 0.0015));
        let costs = crate::provider::Api::BdCosts.row();
        assert_eq!((costs.method, costs.url), ("POST", "https://api.brightdata.com/costs/export/json"));
        assert_eq!((p.links[1].billing, p.cctld["uk"]), (Some("https://brightdata.com/cp/billing"), "gb en"));
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
