//! `fats setup` past the piped check rows (`stats::setup`): way 1, a person at a terminal, walks the provider list,
//! a key page per provider and the agent connection (a menu that exits); way 2, an agent, starts a one-shot local
//! key form for its user (`fats setup oxy`); way 3, a pipe (`printf %s "$KEY" | fats setup oxy`). Also every rule
//! about a key on its way in or out: its shape on a command line (refused there, masked in `?` rows), the status
//! line's tails and the environment's rows, the Bright Data zone pick. Screen words are data (`src/data/screens/`,
//! `lang::screen`), the form is `src/data/setup-form.html`.

use crate::cache::{Cache, jload, jsave, now};
use crate::clean::esc;
use crate::lang::{Cmd, NO_HOLES, Out, bin, draw, paint};
use crate::provider::{KEYS, Links, PROVIDERS, links};
use crate::settings::{self, SWITCHES};
use crate::telemetry::Level;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const LIFE: f64 = 600.0; // a key form's life, s
const FORM: &str = include_str!("data/setup-form.html");

/// The telemetry levels as the screens name them: (the stored level, its option, its word on a row). The stored
/// word (`full`) stays what `config`, the piped rows and `fats setup telemetry <level>` print.
const LEVELS: [(Level, &str, &str); 3] = [
    (Level::Full, "On (anonymous usage stats)", "On (anonymous)"),
    (Level::Minimal, "Minimal (update check only)", "Minimal"),
    (Level::Off, "Off", "Off"),
];

/// The Settings list (`fats setup` → Settings): rows that open a screen, the read-only ones, then Back.
const SETTINGS: [&str; 5] = ["mcp", "cache", "stats", "telemetry", "back"];

/// THE menu rule, on one read of terminal bytes (one key: ESC `[A` or `OA` is ↑): ↑↓ (k j Tab) move round, a
/// number jumps and picks, Enter or → picks, Esc ← ⌫ = back, q (or the terminal gone, no bytes) quits. (row marked,
/// how the menu ends: `\r` pick, ESC back, `q`; None = it goes on).
pub fn step(at: usize, n: usize, k: &[u8]) -> (usize, Option<char>) {
    let digit = std::str::from_utf8(k).ok().and_then(|d| d.parse::<usize>().ok()?.checked_sub(1)).filter(|d| *d < n);
    match k {
        b"\x1b[A" | b"\x1bOA" | b"k" => ((at + n - 1) % n, None),
        b"\x1b[B" | b"\x1bOB" | b"j" | b"\t" => ((at + 1) % n, None),
        b"\x1b" | b"\x1b[D" | b"\x1bOD" | b"\x7f" | b"\x08" => (at, Some('\x1b')),
        b"q" | b"\x03" | b"\x04" | b"" => (at, Some('q')),
        b"\r" | b"\n" | b"\x1b[C" | b"\x1bOC" => (at, Some('\r')),
        _ => (digit.unwrap_or(at), digit.map(|_| '\r')),
    }
}

/// The terminal in raw mode by `stty` (std only: termios through `libc` needs `unsafe`, which this crate forbids),
/// restored on drop; no `stty` (Windows) = no menu, the key form instead (way 2). `.1` = scripted keys (tests).
struct Term(Option<String>, std::collections::VecDeque<&'static [u8]>);

/// A menu's screen lines for its marked row.
type Text<'a> = &'a dyn Fn(usize) -> Vec<String>;

fn stty(args: &[&str]) -> Option<String> {
    let o = Command::new("stty").args(args).stdin(Stdio::inherit()).stderr(Stdio::null()).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

impl Term {
    fn key(&mut self) -> Vec<u8> {
        self.1.pop_front().map(<[u8]>::to_vec).unwrap_or_else(|| {
            let mut buf = [0u8; 16];
            let n = std::io::stdin().read(&mut buf).unwrap_or(0); // raw `min 1`: 0 = the terminal is gone
            buf.get(..n).unwrap_or_default().to_vec()
        })
    }

    /// Every screen whole, same frame: head, rule, the 4-row card, one message line, the screen's rows, keys; one
    /// empty line between blocks, so an empty message or block leaves no gap of its own. In colour (`hue`, `paint`).
    fn draw(&self, text: &[String], msg: &str, rows: &[String], keys: &str) {
        let (rule, head) = (format!(" {}", "─".repeat(76)), page("done").lines().nth(7).unwrap_or_default());
        let (card, own) = text.split_at(7.min(text.len()));
        let top = format!("\n {{bold}}fats setup{{/}}    {}\n{rule}\n{head}\n", marks());
        let end = [&[String::new()][..], rows, &[format!("\n{rule}\n {{dim}}{keys}{{/}}")]].concat();
        let l = [&[top][..], card.get(1..).unwrap_or_default(), &[format!("\n   {msg}\n")], own, &end].concat();
        let mut lines: Vec<&str> = l.iter().flat_map(|s| s.split('\n')).collect();
        lines.dedup_by(|a, b| a.trim().is_empty() && b.trim().is_empty());
        let frame = paint(&lines.iter().map(|l| hue(l)).collect::<Vec<_>>().join("\r\n"), &NO_HOLES);
        #[cfg(test)]
        tests::FRAMES.with_borrow_mut(|f| f.push(frame.clone()));
        print!("\x1b[H\x1b[2J{frame}");
        std::io::stdout().flush().ok();
    }

    /// A numbered menu of `items` under the screen's own lines; `text(i)` follows the marked row; an item led by
    /// `\n` has an empty line above it (a spacer the cursor never stops on); `out` names what ← and q do. (row, end
    /// key).
    fn menu<S: AsRef<str>>(&mut self, text: Text, msg: &str, items: &[S], out: &str) -> (usize, char) {
        let mut at = 0;
        loop {
            let row = |(j, l): (usize, &S)| {
                let (gap, l) = l.as_ref().strip_prefix('\n').map_or(("", l.as_ref()), |l| ("\n", l));
                format!("{gap}   {} {}. {l}", [" ", "▸"][usize::from(j == at)], j + 1)
            };
            let rows: Vec<String> = items.iter().enumerate().map(row).collect();
            self.draw(&text(at), msg, &rows, &format!("↑↓ move · Enter choose (or its number) · {out}"));
            match step(at, items.len(), &self.key()) {
                (i, Some(k)) => return (i, k),
                (i, None) => at = i,
            }
        }
    }

    /// The key, pasted or typed in raw mode, a byte a step: each redraws a mask and the count (`••••  36 chars`, at
    /// most 60 glyphs, never a key byte), ⌫ takes one back, Enter ends it; the rest of that read and of the terminal's
    /// input is then dropped (reads that wait 0.1 s, until one is empty), so a pasted `\r\n` never presses the next
    /// screen's Enter. None = back: Esc, ^C, the terminal gone, Enter alone.
    fn secret(&mut self, text: &[String], msg: &str) -> Option<String> {
        let (mut key, mut read) = (Vec::new(), std::collections::VecDeque::new());
        let foot = "paste the key (it stays hidden) · Enter check it · Enter alone or Esc = back";
        let key = loop {
            let mask = format!("   {}  {} chars", "•".repeat(key.len().min(60)), key.len());
            self.draw(text, msg, &[mask], foot);
            if read.is_empty() {
                read = self.key().into();
            }
            match read.pop_front() {
                Some(b'\r' | b'\n') => break key,
                Some(0x7f | 0x08) => _ = key.pop(),
                Some(b) if b.is_ascii_graphic() => key.push(b),
                Some(0x1b | 0x03 | 0x04) | None => break Vec::new(),
                Some(_) => {}
            }
        };
        if self.0.is_some() && stty(&["min", "0", "time", "1"]).is_some() {
            while !self.key().is_empty() {}
            stty(&["min", "1", "time", "0"]);
        }
        Some(String::from_utf8_lossy(&key).into_owned()).filter(|k| !k.is_empty())
    }
}

/// A frame line's colours (`lang::paint` styles, none under `NO_COLOR`): the marks ✓ green, ○ and – dim, ✗ red (a
/// mark = the glyph and a space), the marked row (▸) bold.
fn hue(line: &str) -> String {
    let marks = [("✓ ", "{green}✓{/} "), ("○ ", "{dim}○{/} "), ("– ", "{dim}–{/} "), ("✗ ", "{red}✗{/} ")];
    let l = marks.iter().fold(line.to_string(), |l, (m, c)| l.replace(m, c));
    if line.contains('▸') { format!("{{bold}}{}{{/}}", l.replace("{/}", "{/}{bold}")) } else { l }
}

impl Drop for Term {
    fn drop(&mut self) {
        self.0.as_ref().map(|s| stty(&[s]));
        print!("\x1b[?25h\x1b[?1049l");
        std::io::stdout().flush().ok();
    }
}

/// Part `name` of the setup screens (`src/data/screens/setup.txt`, each after its `§ name` line).
fn page(name: &str) -> &'static str {
    crate::lang::screen("setup").split("§ ").find_map(|s| s.strip_prefix(name)?.strip_prefix('\n')).unwrap_or_default()
}

/// A row's screen text, filled: title, the card (its top, 4 rows, its bottom), then the screen's own lines.
fn text(p: &str) -> Vec<String> {
    let file = if page(p).is_empty() { "provider" } else { p }; // a provider has no screen of its own
    let status = if p == "cache" { cache_words(&settings::get().cache.value).1 } else { state(p).1 };
    let status = if links(p).is_some() && settings::key(p).is_none() { "no key".into() } else { status }; // list: rank
    fill(page(file), links(p), &[("status", &status)]).lines().map(String::from).collect()
}

/// The holes of setup's screens, the agent's block and the form page: `vars` first, then the provider's `{name}`
/// `{mint}` … and `{b}` (`lang::draw`).
fn fill(t: &str, l: Option<&Links>, vars: &[(&str, &str)]) -> String {
    let own = l.map(|l| [("name", l.name), ("short", l.short), ("mint", l.mint), ("signup", l.signup)]);
    let more = l.map(|l| [("step", l.step), ("rank", l.rank), ("free", l.free), ("cost", l.cost)]);
    let all: Vec<_> = vars.iter().copied().chain(own.into_iter().chain(more).flatten()).chain([("b", bin())]).collect();
    draw(t, &all)
}

/// Every switch with its mark: the head of every screen and the done line.
fn marks() -> String {
    SWITCHES.iter().map(|p| format!("{} {p}", state(p).0)).collect::<Vec<_>>().join("  ")
}

/// A row's mark (✓ used, – off, ○ not set) and its status words.
fn state(p: &str) -> (&'static str, String) {
    let zone = settings::zone().filter(|_| p == "bd").map(|z| format!(" · zone {z}")).unwrap_or_default();
    match (p, settings::on(p), settings::key(p)) {
        ("mcp", ..) => ("✓", "ready · built in · nothing to install".into()),
        ("done", ..) => (" ", "close setup".into()),
        ("back", ..) => (" ", String::new()),
        ("settings", ..) => (" ", "agent connection · cache · stats · telemetry".into()),
        ("cache", ..) => (" ", cache_words(&settings::get().cache.value).0),
        ("stats", ..) => {
            let (pages, spent) = crate::stats::session(&Cache::open());
            (" ", format!("this session: {pages} pages · {spent} → {} stats", bin()))
        }
        ("telemetry", ..) => {
            let (t, what) = (crate::telemetry::level(), "anonymous usage stats, to fix and improve fats");
            let word = LEVELS.iter().find(|l| l.0 == t).map_or("", |l| l.2);
            (["–", "✓"][usize::from(t != Level::Off)], format!("{word} · {what}"))
        }
        ("free", true, _) => ("✓", "ready · no account · no key · $0 · uses your home IP".into()),
        ("free", ..) => ("–", "off · every page goes through a paid provider".into()),
        (_, false, Some(_)) => ("–", "off · not used · key kept".into()),
        (_, _, Some(k)) => ("✓", format!("ready · key {} saved{zone}", crate::stats::tail(&k))),
        _ => ("○", links(p).map_or("", |l| l.rank).into()),
    }
}

/// The Cache words: what is stored now and a page's life (`cache::expired`'s limits). (the menu row, under the
/// dir shortened by `row_path`; the card's Status: the whole dir and the unread life too).
fn cache_words(root: &Path) -> (String, String) {
    let span = |s: u64| if s.is_multiple_of(3600) { format!("{} h", s / 3600) } else { format!("{} min", s / 60) };
    let (ttl, idle) = (span(settings::limit("cache_ttl_s")), span(settings::limit("cache_idle_s")));
    let (bytes, _, metas) = crate::cache::usage(root);
    let pages = metas.iter().filter(|m| !crate::stats::is_serp(m)).count();
    let size = crate::lang::ksize(usize::try_from(bytes).unwrap_or(usize::MAX));
    let row = format!("{} · {pages} pages, {size}B · live ≤ {ttl}", row_path(root, dirs::home_dir().as_deref()));
    (row, format!("{} · pages live ≤ {ttl} ({idle} unread) · {pages} pages, {size}B", root.display()))
}

/// A dir as a menu row shows it: `~` for the home dir; past 32 chars its first two and last two parts around `…`
/// (`/var/folders/…/Caches/fatsearch`).
fn row_path(dir: &Path, home: Option<&Path>) -> String {
    let path = home.and_then(|h| dir.strip_prefix(h).ok()).map_or_else(|| dir.into(), |r| Path::new("~").join(r));
    let parts: Vec<&Path> = path.iter().map(Path::new).collect();
    let short: PathBuf =
        parts.iter().take(3).chain([&Path::new("…")]).chain(parts.iter().rev().take(2).rev()).collect();
    if path.as_os_str().len() > 32 && parts.len() > 5 { short } else { path }.display().to_string()
}

/// THE key check: the free read `fats setup` makes for the provider (`provider::check_key`, $0; gh: none, saved
/// unchecked), for the menu, the form and the pipe (`cli::piped`). Ok = what it shows and Bright Data's zones;
/// Err = (code, why not).
pub fn check(l: &Links, key: &str) -> Result<(String, Vec<String>), (&'static str, String)> {
    if key.len() < 8 {
        return Err(("bad_key", "too short for a key: copy the whole key".into()));
    }
    let Some(t) = KEYS.into_iter().find(|t| t.spec().short == l.short) else { return Ok(("unchecked".into(), vec![])) };
    let read = crate::provider::check_key(t, key.to_string());
    let got = crate::provider::all_within(vec![read], Duration::from_secs(20)).pop().flatten();
    let got = got.ok_or(("provider_timeout", "no answer in 20 s".to_string()))?;
    let said =
        |f: crate::provider::Fail| (f.code, format!("{} said no: {} {}", l.name, f.code, f.why).trim_end().into());
    let (what, zones) = got.map_err(said)?;
    if l.short == "bd" && zones.is_empty() {
        return Err(("no_zone", format!("no_zone · no Web Unlocker zone yet: make one at {}", l.mint)));
    }
    Ok((if zones.is_empty() { what } else { format!("zones {}", zones.join(", ")) }, zones))
}

/// The gh CLI's token (`gh auth token`) for `fats setup gh` while no gh key is set: copied into `auth.json` at
/// setup, never read at fetch time; never in a unit test (no developer credential, as `settings::env`).
fn gh(l: &Links) -> Option<String> {
    (l.short == "gh" && settings::key("gh").is_none() && !cfg!(test)).then_some(())?;
    let o = Command::new("gh").args(["auth", "token"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    Some(String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|t| o.status.success() && !t.is_empty())
}

/// The main list: the free fetch, the providers in rank order, Settings (`SETTINGS`), Done.
fn rows() -> Vec<&'static str> {
    ["free"].into_iter().chain(PROVIDERS.links.iter().map(|l| l.short)).chain(["settings", "done"]).collect()
}

/// The main list until q or Done: the done line.
fn walk(t: &mut Term, only: Option<&'static str>) -> String {
    // `only`'s screen first: q there (None) quits; the marks are read after the walk (what it changed)
    let quit = only.is_some_and(|p| open(t, p).is_none());
    if !quit {
        list_loop(t, &rows(), page("intro").trim_end());
    }
    format!("= setup done · {} · try: {} example.com", marks(), bin())
}

/// A list's loop, the marked row's card above it, `intro` its message: Enter opens a row (`open`) and comes back;
/// the last row (Done, ← Back), Esc or ← leave the list (Some: back to the list above), q quits (None).
fn list_loop(t: &mut Term, list: &[&'static str], intro: &str) -> Option<()> {
    let gap = |p: &str| if matches!(p, "settings" | "done" | "back") { "\n" } else { "" };
    loop {
        // one `state` a row, the title from its page or provider (`text` works the card's Status out again)
        let row = |p: &&str| {
            let ((mark, words), title) =
                (state(p), links(p).map_or(page(p).lines().next().unwrap_or_default(), |l| l.name));
            format!("{}{mark} {title:18} {words}", gap(p)).trim_end().into()
        };
        let items: Vec<String> = list.iter().map(row).collect();
        let card = |i: usize| text(list.get(i).copied().unwrap_or("done")).into_iter().take(7).collect();
        // the main list has no list above it: ← quits, as q
        let out = if list.last() == Some(&"done") { "← or q quit" } else { "← back · q quit" };
        match t.menu(&card, intro, &items, out) {
            (i, '\r') if i + 1 < list.len() => open(t, list.get(i).copied().unwrap_or_default())?,
            (_, 'q') => return None,
            _ => return Some(()),
        }
    }
}

/// A row picked: Settings its list, Cache and Stats nothing (they only show), the others their screen.
fn open(t: &mut Term, p: &'static str) -> Option<()> {
    match p {
        "settings" => list_loop(t, &SETTINGS, ""),
        "cache" | "stats" => Some(()),
        _ => screen(t, p),
    }
}

/// A row's screen buttons that work now: a switch (Turn on/off, Remove this key = off) that THE switch rule
/// (`settings::switched`) refuses is not shown, so no button answers "can't".
fn buttons(p: &str) -> Vec<&'static str> {
    let turn = ["Turn on", "Turn off"][usize::from(settings::on(p))];
    let all: Vec<&str> = match (p, settings::key(p).is_some()) {
        ("free", _) => vec![turn, "← Back"],
        ("mcp", _) => vec!["← Back"],
        ("telemetry", _) => LEVELS.iter().map(|l| l.1).chain(["← Back"]).collect(),
        (_, true) => vec!["Paste a new key", turn, "Remove this key", "← Back"],
        _ => vec!["Add a key", "← Back"],
    };
    let switch = |b: &&str| matches!(*b, "Turn on" | "Turn off" | "Remove this key");
    all.into_iter().filter(|b| !switch(b) || settings::switched(p, *b == "Turn on").is_ok()).collect()
}

/// One row's screen until Back (Some) or q (None); every button's answer is its message.
fn screen(t: &mut Term, p: &'static str) -> Option<()> {
    let mut msg = String::new();
    loop {
        let (text, on, items) = (text(p), settings::on(p), buttons(p));
        let (at, end) = t.menu(&|_| text.clone(), &msg, &items, "← back · q quit");
        let done = match (end, items.get(at).copied().unwrap_or_default(), links(p)) {
            ('q', ..) => return None,
            // saved or not, the screen stays: its rows and Status show the key at once
            ('\r', "Add a key" | "Paste a new key", Some(l)) => Ok(paste(t, l).unwrap_or_else(|m| m)),
            // removed = off, so THE switch rule refuses removing the last key that fetches
            ('\r', "Remove this key", _) => (settings::set_on(p, false).and_then(|()| settings::remove_key(p)))
                .and_then(|()| settings::set_on(p, true))
                .map(|()| bak_row().map_or("○ key removed".into(), |b| format!("○ key removed · {b}"))),
            ('\r', l, _) if let Some((level, _, word)) = LEVELS.iter().find(|o| o.1 == l) => {
                settings::set("telemetry", level.name()).map(|()| format!("✓ telemetry {word} · from the next command"))
            }
            ('\r', "Turn on" | "Turn off", _) => settings::set_on(p, !on).map(|()| format!("✓ {}", state(p).1)),
            _ => return Some(()),
        };
        msg = done.unwrap_or_else(|e| format!("✗ Can't: {e}"));
    }
}

/// Paste the key (gh: the gh CLI's first, `gh`), check it ($0), pick Bright Data's zone when it has more than one
/// (saved: fats never switches it), save. Ok = the screen's message for the saved key; Err = why none was saved.
fn paste(t: &mut Term, l: &'static Links) -> Result<String, String> {
    let open = format!("Open {} (no account yet: {})", l.mint, l.signup);
    let key = gh(l).map_or_else(|| t.secret(&text(l.short), &open).ok_or_else(String::new), Ok)?;
    t.draw(&text(l.short), &format!("… checking the key with {} ($0)", l.name), &[], "checking…");
    let (what, zones) = check(l, &key).map_err(|(_, why)| format!("✗ {why} · not saved"))?;
    let pick = "✓ The key works. Pick the Web Unlocker zone fats should use:";
    let zone = match (zones.len() > 1).then(|| t.menu(&|_| text(l.short), pick, &zones, "← back · q quit")) {
        Some((i, '\r')) => zones.get(i),
        Some(_) => return Err(String::new()),
        None => zones.first(),
    };
    let what = zone.map_or(what, |z| format!("zone {z}"));
    let saved = settings::save_key(l.short, &key, zone.map(String::as_str));
    let tail = crate::stats::tail(&key);
    saved.map(|()| format!("✓ {} works · {what} · key saved {tail}", l.name)).map_err(|e| format!("✗ not saved: {e}"))
}

/// Way 1 (`main`): `fats setup [provider]` with a terminal on both ends and `stty`, `provider`'s screen first; None
/// = piped, an agent, no `stty`, or another command.
pub fn tty(argv: &[String]) -> Option<i32> {
    let cmd = crate::lang::parse(argv);
    let telemetry = (cmd.sub.first().map(String::as_str) == Some("telemetry")).then_some("telemetry");
    let only = cmd.sub.first().and_then(|p| links(p)).map(|l| l.short).or(telemetry);
    let term = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    (term && cmd.has("setup") && cmd.sub.len() < 2 && cmd.words.is_empty() && cmd.mods.is_empty()).then_some(())?;
    let saved = stty(&["-g"]).filter(|_| stty(&["raw", "-echo", "min", "1", "time", "0"]).is_some())?;
    // a menu that exits (q, Done): the done line printed after the terminal is restored (`Term` dropped)
    print!("\x1b[?1049h\x1b[?25l");
    let done = walk(&mut Term(Some(saved), Default::default()), only);
    println!("{done}");
    Some(0)
}

/// Way 2 (`stats::change`): `fats setup <free|oxy|bd|decodo|gh>` without a terminal. gh: the gh CLI's token
/// copied when it has one, else the form; `dry` starts nothing.
pub fn agent(p: &str, dry: bool) -> Out {
    let said = match links(p) {
        None if !settings::on("free") => format!("= setup free · {}", free_off().1),
        None => format!("= setup free · built in, no key to set · $0 · off: {} setup free off", bin()),
        // offline: the form's check could not run, so none starts (as a plain `fats setup` is dry)
        Some(_) if dry || settings::get().offline.value => {
            format!("= dry setup {p} · would start a one-shot key form on 127.0.0.1 · $0")
        }
        Some(l) if gh(l).is_some_and(|k| settings::save_key(p, &k, None).is_ok()) => {
            "= setup gh · copied the token of gh auth token into auth.json · $0".into()
        }
        Some(l) => return form(l),
    };
    Out::new(vec![said])
}

/// The free fetch switched off (`settings.json` `off`): its state (a summary's head) and its `setup` row, the state
/// and the move back.
pub fn free_off() -> (&'static str, String) {
    let state = "free off (turned off)";
    (state, format!("{state} → {} setup free on", bin()))
}

/// THE key shape on a command line: `line` with each key-shaped run of a word (`provider:<key>`, `"<key>"`) as its
/// tail, `…6XBR`. A run is key-shaped at `key_chars` (limits.json) chars or more of `[A-Za-z0-9_+/=-]` (Decodo's
/// token is base64) holding a letter and a digit; a word with `://` or a dot, or led by a `words.json`
/// `path_lead` (`/`, `./`, `~`): a URL, a host, a path, a file, is left whole, and so is the binary's name (a test
/// build's is long). A line holds a key when its masked copy differs (`key_in_argv`).
pub fn masked(line: &str) -> String {
    let key_char = |c: char| c.is_ascii_alphanumeric() || "_+/=-".contains(c);
    let shaped = |r: &&str| {
        let mixed = r.bytes().any(|c| c.is_ascii_digit()) && r.bytes().any(|c| c.is_ascii_alphabetic());
        mixed && r.len() >= settings::limit::<usize>("key_chars") && *r != bin()
    };
    let word = |w: &str| {
        let lead = crate::lang::WORDS.path_lead.iter().any(|l| w.starts_with(l));
        if lead || w.contains("://") || w.contains('.') {
            return w.to_string();
        }
        w.split(|c: char| !key_char(c)).filter(shaped).fold(w.to_string(), |w, r| w.replace(r, &crate::stats::tail(r)))
    };
    line.split(' ').map(word).collect::<Vec<_>>().join(" ")
}

/// The status line's keys, each by its tail, Bright Data's zone and the one file (≤ 100 cols): `keys oxy …6XBR · bd
/// …k2Qf zone fatsrc · decodo none · gh none · auth.json`, then ` · network off` under `FATS_OFFLINE`; no key at all
/// = the setup move; ` · bak` last while `auth.json.bak` holds an old key (`bak_row`).
pub fn keys_line() -> String {
    let names = || settings::SWITCHES.into_iter().filter(|p| *p != "free");
    let off = if settings::get().offline.value { " · network off" } else { "" };
    let net = format!("{off}{}", if bak().is_some() { " · bak" } else { "" });
    if names().all(|p| settings::key(p).is_none()) {
        return format!("keys none → {} setup{net}", bin());
    }
    let named = |p: &str| match (p, crate::stats::key(p)) {
        ("bd", k) if k.starts_with('…') => format!("bd {k} zone {}", settings::zone().unwrap_or("none")),
        (p, k) => format!("{p} {k}"),
    };
    let keys = names().map(named).collect::<Vec<_>>().join(" · ");
    format!("keys {keys} · {}{net}", settings::AUTH)
}

/// The old `auth.json` a replaced or removed key leaves (`settings::save_to`), while it is there.
pub fn bak() -> Option<PathBuf> {
    let f = settings::key_file(&format!("{}.bak", settings::AUTH));
    f.is_file().then_some(f)
}

/// Where a removed or switched-off key's old copy still is, and the move that deletes it (`rm`, the path quoted for
/// a shell): `old key kept in auth.json.bak (0600) · delete it: rm '<path>'`. None = no backup.
pub fn bak_row() -> Option<String> {
    let path = bak()?.display().to_string().replace('\'', r"'\''");
    Some(format!("old key kept in {}.bak (0600) · delete it: rm '{path}'", settings::AUTH))
}

/// The key the environment holds for `p` that fats does not use (`settings::env_key`, never sent), by tail (`fats`,
/// `fats setup`): beside a different saved key, replaced only by a person; where none is saved it IS that
/// provider's `no_key` row (`fats setup` drops its plain one), what `import-env` copies in.
pub fn env_row(p: &str) -> Option<String> {
    let ((var, env), b) = (settings::env_key(p)?, bin());
    let env = crate::stats::tail(&env);
    Some(match settings::key(p) {
        Some(k) => format!("env {var} {env} ≠ saved {} · not used · replace: {b} setup {p}", crate::stats::tail(&k)),
        None => format!("! no_key {p} · env {var} {env} not imported → {b} setup import-env"),
    })
}

/// `env_row` for every provider: the status rows.
pub fn env_rows() -> Vec<String> {
    SWITCHES.into_iter().filter_map(env_row).collect()
}

/// THE argv key rule: a key-shaped word on a command line led by `setup` or a word people type for it (`login`,
/// `keys`, `auth`: `words.json` `aliases`) is refused, never read, saved, echoed or searched (argv lands in shell
/// history and `ps`; `fats login <key>` would be a search sending it to a provider); the key's ways in are the
/// paste and the pipe. On any other command such a word is a word as typed (a sha, a part number).
pub fn key_in_argv(cmd: &Cmd, argv: &[String]) -> Option<Out> {
    // the first word, mods aside: read off `raw`, since a `?` for a later word, or a key making `login <key>` a
    // search, leaves `reserved` empty
    let lead = cmd.raw.split(' ').find(|t| !t.contains(':')).unwrap_or_default();
    let alias = crate::lang::WORDS.aliases.iter().any(|a| a.1 == "setup" && a.0.eq_ignore_ascii_case(lead));
    if !(alias || lead.eq_ignore_ascii_case("setup")) || argv.iter().all(|a| masked(a) == *a) {
        return None;
    }
    let (b, p) = (bin(), cmd.raw.split(' ').find_map(links).map_or("oxy", |l| l.short));
    let ways = format!("paste: {b} setup {p} · pipe: printf %s \"$KEY\" | {b} setup {p}");
    let row = format!("! key_in_argv · never on the command line (shell history, ps) · {ways}");
    Some(Out::fail("key_in_argv", "= ! key_in_argv setup · nothing read, nothing saved · $0".into(), row))
}

/// `fats setup <p>` fed a key on stdin (`printf %s "$KEY" | fats setup oxy`: SSH, CI, no browser): ONE line, its
/// trailing CR/LF stripped, the menu's check ($0, `setup::check`), saved when it passes (a replaced key →
/// `auth.json.bak`; the same key again replaces none); one line, rc 1 when nothing was saved (`FATS_OFFLINE`: no
/// check). None = no provider named, stdin a terminal or `fats mcp`'s protocol, or no byte within 2 s (an agent's
/// shell: `/dev/null`, a pipe never written): the key form.
pub fn piped(cmd: &Cmd) -> Option<Out> {
    let l = cmd.sub.first().and_then(|p| links(p)).filter(|_| cmd.has("setup") && cmd.sub.len() == 1)?;
    if cmd.flag("dry") || std::io::IsTerminal::is_terminal(&std::io::stdin()) || crate::cli::FENCE.get().is_some() {
        return None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = Vec::new();
        std::io::BufRead::read_until(&mut std::io::stdin().lock().take(16 << 10), b'\n', &mut line).ok();
        tx.send(line).ok();
    });
    let line = rx.recv_timeout(Duration::from_secs(2)).ok().filter(|l| !l.is_empty())?;
    let key = String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']).to_string();
    let (p, b) = (l.short, bin());
    let not_saved = |code: &'static str, why: &str| {
        let row = format!("= ! {code} setup {p} · {why} · not saved → printf %s \"$KEY\" | {b} setup {p}");
        Some(Out { rc: 1, ..Out::new(vec![row]).with("error", code) })
    };
    if settings::get().offline.value {
        return not_saved("offline", "FATS_OFFLINE is on: no check (unset FATS_OFFLINE)");
    }
    // the old key goes to `auth.json.bak` only when the new one differs (`settings::save_to`)
    let replaced = settings::key(p).is_some_and(|k| k != key.trim());
    let kept = if replaced { " · the old key in auth.json.bak" } else { "" };
    let (what, zones) = match check(l, &key) {
        Ok(got) => got,
        Err((code, why)) => return not_saved(code, &why),
    };
    let zone = if p == "bd" { pick_zone(&zones, None).ok() } else { None };
    if let Err(e) = settings::save_key(p, &key, zone.as_deref()) {
        return not_saved("cache_write", &e);
    }
    let pick = if p == "bd" && zone.is_none() {
        format!(" · no zone yet → {b} setup bd zone:<name>")
    } else {
        String::new()
    };
    let what = match zone {
        Some(z) => format!("zone {z}"),
        None => what,
    };
    let tail = crate::stats::tail(&key);
    Some(Out::new(vec![format!("= setup {p} ok {what} · key {tail} saved auth.json (600){kept} · $0{pick}")]))
}

/// Bright Data's Web Unlocker zone from its zone names: `zone:`, else the configured one, else the only one.
/// Err = the `! no_zone` row naming the account's unblocker zones.
pub fn pick_zone(names: &[String], arg: Option<&str>) -> Result<String, String> {
    let want = arg.or(settings::zone()).map(String::from);
    match (want, names) {
        (Some(w), _) if names.contains(&w) => Ok(w),
        (None, [one]) => Ok(one.clone()),
        (want, _) => {
            let what = want.map_or("pick one".into(), |w| format!("{w} is not a Web Unlocker zone here"));
            let list = if names.is_empty() { "none (add one in the control panel)".into() } else { names.join(", ") };
            let arg = names.first().map_or("<name>", String::as_str);
            let at = crate::lang::At { id: "setup", arg, ..Default::default() };
            Err(crate::lang::bang("bd", "no_zone", &format!("{what} · unblocker zones: {list}"), &at))
        }
    }
}

/// A key form's run file `<session>/run/setup-<p>.<ext>`: `json` = token, port, deadline (the cache root is its
/// owner's only, `cache::private_root`), there while the run serves (its port written once it listens; a run
/// killed leaves it until the deadline); `log` = the run's output.
fn run_file(c: &Cache, p: &str, ext: &str) -> PathBuf {
    c.root.join(&c.session).join("run").join(format!("setup-{p}.{ext}"))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Form {
    token: String,
    until: f64,
    port: u16,
}

/// Provider `p`'s key form that still waits: its URL.
fn waiting_at(c: &Cache, p: &str) -> Option<String> {
    let f: Form = jload(&run_file(c, p, "json")).filter(|f: &Form| f.port > 0 && f.until > now())?;
    Some(format!("http://127.0.0.1:{}/k/{}/{p}", f.port, f.token))
}

/// Way 2: the key form for `l`, served by a detached run (`--continue setup-<p>`, `serve`) on 127.0.0.1: the block
/// the agent shows its user (its last line: the env row, for a user whose browser is not on this machine). One
/// still waiting is shown again.
fn form(l: &'static Links) -> Out {
    let (c, p) = (Cache::open(), l.short);
    let fill = |url: &str| {
        let page = fill(page("agent"), Some(l), &[("url", url)]);
        Out::new(page.lines().map(String::from).collect()).with("cost", 0.0)
    };
    if let Some(url) = waiting_at(&c, p) {
        return fill(&url);
    }
    let mut token = [0u8; 16];
    let started = getrandom::fill(&mut token).map_err(|e| e.to_string()).and_then(|()| {
        let token = token.iter().map(|x| format!("{x:02x}")).collect();
        jsave(&run_file(&c, p, "json"), &Form { token, until: now() + LIFE, port: 0 })?;
        crate::cli::detach(format!("setup-{p}"), &run_file(&c, p, "log"), None)
    });
    // the run binds its port and writes it: the URL exists once it listens
    let up = || started.is_err() || waiting_at(&c, p).is_some();
    (0..150).take_while(|_| !up()).for_each(|_| std::thread::sleep(Duration::from_millis(20))); // 3 s at most
    let why = || format!("! setup_failed {}", started.as_ref().err().map_or("", String::as_str));
    let failed = || Out::fail("setup_failed", format!("= setup {p} · ! setup_failed · $0"), why());
    waiting_at(&c, p).map_or_else(failed, |url| fill(&url))
}

/// The detached run of `fats setup <p>`: serves its form until one good key or `LIFE`, then exits. Exit code.
pub fn serve(p: &str) -> i32 {
    let c = Cache::open();
    let (Some(l), Some(mut f)) = (links(p), jload::<Form>(&run_file(&c, p, "json"))) else { return 2 };
    let Ok(lis) = TcpListener::bind("127.0.0.1:0") else { return 1 };
    f.port = lis.local_addr().map_or(0, |a| a.port());
    if jsave(&run_file(&c, p, "json"), &f).is_err() || lis.set_nonblocking(true).is_err() {
        return 1;
    }
    while now() < f.until {
        match lis.accept() {
            Ok((stream, _)) if answer(&stream, &f, l) => break,
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    std::fs::remove_file(run_file(&c, p, "json")).ok(); // no file = no form waits
    0
}

/// THE form's route rule: `Host` = `127.0.0.1:<port>` (else 400: DNS rebinding) and an `Origin`, when sent, the
/// form's own (else 400: another page posting), then the page by GET and its `/save` by POST on the token path,
/// nothing else (404); no CORS, no-store, a CSP that runs nothing. true = saved.
fn answer(mut s: &TcpStream, f: &Form, l: &'static Links) -> bool {
    s.set_nonblocking(false).ok();
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut r = std::io::BufReader::new(s.take(1 << 16));
    let head: Vec<String> = (&mut r).lines().map_while(Result::ok).take_while(|l| !l.is_empty()).take(64).collect();
    let get =
        |h: &str| head.iter().skip(1).find_map(|l| Some(l.to_ascii_lowercase().strip_prefix(h)?.trim().to_string()));
    let mut body = vec![0; get("content-length:").and_then(|v| v.parse().ok()).unwrap_or(0usize).min(16384)];
    r.read_exact(&mut body).ok();
    let (page, req) = (format!("/k/{}/{}", f.token, l.short), head.first().map_or("", String::as_str));
    let (save, me) = (format!("{page}/save"), format!("127.0.0.1:{}", f.port));
    let (code, html, done) = match req.split(' ').take(2).collect::<Vec<_>>()[..] {
        _ if get("host:") != Some(me.clone()) || get("origin:").is_some_and(|o| o != format!("http://{me}")) => {
            ("400 Bad Request", String::new(), false)
        }
        ["GET", at] if at == page => ("200 OK", html(l, 2, &[("save", &save), ("err", "")]), false),
        ["POST", at] if at == save => post(l, &body, &save),
        _ => ("404 Not Found", String::new(), false),
    };
    let head = fill(part(0), None, &[("code", code), ("len", &html.len().to_string())]).replace('\n', "\r\n");
    s.write_all(format!("{head}{html}").as_bytes()).ok();
    done
}

/// A POSTed key, checked ($0) and saved: the done page, else the form again with why.
fn post(l: &'static Links, body: &[u8], save: &str) -> (&'static str, String, bool) {
    let key =
        url::form_urlencoded::parse(body).find(|(n, _)| n == "key").map_or(String::new(), |(_, v)| v.trim().into());
    let saved = check(l, &key).map_err(|e| e.1).and_then(|(what, zones)| {
        let (page, zone) = landed(l, &what, &zones);
        settings::save_key(l.short, &key, zone.as_deref()).map(|()| page)
    });
    match saved.map_err(|e| format!("<p class=bad>! {} · try again</p>", esc(&e))) {
        Ok(what) => ("200 OK", html(l, 3, &[("what", &what)]), true),
        Err(bad) => ("200 OK", html(l, 2, &[("save", save), ("err", &bad)]), false),
    }
}

/// The done page's line for a key that passed `check`, and Bright Data's zone saved with it: the configured one
/// or the account's only one (`pick_zone`); else the zones named and the move the agent makes.
fn landed(l: &Links, what: &str, zones: &[String]) -> (String, Option<String>) {
    let (b, works, done) = (bin(), format!("<b>{} works</b>", l.name), "go back to your agent and say <b>done</b>");
    match (l.short == "bd").then(|| pick_zone(zones, None)) {
        Some(Err(_)) => {
            let fix = format!("tell your agent: <b>{b} setup bd zone:&lt;name&gt;</b>");
            (format!("<b>key saved</b> · ✗ no zone yet → {fix} · zones: {}", esc(&zones.join(", "))), None)
        }
        Some(Ok(z)) => (format!("{works} · zone {} · {done}", esc(&z)), Some(z)),
        None => (format!("{works} · {} · {done}", esc(what)), None),
    }
}

/// Part `n` of the form page (2 key, 3 done, `part`) in its frame, `vars` filled in.
fn html(l: &Links, n: usize, vars: &[(&str, &str)]) -> String {
    fill(part(1), Some(l), &[("body", &fill(part(n), None, vars))])
}

/// Part `n` of `setup-form.html`: 0 the HTTP head, 1 the page's frame, 2 the key form, 3 done.
fn part(n: usize) -> &'static str {
    FORM.split("<!-- -->\n").nth(n).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        /// Every frame `Term::draw` painted on this thread, in order.
        pub(super) static FRAMES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Key `i` of a number row (`1` … `9`): it picks row or button `i`.
    fn digit(i: usize) -> &'static [u8] {
        &b"123456789"[i..=i]
    }
    /// Esc as the terminal sends it: back one list.
    const ESC: &[u8] = b"\x1b";

    /// One scripted walk from the main list: every key read, the done line, no frame's message a refusal and
    /// every menu row within 100 columns. The frames drawn, without colour.
    fn walked(script: Vec<&'static [u8]>, what: &str) -> Vec<String> {
        FRAMES.with_borrow_mut(Vec::clear);
        let mut t = Term(None, script.into());
        let done = walk(&mut t, None);
        assert!(t.1.is_empty() && done.starts_with("= setup done · "), "{what}: keys left {:?} · {done}", t.1);
        let frames = FRAMES.take();
        let lines = || frames.iter().flat_map(|f| f.lines()).map(str::trim_end);
        let refused = lines().find(|l| l.trim_start().starts_with("✗ Can't"));
        assert!(refused.is_none(), "{what}: a shown button answered {refused:?}");
        let row =
            |l: &&str| l.trim_start_matches([' ', '▸']).split_once(". ").is_some_and(|(n, _)| n.parse::<u8>().is_ok());
        let wide: Vec<&str> = lines().filter(row).filter(|l| l.chars().count() > 100).collect();
        assert!(wide.is_empty(), "{what}: rows over 100 columns {wide:?}");
        frames
    }

    /// Every button on every screen works, in a keyless install, with Oxylabs and Bright Data keys, and with only
    /// an Oxylabs key and the free fetch off: from the main list (`rows`) and Settings (`SETTINGS`), every row by
    /// its number, then every button its screen shows (`buttons`), each from the state's files in this thread's own
    /// key dir (fake keys, no network: a key that needs a provider's check is pasted as an empty line = back), then
    /// q to the done line (`walked`; ← Back is a button, Esc leaves Settings). A switch pressed twice shows its new
    /// state at once and ends as it began; a key saved keeps its screen.
    #[test]
    fn every_button_on_every_screen_works() {
        let keys = crate::scratch::Scratch::new("setup-buttons");
        settings::tests::KEY_DIR.set(Some(keys.to_path_buf()));
        crate::lang::tests::NO_COLOR.set(Some(true));
        let states: [(&[&str], bool); 3] = [(&[], true), (&["oxy", "bd"], true), (&["oxy"], false)];
        for (keyed, free) in states {
            // the state's two files, written as they are read (`settings::Saved`)
            let reset = || {
                let keys_of = keyed.iter().map(|k| (k.to_string(), "fake-key-0123456789".to_string())).collect();
                let off = if free { vec![] } else { vec!["free".to_string()] };
                let files = [(settings::AUTH, keys_of, vec![]), (settings::PREFS, Default::default(), off)];
                for (f, keys_of, off) in files {
                    let saved = settings::Saved { version: 1, keys: keys_of, off, ..Default::default() };
                    std::fs::write(keys.join(f), serde_json::to_vec(&saved).expect("json")).expect("write");
                }
            };
            let mut at: Vec<(Vec<&'static [u8]>, &str)> = Vec::new();
            for (i, p) in rows().into_iter().enumerate() {
                at.push((vec![digit(i)], p));
                let inner = SETTINGS.into_iter().enumerate().map(|(j, s)| (vec![digit(i), digit(j)], s));
                at.extend(inner.filter(|_| p == "settings"));
            }
            for (to, p) in at {
                reset();
                let what = format!("{keyed:?} free {free} · {p}");
                // a row with a screen: each of its buttons; else the row alone (Settings left by Esc, a row that only
                // shows stays, Done and Back leave the list)
                let out: &[&[u8]] = match p {
                    "done" => &[],
                    "settings" => &[ESC, b"q"],
                    "back" | "cache" | "stats" => &[b"q"],
                    _ => {
                        buttons(p).into_iter().enumerate().for_each(|(k, b)| press(&reset, &to, p, k, b, &what));
                        continue;
                    }
                };
                walked([&to[..], out].concat(), &what);
            }
        }
        settings::tests::KEY_DIR.set(None);
        crate::lang::tests::NO_COLOR.set(None);
    }

    /// Button `b`, the `k`-th on screen `p` reached by `to`, pressed from the state `reset` makes, and what it did
    /// checked.
    fn press(reset: &dyn Fn(), to: &[&'static [u8]], p: &str, k: usize, b: &str, what: &str) {
        reset();
        let what = &format!("{what} · {b}");
        let (on, no_check) = (settings::on(p), !crate::provider::KEYS.into_iter().any(|t| t.spec().short == p));
        // a paste arrives as one read, its line end too (`\r\n`): the `\n` must not press the next screen's Enter
        let pasted: &[u8] = if no_check { b"fake-gh-token-0123\r\n" } else { b"\r\n" };
        let then: &[&[u8]] = match b {
            "Add a key" | "Paste a new key" => &[pasted, b"q"],
            "Turn on" | "Turn off" => &[digit(k), b"q"], // pressed again
            _ => &[b"q"],
        };
        let frames = walked([to, &[digit(k)], then].concat(), what);
        let keyless = links(p).is_some() && settings::key(p).is_none() && b == "Add a key";
        assert!(!keyless || frames.iter().any(|f| f.contains("│ Status  no key")), "{what}: Status no key");
        let level = LEVELS.iter().find(|l| l.1 == b).map(|l| serde_json::Value::from(l.0.name()));
        match b {
            "Turn on" | "Turn off" => {
                let flip = format!("{}. {}", k + 1, ["Turn off", "Turn on"][usize::from(b == "Turn off")]);
                let shown = frames.iter().find(|f| f.contains(&flip));
                assert!(shown.is_some_and(|f| f.contains("│ Status  off") == on), "{what}: {flip} at once");
                assert_eq!(settings::on(p), on, "{what}: pressed twice, as it began");
            }
            "Remove this key" => {
                assert!(settings::key(p).is_none(), "{what}: the key is gone");
                // the removed key's old copy and the move that deletes it, said on the screen it was removed from
                let bak = "○ key removed · old key kept in auth.json.bak (0600) · delete it: rm '";
                assert!(frames.iter().any(|f| f.contains(bak)), "{what}: the backup named");
            }
            "Add a key" | "Paste a new key" if no_check => pasted_frames(&frames, 18, what),
            _ if level.is_some() => assert_eq!(settings::saved(settings::PREFS).set.get("telemetry"), level.as_ref()),
            _ => {}
        }
    }

    /// The frames of an `n`-byte key pasted with `\r\n` (`Term::secret`): one prompt (0 chars), then one frame a
    /// byte with that many mask glyphs and the count, no key byte in any frame; then the check, then the provider's
    /// screen with the key's tail saved (the `\n` pressed nothing: one prompt only).
    fn pasted_frames(frames: &[String], n: usize, what: &str) {
        let at = frames.iter().position(|f| f.contains("   0 chars")).unwrap_or(frames.len());
        for (i, f) in frames.iter().skip(at).take(n + 1).enumerate() {
            let mask = format!("\n   {}  {i} chars\r", "•".repeat(i));
            assert!(f.contains(&mask), "{what}: byte {i} draws its mask: {f}");
        }
        assert!(!frames.iter().any(|f| f.contains("fake-gh") || f.contains("token-0")), "{what}: a key byte shown");
        assert_eq!(frames.iter().filter(|f| f.contains("   0 chars")).count(), 1, "{what}: the \\n pressed Enter");
        let (check, screen) = (frames.get(at + n + 1), frames.get(at + n + 2));
        assert!(check.is_some_and(|f| f.contains("… checking the key with ")), "{what}: {check:?}");
        let saved =
            screen.is_some_and(|f| f.contains(" works · unchecked · key saved …0123") && f.contains(". Paste a"));
        assert!(saved && settings::key("gh").as_deref() == Some("fake-gh-token-0123"), "{what}: {screen:?}");
    }

    /// A frame is in colour (`lang::paint` styles): the marks (✓ green), the marked row bold around them, the
    /// headings bold; under `NO_COLOR` not one escape.
    #[test]
    fn a_frame_is_in_colour_unless_no_color() {
        let t = Term(None, Default::default());
        let frame = |plain| {
            crate::lang::tests::NO_COLOR.set(Some(plain));
            t.draw(&text("oxy"), "✓ Oxylabs Web API works", &["   ▸ 1. ✓ Free fetch".into()], "q quit");
            FRAMES.with_borrow_mut(Vec::pop).unwrap_or_default()
        };
        let (colour, plain) = (frame(false), frame(true));
        crate::lang::tests::NO_COLOR.set(None);
        assert!(colour.contains("\x1b[32m✓\x1b[0m Oxylabs Web API works"), "{colour:?}");
        assert!(colour.contains("\x1b[1m   ▸ 1. \x1b[32m✓\x1b[0m\x1b[1m Free fetch\x1b[0m"), "{colour:?}");
        assert!(colour.contains("\x1b[1mHow it works\x1b[0m") && colour.contains("\x1b[2mq quit\x1b[0m"));
        assert!(!plain.contains('\x1b') && plain.contains("   ▸ 1. ✓ Free fetch") && plain.contains("How it works"));
    }

    /// The key prompt on raw reads: ⌫ takes a byte back, a space or a control byte is no key byte, Enter ends it
    /// (the rest of that read dropped), Esc, ^C, no bytes or Enter alone = back; a frame shows the count only.
    #[test]
    fn the_key_prompt_masks_and_edits() {
        let key = |script: Vec<&'static [u8]>| {
            FRAMES.with_borrow_mut(Vec::clear);
            let mut t = Term(None, script.into());
            (t.secret(&text("oxy"), ""), t.1.len(), FRAMES.take())
        };
        let (got, left, frames) = key(vec![b"ab\x7fc d\t", b"e\r\nq", b"z"]);
        assert_eq!((got.as_deref(), left), (Some("acde"), 1), "the next read is the menu's");
        assert_eq!(frames.len(), 9, "the prompt, then a frame a byte before Enter");
        let shown: Vec<usize> = [0, 1, 2, 1, 2, 2, 3, 3, 4].into();
        for (f, n) in frames.iter().zip(shown) {
            assert!(f.contains(&format!("\n   {}  {n} chars\r", "•".repeat(n))) && !f.contains("acde"), "{f}");
        }
        for script in [vec![&b"abc\x1b"[..]], vec![b"abc\x03"], vec![b"abc"], vec![b"\r"]] {
            assert_eq!(key(script).0, None, "back");
        }
    }

    /// The menu state machine on raw reads: moves wrap (both arrow encodings), a number jumps and picks, Enter or
    /// → picks, Esc/←/⌫ back, q, ^C or no bytes quit.
    #[test]
    fn menu_steps() {
        let up = (step(0, 3, b"\x1b[A"), step(0, 3, b"\x1bOA"), step(2, 3, b"\x1b[B"), step(1, 3, b"j"));
        assert_eq!(up, ((2, None), (2, None), (0, None), (2, None)));
        let pick = (step(1, 3, b"\r"), step(1, 3, b"\x1b[C"), step(0, 3, b"3"), step(0, 3, b"4"));
        assert_eq!(pick, ((1, Some('\r')), (1, Some('\r')), (2, Some('\r')), (0, None)));
        let back = (step(0, 3, b"\x1b"), step(0, 3, b"\x1b[D"), step(0, 3, b"\x7f"), step(0, 3, b"q"));
        assert_eq!(back, ((0, Some('\x1b')), (0, Some('\x1b')), (0, Some('\x1b')), (0, Some('q'))));
        assert_eq!(
            (step(1, 3, b"s"), step(1, 3, b"\x03"), step(1, 3, b"")),
            ((1, None), (1, Some('q')), (1, Some('q')))
        );
    }

    /// Way 1 driven by a scripted key source: into Oxylabs' screen and back, Settings by its number, the agent
    /// screen and back twice, then q: every key used, the done line printed. Reads settings only (no row that
    /// saves, opens or copies).
    #[test]
    fn a_scripted_walk_ends_with_the_done_line() {
        let script: [&[u8]; 10] =
            [b"\x1b[B", b"\r", b"\x1b", b"6", b"\r", b"\x1b", b"\x1b[D", b"\x1bOA", b"\x1bOA", b"q"];
        FRAMES.with_borrow_mut(Vec::clear);
        let mut t = Term(None, script.into());
        let done = walk(&mut t, None);
        assert!(t.1.is_empty(), "every key read: {:?}", t.1);
        assert!(done.starts_with("= setup done · ✓ free ") && done.ends_with(" example.com"), "{done}");
        // ← on the main list quits (no list above it), its footer says so; Settings' ← goes back
        let frames = FRAMES.take();
        assert!(frames.first().is_some_and(|f| f.contains("· ← or q quit")), "{:?}", frames.first());
        assert!(frames.iter().any(|f| f.contains("Agent connection (MCP)") && f.contains("· ← back · q quit")));
        let mut t = Term(None, [&b"\x1b"[..], b"q"].into());
        assert!(walk(&mut t, Some("bd")).starts_with("= setup done") && t.1.is_empty(), "bd's screen, back, q");
    }

    /// The main list: providers, then Settings and Done each under an empty line; Settings lists the agent
    /// connection, Cache, Stats and Telemetry, then Back under an empty line; Telemetry's On saves `full`. Unit
    /// tests share one scratch settings file across processes: this one saves only `full`, the level the others
    /// read already (`off` there turned `weekly_counts_codes_only`'s tally off mid-run).
    #[test]
    fn settings_hold_cache_stats_and_telemetry() {
        let row = |p: &&str| text(p)[0].clone();
        assert_eq!(
            SETTINGS.iter().map(row).collect::<Vec<_>>(),
            ["Agent connection (MCP)", "Cache", "Stats", "Telemetry", "← Back"]
        );
        let row = state("cache").1;
        assert!(row.contains(" pages, ") && row.ends_with("B · live ≤ 1 h"), "{row}");
        let card = text("cache").into_iter().find(|l| l.contains("Status")).unwrap_or_default();
        assert!(card.contains(" · pages live ≤ 1 h (30 min unread) · "), "{card}");
        assert!(state("stats").1.starts_with("this session: 0 pages · $0"), "{}", state("stats").1);
        assert!(state("telemetry").1.ends_with(" · anonymous usage stats, to fix and improve fats"));
        // Settings (6), Telemetry (4), On (1), then back out and q
        let mut t = Term(None, [&b"6"[..], b"4", b"1", b"\x1b", b"\x1b", b"q"].into());
        let done = walk(&mut t, None);
        let saved = settings::saved(settings::PREFS).set.get("telemetry").cloned();
        settings::unset("telemetry").expect("unset");
        assert!(t.1.is_empty() && done.starts_with("= setup done"), "{:?} {done}", t.1);
        assert_eq!(saved, Some("full".into()), "the option's level stored, never its label");
    }

    /// A menu row fits 100 columns with a long cache dir (a scratch HOME under the temp dir): the row shortens the
    /// dir, `~` for home and the middle parts as `…`; the card's Status keeps it whole.
    #[test]
    fn the_cache_row_fits_100_columns() {
        // the OS's own separator, as the dirs it gets (`C:\Users\…\AppData\Local\fatsearch`)
        let os = |p: &str| p.replace('/', std::path::MAIN_SEPARATOR_STR);
        let tmp = os("/var/folders/s8/5n29mppx4z54b1vrnn6zpkrh0000gn/T/tmp.qXxiDel2af");
        let dir = PathBuf::from(os(&format!("{tmp}/Library/Caches/fatsearch")));
        assert!(dir.display().to_string().chars().count() >= 60);
        let (row, card) = cache_words(&dir);
        let line = format!("   ▸ 2.   {:18} {row}", "Cache");
        assert!(line.chars().count() <= 100, "{} chars: {line}", line.chars().count());
        assert!(
            row.starts_with(&os("/var/folders/…/Caches/fatsearch · ")) && card.starts_with(&os(&format!("{tmp}/"))),
            "{row}"
        );
        let home = Path::new(&tmp);
        assert_eq!(row_path(&dir, Some(home)), os("~/Library/Caches/fatsearch"));
        assert_eq!(row_path(Path::new("/srv/fats"), Some(home)), "/srv/fats", "a short dir stays whole");
        let main = ["free", "oxy", "decodo", "bd", "gh", "settings", "done"];
        let row = |p: &&str| format!("   ▸ 2. {} {:18} {}", state(p).0, text(p)[0], state(p).1);
        let wide: Vec<String> = main.iter().chain(&SETTINGS).map(row).filter(|r| r.chars().count() > 100).collect();
        assert!(wide.is_empty(), "rows over 100 columns: {wide:?}");
    }

    /// A Bright Data key with zones but none picked: saved, the page names the zones and the agent's move;
    /// the account's only zone is saved with the key.
    #[test]
    fn a_bd_key_without_its_zone_names_the_move() {
        let (bd, zones) = (links("bd").expect("bd"), ["web_unlocker1".to_string(), "cli_unlocker".into()]);
        let (page, zone) = landed(bd, "", &zones);
        let tail = " setup bd zone:&lt;name&gt;</b> · zones: web_unlocker1, cli_unlocker";
        assert!(zone.is_none() && page.starts_with("<b>key saved</b> · ✗ no zone yet → tell your agent: <b>"));
        assert!(page.ends_with(tail), "{page}");
        assert_eq!(landed(bd, "", &zones[..1]).1.as_deref(), Some("web_unlocker1"), "the only zone is saved");
    }

    #[test]
    fn screens_and_form_parts() {
        let l = links("oxy").expect("oxy");
        let page = html(l, 2, &[("save", "/k/t/oxy/save"), ("err", "")]);
        assert!(page.contains("action=\"/k/t/oxy/save\"") && page.contains(l.mint) && !page.contains("<script"));
        let done = html(l, 3, &[("what", &landed(l, "351 scrapers", &[]).0)]);
        assert!(
            done.contains(
                "<p><b>Oxylabs Web API works</b> · 351 scrapers · go back to your agent and say <b>done</b>."
            )
        );
        let order: Vec<&str> = PROVIDERS.links.iter().map(|l| l.short).collect();
        assert_eq!(order, ["oxy", "decodo", "bd", "gh"], "the list's rank order");
        // a rank is a dated measurement, and only one provider is #1 (the card and the list render it)
        let ranks: Vec<_> = order.iter().map(|p| state(p).1).filter(|r| r.contains("#1")).collect();
        assert_eq!(ranks, ["#1 · 14/19 hard pages (median of 9 runs, 12–16, 2026-10-15)"], "{ranks:?}");
        assert!(
            text("oxy")
                .iter()
                .any(|l| l.ends_with("Rank    #1 · 14/19 hard pages (median of 9 runs, 12–16, 2026-10-15)"))
        );
        for p in ["free", "oxy", "decodo", "bd", "gh", "settings", "mcp", "cache", "stats", "telemetry", "back", "done"]
        {
            assert_eq!(text(p).get(6).map(String::as_str), Some(" ╰─"), "{p}: the card is 4 rows, every screen");
        }
        let hosts: Vec<String> = text("mcp").into_iter().skip(7).filter(|r| r.contains("mcp add")).collect();
        assert_eq!(hosts.len(), 2, "Claude Code and Codex lines, from the one HOSTS table");
        assert_eq!(esc("<a\"&>"), "&lt;a&quot;&amp;&gt;");
    }
}
