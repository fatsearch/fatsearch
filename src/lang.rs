//! LANG: argv → command (parse + repair) and the ONLY place that writes to stdout.

use chrono::Datelike;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::{LazyLock, OnceLock};

/// The language's words (`src/data/words.json`).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, bound(deserialize = "'de: 'static"))]
pub struct Words {
    /// Command words: reserved only when the command holds no free text.
    reserved: Vec<&'static str>,
    /// Reserved only as the whole command (mods and `json` aside), as itself or as an alias: `fats u1 version`
    /// filters the page for `version`, `fats login json` is `setup json`, `fats login page design` searches.
    alone: Vec<&'static str>,
    /// The lists of a page: next to a page id, the first word stays the list and the words after it filter it
    /// (`fats u1 links inmueble`, `fats u1 cards 3 hab`).
    pub lists: Vec<&'static str>,
    /// Other spellings of a reserved word, and what people type for it (`login` → `setup`, `balance` →
    /// `account`): reserved only without free text, like the word itself; `-V` by its case.
    pub aliases: Vec<(&'static str, &'static str)>,
    /// Words people type that name no command (`logout`, `install`): alone (mods aside) they get one `!` line
    /// pointing to what fats has (`{b}` = the binary), never a paid search.
    elsewhere: Vec<(&'static str, &'static str)>,
    /// A leading verb is dropped (`read u2#3`); it also says the rest are words (`search stat`).
    verbs: Vec<&'static str>,
    /// Known mod keys (aliases resolve to these first), and those that take a boolean.
    pub keys: Vec<&'static str>,
    bools: Vec<&'static str>,
    /// The commands a lone word may be a typo of. The reading words (`more`, `full`, `json`, `links`, `help`)
    /// are left out: their neighbours are real searches (`mode`, `fuel`, `jason`, `yelp`).
    pub commands: Vec<&'static str>,
    /// More commands a first word led into a URL or id may be a typo of (`fetchh https://x`, `hlep u1`).
    led: Vec<&'static str>,
    /// Search-engine operators: an unknown key among them is a search word as typed (`filetype:pdf`), never `?`.
    operators: Vec<&'static str>,
    /// The IANA root zone (`tlds-alpha-by-domain.txt`, version 2026101900): a bare name is a host only when its
    /// last label is one (`bare_tld`): `opencode.json`, `main.go` are words.
    tlds: Vec<&'static str>,
    /// What leads a path, never a key (`setup::masked`): `/etc/x`, `./file`, `~/x`.
    pub path_lead: Vec<&'static str>,
    /// The TLDs that are a file's extension too: a lone one-dot name ending in one (`main.rs`) is a file when it
    /// exists, else it asks file or site (`parse`), never a guessed site.
    file_tlds: Vec<&'static str>,
    /// Key and credential file names (`.env.*` = a prefix), matched on the bare name: `cli::secret`.
    pub secret_names: Vec<&'static str>,
    /// A filter for only these words that finds nothing points to the page's media list (`search::grep`).
    pub media: Vec<&'static str>,
    /// Agent hosts by the env marker each sets for the commands it runs (`stats::host_of`): `CLAUDECODE` (Claude
    /// Code's shell), `CODEX_THREAD_ID` (Codex's exec env), `PI_CODING_AGENT` (pi); the first set one wins.
    pub hosts: Vec<(&'static str, &'static str)>,
    /// The hosts `fats status` names: the files its `mcp add` writes (`mcp::read`), then how to add fats (`{b}` =
    /// the binary): Claude Code's user and local scopes in `~/.claude.json`, its project scope in `./.mcp.json`
    /// (code.claude.com/docs/en/mcp), Codex's `$CODEX_HOME/config.toml` (developers.openai.com/codex/mcp), Cursor's
    /// global and project files (cursor.com/docs/context/mcp). The first one's way is the row's while none holds fats.
    pub mcp_hosts: Vec<(&'static str, Vec<&'static str>, &'static str)>,
    /// What an SSH login sets (OpenSSH, ssh(1) ENVIRONMENT): `fats setup <p>` adds the key's SSH way only then.
    pub ssh: Vec<&'static str>,
    /// Words a filter never looks for (`search::terms_of`, the title query).
    pub stop: Vec<&'static str>,
}

pub static WORDS: LazyLock<Words> = LazyLock::new(|| crate::settings::table("words"));

pub const CARD: &str = "\
fats <url|file.pdf>     fetch, outline           fats u3  u3#2  u3#2 l4  page  section  link
fats u3#2 <words>       filter inside            fats <words>  web search → ids  site:ex.com  since:7d
mods key:value  limit:20  match:exact|words  country:de  js:true  wait:30 (then pending)  provider:plain|bd|oxy|decodo  device:mobile
more  up  all  full  json  fats = status  fats setup = keys  fats account = balance  j8.u3 = another session's id";

/// `fats guide`: how to use fats well, the body the installed skill points at (`skills/fatsearch/SKILL.md`), so
/// the guide always matches this binary.
pub const GUIDE: &str = include_str!("data/guide.md");

/// The line every first door ends with (`help`, `version`, the status, `setup`, a word that is no command): where
/// the guide is and what reading it costs.
pub fn to_guide() -> String {
    format!("agents: {} guide = the manual (~{} tok)", bin(), ksize(crate::clean::tok(GUIDE)))
}

/// `--help` anywhere (an en or em dash for `--`: smart punctuation), or `-h`.
const HELP_FLAGS: [&str; 4] = ["--help", "-h", "—help", "–help"];

/// The human screens (`src/data/screens/`: text a human edits without Rust; `paint` fills the `{hole}`s).
/// A `help-<topic>` row is `fats help <topic>`.
pub const SCREENS: [(&str, &str); 11] = crate::settings::embed!("screens/", ".txt": "home", "ready", "yours",
    "help", "help-mcp", "help-setup", "help-search", "tip-page", "tip-search", "setup", "telemetry");

/// The styles a screen names (`{bold}` … `{/}`) and their ANSI SGR codes; none under `NO_COLOR` (no-color.org:
/// set and not empty).
const STYLES: [(&str, u8); 6] = [("bold", 1), ("dim", 2), ("green", 32), ("red", 31), ("cyan", 36), ("/", 0)];

/// A screen with styles only.
pub const NO_HOLES: [(&str, &str); 0] = [];

pub fn screen(name: &str) -> &'static str {
    SCREENS.iter().find(|s| s.0 == name).map_or("", |s| s.1)
}

/// THE hole filler (screens, `errors.json` fixes, table rows): each `{name}` of `holes`, in their order, becomes
/// its value; any other `{` stays (JSON, a stray brace).
pub fn draw(text: &str, holes: &[(&str, impl AsRef<str>)]) -> String {
    holes.iter().fold(text.to_string(), |t, (name, value)| t.replace(&format!("{{{name}}}"), value.as_ref()))
}

/// A human screen: `text`'s holes, then its styles.
pub fn paint(text: &str, holes: &[(&str, impl AsRef<str>)]) -> String {
    let plain = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    #[cfg(test)]
    let plain = tests::NO_COLOR.get().unwrap_or(plain);
    draw(&draw(text, holes), &STYLES.map(|(k, n)| (k, if plain { String::new() } else { format!("\x1b[{n}m") })))
}

/// Whether a human reads this process's answers (`human`), set once by `cli::run`: a row built later (`fix`'s
/// `no_key`) reads it without knowing the command; unset = an agent.
pub static HUMAN: OnceLock<bool> = OnceLock::new();

/// A human reads the answer: stdout is a terminal, or `FATS_HUMAN=1` (`0` = never); never `json`, MCP, a unit test.
pub fn human(json: bool) -> bool {
    let forced = std::env::var("FATS_HUMAN").ok().and_then(|v| crate::settings::boolean(&v));
    let agent = json || cfg!(test) || crate::cli::FENCE.get().is_some();
    !agent && forced.unwrap_or_else(|| std::io::stdout().is_terminal())
}

/// The help a human gets: `help` first (`fats help mcp` = its `help-<topic>` screen) or a help flag anywhere;
/// None = no help asked (the agent's `--help` is the card, `parse`).
pub fn help_screen(argv: &[String]) -> Option<String> {
    let first = argv.first().is_some_and(|a| a.eq_ignore_ascii_case("help"));
    let topic = argv.get(1).filter(|_| first).map(|t| screen(&format!("help-{}", t.to_lowercase())));
    let text = topic.filter(|t| !t.is_empty()).unwrap_or(screen("help"));
    (first || argv.iter().any(|a| HELP_FLAGS.contains(&a.as_str()))).then(|| paint(text, &NO_HOLES))
}

/// A human's keyless failure: a page the free fetch could not read, or a search, while no paid provider is keyed
/// (and on) → the tip to add one (`tip-page`, `tip-search`) instead of the `! no_key` rows; None for anything else.
pub fn tip(cmd: &Cmd, out: &Out, search: bool) -> Option<String> {
    let paid = crate::fetch::keyed().iter().any(|t| t.spec().price > 0.0);
    let keyless = !paid && out.rc != 0 && out.parts.is_empty() && out.lines.iter().any(|l| l.contains("! no_key "));
    let page = |u: &String| paint(screen("tip-page"), &[("site", crate::fetch::site(u))]);
    let tip = if search { Some(paint(screen("tip-search"), &NO_HOLES)) } else { cmd.urls.first().map(page) };
    tip.filter(|_| keyless && cmd.urls.len() < 2 && cmd.verb.is_none() && !cmd.flag("dry"))
}

static BIN: OnceLock<String> = OnceLock::new();

/// Hints echo argv[0]'s file name, so they run as printed; an `.exe` is dropped (on Windows argv[0] is often the
/// file a launcher found, `fats.exe`, while `fats` is what a user types there too).
pub fn bin() -> &'static str {
    BIN.get_or_init(|| {
        let p = std::path::PathBuf::from(std::env::args().next().unwrap_or_default());
        let name = p.extension().filter(|e| e.eq_ignore_ascii_case("exe")).map_or(p.file_name(), |_| p.file_stem());
        name.map(|f| f.to_string_lossy().into_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| "fats".into())
    })
}

/// THE error table (`src/data/errors.json`): every `!` code → the one command that fixes it (a full fix command,
/// never the same failing retry; a 404's site home is none). `{b}` = the binary, `{id}` `{url}` = the page,
/// `{arg}` = a value only the caller knows (the cost wanted, the dir, the nearest command this build runs, the
/// timed-out tier's move). A `*` key matches a code prefix. Walls go to Oxylabs with JavaScript; a PDF a provider
/// sent as a page or cut short goes to Bright Data (its raw bytes are the exact file); transient codes retry the
/// same call.
pub static FIX: LazyLock<Vec<(&str, &str)>> = LazyLock::new(|| crate::settings::table("errors"));
/// The fix for a code the table does not know (an old snapshot's): the transient retry.
const RETRY: &str = "{b} {id} fresh:true";

/// Where a `!` row points: the page id, its URL, the table's `{arg}`, and the tiers this run already tried or
/// cannot use (`fetch::spent`).
#[derive(Default)]
pub struct At<'a> {
    pub id: &'a str,
    pub url: &'a str,
    pub arg: &'a str,
    pub done: &'a [&'a str],
}

/// The fix for a code; a code the table does not know (an old snapshot's) gets the transient retry. A fix that
/// would send a tier in `At::done` (`provider:bd`; `js:true` = oxyjs) is the same failing call: the row says
/// nothing is left instead.
pub fn fix(code: &str, at: &At) -> String {
    let hit = |k: &str| k.strip_suffix('*').map_or(k == code, |p| code.starts_with(p));
    let t = FIX.iter().find(|r| hit(r.0)).map_or(RETRY, |r| r.1);
    let f = draw(t, &[("b", bin()), ("id", at.id), ("url", at.url), ("arg", at.arg)]);
    // a missing key's move: a human at a terminal runs its setup; an agent runs it itself and hands the user the
    // numbered steps it prints (read as a command for the user, an agent asked a plain user to run it)
    if code == "no_key" && !at.arg.is_empty() && HUMAN.get() != Some(&true) {
        return format!("run {f}, give the user its 3 lines");
    }
    // `js:true` = the table's first render rung (`Tier::renders`: oxyjs), the render it adds to bd's
    let js = f.split(' ').any(|w| w == "js:true");
    let js = js.then(|| crate::provider::Tier::ALL.into_iter().find(|t| t.renders())).flatten().map(|t| t.spec().short);
    // the last `provider:` wins, as in `Cmd::get` (a search's `{id}` is its command, mods included)
    let tier = js.or(f.split(' ').filter_map(|w| w.strip_prefix("provider:")).next_back());
    // a mod already in force (`max_bytes:64m` at the ceiling) is the same failing call too
    let sent = tier.is_some_and(|t| at.done.contains(&t));
    let barred = sent || f.split(' ').any(|w| w.contains(':') && at.done.contains(&w));
    // After a render that ran, a keyed paid tier this run did not call (one the site's wear skipped) is the
    // move, never "nothing left"; after a provider's refusal too (it said nothing of the page: bd
    // refused after oxy → oxyjs), never after a page's no-JS failure (`not_pdf`: a render brings no PDF)
    if sent
        && (js.is_some() || crate::provider::REFUSED.contains(&code))
        && let Some((_, arrow)) = crate::fetch::left_ask(at.id, at.url, &crate::provider::Tier::ALL, at.done, |_| true)
    {
        return arrow;
    }
    if barred {
        let open = if at.url.is_empty() { String::new() } else { format!(" · open {} in a browser", at.url) };
        return format!("nothing left to try{open}");
    }
    // a mod the fix sets replaces the command's own (`provider:bd … provider:oxy` → `… provider:oxy`)
    let toks: Vec<&str> = f.split(' ').filter(|w| !w.is_empty()).collect();
    fn key(w: &str) -> Option<&str> {
        w.split_once(':').map(|p| p.0).filter(|k| WORDS.keys.contains(k))
    }
    let later = |i: usize| key(toks[i]).is_some_and(|k| toks[i + 1..].iter().any(|w| key(w) == Some(k)));
    let f = toks.iter().enumerate().filter(|&(i, _)| !later(i)).map(|p| *p.1).collect::<Vec<_>>().join(" ");
    // the command is quoted (`priced`); a retry after a provider's unbilled error says so
    let (cmd, rest) = f.split_once(", ").map_or((f.as_str(), ""), |(c, _)| (c, &f[c.len()..]));
    let quoted = priced(cmd, at.url);
    // a raised `cost:` that can buy nothing here (no paid tier keyed: `≤$0`) is a key's move, never a free retry
    if code == "budget_hit" && quoted == cmd {
        return fix("no_key", &At::default());
    }
    let free = quoted != cmd && crate::provider::REFUSED.contains(&code);
    format!("{quoted}{}{rest}", if free { " · last try not charged" } else { "" })
}

/// THE price glyph. An arrow whose command can spend (`fresh:`, `js:`, `provider:`,
/// `timeout:`) ends with the most it can, `≤$0.0030` = `dry:true`'s `up to` for it (`fetch::worst`).
pub fn priced(cmd: &str, url: &str) -> String {
    let c = crate::fetch::worst(cmd, url).map_or(0.0, |r| r.1);
    if c > 0.0 { format!("{cmd} ≤${c:.4}") } else { cmd.into() }
}

/// A `!`: what is missing (`code`), its measure (`what`), the next command (`next`, "" = none). `row`: it also
/// gets its own `{id} ! …` row, and the `=` line names only its code (`outline::flags`).
#[derive(Default)]
pub struct Flag {
    pub code: String,
    pub what: String,
    pub next: String,
    pub row: bool,
}

impl Flag {
    /// THE `!` printer: `{subject} ! {code} {what} → {next}`, every `!` row (`body` = the `=` line's flags).
    pub fn bang(&self, subject: &str) -> String {
        format!("{subject} ! {}", self.body()).trim_start().to_string()
    }

    /// `{code} {what} → {next}`, one code of a `!` group.
    pub fn body(&self) -> String {
        let head = [&self.code, &self.what].iter().filter(|s| !s.is_empty()).map(|s| s.as_str()).collect::<Vec<_>>();
        if self.next.is_empty() { head.join(" ") } else { format!("{} → {}", head.join(" "), self.next) }
    }
}

/// A stored `! {code} {what} → {fix}{ · pages}` row (`fetch::skipped`) with the reader's arrow: the table's fix for
/// its code and `cost:` arg, as `id`, priced for `url` (the writer's binary, id and price; another build's
/// `fats-xyz setup`); the accounts' pages after it kept.
pub fn restamp(row: &str, id: &str, url: &str) -> String {
    let ((head, f), code) = (row.split_once(" → ").unwrap_or((row, "")), row.split(' ').nth(1).unwrap_or_default());
    // `{arg}` read back: the stored arrow's `cost:`/`zone:` value, else, in a fix naming no page, the switch the
    // row names (`! no_key oxy`, `! off free`), never the stored arrow's wording (a human's or an agent's)
    let t = FIX.iter().find(|r| r.0 == code).map_or("", |r| r.1);
    let named = row.split(' ').nth(2).filter(|_| t.contains("{arg}") && !t.contains("{id}"));
    let arg =
        f.split(' ').find_map(|w| w.strip_prefix("cost:").or(w.strip_prefix("zone:"))).or(named).unwrap_or_default();
    format!("{head} → {}{}", fix(code, &At { id, url, arg, done: &[] }), f.find(" · ").map_or("", |i| &f[i..]))
}

/// A `!` row with the table's fix: `{subject} ! {code} {detail} → {fix}` (`Flag::bang`).
pub fn bang(subject: &str, code: &str, detail: &str, at: &At) -> String {
    Flag { code: code.into(), what: detail.into(), next: fix(code, at), row: true }.bang(subject)
}

/// The items of `it` whose `key` was not seen before, in order (distinct words, one row per account).
pub fn uniq<T, K: PartialEq>(it: impl IntoIterator<Item = T>, key: impl Fn(&T) -> K) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for x in it {
        if !out.iter().any(|o| key(o) == key(&x)) {
            out.push(x);
        }
    }
    out
}

/// First n chars.
pub fn cut(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

pub fn ksize(n: usize) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// A byte size as people write caps: `16 MiB`, `512 KiB`.
pub fn mib(n: u64) -> String {
    if n >= 1 << 20 { format!("{:.0} MiB", n as f64 / (1u64 << 20) as f64) } else { format!("{} KiB", n >> 10) }
}

/// URL without scheme and `www.`, trimmed to n chars.
pub fn short(url: &str, n: usize) -> String {
    let s = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url);
    let s = if s != url { s.strip_prefix("www.").unwrap_or(s) } else { s };
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", cut(s, n - 1)) }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Match {
    Exact,
    Words,
    Near,
    Topic,
}

/// The `match:` values, in `Match` order.
const MATCH: [&str; 4] = ["exact", "words", "near", "topic"];

impl Match {
    pub fn name(self) -> &'static str {
        MATCH[self as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Id {
    pub job: Option<usize>,
    pub u: usize,
    pub sec: Option<usize>,
    pub part: usize,
}

impl Id {
    fn parse(t: &str) -> Option<Id> {
        let (job, rest) = match t.strip_prefix('j').and_then(|r| r.split_once('.')) {
            Some((j, r)) => (Some(num(j)?), r),
            None => (None, t),
        };
        let rest = rest.strip_prefix('u')?;
        let (u, sec) = rest.split_once('#').map_or((rest, None), |(u, s)| (u, Some(s)));
        let mut id = Id { job, u: num(u)?, ..Id::default() };
        if let Some(s) = sec {
            let d = s.trim_end_matches(|c: char| c.is_ascii_lowercase());
            id.sec = Some(num(d)?);
            id.part = part_of(&s[d.len()..])?;
        }
        Some(id)
    }

    /// The section part as typed: `#3`, `#3b`; empty for a page.
    pub fn sec_tail(&self) -> String {
        let part = if self.part > 0 { letters(self.part) } else { String::new() };
        self.sec.map(|k| format!("#{k}{part}")).unwrap_or_default()
    }

    pub fn page(&self) -> String {
        let job = self.job.map(|j| format!("j{j}.")).unwrap_or_default();
        format!("{job}u{}", self.u)
    }
}

/// Part suffix of a section id, bijective base 26: part 0 = `a` (or none) … 25 = `z`, 26 = `aa`, 27 = `ab` ….
pub fn letters(part: usize) -> String {
    let (mut n, mut out) = (part + 1, Vec::new());
    while n > 0 {
        n -= 1;
        out.push(b'a' + (n % 26) as u8);
        n /= 26;
    }
    out.iter().rev().map(|&b| b as char).collect()
}

/// Inverse of `letters`; "" = part 0. None = too long to be a real part.
fn part_of(s: &str) -> Option<usize> {
    Some(s.bytes().try_fold(0usize, |n, b| n.checked_mul(26)?.checked_add((b - b'a') as usize + 1))?.saturating_sub(1))
}

/// `since:` → the first day it admits (`YYYY-MM-DD`), counted back from `today` (days since 1970-01-01):
/// `7d` `2w` `3m` (30 days) `1y`, or a date `2026` `2026-01` `2026-01-15`. None = not a `since:` value.
pub fn since_date(v: &str, today: i64) -> Option<String> {
    let unit =
        v.chars().last().and_then(|u| [('d', 1), ('w', 7), ('m', 30), ('y', 365)].into_iter().find(|x| x.0 == u));
    if let Some((_, days)) = unit {
        let count = num(&v[..v.len() - 1])?.min(100_000) as i64;
        let (year, month, day) = civil(today - count * days);
        return Some(format!("{year:04}-{month:02}-{day:02}"));
    }
    let parts: Vec<&str> = v.split('-').collect();
    let ok = |s: &&str, len: usize, hi: usize| s.len() == len && num(s).is_some_and(|n| (1..=hi).contains(&n));
    let fine = parts.len() <= 3 && ok(&parts[0], 4, 9999) && parts.get(1).is_none_or(|m| ok(m, 2, 12));
    (fine && parts.get(2).is_none_or(|d| ok(d, 2, 31)))
        .then(|| format!("{}-{}-{}", parts[0], parts.get(1).unwrap_or(&"01"), parts.get(2).unwrap_or(&"01")))
}

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian (chrono's range is ±262,000 years).
pub fn civil(z: i64) -> (i32, u32, u32) {
    let d = chrono::DateTime::from_timestamp(z * 86_400, 0).unwrap_or_default().date_naive();
    (d.year(), d.month(), d.day())
}

/// `20`, `1k`, `1.5k` → a count ≥ 1.
fn count(s: &str) -> Option<usize> {
    let (n, k) = s.strip_suffix(['k', 'K']).map_or((s, 1.0), |n| (n, 1000.0));
    let x = n.parse::<f64>().ok().filter(|x| x.is_finite() && *x > 0.0)? * k;
    (1.0..1e9).contains(&x).then_some(x as usize)
}

/// Whole seconds (`5`, `5s`) within `lo..=hi`.
fn secs(s: &str, lo: u64, hi: u64) -> Option<u64> {
    s.strip_suffix('s').unwrap_or(s).parse().ok().filter(|n| (lo..=hi).contains(n))
}

/// A byte size: `512k`, `32m` (binary units, `kb`/`mib` spellings too) or plain bytes.
fn bytes(s: &str) -> Option<u64> {
    let l = s.to_ascii_lowercase();
    let n = l.trim_end_matches(['b', 'i']);
    let kib = || n.strip_suffix('k').map_or((n, 0), |n| (n, 10));
    let (n, shift) = n.strip_suffix('m').map_or_else(kib, |n| (n, 20));
    n.parse::<u64>().ok()?.checked_mul(1 << shift)
}

fn num(s: &str) -> Option<usize> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

#[derive(Default, Debug)]
pub struct Cmd {
    pub ids: Vec<Id>,
    /// Bare job ids (`j3`): the job's command and pages.
    pub jobs: Vec<usize>,
    pub links: Vec<usize>,
    /// Media rows of the page (`u3 m1`): the picture itself, fetched on demand (`cli::asset`).
    pub media: Vec<usize>,
    pub urls: Vec<String>,
    pub files: Vec<String>,
    pub reserved: Vec<&'static str>,
    pub words: Vec<String>,
    pub mods: Vec<(String, String)>,
    /// A settings change's words (`setup free off`, `config set batch_parallel 8`): never a search (`stats::change`).
    pub sub: Vec<String>,
    /// The verb as typed (`search`), first among the non-mod tokens: printed first again, so a replay reads the
    /// same (`search login` stays a search).
    pub verb: Option<String>,
    /// Nothing to act on, mods and `json` aside, or a `?` (`fats`, `fats search`, `fats -v`): the status, and no
    /// arrow leads to it. A lone `site:` is a search, never idle.
    pub idle: bool,
    /// A usage problem: the `?` line to print (exit 2).
    pub ask: Option<String>,
    pub raw: String,
}

impl Cmd {
    pub fn has(&self, r: &str) -> bool {
        self.reserved.contains(&r)
    }

    pub fn get(&self, k: &str) -> Option<&str> {
        self.mods.iter().rev().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    }

    pub fn flag(&self, k: &str) -> bool {
        self.get(k) == Some("true")
    }

    /// Past the cache: `fresh:true`, or `js:true` (a render: what `! js_shell` and `! thin` ask for).
    pub fn fresh(&self) -> bool {
        self.flag("fresh") || self.flag("js")
    }

    /// A count mod (`limit:20`, `max:1k`); validated by the parser, so None = absent.
    pub fn count(&self, k: &str) -> Option<usize> {
        self.get(k).and_then(count)
    }

    /// Canonical form (what the repair table maps every input onto): ids, URLs, files, reserved words, the free
    /// words as typed, then the mods (a fix arrow keeps the user's query in front, as typed).
    pub fn canon(&self) -> String {
        self.with_mod("", None)
    }

    /// The canonical command with mod `k` set to `v` (None = dropped): the `→` fix of a mod row.
    pub fn with_mod(&self, k: &str, v: Option<&str>) -> String {
        let mut p: Vec<String> = self.verb.iter().cloned().collect();
        p.extend(self.ids.iter().map(|i| i.page() + &i.sec_tail()));
        p.extend(self.jobs.iter().map(|j| format!("j{j}")));
        p.extend(self.links.iter().map(|l| format!("l{l}")));
        p.extend(self.media.iter().map(|m| format!("m{m}")));
        p.extend(self.urls.iter().cloned());
        p.extend(self.files.iter().cloned());
        p.extend(self.reserved.iter().map(|r| r.to_string()));
        p.extend(self.words.iter().cloned());
        p.extend(self.mods.iter().filter(|(a, _)| a != k).map(|(k, v)| format!("{k}:{v}")));
        p.extend(v.map(|v| format!("{k}:{v}")));
        p.join(" ")
    }
}

fn is_url(t: &str) -> bool {
    let l = t.to_ascii_lowercase();
    if let Some(r) = l.strip_prefix("https://").or_else(|| l.strip_prefix("http://")) {
        return !r.is_empty();
    }
    // [a-z0-9-]+(\.[a-z0-9-]+)*\.[a-z]{2,}(/\S*)?
    let host = l.split('/').next().unwrap_or("");
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|x| !x.is_empty() && x.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        && labels.last().is_some_and(|x| x.len() >= 2 && x.bytes().all(|b| b.is_ascii_alphabetic()))
}

/// The last label of a bare name (no scheme, no `/`): it names a host only when it is a real TLD (`Words::tlds`:
/// `opencode.json`, `main.go` are words) and the name stands where an address is typed (`as_words`). A scheme, a
/// `/` or `site:` makes a host by its shape alone (`is_url`).
fn bare_tld(low: &str) -> Option<&str> {
    low.rsplit_once('.').filter(|_| !low.contains('/')).map(|(_, tld)| tld)
}

/// A URL's host by its name alone, no lookup (`dry:true` spends nothing, the network included): an IP address, or
/// a name whose last label is a real TLD (`Words::tlds`) a resolver can ask for (`askable`). False = `no_host`:
/// `dead.invalid`, `intranet`, `a..com`, `-a.com`, a 64-char label, none.
pub fn real_host(url: &str) -> bool {
    let host = crate::clean::host_of(url);
    let tld = host.strip_suffix('.').unwrap_or(&host).rsplit('.').next().unwrap_or_default();
    host.parse::<std::net::IpAddr>().is_ok() || (WORDS.tlds.contains(&tld) && askable(&host))
}

/// A name a resolver can ask for, by its shape alone (never its TLD: a test's `*.invalid` is one): every label
/// 1–63 chars, no hyphen at either end (RFC 1035 §2.3.1, §2.3.4; one root dot may end the name). The real run's
/// rule too: `provider::host_check` never looks the rest up.
pub fn askable(host: &str) -> bool {
    let label_ok = |l: &str| (1..=63).contains(&l.len()) && !l.starts_with('-') && !l.ends_with('-');
    host.strip_suffix('.').unwrap_or(host).split('.').all(label_ok)
}

/// A path is a path only when it says so: `-`, a leading `/`, a `shaped` token (`class_of`), a document or secret
/// extension or name (`page.html`, `a.pdf`, `id.pem`, `id_rsa`: `fetch::doc_kind`, `cli::secret`; alone, missing:
/// `no_file`), or any name that is a file here (`README`; the fence first, then one stat). With an id in the command
/// only explicit paths count (free text wins: `u1 price`). Beside search words only a `shaped` or extended path stays
/// one whether it exists or not, and `-` must be the whole command (`as_words`). A missing one-dot name whose extension
/// is a document's AND a real TLD (`notes.md`) is a file or a site: Err = the `?` that asks which (`file_or_site`), $0.
fn is_file(t: &str, any_id: bool) -> Result<bool, String> {
    if t == "-" || t.starts_with('/') {
        return Ok(true);
    }
    let ext = Path::new(t).extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    let doc = ext.as_ref().is_some_and(|e| crate::fetch::doc_kind(&format!(".{e}")).is_some());
    let file = !any_id && !t.contains("://") && (doc || filed(Path::new(t)) || crate::cli::secret(Path::new(t)));
    match file_or_site(t, &WORDS.tlds).filter(|_| file && doc) {
        Some(q) => Err(q),
        None => Ok(file),
    }
}

/// A one-dot name (no `/`; a `\` is `shaped`) whose TLD is in `tlds` and that is no file here: the `?` that asks
/// file or site, $0 (`is_file`; a lone `main.rs`, `parse`).
fn file_or_site(t: &str, tlds: &[&str]) -> Option<String> {
    let tld = bare_tld(&t.to_lowercase()).filter(|d| tlds.contains(d) && t.matches('.').count() == 1).is_some();
    (tld && !filed(Path::new(t))).then(|| format!("? {t}: a missing file or a site · type ./{t} or https://{t}"))
}

/// A token shaped as a path whatever stands beside it: a `./ ../ file:` prefix, a dot-led name (`.env`, `.ssh`; `.NET`,
/// `.io` lead a TLD: words; `.5` and `...` no name), a home (`~/`, `~name/`) or a variable (`$HOME/`, `${HOME}/`)
/// leading a `/`, a `\` path's lead (`.\x`, `..\x`, `~\x`, `\\server\share`; a `\` elsewhere is a word's beside a page,
/// `u1 price\|cost`, and a path's on a search, `parse`), a drive (`C:/x`, `C:\x`), a `/` path of ≥ 2 non-empty segments
/// (`/etc/hosts`; `/DE/` is one: beside words a search word, `as_words`), or a bare one of ≥ 3 that no host leads
/// (`internal/hr/salaries`; `TCP/IP` is a word, `docs.rs/a/b` a URL). A name of digits leads no home or variable:
/// `~5/10`, `$5/mo` are words.
fn shaped(t: &str) -> bool {
    let (b, low) = (t.as_bytes(), t.to_ascii_lowercase());
    let drive = b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\');
    let segments = t.split('/').filter(|s| !s.is_empty()).count();
    let deep = if t.starts_with('/') { segments >= 2 } else { segments >= 3 && !is_url(t) && !t.contains(':') };
    // the name after `~` `$` `.` up to a `/`: a letter, `_` or `{` first; after `.` no TLD (`.NET`, `.io`)
    let name = |r: &str, dot: bool| {
        let n = r.split('/').next().unwrap_or_default();
        n.starts_with(|c: char| c.is_ascii_alphabetic() || "_{".contains(c)) && !(dot && WORDS.tlds.contains(&n))
    };
    let home = |s: char| low.strip_prefix(s).is_some_and(|r| r.contains('/') && name(r, false));
    let dot = low.strip_prefix('.').is_some_and(|r| name(r, true));
    let lead = ["./", "../", "~/", "file:", ".\\", "..\\", "~\\", "\\\\"].iter().any(|p| low.starts_with(p));
    drive || deep || home('~') || home('$') || dot || lead
}

/// A local path as the shell would read it: `~/` is the home directory (`$HOME`; on Windows `%USERPROFILE%`, as
/// Node's `os.homedir()` reads it for Claude Code and Cursor).
pub fn local_path(f: &str) -> std::path::PathBuf {
    f.strip_prefix("~/").map_or_else(|| f.into(), |r| std::env::home_dir().unwrap_or_default().join(r))
}

/// A file here, or a path the `fats mcp` fence refuses by its name alone (`cli::outside`): out there the disk is
/// never looked at, and the path stays a path (`not_allowed`). A bare name never stands outside.
fn filed(p: &Path) -> bool {
    crate::cli::outside(p) || p.is_file()
}

/// Normalise one `key:value` (aliases, liberal booleans). Err = the `?` line.
fn modifier(k: &str, v: &str) -> Result<(String, String), String> {
    let (k, v) = alias(&k.to_ascii_lowercase(), v)?;
    // a threshold for this command (`batch_parallel:8`, `settings::limit_of`)
    if let Some(built) = crate::settings::LIMITS.get(k.as_str()) {
        let fit = crate::settings::fits(&k, &v).map(|_| (k.clone(), v.clone()));
        return fit.ok_or(format!("? {k}:{v} · a number like {k}:{built}"));
    }
    if !WORDS.keys.contains(&k.as_str()) {
        return Err(format!("? {k}: unknown · keys {}", WORDS.keys.join(" ")));
    }
    value(&k, v).map(|v| (k, v))
}

/// A key's other spellings (`n` `geo` `budget` `ua` `loose`) → (key, value).
fn alias(k: &str, v: &str) -> Result<(String, String), String> {
    let (k, v) = match k {
        "n" => ("limit", v.to_string()),
        "geo" | "loc" | "gl" | "country" => ("country", v.to_ascii_lowercase()),
        "budget" => ("cost", v.to_string()),
        "device" | "ua" => match v.to_ascii_lowercase().as_str() {
            "mobile" | "phone" => ("device", "mobile".into()),
            "desktop" | "pc" => ("device", "desktop".into()),
            _ => return Err(format!("? device:{v} · use device:mobile|desktop (Oxylabs only)")),
        },
        "loose" => {
            let n: u32 =
                v.parse().map_err(|_| format!("? loose:{v} · loose takes 0..100 · or match:exact|words|near|topic"))?;
            // 0 exact · 1-49 words · 50-79 near · 80+ topic (`MATCH` order)
            ("match", MATCH[[1, 50, 80].iter().filter(|&&lo| n >= lo).count()].into())
        }
        _ => (k, v.to_string()),
    };
    Ok((k.to_string(), v))
}

/// A known key's value, checked and normalised (`js:1` → `true`, `hedge:5s` → `5`). Err = the `?` line.
fn value(k: &str, v: String) -> Result<String, String> {
    if WORDS.bools.contains(&k) {
        let b = crate::settings::boolean(&v).ok_or_else(|| format!("? {k}:{v} · use {k}:true or {k}:false"))?;
        return Ok(b.to_string());
    }
    let n = match k {
        "hedge" => secs(&v, 0, 60).ok_or("hedge:{v} · seconds before the second provider, 0 to 60: hedge:0"),
        "wait" => secs(&v, 0, 300).ok_or("wait:{v} · seconds before a fetch answers pending, 0 to 300: wait:30"),
        "timeout" => secs(&v, 1, 300).ok_or("timeout:{v} · seconds for bd and oxyjs, 1 to 300: timeout:150"),
        "max_bytes" => bytes(&v)
            .filter(|n| (1 << 10..=crate::cli::MAX_BYTES_CEIL).contains(n))
            .ok_or("max_bytes:{v} · 1k to 64m (KiB, MiB): max_bytes:32m"),
        "fetch" => v.parse().ok().filter(|n| *n <= 50).ok_or("fetch:{v} · results to fetch ahead, 0 to 50: fetch:0"),
        "limit" | "max" | "from" => count(&v).map(|n| n as u64).ok_or("{k}:{v} · {k} takes a count: {k}:20 · {k}:1k"),
        _ => return text_value(k, v),
    };
    n.map(|n| n.to_string()).map_err(|q| format!("? {}", draw(q, &[("k", k), ("v", &v)])))
}

/// `value` for the keys whose value is a word: `match` `provider` `country` `since` `site` `cost`.
fn text_value(k: &str, v: String) -> Result<String, String> {
    let (ok, why) = match k {
        "match" => (MATCH.contains(&v.as_str()), "use match:exact|words|near|topic"),
        "country" => {
            (v.len() == 2 && v.bytes().all(|b| b.is_ascii_alphabetic()), "two letters: country:de · country:us")
        }
        "zone" => (
            v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "a Bright Data zone name: zone:web_unlocker1",
        ),
        "since" => (since_date(&v, 0).is_some(), "an age or a date: since:7d · since:2w · since:2026-01"),
        "cost" => {
            (v.parse::<f64>().is_ok_and(|x| x.is_finite() && x >= 0.0), "cost takes US dollars per call: cost:0.5")
        }
        "provider" => {
            // a JS rung is `provider:oxy js:true`, not a provider of its own (`Tier::renders`)
            use crate::provider::Tier;
            let t = Tier::parse(&v.to_ascii_lowercase()).filter(|t| !t.renders());
            let p = t.map(|t| t.spec().short).or((v.eq_ignore_ascii_case("auto")).then_some("auto"));
            let all: Vec<&str> = Tier::ALL.iter().filter(|t| !t.renders()).map(|t| t.spec().short).collect();
            return p.map(String::from).ok_or(format!("? provider:{v} · use provider:auto|{}", all.join("|")));
        }
        "site" => {
            let s = v.to_ascii_lowercase();
            let s = s.strip_prefix("https://").or(s.strip_prefix("http://")).unwrap_or(&s).trim_end_matches('/');
            let host = is_url(s).then(|| s.to_string());
            return host.ok_or(format!("? site:{v} · a host: site:example.com · site:docs.rs/serde"));
        }
        _ => (true, ""),
    };
    if ok { Ok(v) } else { Err(format!("? {k}:{v} · {why}")) }
}

/// Token-level repairs. Pure; one canonical reading or a `?` line. Beside the tokens: the reserved words typed as a
/// flag (`--cards` → `cards`), so a search never takes one as a word (`flag_word`).
fn repair(argv: &[String]) -> (Vec<String>, Vec<String>) {
    let s = argv.join(" ");
    let s = s.trim().trim_matches('`').trim();
    let s = s.strip_prefix("fats ").unwrap_or(s);
    let (mut out, mut flagged): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let mut it = s.split_whitespace().map(|t| t.trim_matches(|c| matches!(c, '"' | '\'' | '`'))).peekable();
    while let Some(t) = it.next() {
        // a bare `--` ends options in a shell, it is no word (`fats -- rust` searches "rust")
        if t.is_empty() || t == "--" {
            continue;
        }
        let mut t = t.to_string();
        // U2#3 → u2#3 · u2.3 → u2#3 · j8/u2 j8:u2 → j8.u2
        if t.starts_with(['U', 'J']) && t[1..].starts_with(|c: char| c.is_ascii_digit()) {
            t = t.to_lowercase();
        }
        let job =
            t.split_once(['/', ':']).filter(|(a, b)| a.strip_prefix('j').and_then(num).is_some() && b.starts_with('u'));
        if let Some((a, b)) = job {
            t = format!("{a}.{b}");
        }
        let sec = t.strip_prefix('u').and_then(|r| r.split_once('.')).filter(|(a, b)| num(a).and(num(b)).is_some());
        if let Some((a, b)) = sec {
            t = format!("u{a}#{b}");
        }
        // the output switches -v → verbose:true, --json → json:true · --full → full · -n 20 / n=20 → limit:20 ·
        // key=value → key:value; `--help` stays a flag
        if let Some(on) = [("-v", "verbose:true"), ("--json", "json:true")].iter().find(|s| s.0 == t) {
            t = on.1.into();
        } else if let Some(r) = t.strip_prefix("--").filter(|r| *r != "help" && WORDS.reserved.contains(r)) {
            t = r.to_string();
            flagged.push(t.clone());
        } else if t == "-n" && it.peek().is_some_and(|n| num(n).is_some()) {
            t = format!("limit:{}", it.next().unwrap_or_default());
        } else if let Some((k, v)) = t.split_once('=')
            && !k.is_empty()
            && k.bytes().all(|b| b.is_ascii_lowercase())
            && !v.is_empty()
        {
            t = format!("{k}:{v}");
        }
        // l 4 → l4 · u2 #3 → u2#3
        if t == "l" && it.peek().is_some_and(|n| num(n).is_some()) {
            t = format!("l{}", it.next().unwrap_or_default());
        }
        if let (Some(r), Some(prev)) = (t.strip_prefix('#'), out.last_mut())
            && num(r).is_some()
            && Id::parse(prev).is_some_and(|i| i.sec.is_none())
        {
            prev.push_str(&t);
            continue;
        }
        out.push(t);
    }
    (undecorate(out), flagged)
}

/// A pasted arrow's decorations are no words (`fats u1 js:true · bd, then oxyjs ≤$0.0030 ≈25s`, `u1#3 · 1.5k tok · $0`,
/// `u2 · rendered`, `config · FATS_CACHE`): on a command led by an id, a job, a URL with its scheme or
/// `setup`/`stats`/`config`, a `(…)` group (an older build's), a `≤$X` price (`$0` shell-expanded too), a `≈Ns` or
/// `~Ns` time, and a `·` with the words after it up to the next `key:value` are dropped, wherever they stand. Any other
/// command keeps them: `rust (programming language)` is a query.
fn undecorate(toks: Vec<String>) -> Vec<String> {
    let first = toks.first().map(|t| t.to_ascii_lowercase()).unwrap_or_default();
    let job = first.strip_prefix('j').and_then(num).is_some();
    let url = ["http://", "https://"].iter().any(|s| first.starts_with(s));
    if !(Id::parse(&first).is_some() || job || url || ["setup", "stats", "config"].contains(&first.as_str())) {
        return toks;
    }
    let (mut group, mut dot) = (false, false);
    let mut keep = |t: &String| {
        let inside = group || t.starts_with('(');
        group = inside && !t.ends_with(')');
        dot = (dot || t == "·") && key_value(t).is_none();
        let time = t.strip_prefix(['~', '≈']).and_then(|r| r.strip_suffix('s')).is_some_and(|n| num(n).is_some());
        // `≤` alone: an unquoted shell expands `$0` (`≤zsh.0030`)
        !(inside || dot || t.starts_with('≤') || time)
    };
    toks.into_iter().filter(|t| keep(t)).collect()
}

/// A `key:value` by its shape (`limit:5`, `filetype:pdf`); a URL's scheme and a path are none.
fn key_value(t: &str) -> Option<(&str, &str)> {
    let (k, v) = t.split_once(':')?;
    let key = !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphabetic() || b == b'_');
    let scheme = ["http", "https"].contains(&k.to_ascii_lowercase().as_str());
    (key && !scheme && !v.is_empty() && !v.starts_with('/')).then_some((k, v))
}

/// The command a word is a typo of (edits = optimal string alignment: a letter changed, added, dropped, or two
/// neighbours swapped). A lone word: one edit from `commands` (`stts` → `stats`). A first word led
/// into a URL or id: up to two edits, fewer than half the command's length, from `commands` + `led`
/// (`fetchh https://x` → `fetch`, `hlep u1` → `help`).
fn typo(w: &str, led: bool) -> Option<&'static str> {
    let w = w.to_lowercase();
    if WORDS.reserved.contains(&w.as_str()) {
        return None; // a command word itself (`stats` is not a typo of `status`)
    }
    let extra = if led { &WORDS.led[..] } else { &[] };
    let max = |r: &str| if led { 2.min((r.len() - 1) / 2) } else { 1 };
    WORDS.commands.iter().chain(extra).copied().find(|r| (1..=max(r)).contains(&strsim::osa_distance(&w, r)))
}

enum Class {
    Id(Id),
    Job(usize),
    Link(usize),
    Media(usize),
    Url(String),
    File(String),
    Res(&'static str),
    Mod(String, String),
    Num(String),
    Word,
}

pub fn parse(argv: &[String]) -> Cmd {
    let (mut toks, flagged) = repair(argv);
    let mut cmd = Cmd { raw: toks.join(" "), ..Cmd::default() };
    // a verb is the first token, mods aside (`limit:5 search login`): the words after it are meant as words
    let lead = toks.iter().position(|t| !matches!(class_of(t, false, false, true), Ok(Class::Mod(..))));
    cmd.verb = lead.filter(|&i| WORDS.verbs.contains(&toks[i].to_lowercase().as_str())).map(|i| toks.remove(i));
    if let Some(i) = lead.filter(|_| cmd.verb.is_none()) {
        own_words(&mut cmd, &mut toks, i);
    }
    // `--help` or `-h` anywhere (an en or em dash for `--`: smart punctuation), or `help` first, is the card: never
    // a paid search for the other words
    let first_help = cmd.verb.is_none() && lead.is_some_and(|i| toks[i].eq_ignore_ascii_case("help"));
    if first_help || toks.iter().any(|t| HELP_FLAGS.contains(&t.as_str())) {
        toks = vec!["help".into()];
    }
    let has_id = toks.iter().any(|t| Id::parse(t).is_some());
    let (mut cls, mut any_id) = (Vec::new(), false);
    for t in &toks {
        match class_of(t, has_id, any_id, cmd.verb.is_some()) {
            // a directory here is no file and no search word (`/etc`, `./src`); outside the `fats mcp` fence the
            // disk is never looked at: the run says `not_allowed`
            Ok(Class::File(f)) if !crate::cli::outside(&local_path(&f)) && local_path(&f).is_dir() => {
                cmd.ask.get_or_insert(format!("? {f} is a directory · name a file in it"));
            }
            Ok(c) => {
                any_id |= matches!(c, Class::Id(_));
                cls.push((t.clone(), c));
            }
            Err(q) => {
                cmd.ask.get_or_insert(q);
            }
        }
    }
    as_words(&mut cls);
    // free words that are all flags (`--login`, `-x`, `limit:5 -x`; an en or em dash for `--`, `–x`) are an option
    // fats lacks, never a search; any other free word makes every word text, wherever the dash is (doctrine §1:
    // `git rebase --onto`, `rust -tokio` excludes, `-5 degrees`), as does a verb, an id or a file: beside a page
    // the words are a free filter (`u1 -n`); beside a URL they would be a paid `site:` search, so refused. The text
    // keeps a dash as typed.
    let search = cmd.verb.is_none() && !cls.iter().any(|p| matches!(p.1, Class::Id(_) | Class::File(_)));
    let words: Vec<&String> = cls.iter().filter(|p| search && matches!(p.1, Class::Word)).map(|p| &p.0).collect();
    if let Some(t) = words.first().filter(|_| words.iter().all(|t| is_flag(t))) {
        let name = t.trim_start_matches(['-', '–', '—']);
        let fix =
            if WORDS.bools.contains(&name) { format!("mod: {name}:true") } else { format!("keys: {} setup", bin()) };
        cmd.ask.get_or_insert(format!("! unknown flag {t} · {fix} · help: {} --help", bin()));
    }
    // mods and the output mod aside (one `alone` for both): a lone word, or a first word led into a URL or id,
    // may be a command's typo or a word that names no command
    let kinds: Vec<&(String, Class)> =
        cls.iter().filter(|p| !matches!(p.1, Class::Mod(..) | Class::Res("json"))).collect();
    let led = matches!(kinds[..], [(_, Class::Word), (_, Class::Url(_) | Class::Id(_)), ..]);
    let (lone, none) = (matches!(kinds[..], [(_, Class::Word)]), kinds.is_empty());
    // a lone one-dot name ending in a file's extension (`main.rs`) that is no file here is a file or a site: ask, $0
    if let [(t, Class::Url(_))] = kinds[..]
        && let Some(q) = file_or_site(t, &WORDS.file_tlds)
    {
        cmd.ask.get_or_insert(q);
    }
    sort_into(&mut cmd, kinds.len() == 1, cls, any_id);
    // a `\` inside a word is a filter's beside a page (`u1 price\|cost`); on a search it is a local path's
    // (`Documents\salaries`), and a path never reaches a search: `no_file`
    if crate::cli::is_search(&cmd) {
        (cmd.files, cmd.words) = std::mem::take(&mut cmd.words).into_iter().partition(|w| w.contains('\\'));
    }
    flag_word(&mut cmd, &flagged);
    cmd.idle = none && cmd.get("site").is_none() || cmd.ask.is_some();
    if cmd.verb.is_none() && (led || lone) {
        ask_typo(&mut cmd, led);
    }
    // a `?`/`!` row is no search: a closing `json` stays its output mod (`whoami json`)
    if cmd.ask.is_some() && cmd.words.last().is_some_and(|w| w.eq_ignore_ascii_case("json")) {
        cmd.words.pop();
        cmd.reserved.push("json");
    }
    cmd
}

/// A reserved word typed as a flag (`--cards`) that became a search word (no page in the command): a `?` with the
/// two commands it meant, never a paid search for `… cards`.
fn flag_word(cmd: &mut Cmd, flagged: &[String]) {
    let page = !(cmd.ids.is_empty() && cmd.urls.is_empty() && cmd.files.is_empty() && cmd.jobs.is_empty());
    let Some(f) = flagged.iter().find(|f| !page && cmd.words.contains(f)) else { return };
    let rest = cmd.words.iter().filter(|w| *w != f).map(String::as_str).collect::<Vec<_>>().join(" ");
    let (b, search) = (bin(), "is a mod, not a search word · search first:");
    cmd.ask.get_or_insert(format!("? --{f} {search} {b} {rest} · then: {b} u1 {f}"));
}

/// What one token is. `has_id` = the command holds an id (only explicit paths are files then), `any_id` = an id
/// came before this token (a URL-looking word after it is text), `verb` = a leading verb was dropped. Err = the `?`
/// line.
fn class_of(t: &str, has_id: bool, any_id: bool, verb: bool) -> Result<Class, String> {
    let low = t.to_lowercase();
    if let Some(id) = Id::parse(t) {
        return Ok(Class::Id(id));
    }
    if let Some(n) = t.strip_prefix('j').and_then(num) {
        return Ok(Class::Job(n));
    }
    if let Some(n) = t.strip_prefix('l').and_then(num) {
        return Ok(Class::Link(n));
    }
    // `m1` names a picture only next to a page id; alone it is a word (`apple m1 review`)
    if let Some(n) = t.strip_prefix('m').and_then(num).filter(|_| has_id) {
        return Ok(Class::Media(n));
    }
    if let Some((a, w)) = t.split_once(':')
        && Id::parse(a).is_some_and(|i| i.sec.is_some())
        && w.split('-').all(|x| num(x).is_some())
    {
        // `u3#2:41-80`: a line window
        return Err(format!("? {t}: line windows are not in v1 · read the section: {} {a}", bin()));
    }
    // a word people type for a command (`login`), never after a verb: `search login` searches it
    let alias = WORDS.aliases.iter().find(|a| !verb && (a.0 == low || a.0 == t)).map(|a| a.1);
    if let Some(r) = WORDS.reserved.iter().copied().find(|r| *r == low).or(alias) {
        return Ok(Class::Res(r));
    }
    if shaped(t) && !low.starts_with("file:") {
        // a drive (`C:\x`) before any `key:value` reading: a path, never `? c: unknown`
        return Ok(Class::File(t.to_string()));
    }
    if low.starts_with("file:") {
        // a local file (`file:///a/b.pdf` → `/a/b.pdf`, percent-decoded), read like a path (and fenced the same
        // way under `fats mcp`), never a search. A host is another machine on every OS (the URL parser reads
        // `localhost` as none; url's `to_file_path` turns a host into a `\\host\share` path on Windows only)
        let path = url::Url::parse(t).ok().filter(|u| u.scheme() == "file" && u.host().is_none());
        let path = path.and_then(|u| u.to_file_path().ok()).map(|p| Class::File(p.to_string_lossy().into_owned()));
        return path.ok_or(format!("? {t}: a file URL names a local path: {} /path/to/file", bin()));
    }
    if let Some((k, v)) = key_value(t) {
        // a search engine's own query key (`filetype:pdf`) goes to the search as typed
        if WORDS.operators.contains(&k.to_ascii_lowercase().as_str()) {
            return Ok(Class::Word);
        }
        return modifier(k, v).map(|(k, v)| Class::Mod(k, v));
    }
    if is_file(t, has_id)? {
        return Ok(Class::File(t.to_string()));
    }
    if !any_id && is_url(t) && bare_tld(&low).is_none_or(|tld| WORDS.tlds.contains(&tld)) {
        return Ok(Class::Url(if low.starts_with("http") { t.to_string() } else { format!("https://{t}") }));
    }
    if num(t).is_some() {
        return Ok(Class::Num(t.to_string()));
    }
    if t.len() > 1
        && t.trim_start_matches(['u', 'j', 'l']).len() + 1 == t.len()
        && t[1..].bytes().all(|b| b.is_ascii_digit())
    {
        // `l99999999999999999999`: an id past usize, not words to search
        return Err(format!("? {t}: out of range"));
    }
    Ok(Class::Word)
}

/// A token that only looks like a place is a search word: a bare name (no scheme, no `/`, `bare_tld`) names a
/// host unless the command holds a free word (not a host, not a URL: `example.com example.org` = two pages), and
/// then still when it leads the way an address is typed, in lower case (`docs.rs tokio spawn`, mods and a verb
/// aside); mid-query (`contact at acme.com phone`) or written as a name (`Booking.com hotels Lisbon`) it is a
/// word, so no one-site search the agent never asked for.
fn as_words(cls: &mut [(String, Class)]) {
    let free = cls.iter().any(|(t, c)| matches!(c, Class::Word | Class::Num(_)) && !is_flag(t));
    let lead = cls.iter().position(|(t, c)| !matches!(c, Class::Mod(..)) && !is_flag(t));
    for (i, (t, c)) in cls.iter_mut().enumerate() {
        let typed = Some(i) == lead && !t.bytes().any(|b| b.is_ascii_uppercase());
        if matches!(c, Class::Url(_)) && bare_tld(&t.to_lowercase()).is_some() && free && !typed {
            *c = Class::Word;
        }
    }
    // a local path never reaches a search: a `shaped` one or one with an extension is a file wherever it stands
    // (`no_file`, `not_allowed`); beside search words only a bare shape that is no file here is text (`CORP /DE/
    // company`, `TCP/IP tuning` search), unless the `fats mcp` fence refuses it by name (`not_allowed`, the disk
    // never looked at), and `-` (stdin) is the whole command
    if cls.iter().any(|p| matches!(p.1, Class::Word)) {
        for (t, c) in cls.iter_mut() {
            if let Class::File(p) = c
                && (p == "-" || !shaped(t) && Path::new(p).extension().is_none() && !filed(&local_path(p)))
            {
                *c = Class::Word;
            }
        }
    }
}

/// A flag as typed: a dash (an en or em dash for `--`: smart punctuation) and a name.
fn is_flag(t: &str) -> bool {
    t.len() > 1 && t.starts_with(['-', '–', '—'])
}

/// The classified tokens into `cmd`. Reserved words are reserved only when there is no free text, a list word
/// leading the words after a page id aside (`Words::lists`); a bare number with a page id has several readings, so it
/// asks. `alone` = one token, mods and `json` aside (`parse`); an alone-only word (`setup`) beside a page (an id, URL
/// or file) is text: `u1 setup` filters, `setup full` is setup.
fn sort_into(cmd: &mut Cmd, alone: bool, cls: Vec<(String, Class)>, any_id: bool) {
    let words = cls.iter().filter(|(_, c)| matches!(c, Class::Word)).count();
    let nums = cls.iter().filter(|(_, c)| matches!(c, Class::Num(_))).count();
    // a pinned `match:` says "this is a filter": a bare number is then text
    let pinned = cls.iter().any(|(_, c)| matches!(c, Class::Mod(k, _) if k == "match"));
    let free = words > 0 || nums > 1 || (nums > 0 && (!any_id || pinned));
    let page = cls.iter().any(|(_, c)| matches!(c, Class::Id(_) | Class::Url(_) | Class::File(_)));
    let lead = cls.iter().position(|(_, c)| matches!(c, Class::Res(_) | Class::Word | Class::Num(_)));
    // `stats` leading free words filters its rows (`fats stats rust all`: every reserved word a mod), never a search
    let index = lead.is_some_and(|i| matches!(cls[i].1, Class::Res("stats")));
    // `json` closing the words beside a page id or file is the output mod (`u1 price json`); on a search it is a
    // word like every reserved word there (`vite config json`, `example.com rust json`): JSON is `json:true`
    let read = index || cls.iter().any(|(_, c)| matches!(c, Class::Id(_) | Class::File(_)));
    let last = cls.iter().rposition(|(_, c)| !matches!(c, Class::Mod(..)));
    let list = |i, r: &str| index || Some(i) == lead && any_id && WORDS.lists.contains(&r);
    for (i, (t, c)) in cls.into_iter().enumerate() {
        match c {
            Class::Res(r) if r == "json" && Some(i) == last && read => cmd.reserved.push(r),
            Class::Id(i) => cmd.ids.push(i),
            Class::Job(j) => cmd.jobs.push(j),
            Class::Link(l) => cmd.links.push(l),
            Class::Media(m) => cmd.media.push(m),
            Class::Url(u) => cmd.urls.push(u),
            Class::File(f) => cmd.files.push(f),
            Class::Mod(k, v) => cmd.mods.push((k, v)),
            Class::Res(r) if !free && (alone || !page || !WORDS.alone.contains(&r)) || list(i, r) => {
                cmd.reserved.push(r)
            }
            Class::Num(n) if cmd.reserved.iter().any(|r| WORDS.lists.contains(r)) => cmd.words.push(n),
            Class::Num(n) if !free => {
                let q = number_ask(cmd, &n);
                cmd.ask.get_or_insert(q);
            }
            _ => cmd.words.push(t),
        }
    }
}

/// `fats u2#3 4`: link, section or text? Every reading as a command.
fn number_ask(cmd: &Cmd, n: &str) -> String {
    let b = bin();
    match cmd.ids.last() {
        Some(i) => {
            let (full, page) = (cmd.canon(), i.page());
            format!("? {n}: link → {b} {full} l{n} · section → {b} {page}#{n} · text → {b} {full} {n} match:words")
        }
        None => format!("? {n}: link → {b} l{n} · text → {b} {n} match:words"),
    }
}

/// `setup <name> [on|off]`, `config set <name> <value>`, `config unset <name>`: the command's own words, from
/// `toks[i]` (`plain` = the free switch's other name). Nothing else follows `setup` but mods and command words: a
/// word there is a `?` (`? setup oxi → fats setup oxy`, a switch or on/off one edit away), never a paid search.
fn own_words(cmd: &mut Cmd, toks: &mut Vec<String>, i: usize) {
    let at = |k: usize| toks.get(i + k).map(|t| t.to_lowercase()).unwrap_or_default();
    let (verb, on, switches) = (toks[i].to_lowercase(), at(2) == "on" || at(2) == "off", crate::settings::SWITCHES);
    let own = |t: &String| {
        !HELP_FLAGS.contains(&t.as_str())
            && t != "json"
            && !matches!(class_of(t, false, false, true), Ok(Class::Mod(..)))
    };
    let n = match (verb.as_str(), at(1).as_str()) {
        ("setup", w) if switches.contains(&w) || w == "plain" => 1 + usize::from(on),
        // the level is checked by `settings::set`, which names the levels
        ("setup", "telemetry") => 1 + usize::from(toks.get(i + 2).is_some_and(own)),
        ("telemetry", "show") | ("setup", "import-env") => 1,
        // the value is every word up to the mods, whole (`~/Library/Application Support/x` is a cache dir)
        ("config", "set") => 3 + toks.iter().skip(i + 4).take_while(|t| own(t)).count(),
        ("config", "unset") => 2,
        _ => 0,
    };
    let sub = toks.drain(i + 1..(i + 1 + n).min(toks.len())).enumerate();
    let low =
        |(k, t): (usize, String)| if k > 1 { t } else { t.to_lowercase().replacen("plain", "free", (k == 0).into()) };
    cmd.sub = sub.map(low).collect();
    if cmd.sub.len() > 3 {
        let v = cmd.sub.split_off(2).join(" ");
        cmd.sub.push(v);
    }
    let word = |t: &&String| own(t) && matches!(class_of(t, false, false, true), Ok(Class::Word));
    let Some(w) = toks[i + 1..].iter().find(word).filter(|_| verb == "setup") else { return };
    let near = switches.into_iter().chain(["on", "off"]).find(|n| strsim::osa_distance(&w.to_lowercase(), n) == 1);
    let (raw, b, all) = (&cmd.raw, bin(), switches.join("|"));
    let fix = near.map_or(format!(" · {b} setup <{all}|plain> [on|off] · zone:<name>"), |n| {
        format!(" → {b} {}", raw.replace(w.as_str(), n))
    });
    cmd.ask = Some(format!("? {raw}{fix}"));
}

/// A command's typo asks (it is a paid search otherwise; a verb before it says "words"): `led` = a first word
/// led into a URL or id, else a lone word (mods aside: `yelp limit:5` searches).
fn ask_typo(cmd: &mut Cmd, led: bool) {
    let (Some(w), b) = (cmd.words.first().cloned(), bin()) else { return };
    if let Some((_, why)) = WORDS.elsewhere.iter().find(|e| !led && e.0.split(' ').any(|o| w.eq_ignore_ascii_case(o))) {
        cmd.ask.get_or_insert(format!("! {w}{} · {}", draw(why, &[("b", b)]), to_guide()));
    }
    if let Some(r) = typo(&w, led) {
        let fixed: Vec<&str> = cmd.raw.split(' ').map(|t| if t == w { r } else { t }).collect();
        let q = format!("? {w}: a command? {b} {} · a web search: {b} search {}", fixed.join(" "), cmd.raw);
        cmd.ask.get_or_insert(q);
    }
}

/// One command's output. `rec` = the JSON record fields beyond what `emit` derives.
#[derive(Default)]
pub struct Out {
    pub lines: Vec<String>,
    pub rc: i32,
    /// Boxed: an `IndexMap` (key order kept) would make every `Result<_, Out>` large.
    pub rec: Box<serde_json::Map<String, serde_json::Value>>,
    /// One per input of a multi-input command: `json` prints these records, not the summary.
    pub parts: Vec<Out>,
}

impl Out {
    pub fn new(lines: Vec<String>) -> Out {
        Out { lines, ..Out::default() }
    }

    /// THE `?` printer: a usage row, exit 2; a key-shaped word in it (what was typed may be a key) as its tail.
    pub fn ask(line: &str) -> Out {
        Out { rc: 2, ..Out::new(vec![crate::setup::masked(line)]).with("error", "usage") }
    }

    /// Whole-command failure (`= !` line + fix, exit 1).
    pub fn fail(code: &str, eq: String, hint: String) -> Out {
        Out { rc: 1, ..Out::new(vec![eq, hint.clone()]).with("error", code).with("hint", hint) }
    }

    pub fn set(&mut self, k: &str, v: impl Into<serde_json::Value>) -> &mut Out {
        self.rec.insert(k.into(), v.into());
        self
    }

    /// `set` on an answer being built: `Out::new(lines).with("id", id)`.
    pub fn with(mut self, k: &str, v: impl Into<serde_json::Value>) -> Out {
        self.set(k, v);
        self
    }

    /// Rows that qualify the answer (`! bad_setting`, `! not_in_v1 country:de`): right after the `=` line, and
    /// as `warnings` in the record.
    pub fn warn(&mut self, rows: Vec<String>) {
        if rows.is_empty() {
            return;
        }
        // after the `=` line and the `!` rows already there: the cause first, then what else was skipped
        let bang = |l: &String| l.starts_with("! ") || l.split(' ').nth(1) == Some("!");
        let at = self.lines.len().min(1) + self.lines.iter().skip(1).take_while(|l| bang(l)).count();
        self.lines.splice(at..at, rows.iter().cloned());
        let mut all = self.rec.remove("warnings").and_then(|w| w.as_array().cloned()).unwrap_or_default();
        all.extend(rows.into_iter().map(Into::into));
        self.set("warnings", all);
    }

    /// `max:N`: keep the `=` line and as many lines as fit in N tokens; the last row says what was cut.
    pub fn cap(&mut self, max: usize, fix: &str) {
        let mut used = 0;
        let Some(keep) = self.lines.iter().skip(1).position(|l| {
            used += crate::clean::tok(l);
            used > max
        }) else {
            return;
        };
        let cut: Vec<String> = self.lines.drain(keep + 1..).collect();
        let t: usize = cut.iter().map(|l| crate::clean::tok(l)).sum();
        self.lines.push(format!("… max:{max} cut {} lines ~{t}tok → {} {fix}", cut.len(), bin()));
        self.set("cut", cut.len());
    }
}

/// The only printer: lines mode (agent path), or JSON Lines: one record, or one per input (the command's
/// warnings go on each). Every configured secret is masked in both (`redact`).
pub fn emit(o: &Out, json: bool) -> String {
    let text = match (json, o.parts.is_empty()) {
        (false, _) => o.lines.join("\n") + "\n",
        (true, true) => record(o, &[]),
        (true, false) => {
            let warn = o.rec.get("warnings").and_then(|w| w.as_array()).map_or(&[][..], |w| w.as_slice());
            o.parts.iter().map(|p| record(p, warn)).collect()
        }
    };
    redact(&text, &crate::settings::secrets())
}

pub const MASK: &str = "***";
/// A run of this many characters of a secret is masked wherever it shows (a provider may echo part of a key).
const SECRET_RUN: usize = 12;
/// Shorter secrets are masked only whole; under this length a value is not a key and is left alone.
const SECRET_MIN: usize = 8;

/// `text` with every `SECRET_RUN`-char run of a secret (a whole secret of `SECRET_MIN`.. chars) as `MASK`, matched
/// ASCII case-folded (a provider may echo a key upper-cased; folding keeps every byte offset).
pub fn redact(text: &str, secrets: &[String]) -> String {
    let (mut hide, low) = (vec![false; text.len()], text.to_ascii_lowercase());
    for s in secrets.iter().map(|s| s.to_ascii_lowercase()) {
        let at: Vec<usize> = s.char_indices().map(|c| c.0).chain([s.len()]).collect();
        let n = at.len() - 1;
        if n < SECRET_MIN {
            continue;
        }
        let w = n.min(SECRET_RUN);
        for i in 0..=n - w {
            let run = &s[at[i]..at[i + w]];
            for (j, _) in low.match_indices(run) {
                hide[j..j + run.len()].fill(true);
            }
        }
    }
    if !hide.contains(&true) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut masked = false;
    for (i, c) in text.char_indices() {
        match (hide[i], masked) {
            (true, false) => out.push_str(MASK),
            (false, _) => out.push(c),
            _ => {}
        }
        masked = hide[i];
    }
    out
}

fn record(o: &Out, warn: &[serde_json::Value]) -> String {
    let mut r = o.rec.clone();
    if !warn.is_empty() {
        let mut all = warn.to_vec();
        if let Some(serde_json::Value::Array(own)) = r.remove("warnings") {
            all.extend(own);
        }
        r.insert("warnings".into(), all.into());
    }
    let text = o.lines.join("\n");
    for (k, v) in [
        ("provider", "local".into()),
        ("ms", 0.into()),
        ("cost", 0.0.into()),
        ("bytes_in", 0.into()),
        ("cache", false.into()),
        ("id", serde_json::Value::Null),
        ("url", serde_json::Value::Null),
    ] {
        r.entry(k).or_insert(v);
    }
    r.insert("bytes_out".into(), text.len().into());
    r.insert("lines".into(), o.lines.clone().into());
    r.sort_keys();
    serde_json::Value::Object(*r).to_string() + "\n"
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    thread_local! {
        /// `NO_COLOR` as this thread's `paint` reads it (Some = set or not), None = the environment's: a test sees
        /// both sides without touching the process environment.
        pub(crate) static NO_COLOR: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    }

    fn c(s: &str) -> Cmd {
        parse(&s.split(' ').map(String::from).collect::<Vec<_>>())
    }

    /// One filler: a named hole becomes its value, any other brace stays; styles are only `paint`'s.
    #[test]
    fn draw_fills_named_holes_only() {
        assert_eq!(draw("{a}+{a} {c} {\"k\": 1} {", &[("a", "1"), ("b", "2")]), "1+1 {c} {\"k\": 1} {");
        assert_eq!(draw("{bold}x{/}", &NO_HOLES), "{bold}x{/}");
    }

    /// Every screen, filled with the holes its owner fills (`stats::home`, `ready`, `tip`), leaves no hole: a
    /// typo in a data file (`{bodl}`) fails here, never on a user's terminal. Braces in a JSON line stay.
    #[test]
    fn every_screen_fills_every_hole() {
        let mut fills = vec!["ready", "yours", "pages", "id", "last", "spent", "site", "label", "text", "from"];
        fills.extend([
            "status", "name", "short", "mint", "signup", "step", "rank", "free", "cost", "b", "url", "env", "level",
            "life",
        ]);
        let holes: Vec<(&str, &str)> = fills.iter().map(|h| (*h, "X")).collect();
        for (name, text) in SCREENS {
            let out = paint(text, &holes);
            let left = out.split('{').skip(1).find(|r| r.split_once('}').is_some_and(|(h, _)| !h.contains(' ')));
            assert!(!text.is_empty() && left.is_none(), "{name}: {left:?}");
        }
        assert_eq!(screen("help-nope"), "");
        assert_eq!(help_screen(&["help".into(), "nope".into()]), Some(paint(screen("help"), &NO_HOLES)));
        assert_eq!(help_screen(&["rust".into(), "help".into()]), None, "a search for help");
    }

    /// The repair table, one row per case: input → canonical command.
    const REPAIRS: &[(&str, &str)] = &[
        // a typed verb stays first in the canonical form (it replays the same reading)
        ("read u2#3", "read u2#3"),
        ("open u6", "open u6"),
        ("search cheap proxies", "search cheap proxies"),
        ("u2.3", "u2#3"),
        ("u2 #3", "u2#3"),
        ("\"u2#3\"", "u2#3"),
        ("U2#3", "u2#3"),
        ("`fats u2#3`", "u2#3"),
        ("u2#3 --full", "u2#3 full"),
        ("u2 -n 20", "u2 limit:20"),
        ("u2 n=20", "u2 limit:20"),
        ("u2#3 l 4", "u2#3 l4"),
        ("j8/u2", "j8.u2"),
        ("j8:u2", "j8.u2"),
        ("J8.U2#3b", "j8.u2#3b"),
        ("u3 loose:50", "u3 match:near"),
        ("u3 price js:1", "u3 price js:true"),
        ("u3 n:5 geo:FR budget:0.5", "u3 limit:5 country:fr cost:0.5"),
        ("ex.com ua:Phone", "https://ex.com device:mobile"),
        ("u3#2 full price", "u3#2 full price"),
        ("u3#2 links", "u3#2 links"),
        ("u1 links From:11", "u1 links from:11"),
        ("u1 iphone 15", "u1 iphone 15"),
        ("u1 15 match:words", "u1 15 match:words"),
        ("u1 -n", "u1 -n"),
        ("u1 -x -y", "u1 -x -y"),
        ("example.com", "https://example.com"),
        // a bare host needs a real TLD and stands alone or leads the command in lower case; mid-query or written
        // as a name it is a word; a scheme or a `/` makes a host anywhere
        ("opencode mcp config opencode.yaml", "opencode mcp config opencode.yaml"),
        ("pyproject.toml build backend options", "pyproject.toml build backend options"),
        ("nextjs next.config.mjs", "nextjs next.config.mjs"),
        ("main.go error", "main.go error"),
        ("logo.png size", "logo.png size"),
        ("cargo build main.rs", "cargo build main.rs"),
        ("build lib.so", "build lib.so"),
        ("rust x.ai", "rust x.ai"),
        ("contact at acme.com phone", "contact at acme.com phone"),
        ("Purchasing.NYC Lakewood New Jersey", "Purchasing.NYC Lakewood New Jersey"),
        ("Booking.com hotels Lisbon", "Booking.com hotels Lisbon"),
        ("Booking.com", "https://Booking.com"),
        ("fetch docs.rs tokio", "fetch https://docs.rs tokio"),
        ("docs.rs tokio spawn", "https://docs.rs tokio spawn"),
        ("limit:5 docs.rs tokio", "https://docs.rs tokio limit:5"),
        // bare hosts with no free word beside them are pages, a batch
        ("example.com example.org", "https://example.com https://example.org"),
        ("Example.com Example.org limit:5", "https://Example.com https://Example.org limit:5"),
        ("example.com example.org rust", "https://example.com example.org rust"),
        ("main.rs error handling", "https://main.rs error handling"),
        ("rust example.com", "rust example.com"),
        ("example.com rust", "https://example.com rust"),
        ("x.co", "https://x.co"),
        ("a.io", "https://a.io"),
        ("blog.rust-lang.org", "https://blog.rust-lang.org"),
        ("tokio docs.rs/tokio", "https://docs.rs/tokio tokio"),
        ("https://main.rs", "https://main.rs"),
        ("https://ex.com/a?b=1", "https://ex.com/a?b=1"),
        ("u1 v2.0", "u1 v2.0"),
        ("dry:yes u1#2", "u1#2 dry:true"),
        ("l4", "l4"),
        ("help", "help"),
        ("u1 max_bytes:32MiB", "u1 max_bytes:33554432"),
        ("-h", "help"),
        ("--help", "help"),
        ("how to get help", "how to get help"),
        ("u1 hedge:0", "u1 hedge:0"),
        ("u1 hedge:5s", "u1 hedge:5"),
        ("u1 timeout:300", "u1 timeout:300"),
        ("rust news site:HTTPS://Blog.Rust-Lang.org/ since:2w", "rust news site:blog.rust-lang.org since:2w"),
        ("serde site:docs.rs/serde", "serde site:docs.rs/serde"),
        // command words never search: status, version, ls; typos ask (below) unless a verb says "words"
        ("status", "status"),
        ("ls", "status"),
        ("--version", "version"),
        ("-V", "version"),
        ("rust status", "rust status"),
        ("u1 version", "u1 version"),
        ("status json:true", "status json:true"),
        ("search stat", "search stat"),
        ("search login", "search login"),
        ("stats", "stats"),
        ("ale", "ale"),
        // a real word one edit from a reading word searches
        ("yelp", "yelp"),
        ("fuel", "fuel"),
        ("jason", "jason"),
        ("mode", "mode"),
        // a word two edits from a command searches when lone or with mods; before a URL or id only from a long one
        ("acont", "acont"),
        ("acont limit:5", "acont limit:5"),
        ("yelp limit:5", "yelp limit:5"),
        ("go https://go.dev", "https://go.dev go"),
        ("rust u1", "u1 rust"),
        ("search fetchh https://x.com", "search https://x.com fetchh"),
        ("fetchh rust", "fetchh rust"),
        // a file URL decodes to this OS's path: Windows has no rootless `/tmp`, its paths start at a drive
        #[cfg(unix)]
        ("file:///tmp/a%20b.html", "/tmp/a b.html"),
        #[cfg(unix)]
        ("file://localhost/tmp/x", "/tmp/x"),
        #[cfg(windows)]
        ("file:///C:/tmp/a%20b.html", "C:\\tmp\\a b.html"),
        // a word people type for a command is that command, mods and `json` aside; with another word it is text
        ("login", "setup"),
        ("Sign-In", "setup"),
        ("usage", "account"),
        ("-v", "verbose:true"),
        ("rust --json", "rust json:true"),
        ("stat json", "stats json"),
        ("login limit:5", "setup limit:5"),
        ("keys json", "setup json"),
        ("login page design", "login page design"),
        ("u1 key", "u1 key"),
        ("u1 install", "u1 install"),
        // `--help` / `-h` anywhere, `help` first: the card
        ("rust --help", "help"),
        ("rust —help", "help"),
        ("–help", "help"),
        ("-- rust", "rust"),
        // a dash in the text is the text: only a flag reads it as `--`
        ("u1 Rust — the book", "u1 Rust — the book"),
        ("rust — tokio", "rust — tokio"),
        ("tokio —the runtime", "tokio —the runtime"),
        ("1990 – 2000 census", "1990 – 2000 census"),
        ("help login", "help"),
        // a flag beside two words, or after the verb, is text
        ("git rebase --onto", "git rebase --onto"),
        ("search -tokio rust", "search -tokio rust"),
        // a `-word` after a word is the engine's exclusion, not a flag
        ("rust -tokio", "rust -tokio"),
        // a search engine's query key is a search word as typed; a path beside words must exist, `-` be alone
        ("rust filetype:pdf before:2026-01", "rust filetype:pdf before:2026-01"),
        ("NORTHROP GRUMMAN CORP /DE/ company", "NORTHROP GRUMMAN CORP /DE/ company"),
        ("irs liaison committee - tei", "irs liaison committee - tei"),
        ("./nope.md summary", "./nope.md summary"),
        // a `/` path of 2+ segments or a drive is a path beside words too; `.rs` is no document: a host
        ("acme /etc/hosts", "/etc/hosts acme"),
        ("C:\\x acme", "C:\\x acme"),
        ("C:/x", "C:/x"),
        ("docs.rs tokio", "https://docs.rs tokio"),
        // a home, a variable, a `\` or a bare path of 3+ segments that no host leads is a path beside words too;
        // 2 segments are words unless they exist here, a host-led path is a URL, a name of digits leads nothing
        ("acme .\\secret\\plan", ".\\secret\\plan acme"),
        ("acme ~alice/secret", "~alice/secret acme"),
        ("acme ${HOME}/secret", "${HOME}/secret acme"),
        ("acme internal/hr/salaries", "internal/hr/salaries acme"),
        ("acme Documents/plan.key", "Documents/plan.key acme"),
        ("TCP/IP and/or AC/DC", "TCP/IP and/or AC/DC"),
        ("~5/10 $5/mo plan", "~5/10 $5/mo plan"),
        ("docs.rs/tokio/latest/tokio spawn", "https://docs.rs/tokio/latest/tokio spawn"),
        // a dot-led name is a path wherever it stands; one leading a TLD, a digit or no name is a word
        ("summary .env", ".env summary"),
        (".ssh/config", ".ssh/config"),
        ("learn .NET core .io ... .5mm", "learn .NET core .io ... .5mm"),
    ];

    /// Inputs that ask: input → the start of the `?` line.
    const ASKS: &[(&str, &str)] = &[
        ("u2#3 4", "? 4: link → "),
        ("x foo:1", "? foo: unknown · keys limit match"),
        ("rust countr:lt", "? countr: unknown · keys"),
        ("notes.md hello", "? notes.md: a missing file or a site · type ./notes.md or https://notes.md"),
        // a lone name ending in a file's extension that is no file here asks; beside words it leads a site search
        ("main.rs", "? main.rs: a missing file or a site · type ./main.rs or https://main.rs"),
        ("docs.rs limit:5", "? docs.rs: a missing file or a site"),
        // a directory is no file and no search word
        ("acme phone ./src", "? ./src is a directory · name a file in it"),
        ("u1 js:maybe", "? js:maybe · use js:true"),
        ("u1 match:fuzzy", "? match:fuzzy"),
        ("u1 max_bytes:65m", "? max_bytes:65m · 1k to 64m"),
        ("u1 max_bytes:lots", "? max_bytes:lots"),
        ("ex.com device:tablet", "? device:tablet · use device:mobile|desktop"),
        ("u1 hedge:61", "? hedge:61 · seconds before the second provider, 0 to 60"),
        ("u1 hedge:soon", "? hedge:soon"),
        ("u1 timeout:301", "? timeout:301 · seconds for bd and oxyjs, 1 to 300"),
        ("u1 timeout:99999999", "? timeout:99999999"),
        ("ex.com provider:oxyjs", "? provider:oxyjs · use provider:auto|plain|bd|oxy|decodo"),
        ("rust since:lately", "? since:lately · an age or a date"),
        ("rust since:2026-13", "? since:2026-13"),
        ("rust site:not_a_host", "? site:not_a_host · a host"),
        ("stts", "? stts: a command? "),
        ("acount", "? acount: a command? "),
        ("confg", "? confg: a command? "),
        ("cofnig", "? cofnig: a command? "),
        ("verison", "? verison: a command? "),
        ("versoin", "? versoin: a command? "),
        ("rotues", "? rotues: a command? "),
        ("fetchh", "? fetchh: a command? fats fetch · a web search: fats search fetchh"),
        ("fetchh https://x.com", "? fetchh: a command? fats fetch https://x.com · a web search: fats search fetchh"),
        ("fecth u1", "? fecth: a command? fats fetch u1 "),
        ("acont u1", "? acont: a command? fats account u1 "),
        ("hlep u1", "? hlep: a command? fats help u1 "),
        ("stts json:true", "? stts: a command? fats stats json:true "),
        // a file URL on another machine: fats reads local files only
        ("file://host/x", "? file://host/x: a file URL names a local path"),
        ("l99999999999999999999", "? l99999999999999999999: out of range"),
        ("u1 u99999999999999999999", "? u99999999999999999999: out of range"),
        ("--login", "! unknown flag --login · keys: fats setup · help: fats --help"),
        ("-x", "! unknown flag -x · "),
        ("limit:5 -x", "! unknown flag -x · "),
        ("–x", "! unknown flag –x · "),
        // every free word a flag, wherever it stands: nothing to search for
        ("-x -y", "! unknown flag -x · "),
        ("--login -x", "! unknown flag --login · "),
        ("—", "! unknown flag — · "),
        ("--dry ex.com", "! unknown flag --dry · mod: dry:true · "),
        // a mod typed as a flag after search words is no search word: the search, then the mod on a result
        ("fatsearch forum --cards", "? --cards is a mod, not a search word · search first: fats fatsearch forum · "),
        (
            "rust news --full",
            "? --full is a mod, not a search word · search first: fats rust news · then: fats u1 full",
        ),
        ("logout", "! logout is not a fats command · "),
        ("install json", "! install: see docs/installation.md"),
        ("stts json", "? stts: a command? "),
    ];

    /// The root zone list is whole; `site:` and a `/` name a host by shape alone.
    #[test]
    fn tlds_and_host_shapes() {
        assert_eq!((WORDS.tlds.len(), WORDS.tlds[0], WORDS.tlds.last().copied()), (1437, "aaa", Some("zw")));
        // `site:` and a `/` name a host by shape alone, a TLD or not
        assert_eq!(
            (c("rust site:wiki.corp").get("site"), c("wiki.corp/a b").urls[0].as_str()),
            (Some("wiki.corp"), "https://wiki.corp/a")
        );
        // an engine's query key is never one of our keys or thresholds: it would stop being a search word
        let ours = |o: &&&str| WORDS.keys.contains(o) || crate::settings::LIMITS.contains_key(**o);
        assert_eq!(WORDS.operators.iter().find(ours), None);
    }

    /// A name no resolver can ask for is `no_host` by its shape: a label over 63 chars, an empty label, a hyphen
    /// at a label's end (a 64-char label was looked up and blamed on the network).
    #[test]
    fn real_host_needs_askable_labels() {
        let (l63, l64) = ("a".repeat(63), "a".repeat(64));
        let good = [format!("https://{l63}.com/"), "https://a-b.example.com./x".into(), "http://[::1]/".into()];
        let good = good.into_iter().chain(["https://xn--bcher-kva.de/".into(), "https://93.184.215.14/a".into()]);
        for u in good {
            assert!(real_host(&u), "{u}");
        }
        let bad = [format!("https://{l64}.com/"), format!("https://x.{l64}.com/"), "https://a..com/".into()];
        let bad = bad
            .into_iter()
            .chain(["https://-a.com/", "https://a-.com/", "https://a.-com.com/", "https://.a.com/"].map(String::from));
        for u in bad.chain(["https://dead.invalid/a".into(), "https://a.com../".into()]) {
            assert!(!real_host(&u), "{u}");
        }
    }

    /// A `\` makes a path only as a path's lead beside a page: grep alternation and a stray `\` are filter words
    /// there; on a search a `\` inside a word is a local path's, never sent.
    #[test]
    fn a_backslash_is_a_path_on_a_search() {
        let (path, word) = (c("u1 C:\\Users\\x"), c("u1 a\\b"));
        assert!(path.files == ["C:\\Users\\x"] && path.words.is_empty() && word.files.is_empty());
        assert_eq!((word.words, c("u1 price\\|cost").words), (vec!["a\\b".to_string()], vec!["price\\|cost".into()]));
        for q in ["Documents\\salaries 2026", "search a\\b rust", "2026 Documents\\salaries"] {
            let got = c(q);
            assert!(
                got.files.len() == 1 && got.files[0].contains('\\') && !crate::cli::is_search(&got),
                "{q}: {:?}",
                got.files
            );
        }
    }

    /// The no-key move has one rule (`fix`): a bare `setup` without a provider; with one, an agent runs its setup
    /// and hands the user its steps; a stored row read again keeps its provider and the reader's wording.
    #[test]
    fn a_no_key_arrow_is_one_rule() {
        let (b, at) = (bin(), At::default());
        assert_eq!(bang("u3", "no_key", "bd", &at), format!("u3 ! no_key bd → {b} setup"));
        let bd = format!("u3 ! no_key bd → run {b} setup bd, give the user its 3 lines");
        assert_eq!(bang("u3", "no_key", "bd", &At { arg: "bd", ..at }), bd, "an agent runs the key's setup");
        assert_eq!(
            restamp(&format!("! no_key bd → {b} setup bd · https://x"), "u3", ""),
            bd[3..].to_string() + " · https://x"
        );
    }

    #[test]
    fn repair_table() {
        for (i, want) in REPAIRS {
            let got = c(i);
            assert!(got.ask.is_none(), "{i} → {:?}", got.ask);
            assert_eq!(&got.canon(), want, "input: {i}");
        }
        for (i, _) in REPAIRS {
            let (once, twice) = (c(i).canon(), c(&c(i).canon()).canon());
            assert_eq!((c(&once).words, &twice), (c(i).words, &once), "{i}: the canonical form replays");
        }
        // every verb, a mod before or after it: the canonical form and a mod's arrow read the same command back
        let reading =
            |m: &Cmd| (m.verb.clone(), m.words.clone(), m.reserved.clone(), m.ids.iter().map(Id::page).collect());
        for (v, w) in WORDS.verbs.iter().flat_map(|v| ["login", "u1", "stat"].map(|w| (v, w))) {
            for i in [format!("limit:5 {v} {w}"), format!("{v} {w} limit:5"), format!("-v {v} {w}")] {
                let once = c(&i);
                let want: (_, _, _, Vec<String>) = reading(&once);
                assert!(once.verb.is_some() && once.ask.is_none(), "{i}: the verb is read");
                for line in [once.canon(), once.with_mod("limit", Some("3"))] {
                    assert_eq!(reading(&c(&line)), want, "{i} → {line}: the same reading");
                }
            }
        }
        for (i, want) in ASKS {
            let got = c(i).ask.unwrap_or_default().replace(bin(), "fats");
            assert!(got.starts_with(want), "{i} → {got}");
        }
        // a page's mod after a URL is the mod on that page, never a word or a `?` (`cli::run` reads it)
        for m in ["links", "cards", "all", "full", "media"] {
            let u = c(&format!("example.com/a {m}"));
            assert!(u.urls == ["https://example.com/a"] && u.reserved == [m] && u.words.is_empty() && u.ask.is_none());
        }
        // beside a page a mod typed as a flag is the mod
        assert!(c("u1 --cards").ask.is_none() && c("u1 --cards").reserved == ["cards"]);
        // `json` closing the words beside a page id or file is the output mod; on a search (a host's too) it is a
        // word, `json:true` the mod
        let searches = [
            ("rust news json", "rust news json"),
            ("json parser", "json parser"),
            ("example.com rust json", "rust json"),
        ];
        for (i, json, words) in searches.map(|(i, w)| (i, false, w)) {
            let got = c(i);
            assert_eq!((got.has("json"), got.words.join(" ").as_str()), (json, words), "{i}");
        }
        let mod_on = c("rust news json:true");
        assert!(mod_on.flag("json"));
        assert_eq!(mod_on.words, ["rust", "news"]);
        // a `?`/`!` row is no search: its closing `json` stays the switch
        for asked in ["whoami json", "stts json"] {
            assert!(c(asked).has("json") && !c(asked).words.contains(&"json".to_string()), "{asked}");
        }
        assert!(c("u1 price json").has("json") && c("u1 price json").words == ["price"]);
        // `stats` leading words filters its index, never a search; a closing `json` and `all` stay mods
        let st = c("stats rust http all json");
        assert!(st.has("stats") && st.has("all") && st.has("json") && st.words == ["rust", "http"], "{:?}", st.words);
        assert!(c("rust stats").words == ["rust", "stats"], "a search that names stats stays one");
        // two command words and no page are the command (`guide all` = the card), never a search for one of them;
        // beside a page, an alone-only word is text (`u1 setup` filters)
        for i in ["guide all", "setup full", "account more", "login all", "keys links"] {
            assert!(c(i).words.is_empty() && c(i).ask.is_none(), "{i}: {:?} {:?}", c(i).words, c(i).ask);
        }
        assert_eq!((c("u1 setup").words, c("https://a.b setup").words), (vec!["setup".into()], vec!["setup".into()]));
    }

    /// A failed fetch's cause row stays first under the `=` line; skipped tiers and later warnings follow it.
    #[test]
    fn warnings_follow_the_cause() {
        let mut o = Out::new(vec!["= u1 ! js_shell T".into(), "u1 ! js_shell x → fix".into(), "[u1#1] # T".into()]);
        o.warn(vec!["u1 ! no_key oxy → fats setup".into()]);
        o.warn(vec!["u1 ! ignored device".into()]);
        let want =
            ["= u1 ! js_shell T", "u1 ! js_shell x → fix", "u1 ! no_key oxy → fats setup", "u1 ! ignored device"];
        assert_eq!(o.lines[..4], want);
        assert_eq!(o.rec["warnings"], serde_json::json!(want[2..]));
        // a search's rows have no subject: the cause (the default key) stays first all the same
        let mut o = Out::fail("no_key", "= ! no_key search".into(), "! no_key oxy".into());
        o.warn(vec!["! no_key bd".into()]);
        assert_eq!(o.lines, ["= ! no_key search", "! no_key oxy", "! no_key bd"]);
    }

    /// Edits: a change, an addition, a drop, a swap of neighbours, each one; and the typo targets by position.
    #[test]
    fn near_words() {
        for (a, b, n) in [
            ("stat", "stats", 1),
            ("acount", "account", 1),
            ("confg", "config", 1),
            ("cofnig", "config", 1),
            ("sats", "stats", 1),
            ("fetchh", "fetch", 1),
            ("sta", "stats", 2),
            ("acont", "account", 2),
            ("cinfog", "config", 2),
            ("rust", "status", 5),
            ("ab", "ba x", 3),
        ] {
            assert_eq!((strsim::osa_distance(a, b), strsim::osa_distance(b, a)), (n, n), "{a} {b}");
        }
        assert_eq!((typo("stat", false), typo("ale", false), typo("us", false)), (Some("stats"), None, None));
        assert_eq!((typo("stats", false), typo("yelp", false), typo("fuel", false)), (None, None, None));
        assert_eq!(
            (typo("rotues", false), typo("fetchh", false), typo("acont", false)),
            (Some("routes"), Some("fetch"), None)
        );
        assert_eq!(
            (typo("acont", true), typo("hlep", true), typo("gh", true), typo("map", true)),
            (Some("account"), Some("help"), None, Some("mcp"))
        );
    }

    /// Fuzz-ish: a table of hostile inputs never panics and always yields one reading or one `?`.
    #[test]
    fn hostile_inputs() {
        let junk = [
            "", " ", "#", "u", "u#", "u1#", "u1#a", "u99999999999999999999999", "j.u1", "j1.", "l", "l-1", "::", "a:",
            ":b", "http:", "https://", "--", "-n", "-n x", "n=", "=5", "``", "\"", "u1 #", "u1 # 3", "ü1#2", "u1#2zz",
            "\u{0}", "u1\u{a0}links", "🙂 u1", "j8/u", "file:///etc/passwd", "../", "~/", "u1 l 4 l 5", "all all",
        ];
        for j in junk {
            let cmd = c(j);
            cmd.canon();
            let usage = |q: &String| q.starts_with("? ") || q.starts_with("! unknown flag ");
            assert!(cmd.ask.as_ref().is_none_or(usage), "{j} → {:?}", cmd.ask);
        }
        for a in ['a', 'u', '#', '1', ':', '.', ' ', 'l', 'j', '/'] {
            for b in ['a', 'u', '#', '1', ':', '.', ' ', 'l', 'j', '/'] {
                c(&format!("u1{a}{b}2{a}")).canon();
            }
        }
    }

    /// Every printed part id reads back (z → aa, never a shell symbol).
    #[test]
    fn part_ids_round_trip() {
        assert_eq!(
            [letters(0), letters(1), letters(25), letters(26), letters(27), letters(701), letters(702)],
            ["a", "b", "z", "aa", "ab", "zz", "aaa"]
        );
        for p in 0..20_000 {
            let id = Id::parse(&format!("j3.u2#7{}", letters(p))).unwrap_or_default();
            assert_eq!((id.job, id.u, id.sec, id.part), (Some(3), 2, Some(7), p));
        }
        assert_eq!(Id::parse("u2#7").map(|i| i.part), Some(0));
        assert!(Id::parse(&format!("u2#7{}", "z".repeat(40))).is_none());
    }

    /// Every paid arrow is priced and parses (11 had no price, one offered `provider:oxyjs`): for every code
    /// of the table, every `{arg}` a failed copy gets (`fetch::fix_arg`, the search's longer time, a cap) and every
    /// spent tier set, an arrow whose command can spend parses and ends with `≤$X` = `dry:true`'s `up to` for it
    /// (in 0.1.7 its own worst case was the arrow checked against itself; a URL's arrow runs dry on the URL).
    #[test]
    fn every_paid_arrow_is_priced() {
        let args = ["", "js:true", "provider:bd", "provider:oxy js:true", "wait:30", "timeout:30", "0.0025"];
        let dones: [&[&str]; 4] = [&[], &["plain", "bd"], &["plain", "oxyjs"], &["plain", "oxy", "oxyjs"]];
        // a public IP: `dry:true` judges a host by its name alone (`real_host`), never a lookup; nothing is sent
        let cases = args
            .into_iter()
            .flat_map(|a| dones.map(|d| (a, d)))
            .flat_map(|c| [("https://93.184.215.14/a", c), ("", c)]);
        let (mut paid, b) = (0, bin());
        for (url, (arg, done)) in cases.collect::<Vec<_>>() {
            // `{arg}`: a timeout's mods (`fix_arg`), `budget_hit`'s cap in dollars, another code's its own word
            let fits = |r: &&(&str, &str)| match r.0 {
                "budget_hit" => arg == "0.0025",
                "provider_timeout" => arg != "0.0025",
                _ => arg.is_empty() || !r.1.contains("{arg}"),
            };
            for code in FIX.iter().filter(fits).map(|r| r.0.replace('*', "403")) {
                let f = fix(&code, &At { id: "u3", url, arg, done });
                let cmd = f.split(", ").next().unwrap_or_default().split(" ≤$").next().unwrap_or_default();
                let mods = ["fresh:", "js:", "provider:", "timeout:"];
                if !cmd.starts_with(b) || !cmd.split(' ').any(|w| mods.iter().any(|k| w.starts_with(k))) {
                    continue;
                }
                paid += 1;
                let words: Vec<String> = cmd.split(' ').skip(1).map(Into::into).collect();
                assert_eq!(parse(&words).ask, None, "{code} {arg}: {f}");
                let price = f.split(" ≤$").nth(1).and_then(|p| p.get(..6)).unwrap_or_default();
                let dry = words.iter().map(|w| if w == "u3" && !url.is_empty() { url } else { w.as_str() });
                let dry: Vec<String> = dry.map(Into::into).chain(["dry:true".into()]).collect();
                let c = crate::cache::Cache::at(crate::settings::get().cache.value.clone(), "t".into());
                let rows = crate::cli::plan(&parse(&dry), &c).lines.join("\n");
                let up = rows.split(" up to ").nth(1).and_then(|r| r.split([' ', '\n']).next()).unwrap_or("none");
                if !url.is_empty() {
                    assert_eq!(format!("${price}"), up, "{code} {arg}: {f}\n{rows}");
                }
                assert!(f.starts_with(&format!("{cmd} ≤$")), "{code} {arg}: {f}");
            }
        }
        assert!(paid > 500, "{paid}");
    }

    /// Every keyed paid tier has its turn: with two providers spent, a third key's untried tier is the move
    #[test]
    fn a_third_key_is_the_move() {
        let at = At { id: "u3", done: &["plain", "oxy", "bd", "oxyjs"], ..Default::default() };
        assert_eq!(fix("captcha_wall", &at), format!("{} u3 fresh:true provider:decodo", bin()));
    }

    /// One fix per code; every code a provider or the content check can return has a row (a new code without
    /// one fails here, not in front of an agent); the fix for a timeout is never the same failing retry.
    #[test]
    fn error_table() {
        let keys: Vec<&str> = FIX.iter().map(|r| r.0).collect();
        let mut uniq = keys.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), keys.len(), "a code with two fixes");
        let born = [include_str!("provider.rs"), include_str!("fetch.rs"), include_str!("stats.rs")].concat();
        let mut codes: Vec<&str> = born
            .split('"')
            .skip(1)
            .step_by(2)
            .filter(|w| w.contains('_') && w.bytes().all(|b| b.is_ascii_lowercase() || b == b'_' || b.is_ascii_digit()))
            .collect();
        codes.extend(crate::fetch::CHECK.rules.iter().map(|w| w.code).filter(|c| !c.is_empty()));
        codes.extend(["blocked_403", "http_502", "fetch_failed", "no_file", "not_allowed", "too_big", "offline"]);
        // literals in those files that are HTML or JSON names, not codes
        let names = ["b_algo", "result__a", "cf_chl_opt", "search_query", "status_code", "run_js", "max_results"];
        let names = [&names[..], &["brd_json", "dns_wait", "dns_fast_ms"]].concat();
        for c in codes.iter().filter(|c| !names.contains(c)) {
            let hit = keys.iter().any(|k| k.strip_suffix('*').map_or(k == c, |p| c.starts_with(p)));
            assert!(hit, "code {c} has no fix in FIX");
        }
        let at = At { id: "u3", url: "https://ex.com/a", arg: "0.0025", done: &[] };
        let b = bin();
        assert_eq!(
            fix("provider_timeout", &At { arg: "js:true", ..at }),
            format!("{b} u3 fresh:true js:true ≤$0.0030")
        );
        let at = At { id: "u3", url: "https://ex.com/a", arg: "0.0025", done: &[] };
        assert_eq!(
            fix("budget_hit", &at),
            format!("{b} u3 fresh:true cost:0.0025 ≤$0.0025"),
            "dry routes the cap: oxy no-JS + bd"
        );
        assert_eq!(fix("provider_timeout", &At { arg: "", ..at }), format!("{b} u3 fresh:true ≤$0.0040"), "one space");
        assert_eq!(fix("not_found", &at), format!("check the URL · search for it: {b} <words>"), "no home");
        let search = At { id: "provider:bd rules", url: "", ..at };
        assert_eq!(
            fix("provider_refused", &search),
            format!("{b} rules fresh:true provider:oxy ≤$0.0010 · last try not charged"),
            "one provider:, and bd's refusal is one of `provider::REFUSED`, unbilled"
        );
        let ceil = At { url: "big.html", done: &["max_bytes:64m"], ..at };
        assert_eq!(fix("too_big", &ceil), "nothing left to try · open big.html in a browser", "the cap in force");
        assert_eq!(fix("blocked_451", &at), format!("{b} u3 fresh:true provider:oxy js:true ≤$0.0015"));
        assert_eq!(fix("brightdata_refused", &at), format!("{b} u3 fresh:true ≤$0.0040"), "an old code: the retry");
        // the fence's bypass is the user's own choice: a hint an agent runs never carries it
        assert!(FIX.iter().all(|r| !r.1.contains("FATS_ALLOW_PRIVATE")), "a fix prints the private-address bypass");
        assert_eq!(fix("private_address", &at), format!("private address refused · see {b} config"));
        // a fix never names a tier that already ran (or cannot run: bd on a gov host)
        let done = At { done: &["plain", "oxy", "bd"], ..at };
        assert_eq!(fix("not_pdf", &done), "nothing left to try · open https://ex.com/a in a browser");
        let want = format!("{b} u3 fresh:true provider:oxy js:true ≤$0.0015");
        assert_eq!(fix("captcha_wall", &done), want, "oxyjs untried");
        assert_eq!(fix("provider_refused", &done), want, "a refusal's no-JS fix, oxyjs untried");
        let all = At { done: &["plain", "oxy", "bd", "oxyjs", "decodo", "decodojs"], url: "", ..at };
        assert_eq!(fix("captcha_wall", &all), "nothing left to try");
        // oxy no-JS skipped by the site's wear, the render captcha'd: oxy is the move
        let skipped = At { done: &["plain", "oxyjs", "bd"], ..at };
        assert_eq!(fix("captcha_wall", &skipped), format!("{b} u3 fresh:true provider:oxy ≤$0.0025"));
        let free = format!("{b} u3 fresh:true ≤$0.0040 · last try not charged");
        assert_eq!(fix("provider_error", &At { arg: "", ..all }), free, "a transient retry names no tier");
        let named = At { arg: "provider:oxy js:true", done: &["plain", "oxy", "bd"], ..all };
        assert_eq!(fix("provider_error", &named), format!("{want} · last try not charged"), "the tier left");
    }

    /// `since:` ages count back from today; dates fill the missing month and day.
    #[test]
    fn since_dates() {
        let today = 20_738; // 2026-10-12
        assert_eq!(civil(today), (2026, 10, 12));
        assert_eq!(civil(0), (1970, 1, 1));
        for (v, want) in [
            ("7d", "2026-10-05"),
            ("2w", "2026-09-28"),
            ("1m", "2026-09-12"),
            ("1y", "2025-10-12"),
            ("0d", "2026-10-12"),
            ("2026", "2026-01-01"),
            ("2026-01", "2026-01-01"),
            ("2024-02-29", "2024-02-29"),
        ] {
            assert_eq!(since_date(v, today).as_deref(), Some(want), "{v}");
        }
        for v in ["", "d", "7", "7x", "26-01", "2026-1", "2026-00", "2026-01-32", "2026-01-01-01", "-7d", "é7d"] {
            assert_eq!(since_date(v, today), None, "{v}");
        }
    }

    #[test]
    fn helpers() {
        assert_eq!(short("https://www.example.com/abc", 8), "example…");
        assert_eq!(ksize(999), "999");
        assert_eq!(ksize(1250), "1.2k");
        assert_eq!(cut("ąćę", 2), "ąć");
        let o = Out::new(vec!["= x".into()]);
        assert!(emit(&o, true).contains("\"bytes_out\":3"));
    }

    /// A whole key, any 12-char run of it (an echo that cuts the key), and a short key whole are masked; text
    /// around them, multi-byte text and values too short to be keys are left alone.
    #[test]
    fn redactor_masks_keys_and_their_runs() {
        let key = &["Ab3dEf6hIj9lMn2pQr5t".to_string()];
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(redact(&format!("Invalid api key: {}.", key[0]), key), "Invalid api key: ***.");
        assert_eq!(redact("echo dEf6hIj9lMn2 end", key), "echo *** end", "a 12-char run");
        assert_eq!(redact("dEf6hIj9lMn end", key), "dEf6hIj9lMn end", "11 chars: not a run");
        assert_eq!(redact("ąć Ab3dEf6hIj9lMn2pQr5t ąć", key), "ąć *** ąć");
        assert_eq!(redact("a sunflower b", &s(&["sunflower"])), "a *** b", "8-11 chars: whole value");
        assert_eq!(redact("x y xyz", &s(&["x", "xyz"])), "x y xyz", "too short to be a key");
        assert_eq!(redact("ąćęłńóśźż-ąćę", &s(&["ąćęłńóśźż-ąć"])), "***ę", "runs on char boundaries");
    }
}
