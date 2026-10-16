//! LANG: argv → command (parse + repair) and the ONLY place that writes to stdout.

use chrono::Datelike;
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
    aliases: Vec<(&'static str, &'static str)>,
    /// Words people type that name no command (`logout`, `install`): alone (mods aside) they get one `!` line
    /// pointing to what fats has (`{b}` = the binary), never a paid search.
    elsewhere: Vec<(&'static str, &'static str)>,
    /// A leading verb is dropped (`read u2#3`); it also says the rest are words (`search stat`).
    verbs: Vec<&'static str>,
    /// Known mod keys (aliases resolve to these first), and those that take a boolean.
    keys: Vec<&'static str>,
    bools: Vec<&'static str>,
    /// The commands a lone word may be a typo of. The reading words (`more`, `full`, `json`, `links`, `help`)
    /// are left out: their neighbours are real searches (`mode`, `fuel`, `jason`, `yelp`).
    pub commands: Vec<&'static str>,
    /// More commands a first word led into a URL or id may be a typo of (`fetchh https://x`, `hlep u1`).
    led: Vec<&'static str>,
    /// Extensions that make a token a path (`page.html`, `a.pdf`), whether or not the file exists.
    doc_ext: Vec<&'static str>,
    /// A filter for only these words that finds nothing points to the page's media list (`search::grep`).
    pub media: Vec<&'static str>,
    /// Words a filter never looks for (`search::terms_of`, the title query).
    pub stop: Vec<&'static str>,
}

pub static WORDS: LazyLock<Words> = LazyLock::new(|| crate::settings::table("words"));

pub const CARD: &str = "\
fats <url|file.pdf>     fetch, outline           fats u3  u3#2  u3#2 l4  page  section  link
fats u3#2 <words>       filter inside            fats <words>  web search → ids  site:ex.com  since:7d
mods key:value   limit:20  match:exact|words  country:de  js:true  cost:0.5  wait:30 (s, then pending)  provider:plain|bd|oxy|auto (auto = table order)  device:mobile
more  up  all  full  json     fats = status  fats setup = keys  fats account = balance, usage     j8.u3 = id from another session";

static BIN: OnceLock<String> = OnceLock::new();

/// Hints echo argv[0]'s file name, so they run as printed.
pub fn bin() -> &'static str {
    BIN.get_or_init(|| {
        std::env::args()
            .next()
            .and_then(|a| Path::new(&a).file_name().map(|f| f.to_string_lossy().into_owned()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "fats".into())
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
    let f = t.replace("{b}", bin()).replace("{id}", at.id).replace("{url}", at.url).replace("{arg}", at.arg);
    let js = f.split(' ').any(|w| w == "js:true").then_some("oxyjs");
    // the last `provider:` wins, as in `Cmd::get` (a search's `{id}` is its command, mods included)
    let tier = js.or(f.split(' ').filter_map(|w| w.strip_prefix("provider:")).next_back());
    // a mod already in force (`max_bytes:64m` at the ceiling) is the same failing call too
    let sent = tier.is_some_and(|t| at.done.contains(&t));
    let barred = sent || f.split(' ').any(|w| w.contains(':') && at.done.contains(&w));
    // After a render that ran, a keyed paid tier this run did not call (one the site's wear skipped) is the
    // move, never "nothing left"; after a provider's refusal too (it said nothing of the page: bd
    // refused after oxy → oxyjs), never after a page's no-JS failure (`not_pdf`: a render brings no PDF)
    let left =
        crate::provider::Tier::ALL.into_iter().find(|t| t.spec().price > 0.0 && !at.done.contains(&t.spec().short));
    if let Some(t) = left.filter(|_| sent && (js.is_some() || crate::provider::REFUSED.contains(&code))) {
        return priced(&format!("{} {} fresh:true {}", bin(), at.id, t.ask()), at.url);
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
    let free = quoted != cmd && crate::provider::REFUSED.contains(&code);
    format!("{quoted}{}{rest}", if free { " (last try not charged)" } else { "" })
}

/// THE price glyph. An arrow whose command can spend (`fresh:`, `js:`, `provider:`,
/// `timeout:`) ends with the most it can, `≤$0.0030` = `dry:true`'s `up to` for it (`fetch::worst`).
pub fn priced(cmd: &str, url: &str) -> String {
    let c = crate::fetch::worst(cmd, url);
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
    let arg = f.split(' ').find_map(|w| w.strip_prefix("cost:")).unwrap_or_default();
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

/// A path is a path only when it says so: `-`, `./ ../ / ~/`, a document extension (`page.html`, `a.pdf`), or a
/// dotted name that exists here (`notes.md`). A bare word is text even if a file has that name (`price`,
/// `README`), and with an id in the command only explicit paths count (free text wins).
fn is_file(t: &str, any_id: bool) -> bool {
    if t == "-" || ["./", "../", "/", "~/"].iter().any(|p| t.starts_with(p)) {
        return true;
    }
    let ext = Path::new(t).extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    !any_id && !t.contains("://") && ext.is_some_and(|e| WORDS.doc_ext.contains(&e.as_str()) || Path::new(t).is_file())
}

/// Normalise one `key:value` (aliases, liberal booleans). Err = the `?` line.
fn modifier(k: &str, v: &str) -> Result<(String, String), String> {
    let (k, v) = alias(&k.to_ascii_lowercase(), v)?;
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
        "device" | "ua" => {
            let d = match v.to_ascii_lowercase().as_str() {
                "mobile" | "phone" => "mobile",
                "desktop" | "pc" => "desktop",
                _ => return Err(format!("? device:{v} · use device:mobile|desktop (Oxylabs only)")),
            };
            ("device", d.into())
        }
        "loose" => {
            let n: u32 =
                v.parse().map_err(|_| format!("? loose:{v} · loose takes 0..100 · or match:exact|words|near|topic"))?;
            let m = match n {
                0 => "exact",
                1..=49 => "words",
                50..=79 => "near",
                _ => "topic",
            };
            ("match", m.into())
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
    n.map(|n| n.to_string()).map_err(|q| format!("? {}", q.replace("{k}", k).replace("{v}", &v)))
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
            // the JS tier is `provider:oxy js:true`, not a provider of its own
            let t =
                crate::provider::Tier::parse(&v.to_ascii_lowercase()).filter(|t| *t != crate::provider::Tier::OxyJs);
            let p = t.map(|t| t.spec().short).or((v.eq_ignore_ascii_case("auto")).then_some("auto"));
            return p.map(String::from).ok_or(format!("? provider:{v} · use provider:auto|plain|bd|oxy"));
        }
        "site" => {
            let s = v.to_ascii_lowercase();
            let s = s.strip_prefix("https://").or(s.strip_prefix("http://")).unwrap_or(&s).trim_end_matches('/');
            return is_url(s)
                .then(|| s.to_string())
                .ok_or(format!("? site:{v} · a host: site:example.com · site:docs.rs/serde"));
        }
        _ => (true, ""),
    };
    if ok { Ok(v) } else { Err(format!("? {k}:{v} · {why}")) }
}

/// Token-level repairs. Pure; one canonical reading or a `?` line.
fn repair(argv: &[String]) -> Vec<String> {
    let s = argv.join(" ");
    let s = s.trim().trim_matches('`').trim();
    let s = s.strip_prefix("fats ").unwrap_or(s);
    let mut out: Vec<String> = Vec::new();
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
        if let Some((a, b)) = t.split_once(['/', ':'])
            && a.starts_with('j')
            && num(&a[1..]).is_some()
            && b.starts_with('u')
        {
            t = format!("{a}.{b}");
        }
        if let Some(r) = t.strip_prefix('u')
            && let Some((a, b)) = r.split_once('.')
            && num(a).is_some()
            && num(b).is_some()
        {
            t = format!("u{a}#{b}");
        }
        // --full → full · -v → verbose:true · -n 20 / n=20 → limit:20 · key=value → key:value
        if let Some(r) = t.strip_prefix("--").filter(|r| WORDS.reserved.contains(r)) {
            t = r.to_string();
        } else if t == "-v" {
            t = "verbose:true".into();
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
    out
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
    let mut toks = repair(argv);
    let mut cmd = Cmd { raw: toks.join(" "), ..Cmd::default() };
    // a verb is the first token, mods aside (`limit:5 search login`): the words after it are meant as words
    let lead = toks.iter().position(|t| !matches!(class_of(t, false, false, true), Ok(Class::Mod(..))));
    cmd.verb = lead.filter(|&i| WORDS.verbs.contains(&toks[i].to_lowercase().as_str())).map(|i| toks.remove(i));
    // `--help` or `-h` anywhere (an en or em dash for `--`: smart punctuation), or `help` first, is the card: never
    // a paid search for the other words
    let first_help = cmd.verb.is_none() && lead.is_some_and(|i| toks[i].eq_ignore_ascii_case("help"));
    if first_help || toks.iter().any(|t| ["--help", "-h", "—help", "–help"].contains(&t.as_str())) {
        toks = vec!["guide".into()];
    }
    let has_id = toks.iter().any(|t| Id::parse(t).is_some());
    let (mut cls, mut any_id) = (Vec::new(), false);
    for t in &toks {
        match class_of(t, has_id, any_id, cmd.verb.is_some()) {
            Ok(c) => {
                any_id |= matches!(c, Class::Id(_));
                cls.push((t.clone(), c));
            }
            Err(q) => {
                cmd.ask.get_or_insert(q);
            }
        }
    }
    // free words that are all flags (`--login`, `-x`, `limit:5 -x`; an en or em dash for `--`, `–x`) are an option
    // fats lacks, never a search; any other free word makes every word text, wherever the dash is (doctrine §1:
    // `git rebase --onto`, `rust -tokio` excludes, `-5 degrees`), as does a verb, an id or a file: beside a page
    // the words are a free filter (`u1 -n`); beside a URL they would be a paid `site:` search, so refused. The text
    // keeps a dash as typed.
    let search = cmd.verb.is_none() && !cls.iter().any(|p| matches!(p.1, Class::Id(_) | Class::File(_)));
    let words: Vec<&String> = cls.iter().filter(|p| search && matches!(p.1, Class::Word)).map(|p| &p.0).collect();
    if let Some(t) = words.first().filter(|_| words.iter().all(|t| t.len() > 1 && t.starts_with(['-', '–', '—']))) {
        let name = t.trim_start_matches(['-', '–', '—']);
        let fix =
            if WORDS.bools.contains(&name) { format!("mod: {name}:true") } else { format!("keys: {} setup", bin()) };
        cmd.ask.get_or_insert(format!("! unknown flag {t} · {fix} · help: {} --help", bin()));
    }
    // mods and the output mod aside (one `alone` for both): a lone word, or a first word led into a URL or id,
    // may be a command's typo or a word that names no command
    let kinds: Vec<&Class> =
        cls.iter().map(|p| &p.1).filter(|c| !matches!(c, Class::Mod(..) | Class::Res("json"))).collect();
    let led = matches!(kinds[..], [Class::Word, Class::Url(_) | Class::Id(_), ..]);
    let (lone, none) = (matches!(kinds[..], [Class::Word]), kinds.is_empty());
    sort_into(&mut cmd, kinds.len() == 1, cls, any_id);
    cmd.idle = none && cmd.get("site").is_none() || cmd.ask.is_some();
    if cmd.verb.is_none() && (led || lone) {
        ask_typo(&mut cmd, led);
    }
    cmd
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
    if low.starts_with("file:") {
        // a local file (`file:///a/b.pdf` → `/a/b.pdf`, percent-decoded), read like a path (and fenced the same
        // way under `fats mcp`), never a search
        let path = url::Url::parse(t).ok().filter(|u| u.scheme() == "file").and_then(|u| u.to_file_path().ok());
        let path = path.map(|p| Class::File(p.to_string_lossy().into_owned()));
        return path.ok_or(format!("? {t}: a file URL names a local path: {} /path/to/file", bin()));
    }
    if let Some((k, v)) = t.split_once(':')
        && !k.is_empty()
        && k.bytes().all(|b| b.is_ascii_alphabetic() || b == b'_')
        && !v.starts_with('/')
        && !v.is_empty()
        && !["http", "https"].contains(&low.split(':').next().unwrap_or(""))
    {
        return modifier(k, v).map(|(k, v)| Class::Mod(k, v));
    }
    if is_file(t, has_id) {
        return Ok(Class::File(t.to_string()));
    }
    if !any_id && is_url(t) {
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
    // `json` closing the words is the output mod, never a word (`rust news json`)
    let last = cls.iter().rposition(|(_, c)| !matches!(c, Class::Mod(..)));
    let lead = cls.iter().position(|(_, c)| matches!(c, Class::Res(_) | Class::Word | Class::Num(_)));
    let list = |i, r: &str| any_id && Some(i) == lead && WORDS.lists.contains(&r);
    for (i, (t, c)) in cls.into_iter().enumerate() {
        match c {
            Class::Res(r) if r == "json" && Some(i) == last => cmd.reserved.push(r),
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
            let full = cmd.canon();
            format!(
                "? {n}: link → {b} {full} l{n} · section → {b} {}#{n} · text → {b} {full} {n} match:words",
                i.page()
            )
        }
        None => format!("? {n}: link → {b} l{n} · text → {b} {n} match:words"),
    }
}

/// A command's typo asks (it is a paid search otherwise; a verb before it says "words"): `led` = a first word
/// led into a URL or id, else a lone word (mods aside: `yelp limit:5` searches).
fn ask_typo(cmd: &mut Cmd, led: bool) {
    let (Some(w), b) = (cmd.words.first().cloned(), bin()) else { return };
    if let Some((_, why)) = WORDS.elsewhere.iter().find(|e| !led && e.0.split(' ').any(|o| w.eq_ignore_ascii_case(o))) {
        cmd.ask.get_or_insert(format!("! {w}{}", why.replace("{b}", b)));
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

    /// Usage problem (`?` line, exit 2).
    pub fn ask(line: String) -> Out {
        Out { rc: 2, ..Out::new(vec![line]).with("error", "usage") }
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
        let bang = |l: &String| l.split(' ').nth(1) == Some("!");
        let at = self.lines.len().min(1) + self.lines.iter().skip(1).take_while(|l| bang(l)).count();
        self.lines.splice(at..at, rows.iter().cloned());
        let mut all = match self.rec.remove("warnings") {
            Some(serde_json::Value::Array(old)) => old,
            _ => Vec::new(),
        };
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

/// `text` with every `SECRET_RUN`-char run of a secret (a whole secret of `SECRET_MIN`.. chars) as `MASK`.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut hide = vec![false; text.len()];
    for s in secrets {
        let at: Vec<usize> = s.char_indices().map(|c| c.0).chain([s.len()]).collect();
        let n = at.len() - 1;
        if n < SECRET_MIN {
            continue;
        }
        let w = n.min(SECRET_RUN);
        for i in 0..=n - w {
            let run = &s[at[i]..at[i + w]];
            for (j, _) in text.match_indices(run) {
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
mod tests {
    use super::*;

    fn c(s: &str) -> Cmd {
        parse(&s.split(' ').map(String::from).collect::<Vec<_>>())
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
        ("https://ex.com/a?b=1", "https://ex.com/a?b=1"),
        ("u1 v2.0", "u1 v2.0"),
        ("dry:yes u1#2", "u1#2 dry:true"),
        ("l4", "l4"),
        ("help", "guide"),
        ("u1 max_bytes:32MiB", "u1 max_bytes:33554432"),
        ("-h", "guide"),
        ("--help", "guide"),
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
        ("file:///tmp/a%20b.html", "/tmp/a b.html"),
        // a word people type for a command is that command, mods and `json` aside; with another word it is text
        ("login", "setup"),
        ("Sign-In", "setup"),
        ("usage", "account"),
        ("-v", "verbose:true"),
        ("stat json", "stats json"),
        ("login limit:5", "setup limit:5"),
        ("keys json", "setup json"),
        ("login page design", "login page design"),
        ("u1 key", "u1 key"),
        ("u1 install", "u1 install"),
        // `--help` / `-h` anywhere, `help` first: the card
        ("rust --help", "guide"),
        ("rust —help", "guide"),
        ("–help", "guide"),
        ("-- rust", "rust"),
        // a dash in the text is the text: only a flag reads it as `--`
        ("u1 Rust — the book", "u1 Rust — the book"),
        ("rust — tokio", "rust — tokio"),
        ("tokio —the runtime", "tokio —the runtime"),
        ("1990 – 2000 census", "1990 – 2000 census"),
        ("help login", "guide"),
        // a flag beside two words, or after the verb, is text
        ("git rebase --onto", "git rebase --onto"),
        ("search -tokio rust", "search -tokio rust"),
        // a `-word` after a word is the engine's exclusion, not a flag
        ("rust -tokio", "rust -tokio"),
    ];

    /// Inputs that ask: input → the start of the `?` line.
    const ASKS: &[(&str, &str)] = &[
        ("u2#3 4", "? 4: link → "),
        ("x foo:1", "? foo: unknown · keys limit match"),
        ("u1 js:maybe", "? js:maybe · use js:true"),
        ("u1 match:fuzzy", "? match:fuzzy"),
        ("u1 max_bytes:65m", "? max_bytes:65m · 1k to 64m"),
        ("u1 max_bytes:lots", "? max_bytes:lots"),
        ("ex.com device:tablet", "? device:tablet · use device:mobile|desktop"),
        ("u1 hedge:61", "? hedge:61 · seconds before the second provider, 0 to 60"),
        ("u1 hedge:soon", "? hedge:soon"),
        ("u1 timeout:301", "? timeout:301 · seconds for bd and oxyjs, 1 to 300"),
        ("u1 timeout:99999999", "? timeout:99999999"),
        ("ex.com provider:oxyjs", "? provider:oxyjs · use provider:auto|plain|bd|oxy"),
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
        ("logout", "! logout is not a fats command · "),
        ("install json", "! install: see docs/installation.md"),
        ("stts json", "? stts: a command? "),
    ];

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
        // `json` closing the words is the output mod; anywhere else it is a word
        for (i, json, words) in [("rust news json", true, "rust news"), ("json parser", false, "json parser")] {
            let got = c(i);
            assert_eq!((got.has("json"), got.words.join(" ").as_str()), (json, words), "{i}");
        }
        assert!(c("u1 price json").has("json") && c("u1 price json").words == ["price"]);
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
        let args = ["", "js:true", "timeout:150", "provider:bd timeout:150", "provider:oxy js:true timeout:150"];
        let args = args.into_iter().chain(["wait:30", "timeout:30", "0.0025"]);
        let dones: [&[&str]; 4] = [&[], &["plain", "bd"], &["plain", "oxyjs"], &["plain", "oxy", "oxyjs"]];
        // a public IP: `dry:true` resolves a host name (`host_hop`), an IP never; nothing is sent
        let cases = args.flat_map(|a| dones.map(|d| (a, d))).flat_map(|c| [("https://93.184.215.14/a", c), ("", c)]);
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
        let names =
            ["b_algo", "result__a", "cf_chl_opt", "search_query", "status_code", "run_js", "max_results", "brd_json"];
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
            format!("{b} rules fresh:true provider:oxy ≤$0.0010 (last try not charged)"),
            "one provider:, and bd's refusal is one of `provider::REFUSED`, unbilled"
        );
        let ceil = At { url: "big.html", done: &["max_bytes:64m"], ..at };
        assert_eq!(fix("too_big", &ceil), "nothing left to try · open big.html in a browser", "the cap in force");
        assert_eq!(fix("blocked_451", &at), format!("{b} u3 fresh:true provider:oxy js:true ≤$0.0015"));
        assert_eq!(fix("brightdata_refused", &at), format!("{b} u3 fresh:true ≤$0.0040"), "an old code: the retry");
        assert_eq!(bang("u3", "no_key", "bd", &at), format!("u3 ! no_key bd → {b} setup"));
        // the fence's bypass is the operator's call: a hint an agent runs never carries it
        assert!(FIX.iter().all(|r| !r.1.contains("FATS_ALLOW_PRIVATE")), "a fix prints the private-address bypass");
        assert_eq!(fix("private_address", &at), format!("private address refused · see {b} config"));
        // a fix never names a tier that already ran (or cannot run: bd on a gov host)
        let done = At { done: &["plain", "oxy", "bd"], ..at };
        assert_eq!(fix("not_pdf", &done), "nothing left to try · open https://ex.com/a in a browser");
        let want = format!("{b} u3 fresh:true provider:oxy js:true ≤$0.0015");
        assert_eq!(fix("captcha_wall", &done), want, "oxyjs untried");
        assert_eq!(fix("provider_refused", &done), want, "a refusal's no-JS fix, oxyjs untried");
        let all = At { done: &["plain", "oxy", "bd", "oxyjs"], url: "", ..at };
        assert_eq!(fix("captcha_wall", &all), "nothing left to try");
        // oxy no-JS skipped by the site's wear, the render captcha'd: oxy is the move
        let skipped = At { done: &["plain", "oxyjs", "bd"], ..at };
        assert_eq!(fix("captcha_wall", &skipped), format!("{b} u3 fresh:true provider:oxy ≤$0.0025"));
        let free = format!("{b} u3 fresh:true ≤$0.0040 (last try not charged)");
        assert_eq!(fix("provider_error", &At { arg: "", ..all }), free, "a transient retry names no tier");
        let named = At { arg: "provider:oxy js:true", done: &["plain", "oxy", "bd"], ..all };
        assert_eq!(fix("provider_error", &named), format!("{want} (last try not charged)"), "the tier left");
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
