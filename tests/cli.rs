//! End-to-end checks on the built binary: concurrency, line shapes and past regressions.
//! Each test runs in its own throwaway cache dir.

#[path = "../src/scratch.rs"]
mod scratch;

use scratch::Scratch;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Sandbox {
    dir: Scratch,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let dir = Scratch::new(&format!("it-{name}"));
        std::fs::create_dir_all(dir.join("work")).expect("sandbox");
        Sandbox { dir }
    }

    fn cmd(&self, args: &str) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_fats"));
        // the caller's environment never leaks in: a setting or zone exported in the shell running the tests
        // changes dry routes and hints; Windows keeps `SystemRoot`: without it Winsock does not start, every
        // connect fails (`connect_failed` to the local site). The home is the sandbox's (`HOME`, on Windows
        // `USERPROFILE`), `%APPDATA%` and `%LOCALAPPDATA%` too: Windows' default key dir and cache, never the
        // runner's own (a refused `FATS_CACHE` falls back to the cache and sweeps it)
        c.env_clear()
            .envs(["PATH", "SystemRoot"].into_iter().filter_map(|k| Some((k, std::env::var_os(k)?))))
            .env("HOME", self.dir.join("home"))
            .env("USERPROFILE", self.dir.join("home"))
            .env("APPDATA", self.dir.join("home").join("AppData").join("Roaming"))
            .env("LOCALAPPDATA", self.dir.join("home").join("AppData").join("Local"))
            .args(args.split_whitespace())
            .current_dir(self.dir.join("work"))
            .envs(self.env());
        c
    }

    /// The `FATS_*` settings every child gets: the sandbox's cache, session and key dir, offline.
    fn env(&self) -> [(&'static str, PathBuf); 5] {
        [
            ("FATS_TELEMETRY", "0".into()), // a test that unsets FATS_OFFLINE still sends nothing
            ("FATS_CACHE", self.dir.join("cache")),
            ("FATS_SESSION", "t".into()),
            ("FATS_KEY_DIR", self.dir.join("keys")),
            ("FATS_OFFLINE", "1".into()),
        ]
    }

    /// The child's environment in this process too, for an in-process expectation (`fats::lang::fix` prices
    /// its move from the keys it finds): the runner's own key dir and exported keys made it pass only where
    /// both keys existed. Call before the first in-process settings read; the assert proves it was first.
    #[expect(unsafe_code, reason = "set_var before this process's first settings read")]
    fn env_here(&self) {
        let shell = std::env::vars_os().map(|(k, _)| k).filter(|k| {
            let k = k.to_string_lossy();
            k.starts_with("FATS_") || k.starts_with("OXYLABS_") || k.starts_with("BRIGHTDATA_") || k == "GH_TOKEN"
        });
        // SAFETY: no other test in this binary reads settings or the environment in-process; children spawned
        // meanwhile get `env_clear` and their own values (std's `Command` reads the environment under the same
        // lock `set_var` takes).
        unsafe {
            for k in shell.collect::<Vec<_>>() {
                std::env::remove_var(k);
            }
            for (k, v) in self.env() {
                std::env::set_var(k, v);
            }
        }
        assert_eq!(fats::settings::get().key_dir.value, self.dir.join("keys"), "settings were read before");
    }

    fn run(&self, args: &str) -> (String, i32) {
        let o: Output = self.cmd(args).output().expect("run fats");
        (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code().unwrap_or(-1))
    }

    /// `keys` (provider name → key) as the key dir's `auth.json`, in place of what it held: fats reads no key from
    /// the environment.
    fn auth(&self, keys: &[(&str, &str)]) {
        scratch::auth(&self.dir.join("keys"), keys);
    }

    /// Both providers' keys (fake values) and the Bright Data zone in the key dir, as 0.1 kept them (moved into
    /// `auth.json` by the first command).
    fn keys(&self) {
        let k = self.dir.join("keys");
        std::fs::create_dir_all(&k).expect("key dir");
        for (f, v) in
            [("oxylabs.key", "oxyK3y-cli"), ("brightdata.key", "bdK3y-cli"), ("brightdata.zone", "my_unlocker")]
        {
            std::fs::write(k.join(f), v).expect("key");
            #[cfg(unix)]
            std::fs::set_permissions(k.join(f), std::os::unix::fs::PermissionsExt::from_mode(0o600)).expect("chmod");
        }
    }

    fn file(&self, name: &str, body: &str) {
        std::fs::write(self.dir.join("work").join(name), body).expect("write fixture");
    }
}

/// Output line shapes: line 1 is `=` or `?`, every `!` row carries a `→` fix, every printed id reads back.
fn shape(out: &str) {
    let first = out.lines().next().unwrap_or_default();
    assert!(first.starts_with('=') || first.starts_with('?'), "first line: {out}");
    for l in out.lines() {
        if !l.starts_with('=') && (l.contains(" ! ") || l.starts_with('!')) {
            assert!(l.contains('→'), "! without a fix: {l}");
        }
        for t in l.split_whitespace().map(|t| t.trim_start_matches('[').trim_end_matches([']', ',', '·'])) {
            let looks = t.starts_with('u') && t[1..].starts_with(|c: char| c.is_ascii_digit())
                || t.starts_with('j') && t.contains(".u");
            if looks && t.contains('#') {
                let c = fats::lang::parse(&[t.to_string()]);
                assert!(c.ask.is_none() && c.ids.len() == 1, "id does not read back: {t} in {l}");
            }
        }
    }
}

fn page(title: &str, body: &str) -> String {
    format!("<html><head><title>{title}</title></head><body><h1>{title}</h1>{body}</body></html>")
}

/// 8 agents ingest into one session at once: 8 distinct ids, 8 complete snapshots, nothing deleted.
#[test]
fn parallel_ingest_keeps_every_page() {
    let s = Sandbox::new("parallel");
    for i in 0..8 {
        s.file(&format!("p{i}.html"), &page(&format!("Page number {i}"), "<p>some text here</p>"));
    }
    let kids: Vec<_> = (0..8)
        .map(|i| s.cmd(&format!("./p{i}.html")).stdout(std::process::Stdio::piped()).spawn().expect("spawn"))
        .collect();
    let mut ids: Vec<String> = Vec::new();
    for k in kids {
        let o = k.wait_with_output().expect("wait");
        let out = String::from_utf8_lossy(&o.stdout).into_owned();
        assert_eq!(o.status.code(), Some(0), "{out}");
        let head = out.lines().next().unwrap_or_default();
        let id = head.split_whitespace().nth(1).unwrap_or_default();
        ids.push(id.rsplit('.').next().unwrap_or_default().to_string());
    }
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 8, "ids {ids:?}");
    let mut titles: Vec<String> = (1..=8)
        .map(|n| {
            let (out, rc) = s.run(&format!("u{n}"));
            assert_eq!(rc, 0, "u{n}: {out}");
            out.lines().next().unwrap_or_default().split(" · ").next().unwrap_or_default().to_string()
        })
        .collect();
    titles.iter_mut().for_each(|t| *t = t.split_whitespace().skip(2).collect::<Vec<_>>().join(" "));
    titles.sort();
    titles.dedup();
    assert_eq!(titles.len(), 8, "titles {titles:?}");
    for n in 1..=8 {
        let d = s.dir.join("cache/t").join(format!("u{n}"));
        for f in ["raw.html", "meta.json", "doc.json"] {
            assert!(d.join(f).exists(), "{} missing", d.join(f).display());
        }
    }
}

/// A stray `uN` dir (left by a crash or an older build) is never deleted: the snapshot goes next to it.
/// Nothing spent is `$0.0000`: an empty `sum::<f64>()` is −0.0 and printed `$-0.0000`.
#[test]
fn empty_session_spent_nothing() {
    let s = Sandbox::new("spent");
    let (out, rc) = s.run("stats");
    assert_eq!(rc, 0, "{out}");
    assert!(out.starts_with("= stats s:t · 0 searches · 0 fetches · 0ms spent $0.0000\n"), "{out}");
}

#[test]
fn existing_dir_is_never_deleted() {
    let s = Sandbox::new("stray");
    let stray = s.dir.join("cache/t/u1");
    std::fs::create_dir_all(&stray).expect("stray dir");
    std::fs::write(stray.join("keep.txt"), "x").expect("stray file");
    s.file("a.html", &page("Alpha page", "<p>alpha</p>"));
    let (out, rc) = s.run("./a.html");
    assert_eq!(rc, 0, "{out}");
    assert!(stray.join("keep.txt").exists());
    assert!(s.dir.join("cache/t/u1-2/raw.html").exists());
    assert!(s.run("u1").0.contains("Alpha page"));
}

const SECTIONS: &str = "<title>T</title><h2>Alpha</h2><p>apple pie recipe</p><h2>Beta</h2><p>apple tart recipe</p>\
    <h3>Beta sub</h3><p>apple crumble</p><h2>Gamma</h2><p>pear</p>";

/// `u1#2 <words>` filters section 2 and its subsections only, and says so on the `=` line.
#[test]
fn section_filter_stays_in_the_section() {
    let s = Sandbox::new("secfilter");
    s.file("s.html", SECTIONS);
    assert_eq!(s.run("./s.html").1, 0);
    let (out, rc) = s.run("u1#2 apple");
    assert_eq!(rc, 0, "{out}");
    assert!(out.starts_with("= j1.u1#2 filter \"apple\" · 2 hit"), "{out}");
    let rows: Vec<&str> = out.lines().filter(|l| l.starts_with("u1#")).collect();
    assert!(rows.iter().all(|l| l.starts_with("u1#2 ") || l.starts_with("u1#3 ")), "{out}");
    let (out, rc) = s.run("u1#9 apple");
    assert_eq!((rc, out.starts_with("? u1#9 unknown")), (2, true), "{out}");
    assert!(s.run("u1#2").0.contains("\n### Beta sub\n"), "a subsection's head as the page has it, no id");
}

/// Grep alternation as filter words (`pie\|pear`, `pie|pear`) is no path and no regex: its words are filtered, the
/// `=` line names them, no `?` row (a sure repair is silent).
#[test]
fn piped_filter_words_are_words() {
    let s = Sandbox::new("pipefilter");
    s.file("s.html", SECTIONS);
    assert_eq!(s.run("./s.html").1, 0);
    for q in ["pie\\|pear", "pie|pear", "u1#2 pie\\|pear"] {
        let (out, rc) = s.run(if q.starts_with('u') { q.to_string() } else { format!("u1 {q}") }.as_str());
        let head = out.lines().next().unwrap_or_default();
        assert!(rc == 0 && head.contains(" filter \"pie pear\" · ") && !out.contains("\n? "), "{q}: {out}");
    }
}

/// A section of ~30 parts: hints go `… → u1#1z`, `u1#1aa`, and every printed id reads back.
#[test]
fn long_section_part_ids_read_back() {
    let s = Sandbox::new("parts");
    let para: String =
        (0..700).map(|i| format!("<p>{}</p>", format!("line {i} of a very long section ").repeat(6))).collect();
    s.file("long.html", &format!("<title>L</title><h2>One</h2>{para}<h2>Two</h2><p>end</p>"));
    shape(&s.run("./long.html").0);
    let (out, rc) = s.run("u1#1aa");
    assert_eq!(rc, 0, "{out}");
    assert!(out.starts_with("= j1.u1#1aa One · part 27/"), "{out}");
    assert!(out.contains("→ fats u1#1ab"), "{out}");
    shape(&out);
    shape(&s.run("u1#1z").0);
}

/// A filter word that is also a file name stays a word beside a page id; alone, a name that is a file here is it.
#[test]
fn file_names_do_not_steal_words() {
    let s = Sandbox::new("files");
    s.file("s.html", SECTIONS);
    s.file("apple", "not a page");
    s.file("notes.md", "<p>local notes</p>");
    assert_eq!(s.run("./s.html").1, 0);
    let (out, rc) = s.run("u1 apple");
    assert_eq!(rc, 0, "{out}");
    assert!(out.starts_with("= j1.u1 filter \"apple\""), "{out}");
    assert!(s.run("u1 notes.md").0.starts_with("= j1.u1 filter \"notes.md\""));
    let dry = s.run("apple dry:true").0;
    assert!(dry.contains("\napple 10B local") && !dry.contains("search \""), "a bare name that is a file here: {dry}");
    for help in ["help", "-h", "--help"] {
        assert_eq!(s.run(help), (format!("{}\n{}\n", fats::lang::CARD, to_guide()), 0), "{help} is the card");
    }
    assert!(
        fats::lang::CARD.starts_with("fats <url|file.pdf>") && fats::lang::CARD.contains("fats <words>  web search")
    );
    assert!(!fats::lang::CARD.contains("soon"));
    assert!(fats::lang::CARD.contains(" provider:plain|bd|oxy|decodo "), "every provider a cold agent can name");
    assert!(fats::lang::CARD.len() < 500, "--help is every cold agent's first read: {} B", fats::lang::CARD.len());
    assert!(s.run("notes.md").0.starts_with("= j2.u2 "), "an existing dotted name is a file");
    assert!(s.run("s.html").0.starts_with("= j3.u1 T"), "a document extension is a file: u1's, read again");
}

/// The pointer every first door ends with, as the `fats` binary prints it.
fn to_guide() -> String {
    format!("agents: fats guide = the manual (~{} tok)", fats::lang::ksize(fats::clean::tok(fats::lang::GUIDE)))
}

/// `fats guide` = the guide file byte for byte (one source, the binary's own version); the installed skill only
/// points at it; `help` stays the short card; every door a new agent tries first ends with the same pointer.
#[test]
fn the_guide_is_one_call_from_every_door() {
    let s = Sandbox::new("guide");
    assert_eq!(s.run("guide"), (fats::lang::GUIDE.to_string(), 0));
    let body = include_str!("../src/data/guide.md");
    assert!(body.starts_with("# fatsearch") && body.contains("## Rules"), "the guide is the whole how-to");
    assert!(body.len() <= 5200, "the guide is one read, about 2k tok: {} B", body.len());
    // a `!` row's move may be `fresh:true` (`budget_hit`): the guide never bans the fix its rows print
    assert!(!body.contains("Never fresh:true"), "the guide and its rows agree");
    let skill = include_str!("../skills/fatsearch/SKILL.md");
    assert!(skill.contains("Run `fats guide` before the first use") && skill.contains("fatsearch (command: `fats`)"));
    assert!(skill.len() < 2000, "the installed skill is a pointer, the guide lives in the binary: {} B", skill.len());
    // an install check is one more turn before the user's question: the skill connects, then asks
    assert!(!skill.contains("Verify") && !skill.contains("example.com"), "no verify step");
    // a plain user gets one provider's 3 lines, then the task: no question step, no MCP unless asked
    assert!(skill.contains("web search needs ONE key") && !skill.contains("\n5. "), "one key, then the task");
    let guide = ["9. No URL? Still the web", "ONE key: Oxylabs (default)", "Give the human its 3 lines only"];
    assert!(guide.iter().all(|g| body.contains(g)), "the guide says the same");
    for help in ["rust --help", "commands", "help login"] {
        assert_eq!(s.run(help).0, format!("{}\n{}\n", fats::lang::CARD, to_guide()), "{help} is the card");
    }
    assert!(fats::lang::CARD.len() <= 500, "the card rides every MCP connect: {} B", fats::lang::CARD.len());
    // a cap is the user's call: the first reads name no `cost:` value an agent would copy to stay free
    assert!(!fats::lang::CARD.contains("cost:") && !fats::lang::GUIDE.contains("cost:0"), "no free-only lever");
    let help = s.run("help").0;
    assert!(help.len() <= 500, "--help, the pointer with it, is every cold agent's first read: {} B", help.len());
    let first = "First time? Call fats(\"guide\")";
    let mcp = fats::mcp::instructions();
    assert!(mcp.len() <= 1200, "the instructions ride every MCP connect: {} B", mcp.len());
    assert!(mcp.contains("fats setup telemetry off") && fats::lang::GUIDE.contains("fats setup telemetry off"));
    assert!(fats::mcp::instructions().ends_with(first) && fats::mcp::DESCRIPTION.ends_with(&format!("{first}.")));
    assert!(!fats::mcp::instructions().contains("## Rules"), "the guide costs every connect: it stays a call away");
    for door in ["", "status", "--version", "whoami", "info", "install", "setup dry:true"] {
        let out = s.run(door).0;
        // the status doors say it on line 2, under the `=` line; the others end with it
        let ok = match door {
            "" | "status" => out.lines().nth(1) == Some(to_guide().as_str()),
            _ => out.trim_end().ends_with(&to_guide()),
        };
        assert!(ok, "{door:?}: {out}");
    }
}

/// A cached zip (even one stored as a PDF's kind) is `! not_in_v1` with a move that works today; a broken PDF is
/// `! parse_failed`. Neither is ever code `ok`.
#[test]
fn cached_zip_and_broken_pdf_are_records() {
    let s = Sandbox::new("bin");
    let c = s.dir.join("cache");
    let meta =
        r#"{"url":"https://ex.com/a.pdf","provider":"plain","ok":true,"code":"ok","t":1,"bytes":9,"kind":"pdf"}"#;
    for (u, raw, body) in [("u1", "raw.pdf", &b"%PDF-1.7 x"[..]), ("u2", "raw.pdf", &b"PK\x03\x04zip"[..])] {
        std::fs::create_dir_all(c.join("t").join(u)).expect("dir");
        std::fs::write(c.join("t").join(u).join(raw), body).expect("raw");
        std::fs::write(c.join("t").join(u).join("meta.json"), meta.replace("a.pdf", u)).expect("meta");
    }
    let state = r#"{"u":[{"url":"https://ex.com/u1","dir":"t/u1"},{"url":"https://ex.com/u2","dir":"t/u2"}]}"#;
    std::fs::write(c.join("t/state.json"), state).expect("state");
    std::fs::write(c.join("index.json"), r#"{"https://ex.com/u2":{"dir":"t/u2","t":1}}"#).expect("index");
    let (out, rc) = s.run("u1");
    assert_eq!(rc, 1, "{out}");
    assert!(out.starts_with("= u1 ! parse_failed · ex.com/u1 · pdf: "), "{out}");
    shape(&out);
    let (out, rc) = s.run("u2");
    assert_eq!(rc, 1, "{out}");
    assert!(out.starts_with("= u2 ! not_in_v1 zip (docx/xlsx)") && out.contains("→ unzip -p "), "{out}");
    shape(&out);
    let (out, _) = s.run("https://ex.com/u2");
    assert!(out.contains("! not_in_v1 zip"), "a cached zip URL is cached, not offline: {out}");
}

/// A local PDF: one section per page, the PDF's title, pages and read time on the `=` line, the empty page
/// counted in a `!` row; `<file.pdf> <words>` reads then filters; the cached raw.pdf rebuilds the same doc.
#[test]
fn local_pdf_reads_by_page() {
    let s = Sandbox::new("localpdf");
    // two pages: page 1 has text and an Info title, page 2 has none
    std::fs::write(s.dir.join("work/menu.pdf"), include_bytes!("fixtures/two-pages.pdf")).expect("pdf");
    let (out, rc) = s.run("./menu.pdf");
    assert_eq!(rc, 0, "{out}");
    let head = out.lines().next().unwrap_or_default();
    assert!(head.starts_with("= j1.u1 Tiny Test Doc · pdf · 2 pages · read "), "{out}");
    assert!(out.contains("u1 ! no_text 1 of 2 pages have no text layer (pages 2)"), "{out}");
    shape(&out);
    let (out, rc) = s.run("u1#1");
    assert_eq!(rc, 0, "{out}");
    assert!(out.contains("tuna tartare 23"), "{out}");
    let (out, rc) = s.run("./menu.pdf tuna");
    assert_eq!(rc, 0, "{out}");
    assert!(out.starts_with("= j2.u1 filter \"tuna\" · 1 hit"), "the same file again is a read of u1: {out}");
    assert!(out.contains("u1#1 ") && out.contains("23"), "{out}");
    shape(&out);
    assert!(!s.dir.join("cache/t/u2").exists(), "the same file again is u1's snapshot");
    std::fs::remove_file(s.dir.join("cache/t/u1/doc.json")).expect("doc.json");
    assert!(s.run("u1#1").0.contains("tuna tartare 23"), "rebuilt from raw.pdf");
}

/// A refused setting is a `!` row right after the `=` line (and in json `warnings`), and `config` names it.
#[test]
fn refused_settings_are_reported() {
    let s = Sandbox::new("settings");
    let o = s.cmd("config").env("FATS_CHUNK", "abc").env("FATS_JSON", "yes").output().expect("run");
    let out = String::from_utf8_lossy(&o.stdout);
    let rec: serde_json::Value = serde_json::from_str(out.trim()).expect("FATS_JSON=yes is honoured");
    let lines: Vec<&str> = rec["lines"].as_array().expect("lines").iter().filter_map(|v| v.as_str()).collect();
    assert!(lines[0].starts_with("= config") && lines[1].starts_with("! bad_setting FATS_CHUNK=abc"), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("chunk 300tok · default (refused FATS_CHUNK=abc)")), "{lines:?}");
    assert_eq!(rec["warnings"].as_array().map(Vec::len), Some(1));
}

/// Every accepted mod is honoured or named in a `!` row; `dry:true` tells the truth about a link follow.
#[test]
fn mods_are_honoured_or_named() {
    let s = Sandbox::new("mods");
    let links: String =
        (1..=30).map(|i| format!("<p>item {i} <a href=\"https://ex.com/p{i}\">go {i}</a></p>")).collect();
    s.file("l.html", &page("Links", &links));
    assert_eq!(s.run("./l.html").1, 0);
    let (out, _) = s.run("u1 links limit:10");
    assert_eq!(out.lines().filter(|l| l.starts_with('l')).count(), 10, "{out}");
    assert!(out.contains("+20 → fats u1 links from:11"), "{out}");
    let (out, _) = s.run("u1 links from:11 limit:10");
    assert!(out.lines().nth(1).is_some_and(|l| l.starts_with("l11 ")), "from: resumes after what was shown: {out}");
    assert!(s.run("u1 links from:0").0.starts_with("? from:0 · from takes a count"), "rows count from 1");
    assert!(
        s.run("u1 from:5")
            .0
            .contains("! ignored from:5 · only a links, cards, media or comments list pages → fats u1\n")
    );
    let (out, _) = s.run("u1 item limit:3");
    assert_eq!(out.lines().filter(|l| l.starts_with("u1#")).count(), 3, "{out}");
    let (out, rc) = s.run("u1 links max:40");
    assert_eq!(rc, 0);
    assert!(out.lines().last().is_some_and(|l| l.starts_with("… max:40 cut ") && l.contains("max:80")), "{out}");
    let (out, rc) = s.run("u1 since:7d");
    assert_eq!(rc, 0);
    assert!(out.lines().nth(1).is_some_and(|l| l.starts_with("! ignored since:7d") && l.ends_with("→ fats u1")));
    shape(&out);
    assert_eq!(s.run("u1 limit:abc").1, 2);
    let (out, _) = s.run("u1 l4 dry:true");
    assert!(out.contains("u1 l4 ex.com/p4 not cached → ! offline"), "{out}");
    assert!(!out.contains("local read"), "a link follow is not a local read: {out}");
}

/// `dry:true` names the route, the hedge, the country and the device a fetch would use; device:mobile is
/// Oxylabs-only: dry:true and config say so. An IP address host: a plan's free host lookup (`provider::host_check`)
/// never runs for one, so no resolver is asked (the country from a domain: `redact.rs` `dry_country_from_domain`).
#[test]
fn dry_names_route_country_device() {
    let s = Sandbox::new("dry");
    s.file("l.html", &page("Links", "<p>one stored page: the next new id is u2</p>"));
    assert_eq!(s.run("./l.html").1, 0);
    let o = s.cmd("https://1.1.1.1/ device:mobile dry:true").env_remove("FATS_OFFLINE").output().expect("run");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("· device:mobile (oxy, decodo; bd serves desktop)"), "{out}");
    let keyed = |cmd: &str| {
        let mut c = s.cmd(cmd);
        c.env_remove("FATS_OFFLINE").env("BRIGHTDATA_UNLOCKER_ZONE", "z");
        String::from_utf8_lossy(&c.output().expect("run").stdout).into_owned()
    };
    s.auth(&[("oxy", "x"), ("bd", "y")]);
    let out = keyed("https://1.1.1.1/ dry:true");
    // no country named, a gTLD: no `country:` row (oxy sends the home US, bd its own best: `config`)
    let ladder = "fetch plain → oxy (after a captcha) → bd ∥ oxyjs after 5s → new u2 · up to $0.0040\n";
    assert!(out.contains(ladder) && !out.contains("no_key"), "no row for a provider never set up: {out}");
    assert!(out.contains("\nhedge search 2.5 s · fetch 5 s · default\n"), "{out}");
    let out = keyed("https://1.1.1.1/ hedge:0 dry:true");
    assert!(
        out.contains("fetch plain → oxy (after a captcha) → bd ∥ oxyjs after 0s → new u2"),
        "hedge:0 parses and races: {out}"
    );
    // a paid ask: one provider at a time, bd first, the next only on its failure
    let out = keyed("https://1.1.1.1/ js:true dry:true");
    assert!(
        out.contains("fetch bd → oxyjs → new u2 · up to $0.0030 · ≈25s · hedge:off, one provider at a time"),
        "{out}"
    );
    s.auth(&[("oxy", "x")]);
    let o = s
        .cmd("https://1.1.1.1/a https://1.1.1.1/b country:pl dry:true")
        .env_remove("FATS_OFFLINE")
        .output()
        .expect("run");
    let out = String::from_utf8_lossy(&o.stdout);
    // one key: every rung of that provider; no retry: 14 s of the 90 s wall < oxyjs' 60 s
    assert!(out.contains("fetch plain → oxy → oxyjs → new u2 · up to $0.0025 · country:PL\n"), "{out}");
    let cmd = "https://1.1.1.1/pricing country:fr dry:true";
    let o = s.cmd(cmd).env_remove("FATS_OFFLINE").output();
    let out = String::from_utf8_lossy(&o.expect("run").stdout).into_owned();
    assert!(out.contains("country:FR") && !out.contains("! ignored"), "a page fetch honours country: {out}");
    s.auth(&[]);
    let out = s.run("gh rust country:fr dry:true").0;
    assert!(
        out.contains("! ignored country:fr · only a web search or a page fetch uses it"),
        "GitHub has no country: {out}"
    );
    let o = s.cmd("https://1.1.1.1/ js:true dry:true").env_remove("FATS_OFFLINE").output().expect("run");
    let out = String::from_utf8_lossy(&o.stdout);
    // what each key would buy here, its price, and where to get it
    let bd = "! no_key bd · would try unblock+JS ≤$0.0015 → run fats setup bd, give the user its 3 lines";
    let oxy = "! no_key oxy · would try JS render ≤$0.0015 → run fats setup oxy, give the user its 3 lines";
    // the run stores its failure in an id all the same: dry names it
    let row = out.split("nothing can fetch it · new u").nth(1).and_then(|r| r.split_once(' ')).map(|r| r.1);
    assert!(row.is_some_and(|r| r.starts_with(&format!("· $0 · {bd} · {oxy} · ! no_key decodo"))), "{out}");
    assert!(s.run("config").0.contains("device desktop · default · device:mobile (oxy, decodo)"));
    // The exit country of a paid call that names none, and where it came from
    let o = s.cmd("config").env("FATS_COUNTRY", "fr").output().expect("run");
    let row = "\ncountry country:, else the domain's ccTLD (24 European, .uk → GB), else FR · env \
               FATS_COUNTRY · country:\n";
    assert!(String::from_utf8_lossy(&o.stdout).contains(row), "{o:?}");
    let us = "else US on oxy (bd: its best for the site) · default · country:\n";
    assert!(s.run("config").0.contains(us), "a Bright Data page names no default country");
}

/// Exact hits first, then the all-words (stemmed) hits after them: an exact hit never hides an inflected one
/// (`raided` exact in #3 only, the answer `70 raids` in #2 and #4; each line also hits as its
/// title's carried context).
#[test]
fn exact_hits_then_inflected_ones() {
    let s = Sandbox::new("exactall");
    let body = "<h2>Operation</h2><p>Police carried out 70 raids, dozens of people detained.</p>\
                <h2>Raided</h2><p>Raided offices: computers and SIM cards were seized.</p>\
                <h2>Europol</h2><p>50 people detained and 70 raids carried out.</p>";
    s.file("k.html", &page("Fraud", body));
    assert_eq!(s.run("./k.html").1, 0);
    let out = s.run("u1 raided").0;
    let rows: Vec<&str> = out.lines().skip(1).map(|l| l.split(' ').next().unwrap_or_default()).collect();
    // one row a section: the title row of #3 is its line's hit, #2's and #4's carry their line's hit
    assert!(out.starts_with("= j1.u1 filter \"raided\" · 3 hit · match:exact+all auto"), "{out}");
    assert_eq!(rows[..3], ["u1#3", "u1#2", "u1#4"], "exact first: {out}");
    assert!(s.run("u1 raids").0.contains("2 hit · match:exact · "), "no all-words rows beyond the exact ones");
}

/// The most-words rung is never emptied by a floor: 1 of 4 words still shows its rows, labelled `match:1/4`
/// (`director prezes company` matched only on the decisive `prezes` row, 0 hit before);
/// 0 hit = no word anywhere. A weak served row yields to an island that holds all the words.
#[test]
fn weak_matches_show_the_best_rows() {
    let s = Sandbox::new("weak");
    s.file("w.html", &page("Fuel", "<p>Diesel costs 1.52 today</p><p>Petrol station map</p>"));
    assert_eq!(s.run("./w.html").1, 0);
    let (out, rc) = s.run("u1 cheapest portuguese diesel proxy");
    assert_eq!(rc, 0);
    assert!(
        out.starts_with("= j1.u1 filter \"cheapest portuguese diesel proxy\" · 1 hit · match:1/4 words auto"),
        "{out}"
    );
    assert!(out.contains("Diesel costs 1.52 today"), "{out}");
    assert!(s.run("u1 cheapest diesel today proxy").0.contains("· match:2/4 words auto"));
    assert!(s.run("u1 cheapest portuguese diesel proxy match:words").0.contains("1 hit · match:1/4 words"));
    let none = s.run("u1 kerosene airport").0;
    assert!(
        none.starts_with("= j1.u1 filter \"kerosene airport\" · 0 hit · tried exact, all words, any word · outline")
    );
    let rek =
        "<table><tr><td>Kod firmy</td><td>304512345</td></tr><tr><td>Prezes</td><td>Jan Kowalski</td></tr></table>";
    s.file("r.html", &page("Przykład sp. z o.o.", rek));
    s.run("./r.html");
    assert!(s.run("u2 director prezes company").0.contains("Jan Kowalski"), "one decisive word");
    assert!(s.run("u2 company's registration code kod firmy").0.contains("304512345"), "two of five words");
    let isl = format!(
        "<p>Fuel prices</p><template><p>{}</p><p>diesel prices at the airport pump</p></template>",
        "x ".repeat(20)
    );
    s.file("i.html", &page("Pump", &isl));
    s.run("./i.html");
    let out = s.run("u3 diesel airport pump").0;
    assert!(out.contains("match:all auto · in islands"), "an island with all words beats a weak served row: {out}");
}

/// A bare `more` goes on with what the last read showed: the next part of `full`, of a section, else every
/// section (`all`); after another page's list it is that page's.
#[test]
fn bare_more_goes_on() {
    let s = Sandbox::new("baremore");
    let para = "<p>".to_string() + &"a long line of words that fills the reader ".repeat(40) + "</p>";
    s.file("long.html", &page("Long", &format!("<h2>One</h2>{}<h2>Two</h2>{para}", para.repeat(6))));
    assert_eq!(s.run("./long.html").1, 0);
    assert!(s.run("u1 full").0.starts_with("= j1.u1 full · part 1/7 "));
    assert!(s.run("more").0.starts_with("= j1.u1 full · part 2/7 "), "full goes on");
    let sec = s.run("u1#1").0;
    assert!(sec.starts_with("= j1.u1#1 Long · part 1/7 "), "{sec}");
    assert!(s.run("more").0.starts_with("= j1.u1#1b Long · part 2/7 "), "the section goes on");
    s.file("short.html", &page("Short", "<p>short</p>"));
    s.run("./short.html");
    let all = s.run("more").0;
    assert!(all.starts_with("= j2.u2 ") && all.contains("short"), "the current page: {all}");
}

/// The agent's word for the media list: `images` that hits no text points to `media`; a word
/// that is not a media word gets no such row; `media` pages the list by `from:N`, which the hint
/// names, so it goes on after a list of another page; the last page says so.
#[test]
fn media_words_point_to_media() {
    let s = Sandbox::new("mediaw");
    let pics: String = (1..=200).map(|i| format!("<img src='/p/photo-{i:03}a.jpg' alt='photo {i:03}'>")).collect();
    s.file("m.html", &page("Pics", &format!("<p>Some text here</p>{pics}")));
    assert_eq!(s.run("./m.html").1, 0);
    let out = s.run("u1 images").0;
    assert!(out.contains(" · 0 hit · ") && out.contains("\n→ fats u1 media"), "{out}");
    assert!(!s.run("u1 zebra").0.contains("→ fats u1 media"));
    let first = s.run("u1 media").0;
    let more = first.lines().last().unwrap_or_default();
    let n = first.lines().filter(|l| l.starts_with('m')).count();
    assert!(more.starts_with('+') && more.ends_with(&format!(" → fats u1 media from:{}", n + 1)), "{first}");
    s.file("o.html", &page("Other", &format!("<p>Other text</p>{pics}")));
    assert_eq!(s.run("./o.html").1, 0);
    assert!(s.run("u2 media").0.ends_with(&format!("{more}\n").replace("u1", "u2")), "the other list");
    let next = s.run(&format!("u1 media from:{}", n + 1)).0;
    assert!(next.lines().nth(1).unwrap_or_default().starts_with(&format!("m{} ", n + 1)), "{next}");
    let last = no_drive(&s.run("u1 media from:200").0);
    assert!(
        last.starts_with("= ")
            && last.ends_with("m200 photo jpg | photo 200 → file:///p/photo-200a.jpg\n(end of media, 200)\n"),
        "{last}"
    );
}

/// A thread by its structure: an indent table (a spacer's `width` = depth) and nested items with a served
/// `<time>` are comments, offered in the outline's last line; `comments` lists the top level, most replies
/// first, a row its count and first words past the byline; a row's id opens the comment: its direct replies,
/// then its text; `full` reads its whole tree, a reply indented under its parent. A side list of dated cards,
/// a short comment and a heading inside one change no count. Flat dated posts are a feed, never a thread: no
/// count, and `comments` says none is served; so is a live blog whose only second top was a pruned side card.
#[test]
fn a_thread_lists_its_top_comments() {
    let s = Sandbox::new("threads");
    for f in
        ["thread-indent-table", "thread-nested-list", "thread-flat-counts", "thread-article-items", "thread-liveblog"]
    {
        let f = format!("{f}.html");
        s.file(&f, &std::fs::read_to_string(format!("tests/fixtures/{f}")).expect("fixture"));
        assert_eq!(s.run(&format!("./{f}")).1, 0, "{f}");
    }
    let u1 = s.run("u1").0;
    assert!(u1.contains(" · 12 comments served · list: fats u1 comments · filter: "), "the verb is offered: {u1}");
    let top = s.run("u1 comments").0;
    let rows: Vec<String> = top.lines().map(|l| l.split(" · ").take(2).collect::<Vec<_>>().join(" ")).collect();
    assert!(top.starts_with("= j1.u1 12 comments served · 5 top-level · open one: fats u1#<k>"), "{top}");
    assert_eq!(
        rows[1..],
        [
            "u1#2 replies 3", "u1#9 replies 3", "u1#6 replies 1", "u1#8 replies 0", "u1#13 replies 0",
            "(end of comments, 5)"
        ]
    );
    assert!(top.contains("\nu1#2 · replies 3 · u1 1 hours ago Comment 1: this comment says"), "{top}");
    let sub = s.run("u1#9").0;
    assert!(sub.starts_with("= j1.u1#9 replies 3 · part 1/1 · up fats u1 · next fats u1#13 · "), "{sub}");
    let direct: Vec<&str> = sub.lines().skip(1).take(4).map(|l| l.split(" · ").next().unwrap_or_default()).collect();
    assert_eq!(
        direct,
        [
            "u1#11",
            "u1#10",
            "(end of comments, 2)",
            "|  | u8 8 hours ago Comment 8: this comment says one thing in a sentence long enough to be read as prose. |"
        ],
        "direct replies, their end, then its text: {sub}"
    );
    assert!(sub.contains("Comment 8:") && !sub.contains("Comment 12:") && !sub.contains("Comment 7:"), "{sub}");
    let full = s.run("u1#9 full").0;
    let tree: Vec<&str> = full.lines().filter(|l| l.contains("replies")).collect();
    assert_eq!(tree, ["[u1#9] replies 3", "  [u1#10] replies 0", "  [u1#11] replies 1", "    [u1#12] replies 0"]);
    let list = s.run("u2 comments").0;
    assert!(
        list.starts_with("= j2.u2 11 comments served · 4 top-level") && list.contains("\nu2#7 · replies 4 · "),
        "{list}"
    );
    assert!(s.run("u2 comments limit:1").0.contains("\n+3 → fats u2 comments from:2\n"));
    assert_eq!(s.run("u1 comments limit:20").0.lines().count(), 7, "every row asked fits: no cap below `limit:`");
    assert!(s.run("u1 comments limit:25").0.contains("\n! ignored limit:25 · max 20\n"), "rows stay ≤ 20");
    let flat = s.run("u3").0;
    assert!(!flat.contains("comments"), "a feed: {flat}");
    assert!(s.run("u3 comments").0.starts_with("= j3.u3 comments · no thread found · "), "a feed is no thread");
    let blog = s.run("u4 comments").0;
    assert!(blog.starts_with("= j4.u4 12 comments served · 4 top-level"), "no side card: {blog}");
    assert!(blog.contains("\nu4#7 · replies 2 · u5 Oct 5 Hi 5\n"), "a short comment keeps its replies: {blog}");
    assert!(blog.contains("\nu4#3 · replies 3 · Comment 1: this comment"), "no byline: {blog}");
    assert!(blog.contains("\nu4#10 · replies 0 · Heading inside 8 Comment 8"), "its heading is its text: {blog}");
    let thoughts = s.run("u4#2").0; // a heading over the thread: each comment's id, indented by its depth
    assert!(thoughts.contains("\n#3 replies 3\nu1 Oct 1\n") && thoughts.contains("\n  #4 replies 1\n"), "{thoughts}");
    assert!(!s.run("u5").0.contains("comments"), "a live blog is no thread");
    assert!(s.run("u5 comments").0.contains(" comments · no thread found · "));
    let leaf = s.run("u1#10 comments").0;
    assert!(leaf.starts_with("= j1.u1#10 comments · no replies · list: fats u1 comments · "), "{leaf}");
}

/// A few dated comments under the thread bar (`thread_rows`) are no thread, and `comments` says so without denying
/// them: it points at the page's sections, where they are served as text.
#[test]
fn a_short_thread_is_never_denied() {
    let s = Sandbox::new("short-thread");
    let post = |i: u8| {
        format!(
            "<article><a href=/u/{i}>u{i}</a> <time datetime=2026-10-0{i}>Oct {i}</time><p>Comment {i}: \
                 this one says a single thing in a sentence long enough to be read as prose.</p></article>"
        )
    };
    let three = format!(
        "<html><head><title>Short talk</title></head><body><h1>Short talk</h1>{}</body></html>",
        (1..=3).map(post).collect::<String>()
    );
    s.file("three.html", &three);
    let page = s.run("./three.html").0;
    assert!(page.contains("Comment 3: this one says") && !page.contains("comments served"), "{page}");
    let none = s.run("u1 comments").0;
    assert!(none.starts_with("= j1.u1 comments · no thread found · 1 sections: fats u1 · "), "{none}");
}

/// A thread that states a bigger size than it serves says so on every count of the whole thread, never a bare N.
#[test]
fn a_thread_names_the_comments_it_lacks() {
    let s = Sandbox::new("said");
    // the page states 40 (the last count before its first comment; a `comment-id` is no count): never a bare 12
    let blog = std::fs::read_to_string("tests/fixtures/thread-article-items.html").expect("fixture");
    let said = r#"<body><p data-comment-count="40">40 comments</p><div data-comment-id="99999"></div>"#;
    s.file("said.html", &blog.replacen("<body>", said, 1));
    let u1 = s.run("./said.html").0;
    assert!(u1.contains(" · 12 of 40 comments served (rest not in this copy) · list: fats u1 comments · "), "{u1}");
    let rest = s.run("u1 comments").0;
    assert!(rest.starts_with("= j1.u1 12 of 40 comments served (rest not in this copy) · 4 top-level"), "{rest}");
    assert!(
        s.run("u1#4 comments").0.starts_with("= j1.u1#4 3 comments served · 2 direct"),
        "one comment's replies: no `of`"
    );
    assert!(s.run("u1#0").0.contains("\ncomments: 40"), "the stated size is in the page's info");
}

/// The post's own counts are in the first answer: the first element with a count attribute is the post, its score
/// and time come from it alone, never from a comment (here in a `<template>`) nor from a thread the copy lacks.
#[test]
fn the_first_answer_names_the_posts_counts() {
    let s = Sandbox::new("postcounts");
    let post =
        r#"<body><x-post comment-count="296" score="26" created-timestamp="2025-12-11T09:49:02.123+0000"></x-post>"#;
    let comment =
        r#"<x-comment score="90" comment-count="4" created-timestamp="2025-12-13"><p>A reply</p></x-comment>"#;
    let page =
        include_str!("fixtures/reddit-thin-template.html").replacen("<body>", post, 1).replace("{comments}", comment);
    s.file("post.html", &page);
    let u1 = s.run("./post.html").0;
    assert!(u1.contains("\nu1#0 info | comments 296 · score 26 · posted 2025-12-11\n"), "{u1}");
    let info = s.run("u1#0").0;
    assert!(info.contains("\ncomments: 296\n") && info.contains("\nscore: 26\n") && !info.contains("90"), "{info}");
}

/// A picture in an ad slot is listed, not counted: a tag, class or id word `ad`/`ads`/`advertisement` on a
/// short card with no number. `advert` (a classifieds site's own name for a listing) and a card with a price keep
/// their photo.
#[test]
fn ad_slot_photo_is_listed_not_counted() {
    let s = Sandbox::new("adslot");
    let pic = |f: &str| format!("<img src='/p/{f}.jpg' alt='{f} photo' width=800 height=600>");
    let card = |cls: &str, f: &str, text: &str| format!("<div class='{cls}'>{}<p>{text}</p></div>", pic(f));
    let cards = [
        card("ad-slot", "promo", "Try our new app today"),
        format!("<x-ad-post>{}<p>Join the best community</p></x-ad-post>", pic("post")),
        card("advert-listing", "flat", "Flat in the old town"),
        card("ads", "bike", "Bike for sale, 120 €"),
    ];
    s.file("a.html", &page("Listings", &format!("<p>Some text here</p>{}", cards.concat())));
    let out = s.run("./a.html").0;
    assert!(out.lines().next().unwrap_or_default().contains(" · 2 photos"), "{out}");
    let media = no_drive(&s.run("u1 media").0);
    for f in ["promo", "post"] {
        assert!(media.contains(&format!("(not counted) | {f} photo → file:///p/{f}.jpg")), "{media}");
    }
    for f in ["flat", "bike"] {
        assert!(media.contains(&format!("800x600 jpg | {f} photo → file:///p/{f}.jpg")), "{media}");
    }
}

/// A page read by id is a cache read: its `=` line names the age; `fresh:true` re-reads the source as a new id.
#[test]
fn cached_reads_show_their_age() {
    let s = Sandbox::new("age");
    s.file("a.html", SECTIONS);
    let (out, _) = s.run("./a.html");
    assert!(out.lines().next().is_some_and(|l| l.contains("local ") && !l.contains(" · cache ")), "{out}");
    for cmd in ["u1", "u1#1", "u1 links", "u1 alpha"] {
        let (out, _) = s.run(cmd);
        assert!(out.lines().next().is_some_and(|l| l.contains(" · cache ") && !l.contains("spent")), "{cmd}: {out}");
    }
    let (out, rc) = s.run("u1 fresh:true");
    assert_eq!(rc, 0);
    assert!(out.starts_with("= j2.u2 ") && !out.contains(" · cache "), "a refetch is a new id: {out}");
    assert_eq!(s.run("u9 fresh:true").1, 2, "an unknown id is a question, not 'cached'");
}

/// A whole page's `=` figure is what lands in context, its `=` line and rows, with the line's HOW (`local 1ms`,
/// `cache 0s`) counted at a fixed width: the fetch, a second fetch, the cache reads and the `stats` row print one
/// size (it was 716 or 717 by the ms digits, 161 vs 150 on a cache read, and `stats` printed the page's size), and
/// it is within ±10% of the printed bytes' own count.
#[test]
fn a_whole_read_prints_one_size() {
    let size = |out: &str, line: usize| {
        let row = out.lines().nth(line).unwrap_or_default();
        row.split(" · ").find(|g| g.ends_with("tok")).map(String::from).unwrap_or_else(|| panic!("{out}"))
    };
    let small = "<title>Example Domain</title><h1>Example Domain</h1><p>This domain is for use in illustrative \
        examples in documents. You may use this domain in literature without prior coordination.</p>";
    let shop = std::fs::read_to_string("tests/fixtures/cards-products.html").expect("fixture");
    for page in [small, &shop] {
        let s = Sandbox::new("size");
        s.file("p.html", page);
        let first = s.run("./p.html").0;
        let reads = [first.clone(), s.run("u1").0, s.run("./p.html").0, s.run("j2.u1").0];
        let mut sizes: Vec<String> = reads.iter().map(|out| size(out, 0)).collect();
        sizes.push(size(&s.run("stats").0, 1));
        assert!(sizes.iter().all(|x| *x == sizes[0]), "{sizes:?}");
        let figure = sizes[0].trim_end_matches("tok").parse::<f64>().expect("a figure");
        let real = fats::clean::tok(&first) as f64;
        assert!((figure - real).abs() <= 0.1 * real, "{figure} vs {real} for\n{first}");
    }
}

/// No `FATS_SESSION`: one session per user, so two invocations from different directories (two shells, two
/// tasks) read the same ids, and a bare `uN` from an older job says how stale it is.
#[test]
fn one_session_per_user() {
    let s = Sandbox::new("session");
    s.file("a.html", SECTIONS);
    s.file("b.html", &page("Bee", "bee hive honey"));
    let other = s.dir.join("elsewhere");
    std::fs::create_dir_all(&other).expect("dir");
    let run = |dir: &std::path::Path, args: &str| {
        let o = s.cmd(args).env_remove("FATS_SESSION").current_dir(dir).output().expect("run");
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let work = s.dir.join("work");
    assert!(run(&work, "./a.html").starts_with("= j1.u1 "));
    assert!(run(&work, "./b.html").starts_with("= j2.u2 "));
    let out = run(&other, "u1");
    assert!(out.starts_with("= j1.u1 ") && out.contains("(1 job ago; newest j2)"), "{out}");
    assert!(!run(&other, "j1.u1").contains("job ago"), "a named job is not stale");
    assert!(!run(&other, "u2").contains("ago;"), "the newest page is not stale");
    assert!(run(&other, "status").contains("= s:default "));
}

/// `<noscript>` is dropped only when meta says the body is a browser render, whichever provider stored it.
#[test]
fn noscript_follows_the_render_flag() {
    let s = Sandbox::new("render");
    let body = page("R", "<p>visible text</p><noscript>enable javascript please</noscript>");
    s.file("r.html", &body);
    assert!(s.run("./r.html").0.contains("enable javascript"), "a local file is not a render");
    let c = s.dir.join("cache/t");
    for (u, render, prov) in [("u2", "true", "plain"), ("u3", "false", "oxylabs")] {
        std::fs::create_dir_all(c.join(u)).expect("dir");
        std::fs::write(c.join(u).join("raw.html"), &body).expect("raw");
        let meta = format!(r#"{{"url":"https://ex.com/{u}","provider":"{prov}","t":1,"render":{render}}}"#);
        std::fs::write(c.join(u).join("meta.json"), meta).expect("meta");
    }
    let st: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(c.join("state.json")).expect("state")).expect("state json");
    let mut u = st["u"].as_array().expect("u").clone();
    u.push(serde_json::json!({"url": "https://ex.com/u2", "dir": "t/u2"}));
    u.push(serde_json::json!({"url": "https://ex.com/u3", "dir": "t/u3"}));
    std::fs::write(c.join("state.json"), serde_json::json!({ "u": u }).to_string()).expect("state");
    assert!(!s.run("u2").0.contains("enable javascript"), "render:true drops noscript");
    assert!(s.run("u3").0.contains("enable javascript"), "render:false keeps it, even from oxylabs");
}

/// `thin`: the post is served, the replies sit in a `<template>` and a JSON island (one serialised twice); the
/// `=` line says how much the bytes hold. Ids, URLs and keys are no prose; an article and the fixtures never fire.
/// A file has no JS render and no island holds its words, so nothing gets more: a COUNT, never a lone `!`.
#[test]
fn thin_page_says_what_the_bytes_hold() {
    let s = Sandbox::new("thin");
    let reply = "<p>I tried this on three sites and it held up well.</p>".repeat(40);
    let store =
        serde_json::json!({ "c": "a reply that says the same thing again".repeat(30), "id": "t3_x".repeat(500) });
    let island = serde_json::json!({ "store": store.to_string(), "url": "https://ex.com/".repeat(300) });
    let body =
        format!("<p>Does it work?</p><template>{reply}</template><script type=application/json>{island}</script>");
    s.file("thread.html", &page("Thread", &body));
    let head = s.run("./thread.html").0.lines().next().unwrap_or_default().to_string();
    assert!(head.starts_with("= j1.u1 Thread · ") && head.ends_with(" · thin 8 of ~1.5k visible"), "{head}");
    assert!(!head.contains('!'), "no next step, no `!`: {head}");
    assert!(s.run("u1#1").0.lines().next().is_some_and(|l| l.ends_with(" · thin 8 of ~1.5k visible")));
    // a thin page: its filter reads the islands too, hits there said to be there
    let hit = s.run("u1 three sites").0;
    assert!(hit.contains(" hit · match:exact · in islands") && hit.contains("\nu1#2 I tried this on three"), "{hit}");
    // never the biggest island; a file has no JS render, so no `→` at all
    let data = serde_json::json!({ "c": "a reply that says the same thing again".repeat(80) });
    s.file("data.html", &page("Data", &format!("<p>Does it work?</p><script type=application/json>{data}</script>")));
    let head = s.run("./data.html").0.lines().next().unwrap_or_default().to_string();
    assert!(head.contains(" · thin ") && !head.contains('→') && !head.contains('!'), "{head}");
    s.file("post.html", &page("Post", &format!("<article>{reply}</article><template>{reply}</template>")));
    assert!(!s.run("./post.html").0.contains(" thin "), "an article served whole");
    for name in ["price-sup-digits.html", "price-rows.html"] {
        let fixture = std::fs::read_to_string(format!("tests/fixtures/{name}")).expect("fixture");
        s.file(name, &fixture);
        assert!(!s.run(&format!("./{name}")).0.contains(" thin "), "{name}");
    }
    // a served post, its replies in a template (Reddit): not thin, the `=` line counts the island; a
    // reply word is not in the post, so the islands answer and say so; a post word stays in the post
    let post = "<p>The post says the hinge held for a year of daily use. </p>".repeat(30);
    s.file("reddit.html", &page("Thread", &format!("<article>{post}</article><template>{reply}</template>")));
    let out = s.run("./reddit.html").0;
    assert!(out.starts_with("= j6.u6 Thread · ") && out.contains(" sections +1 island · ") && !out.contains("thin"));
    let hit = s.run("u6 three sites").0;
    assert!(hit.contains(" hit · match:exact · in islands") && hit.contains("\nu6#3 I tried this on three"), "{hit}");
    let own = s.run("u6 hinge held").0;
    assert!(own.contains(" hit · match:exact") && !own.contains("in islands") && !own.contains("u6#3"), "{own}");
}

/// A listing (a property search, a product grid, a job board: tests/fixtures/cards-*.html): each repeated block
/// is ONE row, `▸ title [lN] · pieces`, so a filter hit inside one is the whole card with its link; the
/// outline counts them and `fats uN cards [<words>]` lists them; prose stays lines.
#[test]
fn listing_cards_are_rows() {
    let s = Sandbox::new("cards");
    for f in ["cards-property", "cards-products", "cards-jobs"] {
        s.file(&format!("{f}.html"), &std::fs::read_to_string(format!("tests/fixtures/{f}.html")).expect("fixture"));
    }
    let out = s.run("./cards-property.html").0;
    assert!(out.contains("· links: fats u1 links · cards: fats u1 cards\n"), "{out}");
    assert!(
        out.contains("\n▸ Flat in Oak Street, Northtown [l1] · 1/12 · 850 €/month · 3 rooms 85 m² 1st floor · "),
        "{out}"
    );
    // a card is a shape, not a size: a 145-word teaser keeps it a card; the list stops before the teaser and a
    // hit in it brings the card's title, link and price first
    let list = s.run("u1 cards").0;
    let park = " ▸ Flat in Park Row, Westbury [l5] · 1/16 · 890 €/month · 3 rooms 90 m² 1st floor";
    assert!(list.contains(" cards 5 ") && list.contains(&format!("{park}\n")), "{list}");
    let hit = s.run("u1 conservatory").0;
    assert!(hit.contains(&format!("{park} …")) && hit.contains("heated conservatory"), "{hit}");
    // a hit on the specs is the whole card: its title and link come with it
    let hit = s.run("u1 110 m²").0;
    assert!(
        hit.contains(" 1 hit ") && hit.contains("\nu1#2 ▸ Flat in Hill View, Eastfield [l4] · 1/15 · 1100 €/month"),
        "{hit}"
    );
    // a list word alone names the page it needs
    assert_eq!(s.run("cards").0, "? cards needs a page: fats u1 cards\n");
    let out = s.run("./cards-products.html").0;
    assert_eq!(out.matches("\n▸ ").count(), 6, "{out}");
    assert!(
        out.contains("\nOur 1 guide from 2019 explains the care steps [l7] in plain words"),
        "prose is no card: {out}"
    );
    assert!(s.run("u2 all").0.contains("\nu2#2 Results (283) | ▸ Steel water bottle 750 ml [l1]"));
    let mug = s.run("u2 cards mug").0;
    assert!(
        mug.contains(" cards 1 of 6 \"mug\" ")
            && mug.contains("\nu2#2 ▸ Insulated coffee mug 350 ml [l3] · 4.7 out of 5 (2,940) · $15.00"),
        "{mug}"
    );
    assert!(s.run("u2 cards limit:2").0.ends_with("\n+4 → fats u2 cards from:3\n"));
    s.run("./cards-jobs.html");
    let jobs = s.run("u3 cards Cork").0;
    assert!(
        jobs.contains(
            "\nu3#1 ▸ Rust backend engineer [l1] · Ferrous Ltd · Cork · 4,000–5,500 EUR · Posted 2 days ago\n"
        ),
        "{jobs}"
    );
    assert!(jobs.contains(" cards 2 of 5 ") && jobs.ends_with("(end of cards Cork, 2)\n"), "{jobs}");
    // a `limit:` past `list_rows` is named; the token cap may cut it further, and the pager says where it goes on
    let li = |i| {
        format!(
            "<li><h3><a href=https://ex.com/i/{i}>Flat {i} on a long street with a long name</a></h3>\
        <p>{i}00 € · three rooms · ninety square metres · first floor · balcony · lift · parking</p></li>"
        )
    };
    s.file("many.html", &format!("<ul>{}</ul>", (10..90).map(li).collect::<String>()));
    s.run("./many.html");
    let many = s.run("u4 cards limit:80").0;
    let cut = many.lines().filter(|l| l.starts_with("u4#")).count();
    assert!((1..=20).contains(&cut) && many.contains("\n! ignored limit:80 · max 20\nu4#1 "), "{many}");
    assert!(many.ends_with(&format!("+{} → fats u4 cards from:{}\n", 80 - cut, cut + 1)), "{many}");
}

/// `fats uN links <words>` keeps the links whose URL (percent-decoded) and anchor text hold every word, and shows
/// that text; a link around a card's blocks (tests/fixtures/cards-property.html) has their text as its anchor.
#[test]
fn links_filter_by_url_and_anchor() {
    let s = Sandbox::new("links");
    let page = std::fs::read_to_string("tests/fixtures/cards-property.html").expect("fixture");
    s.file("cards-property.html", &page);
    s.run("./cards-property.html");
    let links = s.run("u1 links flat 1003").0;
    assert!(
        links.contains(" links 1 of 6 \"flat 1003\" ")
            && links.contains("\nl4 homes.example/flat/1003/ · Flat in Hill View, Eastfield\n"),
        "{links}"
    );
    assert!(s.run("u1 links señorial").0.contains(" links 1 of 6 \"señorial\" "), "the URL is matched decoded");
    assert!(s.run("u1 links flat").0.ends_with("(end of links flat, 5)\n"));
}

/// An unwritable cache is one `! cache_write` record with the fix, never a panic or a half-stored page.
#[cfg(unix)]
#[test]
fn unwritable_cache_is_a_record() {
    let s = Sandbox::new("rocache");
    s.file("a.html", SECTIONS);
    std::fs::create_dir_all(s.dir.join("cache")).expect("cache dir");
    std::fs::set_permissions(s.dir.join("cache"), std::os::unix::fs::PermissionsExt::from_mode(0o500)).expect("ro");
    let (out, rc) = s.run("./a.html");
    std::fs::set_permissions(s.dir.join("cache"), std::os::unix::fs::PermissionsExt::from_mode(0o700)).expect("rw");
    assert_eq!(rc, 1, "{out}");
    assert!(out.starts_with("= ! cache_write") && out.contains("→ fats config"), "{out}");
    shape(&out);
}

/// `FATS_CACHE` is made absolute once at start (a relative dir is the cwd's, `config` and status show where);
/// `~/x` (no shell expanded it: an MCP JSON `env` block), `/`, `$HOME`, a dir fats cannot make (a host started in
/// `/`) and one in the key dir (a sweep deletes in it) are refused for the default, one `! bad_setting` row, never
/// a literal `~` dir.
#[test]
fn the_cache_root_is_absolute_or_refused() {
    let s = Sandbox::new("relroot");
    let config = |v: &str| {
        String::from_utf8_lossy(&s.cmd("config").env("FATS_CACHE", v).output().expect("run").stdout).into_owned()
    };
    let out = config("c");
    // the cwd as the OS hands it to the run: Unix resolves links (macOS `/var` → `/private/var`), Windows keeps the
    // name it was given (`RUNNER~1`, no `\\?\` prefix)
    let cwd = s.dir.join("work");
    #[cfg(unix)]
    let cwd = cwd.canonicalize().expect("cwd");
    let abs = cwd.join("c");
    assert!(out.contains(&format!("\ncache {} · env FATS_CACHE · swept\n", abs.display())), "{out}");
    let status = s.cmd("").env("FATS_CACHE", "c").output().expect("run").stdout;
    assert!(String::from_utf8_lossy(&status).contains(&abs.display().to_string()), "status names it too");
    // the home it refuses: `$HOME` on Unix (the sandbox's); on Windows the profile folder, which no env moves (the
    // runner's own: refused by its name before any write)
    let home = if cfg!(windows) { dirs::home_dir().expect("home") } else { s.dir.join("home") };
    let (home, keys) = (home.display().to_string(), s.dir.join("keys/x").display().to_string());
    // a dir under a file: no OS can make it (Windows makes `/dev/null/c` as `C:\dev\null\c`)
    s.file("f", "a file");
    let under_file = s.dir.join("work").join("f").join("c").display().to_string();
    for bad in ["~/x", &under_file, "/", &home, &keys] {
        let out = config(bad);
        assert!(out.contains(&format!("! bad_setting FATS_CACHE={bad} · want a dir fats can make")), "{out}");
        assert!(out.contains("\ncache ") && out.contains(" · default · swept\n"), "{out}");
    }
    assert!(!s.dir.join("work/~").exists(), "no literal ~ dir");
}

/// A page dir outside the cache (`<sandbox>/outside`, a stored page) and session `t`'s state.json naming `dirs`.
fn escape_from(s: &Sandbox, dirs: &[&str]) -> PathBuf {
    let outside = s.dir.join("outside");
    std::fs::create_dir_all(&outside).expect("outside");
    std::fs::write(outside.join("raw.html"), SECTIONS).expect("raw");
    std::fs::write(outside.join("meta.json"), r#"{"url":"https://ex.com/","t":1}"#).expect("meta");
    let u: Vec<_> = dirs.iter().map(|d| serde_json::json!({"url": "https://ex.com/", "dir": d})).collect();
    std::fs::create_dir_all(s.dir.join("cache/t")).expect("session");
    std::fs::write(s.dir.join("cache/t/state.json"), serde_json::json!({ "u": u }).to_string()).expect("state");
    outside
}

/// Each id reads `snapshot_missing` (rc 1) and nothing is written outside the cache.
fn refused(s: &Sandbox, outside: &std::path::Path, ids: &[&str]) {
    for id in ids {
        let (out, rc) = s.run(id);
        assert_eq!(rc, 1, "{out}");
        assert!(out.starts_with(&format!("= {id} ! snapshot_missing")), "{out}");
    }
    assert!(!outside.join("doc.json").exists(), "nothing is written outside the cache");
}

/// A tampered state.json cannot point a read (or the doc.json it writes) outside the cache root: not by `../`,
/// not by an absolute dir (on Windows one that names a drive).
#[test]
fn cache_paths_stay_inside_the_root() {
    let s = Sandbox::new("escape");
    let abs = s.dir.join("outside").display().to_string();
    let outside = escape_from(&s, &["../outside", &abs]);
    refused(&s, &outside, &["u1", "u2"]);
}

/// Nor by a symlink: a page dir inside the cache that links outside it is not read, a session dir that links out
/// of the cache is not written to.
#[cfg(unix)]
#[test]
fn cache_symlinks_stay_inside_the_root() {
    let s = Sandbox::new("escape-link");
    let outside = escape_from(&s, &["t/u1"]);
    std::os::unix::fs::symlink(&outside, s.dir.join("cache/t/u1")).expect("symlink");
    refused(&s, &outside, &["u1"]);
    let o2 = s.dir.join("outside2");
    std::fs::create_dir_all(&o2).expect("outside2");
    std::os::unix::fs::symlink(&o2, s.dir.join("cache/linked")).expect("symlink");
    s.file("a.html", SECTIONS);
    let o = s.cmd("./a.html").env("FATS_SESSION", "linked").output().expect("run fats");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.starts_with("= ! cache_write") && out.contains("links out of the cache"), "{out}");
    assert_eq!(std::fs::read_dir(&o2).map(|d| d.count()).ok(), Some(0), "nothing is written outside the cache");
}

/// A failed fetch's row carries the error table's fix for its code, not a blind `fresh:true`.
#[test]
fn failed_fetch_rows_carry_the_table_fix() {
    let s = Sandbox::new("hint");
    let c = s.dir.join("cache/t");
    let mut u = Vec::new();
    for (i, code) in ["provider_timeout", "js_shell", "not_found"].iter().enumerate() {
        let d = c.join(format!("u{}", i + 1));
        std::fs::create_dir_all(&d).expect("dir");
        std::fs::write(d.join("raw.html"), SECTIONS).expect("raw");
        let meta = format!(
            r#"{{"url":"https://ex.com/{i}","provider":"oxylabs","country":"DE","ok":false,"code":"{code}","t":1,
                "hops":[{{"provider":"oxylabs","code":"{code}"}}]}}"#
        );
        std::fs::write(d.join("meta.json"), meta).expect("meta");
        u.push(serde_json::json!({"url": format!("https://ex.com/{i}"), "dir": format!("t/u{}", i + 1)}));
    }
    std::fs::write(c.join("state.json"), serde_json::json!({ "u": u }).to_string()).expect("state");
    // no key: a hint never names a provider the user has no key for
    let none = s.run("u2").0;
    assert!(none.contains("u2 ! js_shell") && !none.contains("provider:") && !none.contains("js:true"), "{none}");
    s.keys();
    s.env_here();
    for (id, code, arg) in [("u1", "provider_timeout", "js:true"), ("u2", "js_shell", ""), ("u3", "not_found", "")] {
        let (out, _) = s.run(id);
        shape(&out);
        let row = out.lines().find(|l| l.starts_with(&format!("{id} ! {code}"))).unwrap_or_default();
        let url = format!("https://ex.com/{}", &id[1..].parse::<usize>().unwrap() - 1);
        let at = fats::lang::At { id, url: &url, arg, ..Default::default() };
        let want = fats::lang::fix(code, &at).replace(fats::lang::bin(), "fats");
        assert!(row.ends_with(&format!("→ {want}")), "{out}");
    }
    assert!(s.run("u1").0.contains("→ fats u1 fresh:true js:true"), "oxy without JS timed out: render instead");
    assert!(s.run("stats").0.contains(" · oxylabs@DE "), "stats names the exit country");
}

/// The geo rule: an Oxylabs geo copy whose call sent US (the default where nothing names a
/// country) never suggests `country:us` again (the same request, the same copy, a paid loop): the URL's ccTLD
/// country when it has one, else `provider:bd` with its key, else a COUNT that says what was sent; `stats` names
/// it (`oxylabs-js@US`).
#[test]
fn a_geo_copy_after_us_asks_the_cctld_then_bd_then_counts() {
    let s = Sandbox::new("geo");
    let mut rows = vec![];
    for (n, url) in [(1, "https://ex.com/a"), (2, "https://www.lemonde.fr/a")] {
        let d = s.dir.join(format!("cache/t/u{n}"));
        std::fs::create_dir_all(&d).expect("dir");
        std::fs::write(d.join("raw.html"), SECTIONS).expect("raw");
        let meta = r#"{"url":"URL","provider":"oxylabs-js","country":"US","ok":true,"code":"ok","t":1,
            "hops":[{"provider":"oxylabs-js","code":"ok","why":"lang:it","cost":0.0015}]}"#;
        std::fs::write(d.join("meta.json"), meta.replace("URL", url)).expect("meta");
        rows.push(serde_json::json!({"url": url, "dir": format!("t/u{n}")}));
    }
    std::fs::write(s.dir.join("cache/t/state.json"), serde_json::json!({"u": rows}).to_string()).expect("state");
    s.auth(&[("oxy", "x")]);
    let oxy_only = s.cmd("u1").output().expect("run");
    let out = String::from_utf8_lossy(&oxy_only.stdout);
    assert!(out.lines().next().is_some_and(|l| l.contains(" · lang:it · sent US")), "{out}");
    assert!(!out.contains("u1 ! lang:it") && !out.contains("country:us"), "no paid loop: {out}");
    let oxy_only = s.cmd("u2").output().expect("run");
    let out = String::from_utf8_lossy(&oxy_only.stdout);
    assert!(out.contains("u2 ! lang:it · served in it, sent US · the provider's geo copy → fats u2 country:fr "));
    s.keys();
    let (out, _) = s.run("u1");
    assert!(
        out.contains("u1 ! lang:it · served in it, sent US · the provider's geo copy → fats u1 provider:bd "),
        "{out}"
    );
    assert!(!out.contains("country:us"), "{out}");
    assert!(s.run("stats").0.contains(" · oxylabs-js@US "), "stats names the country sent");
}

/// A windows-1250 page (Central European sites still serve it) reads as Czech, not U+FFFD, and stats names the charset.
#[test]
fn page_declared_charset_is_decoded() {
    let s = Sandbox::new("cp1250");
    let body = b"<html><head><meta charset=\"windows-1250\"><title>\x8Elu\x9Duj\xEDc\xED cena</title></head>\
        <body><h1>\x8Elu\x9Duj\xEDc\xED cena</h1><p>Cena 120 \x80 za kilogram</p></body></html>";
    std::fs::write(s.dir.join("work/cz.html"), body).expect("fixture");
    let (out, rc) = s.run("./cz.html");
    assert_eq!(rc, 0, "{out}");
    assert!(
        out.contains("Žluťující cena") && out.contains("Cena 120 € za kilogram") && !out.contains('\u{fffd}'),
        "{out}"
    );
    assert!(s.run("stats full").0.contains("windows-1250"), "stats names the decoded charset");
    let raw = std::fs::read(s.dir.join("cache/t/u1/raw.html")).expect("raw");
    assert!(String::from_utf8(raw).is_ok_and(|r| r.contains("Žluťující")), "the cache stores UTF-8");
}

/// Input over the cap is refused before it is cleaned or stored.
#[test]
fn oversized_input_is_refused() {
    let s = Sandbox::new("big");
    // over the cap in bytes, cheap to clean once let through (one comment, not 666k paragraphs: 30 s in debug)
    let mut p = b"<p>twenty megabytes of page</p><!--".to_vec();
    p.resize(20_000_000 - 3, b' ');
    p.extend(b"-->");
    std::fs::write(s.dir.join("work/big.html"), &p).expect("20 MB file");
    let (out, rc) = s.run("./big.html");
    assert_eq!(rc, 1, "{out}");
    assert!(out.starts_with("= ! too_big ./big.html · over the 16 MiB input cap (ceiling 64 MiB)"), "{out}");
    assert!(out.ends_with("! too_big → fats ./big.html max_bytes:64m\n"), "{out}");
    assert!(!s.dir.join("cache/t/u1").exists(), "nothing stored");
    s.file("small.html", &page("Small", &"<p>words here</p>".repeat(100)));
    assert!(s.run("./small.html max_bytes:1k").0.starts_with("= ! too_big ./small.html · over the 1 KiB"));
    let (out, rc) = s.run("./big.html max_bytes:32m");
    assert_eq!(rc, 0, "max_bytes raises the cap: {}", out.lines().next().unwrap_or_default());
    assert!(s.run("config").0.contains("max_bytes 16 MiB (ceiling 64 MiB)"));
    // the built-in tables (src/data/*.json) are counted where every other setting is shown
    assert!(s.run("config").0.contains(
        "
tables 8 files · 2121 rows · built-in · providers 49, errors 53, check 86, sites 69, clean 186, words 1613, limits 55, telemetry 10
"
    ));
    // every behaviour threshold is a row of its own
    assert!(s.run("config").0.contains("\nisland_json_bytes 20480 · build · limits.json\n"));
}

/// Small edges: zero links, no jobs, an unknown id (dry too), a line window, a second id, and a last line that is
/// the next move.
#[test]
fn edges_answer_cleanly() {
    let s = Sandbox::new("edges");
    s.file("a.html", SECTIONS);
    assert!(s.run("j3.u1").0.starts_with("? j3 unknown · no jobs yet"));
    assert!(s.run("u4").0.starts_with("? u4 unknown · have none · fats <url>"));
    s.run("./a.html");
    assert!(s.run("u4").0.starts_with("? u4 unknown · have u1-u1 · last job j1"));
    assert!(s.run("j1.u4").0.starts_with("? j1.u4 unknown · have u1-u1 · last job j1"));
    // a dry run of an id the session lacks says what the run would: the same `?`, rc 2
    let (out, rc) = s.run("u4 dry:true");
    assert!(out.starts_with("? u4 unknown · have u1-u1 · last job j1") && rc == 2, "{out}");
    assert!(s.run("u1 dry:true").0.contains("\nu1 local read · $0"), "a held id still plans its read");
    assert!(s.run("u1 l3").0.starts_with("? l3 unknown on u1 · no links"));
    let (out, rc) = s.run("u1#1:12");
    assert_eq!(rc, 2);
    assert!(out.starts_with("? u1#1:12: line windows are not in v1 · read the section: fats u1#1"), "{out}");
    let (out, _) = s.run("u1 u2 apple");
    assert!(out.lines().nth(1).is_some_and(|l| l == "u2 ! not_in_v1 multi-page filter → fats u2 apple"), "{out}");
    for cmd in ["stats", "u1 dry:true", "u1 links"] {
        let (out, _) = s.run(cmd);
        let last = out.lines().last().unwrap_or_default();
        assert!(last.contains("fats "), "{cmd}: last line is a next move: {out}");
    }
}

/// `auth.json` readable by others is flagged in status with the chmod fix; its content is never printed.
#[cfg(unix)]
#[test]
fn loose_key_file_is_flagged() {
    use std::os::unix::fs::PermissionsExt;
    let s = Sandbox::new("keys");
    let k = s.dir.join("keys");
    std::fs::create_dir_all(&k).expect("key dir");
    std::fs::write(k.join("auth.json"), r#"{"version":1,"keys":{"oxy":"SECRET-VALUE"}}"#).expect("key");
    std::fs::set_permissions(k.join("auth.json"), std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let status = || {
        let o = s.cmd("").output().expect("run");
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let out = status();
    assert!(out.contains("\nkeys oxy … · bd none · ") && out.contains("! key_mode auth.json is 644"), "{out}");
    assert!(out.contains("→ chmod 600 ") && !out.contains("SECRET"), "{out}");
    std::fs::set_permissions(k.join("auth.json"), std::fs::Permissions::from_mode(0o600)).expect("chmod");
    assert!(!status().contains("key_mode"));
}

/// Keys live in `auth.json` only: an exported key, `./.env` and the Bright Data CLI's credentials are never read.
/// The 0.1 key files move into `auth.json` / `settings.json` once (0600) and are deleted; no key ever prints.
#[test]
fn keys_come_from_auth_json_only() {
    let s = Sandbox::new("setup");
    let bare = |args: &str| {
        let o = s.cmd(args).env("OXYLABS_WEB_API_KEY", "oxy-exported-Zq82").output().expect("run");
        (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code().unwrap_or(-1))
    };
    s.file(".env", "export OXYLABS_WEB_API_KEY=\"oxy-dotenv-Zq81\"\n");
    let conf = if cfg!(target_os = "macos") { "Library/Application Support" } else { ".config" };
    let bdata = s.dir.join("home").join(conf).join("brightdata-cli");
    std::fs::create_dir_all(&bdata).expect("bdata dir");
    std::fs::write(bdata.join("credentials.json"), r#"{"api_key": "bd-cli-Kx7"}"#).expect("bdata key");
    let (out, _) = bare("");
    let keys = "\nkeys none · search needs oxy or bd (one is enough; oxy costs least per search) → run fats setup oxy, give the \
                user its 3 lines · network off\n";
    assert!(out.contains(keys), "{out}");
    let row = "\n! no_key oxy · env OXYLABS_WEB_API_KEY …Zq82 not imported → fats setup import-env\n";
    assert!(out.contains(row) && !out.contains("oxy-exported"), "{out}");
    // `fats setup`: one row for the missing key, the env's (it names the move), never the plain one beside it
    let (out, _) = bare("setup");
    assert!(out.contains(row) && out.matches("! no_key oxy").count() == 1, "{out}");
    let k = s.dir.join("keys");
    std::fs::create_dir_all(&k).expect("key dir");
    std::fs::write(k.join("oxylabs.key"), "oxy-legacy-Q9\n").expect("legacy key");
    std::fs::write(k.join("brightdata.zone"), "zone_a\n").expect("legacy zone");
    let (out, rc) = bare("setup");
    assert!(rc == 0 && out.contains("\noxy key … auth.json · would read "), "{out}");
    assert!(out.contains("\nenv OXYLABS_WEB_API_KEY …Zq82 ≠ saved … · not used · replace: fats setup oxy\n"), "{out}");
    assert!(!k.join("oxylabs.key").exists() && !k.join("brightdata.zone").exists(), "moved");
    let auth = std::fs::read_to_string(k.join("auth.json")).unwrap_or_default();
    let prefs = std::fs::read_to_string(k.join("settings.json")).unwrap_or_default();
    assert!(auth.contains("\"oxy-legacy-Q9\"") && prefs.contains("\"bd_zone\": \"zone_a\""), "{auth} {prefs}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(k.join("auth.json")).map(|m| m.permissions().mode() & 0o777).unwrap_or(0);
        assert_eq!(mode, 0o600);
    }
    assert!(!out.contains("oxy-exported") && !out.contains("Kx7") && !out.contains("Q9"), "a tail only: {out}");
}

/// A Bright Data key saved without its zone is `! no_zone` with the zone move, never `no_key` (the key is there);
/// `setup bd zone:<name>` is that move: the setup check with the zone, never a second key form.
#[test]
fn a_bd_key_without_a_zone_is_no_zone() {
    let s = Sandbox::new("nozone");
    let k = s.dir.join("keys");
    std::fs::create_dir_all(&k).expect("key dir");
    std::fs::write(k.join("auth.json"), r#"{"version":1,"keys":{"bd":"bdK3y-nozone"}}"#).expect("key");
    #[cfg(unix)]
    std::fs::set_permissions(k.join("auth.json"), std::os::unix::fs::PermissionsExt::from_mode(0o600)).expect("chmod");
    let o = s.cmd("https://1.1.1.1/x provider:bd dry:true").env_remove("FATS_OFFLINE").output().expect("run");
    let dry = String::from_utf8_lossy(&o.stdout).into_owned();
    let row = " · ! no_zone bd · key saved, no Web Unlocker zone yet → fats setup bd zone:<name>";
    assert!(dry.contains(row) && !dry.contains("no_key"), "{dry}");
    let (out, rc) = s.run("setup bd zone:web_unlocker1 dry:true");
    assert!(rc == 0 && out.starts_with("= dry setup ") && out.contains("bd key … auth.json"), "{out}");
    // every zone move is spelled `setup bd zone:<name>`; the short `setup zone:<name>` is the same move
    assert_eq!(s.run("setup zone:web_unlocker1 dry:true"), (out, rc));
}

/// A key-shaped word on a `fats setup` command line is refused, rc 1, never read, saved or echoed (shell history,
/// `ps`); on any other command it is a word as typed (a sha, a part number). Every `?` row shows such a word as
/// its tail. Base64 (`+/=`) is key-shaped; a URL, a word with a dot, a path led by `/`, `./` or `~` never is.
#[test]
fn a_key_on_the_setup_command_line_is_refused() {
    let s = Sandbox::new("argv-key");
    let k = "abcDEF0123456789ghiJKL6XBR";
    let ways = |p: &str| {
        let pipe = format!("pipe: printf %s \"$KEY\" | fats setup {p}");
        format!("! key_in_argv · never on the command line (shell history, ps) · paste: fats setup {p} · {pipe}")
    };
    for (args, p) in [("setup oxy {k}", "oxy"), ("setup bd {k}", "bd"), ("setup {k}", "oxy"), ("setup {k} json", "oxy")]
    {
        let (out, rc) = s.run(&args.replace("{k}", k));
        let out = out.replace("\\\"", "\""); // `json` escapes the quotes
        assert!(rc == 1 && out.contains(&ways(p)) && !out.contains(&k[..12]), "{args}: {out}");
    }
    assert!(!s.dir.join("keys/auth.json").exists(), "nothing saved");
    // other commands: a word as typed; a `?` row: the tail
    let (out, _) = s.run(&format!("{k} x"));
    assert!(out.contains(k) && !out.contains("key_in_argv"), "{out}");
    let (out, rc) = s.run(&format!("x provider:{k}"));
    assert!(rc == 2 && out.starts_with("? provider:…6XBR · ") && !out.contains(&k[..12]), "{out}");
    let b64 = "ZGNkMHVzZXI6UGFzczEyMzQ1Njc4OTA+/x==";
    let (out, rc) = s.run(&format!("setup decodo {b64}"));
    assert!(rc == 1 && out.contains(&ways("decodo")) && !out.contains(&b64[..12]), "{out}");
    // short, one-kind, a URL, dotted (a sha's file), a path: no key
    let paths = ["/api/v1/abcDEF0123456789ghiJKL", "./abcDEF0123456789ghiJKL6XBR", "~/abcDEF0123456789ghiJKL6XBR"];
    let words = ["abcDEF0123456789ghi", "abcdefghijklmnopqrstuvwxyz", "example.com/abcDEF0123456789ghiJKL6XBR"];
    let urls = ["https://ex.com/abcDEF0123456789ghiJKL6XBR", "4f2a9c0d1e3b5a7c9e1f3a5b7c9d1e3f5a7b9c1d.patch"];
    for w in words.into_iter().chain(paths).chain(urls) {
        let (out, _) = s.run(&format!("setup oxy {w}"));
        assert!(!out.contains("key_in_argv"), "{w}: {out}");
    }
}

/// `fats setup <p>` reads a piped key (SSH, CI): ONE line, its CR/LF stripped, the menu's check ($0), saved, a
/// replaced key kept in `auth.json.bak` (the same key again replaces none: no `.bak` said or written); a bad one
/// is `! bad_key`, rc 1, at once; under `FATS_OFFLINE` nothing is checked or saved. Never the key form: no run
/// file (the form's detached run writes one), no key byte printed. (No case here reaches a network.)
#[test]
fn a_piped_key_is_checked_and_saved() {
    use std::io::Write;
    let s = Sandbox::new("pipe-key");
    let offline = |args: &str, input: &str, off: bool| {
        let mut c = s.cmd(args);
        if !off {
            c.env_remove("FATS_OFFLINE");
        }
        let mut child =
            c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().expect("run");
        child.stdin.take().expect("stdin").write_all(input.as_bytes()).expect("write");
        let o = child.wait_with_output().expect("wait");
        (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code().unwrap_or(-1))
    };
    let pipe = |args: &str, input: &str| offline(args, input, false);
    let (out, rc) = offline("setup gh", "ghp-fake-0123456789AB", true);
    let off = "= ! offline setup gh · FATS_OFFLINE is on: no check (unset FATS_OFFLINE) · not saved → printf %s";
    assert!(rc == 1 && out.starts_with(off) && !s.dir.join("keys/auth.json").exists(), "{out}");
    let t0 = std::time::Instant::now();
    let (out, rc) = pipe("setup oxy", "notakey");
    let bad = "= ! bad_key setup oxy · too short for a key: copy the whole key · not saved → printf %s \"$KEY\" | fats";
    assert!(rc == 1 && out.starts_with(bad) && t0.elapsed().as_secs() < 3, "{out}");
    assert!(!s.dir.join("cache/t/run/setup-oxy.json").exists() && !s.dir.join("keys/auth.json").exists());
    let (out, rc) = pipe("setup gh", "ghp-fake-0123456789AB\r\n");
    let first = "= setup gh ok unchecked · key …89AB saved auth.json (600) · $0\n";
    assert!(rc == 0 && out.starts_with(first), "{out}");
    let (out, rc) = pipe("setup gh", "ghp-fake-0123456789AB\n");
    assert!(rc == 0 && out.starts_with(first) && !s.dir.join("keys/auth.json.bak").exists(), "the same key: {out}");
    let (out, rc) = pipe("setup gh", "ghp-fake-0123456789CD\nnext line\n");
    let kept = "key …89CD saved auth.json (600) · the old key in auth.json.bak · $0";
    assert!(rc == 0 && out.contains(kept) && !out.contains("0123456789"), "{out}");
    let read = |f: &str| std::fs::read_to_string(s.dir.join("keys").join(f)).unwrap_or_default();
    assert!(read("auth.json").contains("\"ghp-fake-0123456789CD\"") && read("auth.json.bak").contains("89AB\""));
}

/// A pipe held open and never written (an agent's shell that keeps stdin) is no key: after 2 s `fats setup oxy`
/// takes the form path, no hang; under `FATS_OFFLINE` that path starts no form (its check could not run): the dry
/// row, no run file.
#[test]
fn a_silent_pipe_falls_back_to_the_form_in_2_s() {
    let s = Sandbox::new("pipe-silent");
    let t0 = std::time::Instant::now();
    let mut child = s
        .cmd("setup oxy")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("run");
    let held = child.stdin.take(); // open until the answer is in
    let o = child.wait_with_output().expect("wait");
    let (out, took) = (String::from_utf8_lossy(&o.stdout), t0.elapsed());
    drop(held);
    assert!(took.as_millis() >= 1900 && took.as_secs() < 5, "{took:?}");
    assert_eq!(out, "= dry setup oxy · would start a one-shot key form on 127.0.0.1 · $0\n");
    assert!(!s.dir.join("cache/t/run/setup-oxy.json").exists(), "no form server");
}

/// An old key file beside the keys (a replaced or removed key's, `auth.json.bak`) is named where a key is switched
/// off, with the `rm` that deletes it, and by ` · bak` at the end of the status keys row and the config `key_dir`
/// row, until it is deleted.
#[test]
fn a_key_backup_is_named_until_deleted() {
    let s = Sandbox::new("bak");
    let run = |args: &str| String::from_utf8_lossy(&s.cmd(args).output().expect("run").stdout).into_owned();
    let k = s.dir.join("keys");
    std::fs::create_dir_all(&k).expect("key dir");
    std::fs::write(k.join("auth.json"), r#"{"version":1,"keys":{"oxy":"k-123456789"}}"#).expect("auth");
    std::fs::write(k.join("auth.json.bak"), r#"{"version":1,"keys":{"oxy":"k-old456789"}}"#).expect("bak");
    let bak = format!("old key kept in auth.json.bak (0600) · delete it: rm '{}'", k.join("auth.json.bak").display());
    let off = "= setup oxy off · saved settings.json · from the next command\n";
    assert_eq!(run("setup oxy off"), format!("{off}{bak}\n"));
    let (health, cfg) = (run(""), run("config"));
    assert!(health.contains(" · auth.json · network off · bak\n"), "{health}");
    assert!(cfg.contains(&format!("\nkey_dir {} · env FATS_KEY_DIR · bak\n", k.display())), "{cfg}");
    std::fs::remove_file(k.join("auth.json.bak")).expect("rm");
    assert!(!run("").contains(" · bak\n") && !run("config").contains(" · bak\n"), "gone with its file");
    assert_eq!(run("setup oxy off"), off);
}

/// `fats setup` names a key form the user holds: one still waiting with its minutes left and URL, one whose time ran
/// out, or whose run no longer answers on its port, as dead with the move to a new link; a saved key's provider
/// shows none.
#[test]
fn setup_names_a_waiting_or_dead_form() {
    let s = Sandbox::new("formrow");
    let form = s.dir.join("cache/t/run/setup-oxy.json");
    std::fs::create_dir_all(form.parent().expect("run dir")).expect("run dir");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let run = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = run.local_addr().expect("port").port();
    let put = |until: f64| std::fs::write(&form, format!(r#"{{"token":"ab12","until":{until},"port":{port}}}"#));
    put(now + 23.0 * 60.0 - 5.0).expect("form");
    let wait = format!("\noxy form waiting · 23 min left · http://127.0.0.1:{port}/k/ab12/oxy\n");
    assert!(s.run("setup").0.contains(&wait), "{}", s.run("setup").0);
    let dead = "\n! form_expired oxy · the link the user got is dead → fats setup oxy · new link, same 3 steps\n";
    let mut run = Some(run);
    for until in [now - 1.0, now + 600.0] {
        put(until).expect("form");
        let out = s.run("setup").0;
        assert!(out.contains(dead) && !out.contains("form waiting") && !out.contains("form_expired bd"), "{out}");
        run.take(); // the run is gone before its deadline
    }
}

/// `setup <name> on|off` never leaves nothing that can fetch; an off tier reads like a missing key in `config`;
/// `config set|unset` keep a threshold in `settings.json`, a mod beats it, a value of the wrong kind is refused.
#[test]
fn switches_and_settings_rows() {
    let s = Sandbox::new("switch");
    let run = |args: &str| {
        let o = s.cmd(args).output().expect("run");
        (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code().unwrap_or(-1))
    };
    let nothing = ("! refused · nothing left that can fetch: turn one on or add a key\n".into(), 1);
    assert_eq!(run("setup free off"), nothing);
    let k = s.dir.join("keys");
    std::fs::create_dir_all(&k).expect("key dir");
    std::fs::write(k.join("auth.json"), r#"{"version":1,"keys":{"oxy":"k-123456789"}}"#).expect("auth");
    assert_eq!(run("setup free off").0, "= setup free off · saved settings.json · from the next command\n");
    assert!(run("config").0.contains("\nplain 10s $0.00/1k · build · off (settings.json)\n"));
    // the off switch's own row, never an empty `! no_key plain · would try  ≤$0.0000`
    let o = s.cmd("https://1.1.1.1/x provider:plain dry:true").env_remove("FATS_OFFLINE").output().expect("run");
    let dry = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(dry.contains(" · ! off free · free fetch turned off → fats setup free on\n"), "{dry}");
    // nothing after `setup` but a switch, on|off and mods: a typo is a `?` at $0, never a web search
    assert_eq!(run("setup oxi"), ("? setup oxi → fats setup oxy\n".into(), 2));
    assert_eq!(run("setup plain on"), ("= setup free on · saved settings.json · from the next command\n".into(), 0));
    let (out, rc) = run("setup foo bar");
    assert!(rc == 2 && out.starts_with("? setup foo bar · fats setup <free|oxy|bd|decodo|gh|plain> [on|off]"), "{out}");
    assert!(run("stats").0.starts_with("= stats s:t · 0 searches · "), "no search ran");
    assert_eq!(run("setup free off").1, 0);
    assert_eq!(run("setup oxy off"), nothing);
    assert_eq!(run("setup free on").1, 0);
    assert_eq!(run("setup oxy off").1, 0);
    assert!(run("config").0.contains("\noxy 10s $1.00/1k · build · off (settings.json)\n"));
    let health = run("").0;
    assert!(health.contains("\noxy off (key kept)\n") && health.contains("\noxyjs off (key kept)\n"), "{health}");
    assert!(health.contains("\nbd no key\n") && health.contains("\nplain - no calls yet\n"), "{health}");
    assert_eq!(run("config set batch_parallel 6").1, 0);
    assert_eq!(run("config set batch_parallel 2.5").0, "! refused · batch_parallel 2.5 · want a number ≥ 1 like 8\n");
    // THE reader's check (`settings::fits` and the env readers): 0 lanes, a chunk past its range, a cut value
    assert_eq!(run("config set batch_parallel 0").0, "! refused · batch_parallel 0 · want a number ≥ 1 like 8\n");
    assert_eq!(run("config set chunk 99999"), ("! refused · chunk 99999 · want an integer 50-10000\n".into(), 1));
    assert_eq!(run("config set country de x").0, "! refused · country de x · want two letters: us · de\n");
    let spaced = s.dir.join("my cache").display().to_string();
    assert_eq!(run(&format!("config set cache {spaced}")).1, 0);
    let cfg = s.cmd("config").env_remove("FATS_CACHE").output().expect("run").stdout;
    assert!(String::from_utf8_lossy(&cfg).contains(&format!("\ncache {spaced} · settings.json · ")), "whole value");
    assert_eq!(run("config unset cache").1, 0);
    // a refused `settings.json` value is named by its file and its `config` move, never an env var nobody set
    let (prefs, bad) = (s.dir.join("keys/settings.json"), "! bad_setting settings.json chunk=99999 · want an integer");
    let kept = std::fs::read_to_string(&prefs).expect("prefs");
    let mut set: serde_json::Value = serde_json::from_str(&kept).expect("prefs json");
    set["set"]["chunk"] = "99999".into();
    std::fs::write(&prefs, set.to_string()).expect("prefs");
    let cfg = run("config").0;
    assert!(cfg.contains(&format!("{bad} 50-10000 · using 300 → fats config unset chunk or set it again\n")), "{cfg}");
    assert!(cfg.contains("\nchunk 300tok · default (refused settings.json chunk=99999)\n"), "{cfg}");
    std::fs::write(&prefs, kept).expect("prefs");
    let lanes = " · lanes (RAM guard, not a provider limit)\n";
    assert!(run("config").0.contains(&format!("\nbatch_parallel 6 · settings.json{lanes}")));
    assert!(run("config batch_parallel:3").0.contains(&format!("\nbatch_parallel 3 · mod{lanes}")));
    assert_eq!(run("config unset batch_parallel").1, 0);
    assert!(run("config").0.contains(&format!("\nbatch_parallel 8 · build · limits.json{lanes}")));
    assert!(run("config").0.contains("\nplain_host_pace_ms 1000 · build · limits.json · plain waits its turn a host"));
}

/// A threshold has a `config` row, never a literal; a name in the env or `settings.json` that no
/// setting reads (a removed one, `render_fix_s`) is one `! unknown_setting` row in `config`, never on every answer.
#[test]
fn every_setting_name_is_a_row() {
    let s = Sandbox::new("names");
    let run = |args: &str| String::from_utf8_lossy(&s.cmd(args).output().expect("run").stdout).into_owned();
    let config = run("config");
    assert!(
        config.contains("\ndns_fast_ms 1000 · build · limits.json\n") && config.contains("\nreason_chars 50 · build")
    );
    assert!(!config.contains("unknown_setting"), "{config}");
    assert!(run("config set batch_parallel 6").starts_with("= config set batch_parallel"), "the file made");
    let prefs = s.dir.join("keys/settings.json");
    let mut set: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&prefs).expect("prefs")).expect("json");
    set["set"]["render_fix_s"] = 150.into();
    std::fs::write(&prefs, set.to_string()).expect("prefs");
    let cfg = s.cmd("config").env("FATS_RENDER_FIX_S", "150").output().expect("run").stdout;
    let cfg = String::from_utf8_lossy(&cfg).into_owned();
    let row = "\n! unknown_setting env FATS_RENDER_FIX_S · read by nothing → unset FATS_RENDER_FIX_S\n";
    assert!(cfg.contains(row), "{cfg}");
    let row = "\n! unknown_setting settings.json render_fix_s · read by nothing → fats config unset render_fix_s\n";
    assert!(cfg.contains(row) && !run("stats").contains("unknown_setting"), "{cfg}");
}

/// The free fetch switched off is its state and the move back, as `setup free`|`plain` and in the setup summary;
/// a mod's own `config` row is set per call (`config` names it `cost:`), never an unknown name.
#[test]
fn free_off_and_per_call_rows() {
    let s = Sandbox::new("freeoff");
    let run = |args: &str| {
        let o = s.cmd(args).output().expect("run");
        (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code().unwrap_or(-1))
    };
    let k = s.dir.join("keys");
    std::fs::create_dir_all(&k).expect("key dir");
    std::fs::write(k.join("auth.json"), r#"{"version":1,"keys":{"oxy":"k-123456789"}}"#).expect("auth");
    assert!(run("setup plain").0.contains("built in, no key to set"));
    assert_eq!(run("setup free off").1, 0);
    let off = "free off (turned off) → fats setup free on";
    assert_eq!(run("setup plain"), (format!("= setup free · {off}\n"), 0));
    let sum = run("setup dry:true").0;
    assert!(
        sum.starts_with("= dry setup free off (turned off) · oxy ") && sum.contains(&format!("\n{off}\n")),
        "{sum}"
    );
    assert_eq!(run("config set cost_cap -1"), ("! refused · cost_cap is per call: the cost: mod\n".into(), 1));
    assert_eq!(run("config set wait abc").0, "! refused · wait is per call: the wait: mod\n");
}

/// The free fetch off with both keys: the page ladder starts at oxy without JS, then bd ∥ oxyjs, as `dry:true` and
/// `config` print it (with free on, oxy runs only after plain's captcha).
#[test]
fn free_off_ladder_starts_at_the_no_js_rung() {
    let s = Sandbox::new("freeladder");
    s.auth(&[("oxy", "x"), ("bd", "y")]);
    let keyed = |cmd: &str| {
        let mut c = s.cmd(cmd);
        c.env_remove("FATS_OFFLINE").env("BRIGHTDATA_UNLOCKER_ZONE", "z");
        String::from_utf8_lossy(&c.output().expect("run").stdout).into_owned()
    };
    assert!(keyed("config").contains("auto: plain → oxy (after a captcha) → bd ∥ oxyjs after 5s · "));
    assert!(keyed("setup free off").starts_with("= setup free off · saved"));
    let out = keyed("https://1.1.1.1/ dry:true");
    assert!(out.contains("fetch oxy → bd ∥ oxyjs after 5s → new u1 · up to $0.0040\n"), "{out}");
    assert!(!out.contains("no_key") && !out.contains("! off"), "{out}");
    let cfg = keyed("config");
    assert!(cfg.contains("auto: oxy → bd ∥ oxyjs after 5s · "), "{cfg}");
}

/// Links are a list like any other: 300 ~tok then `more`; `fats j1` shows a job; every json record has id + url,
/// and a cache read says `bytes_in: 0, cache: true`.
#[test]
fn links_jobs_and_record_fields() {
    let s = Sandbox::new("linkjob");
    let links: String = (1..=200)
        .map(|i| format!("<a href=\"https://example.com/a/long/path/to/a/page/number/{i}\">p{i}</a> "))
        .collect();
    s.file("l.html", &page("Links", &links));
    s.run("./l.html");
    let (out, _) = s.run("u1 links");
    let rows = out.lines().filter(|l| l.starts_with('l')).count();
    let t: usize = out.lines().filter(|l| l.starts_with('l')).map(|l| l.chars().count().div_ceil(4)).sum();
    assert!(rows > 3 && rows < 50 && t <= 300, "{rows} rows {t}tok: {out}");
    assert!(out.contains(&format!("+{} → fats u1 links from:{}", 200 - rows, rows + 1)), "{out}");
    let (out, rc) = s.run("j1");
    assert_eq!(rc, 0, "{out}");
    assert!(out.starts_with("= j1 1 pages ·") && out.contains(" · local ") && out.ends_with("open: fats j1.u1\n"));
    shape(&out);
    assert!(s.run("j1 apple").0.contains("j1 ! not_in_v1 multi-page filter → fats j1.u1 apple"));
    assert!(s.run("j9").0.starts_with("? j9 unknown · last job j1"));
    let rec =
        |a: &str| -> serde_json::Value { serde_json::from_str(&s.run(&format!("{a} json:true")).0).expect("json") };
    let r = rec("u1");
    assert_eq!((r["bytes_in"].as_u64(), r["cache"].as_bool(), r["id"].as_str()), (Some(0), Some(true), Some("j1.u1")));
    assert_eq!(rec("u1#1")["id"].as_str(), Some("j1.u1#1"), "a section read keeps its section");
    let r = rec("u7");
    assert_eq!((r["id"].as_str(), r["url"].is_null()), (Some("u7"), true), "{r}");
    let r = rec("stats");
    assert!(r["id"].is_null() && r["url"].is_null(), "{r}");
    s.file("m.html", &page("M", "<p>x</p>"));
    let r = rec("./m.html");
    assert_eq!((r["cache"].as_bool(), r["bytes_in"].as_u64().is_some_and(|b| b > 0)), (Some(false), true), "{r}");
}

/// One `fats mcp` session in the sandbox: initialize, tools/list, then the tool calls (local reads in and out of
/// the fence, stdin, an empty command, a wrong tool, a wrong argument). The answers by request id.
/// initialize (id 1) → tools/list (2) → one tools/call per `calls` entry (ids 3, 4, …): every answer by id.
fn mcp_session(s: &Sandbox, calls: &[serde_json::Value]) -> std::collections::HashMap<u64, serde_json::Value> {
    use std::io::Write;
    let mut reqs = vec![
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "it", "version": "0"}}})
        .to_string(),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.into(),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#.into(),
    ];
    reqs.extend(calls.iter().zip(3..).map(|(args, id)| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": args}).to_string()
    }));
    let mut child = s
        .cmd("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn fats mcp");
    let mut stdin = child.stdin.take().expect("stdin");
    stdin.write_all((reqs.join("\n") + "\n").as_bytes()).expect("write requests");
    drop(stdin); // EOF = the parent went away: the server exits
    let o = child.wait_with_output().expect("fats mcp exits");
    assert_eq!(o.status.code(), Some(0));
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| Some((v["id"].as_u64()?, v)))
        .collect()
}

/// The status's mcp row reads each host's config where its own `mcp add` writes it: Claude Code's user scope (top
/// level) and local scope (`projects.<this dir>`; another project's server is not this one's), its `./.mcp.json`;
/// Codex under `$CODEX_HOME`, else `~/.codex`; Cursor's global and project `mcp.json`.
#[test]
fn mcp_row_reads_where_each_host_writes() {
    let s = Sandbox::new("mcp-hosts");
    let (home, work) = (s.dir.join("home"), std::fs::canonicalize(s.dir.join("work")).expect("work"));
    std::fs::create_dir_all(home.join(".codex")).expect("dir");
    let row = |codex: Option<&std::path::Path>| {
        let mut c = s.cmd("");
        c.envs(codex.map(|d| ("CODEX_HOME", d)));
        let out = String::from_utf8_lossy(&c.output().expect("run").stdout).into_owned();
        out.lines().find(|l| l.starts_with("mcp: ")).unwrap_or_default().to_string()
    };
    let server = r#"{"mcpServers":{"fats":{"command":"fats","args":["mcp"]}}}"#;
    let other = r#"{"mcpServers":{"other":{}},"projects":{"/elsewhere":{"mcpServers":{"fats":{}}}}}"#;
    std::fs::write(home.join(".claude.json"), other).expect("w");
    std::fs::write(home.join(".codex/config.toml"), "[mcp_servers.fatsearch]\ncommand = \"x\"\n").expect("w");
    assert!(row(None).starts_with("mcp: not added to claude, codex, cursor"), "{}", row(None));
    let local = serde_json::json!({"projects": {work.to_string_lossy(): serde_json::from_str::<serde_json::Value>(server).expect("json")}});
    std::fs::write(home.join(".claude.json"), local.to_string()).expect("w");
    std::fs::write(home.join(".codex/config.toml"), "[mcp_servers.fats]\ncommand = \"fats\"\n").expect("w");
    assert_eq!(row(None), "mcp: added to claude, codex · stdio, one tool fats(cmd)");
    // `CODEX_HOME` moves Codex's file; the project files count for Claude Code and Cursor
    let codex = s.dir.join("codex-home");
    std::fs::create_dir_all(&codex).expect("dir");
    std::fs::write(home.join(".claude.json"), "{}").expect("w");
    std::fs::write(work.join(".mcp.json"), server).expect("w");
    std::fs::create_dir_all(work.join(".cursor")).expect("dir");
    std::fs::write(work.join(".cursor/mcp.json"), server).expect("w");
    assert_eq!(row(Some(&codex)), "mcp: added to claude, cursor · stdio, one tool fats(cmd)");
}

/// `fats mcp` over stdio, as a client sees it: initialize → tools/list → tools/call. The tool text is the CLI's
/// text; local reads stay inside the working dir + cache; only a non-`fats` request is a protocol error.
#[test]
fn mcp_serves_the_cli_text() {
    let s = Sandbox::new("mcp");
    s.file("a.html", &page("Menu", "<h2>Mains</h2><p>Tuna tartare 23</p><h2>Soups</h2><p>Borscht 9,00 €</p>"));
    let (cli, _) = s.run("./a.html");
    assert!(cli.starts_with("= j1.u1 Menu"), "{cli}");
    std::fs::write(s.dir.join("secret.html"), page("S", "<p>outside</p>")).expect("outside file");
    #[cfg(unix)]
    std::os::unix::fs::symlink(s.dir.join("secret.html"), s.dir.join("work/link.html")).expect("symlink");
    let (status, _) = s.run("");
    let add = "\nmcp: not added to claude, codex, cursor · add: claude mcp add --scope user fats -- fats mcp\n";
    assert!(status.contains(add), "{status}");
    assert!(s.run("config").0.contains("\nmcp fats mcp · stdio"), "config names the server");
    let config = s.run("config").0;
    assert!(
        config.contains(
            "\ninstant wait:30s then a fetch or a search slower than wait: (bd) is pending, a gh/account/setup call is cut (provider_timeout wait:) · poll 20ms · a run lives while it holds its lock, else abandoned · prefetch fetch:1"
        ) && config.contains("on no address a third party: https://1.1.1.1/dns-query (A then AAAA, 1.5s each, the host name only)"),
        "config names every instant-mode wait and the third-party resolver: {config}"
    );

    let fats = |cmd: &str| serde_json::json!({"name": "fats", "arguments": {"cmd": cmd}});
    let calls = [fats("u1"), fats("fats u1 tuna"), fats("../secret.html"), fats("./link.html"), fats("-"), fats("")];
    let mut calls = calls.to_vec();
    calls.push(serde_json::json!({"name": "nope", "arguments": {"cmd": "u1"}}));
    calls.push(serde_json::json!({"name": "fats", "arguments": {"cmd": 3}}));
    let by_id = mcp_session(&s, &calls);
    let text = |id: u64| by_id[&id]["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
    let is_err = |id: u64| by_id[&id]["result"]["isError"].as_bool();

    let init = &by_id[&1]["result"];
    assert_eq!(init["serverInfo"]["name"], "fats");
    let instructions = init["instructions"].as_str().unwrap_or_default();
    assert!(instructions.contains(fats::lang::CARD) && instructions.ends_with("First time? Call fats(\"guide\")"));
    let tools = by_id[&2]["result"]["tools"].as_array().cloned().unwrap_or_default();
    assert_eq!(tools[0]["description"].as_str(), Some(fats::mcp::DESCRIPTION));
    assert_eq!(tools.len(), 1, "one tool");
    assert_eq!(
        (tools[0]["name"].as_str(), tools[0]["inputSchema"]["required"][0].as_str()),
        (Some("fats"), Some("cmd"))
    );
    let ann = &tools[0]["annotations"];
    let hints = (ann["readOnlyHint"].as_bool(), ann["destructiveHint"].as_bool(), ann["openWorldHint"].as_bool());
    assert_eq!(hints, (Some(false), Some(false), Some(true)), "it spends credit and writes the cache");
    let desc = tools[0]["description"].as_str().unwrap_or_default();
    assert!(desc.chars().count() / 4 <= 70, "description ~60 tok: {desc}");

    // the same text as the CLI (the `=` line's cache age aside)
    let (cli, _) = s.run("u1");
    let body =
        |t: &str| t.split_once('\n').map(|(a, b)| (a.split(" · cache").next().unwrap_or(a).to_string(), b.to_string()));
    assert_eq!(body(&text(3)), body(&cli), "mcp: {}\ncli: {cli}", text(3));
    assert_eq!(is_err(3), Some(false));
    assert!(text(4).contains("Tuna tartare 23"), "{}", text(4));
    shape(&text(4));
    let refused: &[u64] = if cfg!(unix) { &[5, 6, 7] } else { &[5, 7] };
    for &id in refused {
        assert!(text(id).starts_with("= ! not_allowed "), "{id}: {}", text(id));
        assert!(!text(id).contains("outside</"), "{id}: read the file");
        shape(&text(id));
        assert_eq!(is_err(id), Some(true));
    }
    assert!(text(8).contains("mcp: serving"), "{}", text(8));
    for id in [9, 10] {
        assert_eq!(by_id[&id]["error"]["code"].as_i64(), Some(-32602), "{id}: {}", by_id[&id]);
    }
}

/// Under `fats mcp` the fence comes before any look at the disk: a path outside it answers the same whether it
/// exists or not, alone and beside search words (refused: a path is never searched), a dry run too.
#[test]
fn mcp_never_tells_whether_a_path_outside_exists() {
    let s = Sandbox::new("mcp-probe");
    std::fs::write(s.dir.join("secret.md"), "# S\noutside\n").expect("outside file");
    let fats = |cmd: &str| serde_json::json!({"name": "fats", "arguments": {"cmd": cmd}});
    let cmds = ["../secret.md", "../nope.md", "../secret.md acme phone", "../nope.md acme phone", "acme /etc/hosts"];
    let cmds: Vec<String> = cmds.iter().flat_map(|c| [c.to_string(), format!("{c} dry:true")]).collect();
    let by_id = mcp_session(&s, &cmds.iter().map(|c| fats(c)).collect::<Vec<_>>());
    let text =
        |i: usize| by_id[&(i as u64 + 3)]["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
    for (a, b) in [(0, 2), (1, 3), (4, 6), (5, 7)] {
        let (there, gone) = (text(a), text(b).replace("nope.md", "secret.md"));
        assert_eq!(there, gone, "{} vs {}", cmds[a], cmds[b]);
    }
    assert!(text(0).starts_with("= ! not_allowed ../secret.md "), "{}", text(0));
    assert!(text(1).contains("\n! not_allowed ../secret.md · $0"), "the dry run says the run's verdict: {}", text(1));
    assert!(text(4).starts_with("= ! not_allowed ../secret.md "), "{}", text(4));
    // an outside path of 2+ segments beside words is refused, never searched
    assert!(text(8).starts_with("= ! not_allowed /etc/hosts "), "{}", text(8));
}

/// A loopback HTTP server for one test (loopback is not "the network"): answers each connection with `reply`
/// (given the request head) and returns its address.
fn serve(reply: impl Fn(&str, &mut std::net::TcpStream) -> std::io::Result<()> + Send + 'static) -> String {
    use std::io::Read;
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = format!("http://{}", l.local_addr().expect("addr"));
    std::thread::spawn(move || {
        for mut c in l.incoming().flatten() {
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && c.read(&mut b).is_ok_and(|n| n == 1) {
                head.push(b[0]);
            }
            reply(&String::from_utf8_lossy(&head), &mut c).ok();
        }
    });
    addr
}

/// `c` run with a terminal for its stdin, stdout and stderr (macOS `script(1)`'s pty: no `unsafe` openpty), its
/// one output (stderr merged, as a person sees it).
#[cfg(target_os = "macos")]
fn in_terminal(c: &Command) -> String {
    let mut t = Command::new("/usr/bin/script");
    t.env_clear().envs(c.get_envs().filter_map(|(k, v)| Some((k, v?))));
    t.current_dir(c.get_current_dir().unwrap_or(std::path::Path::new(".")));
    t.arg("-q").arg("/dev/null").arg(c.get_program()).args(c.get_args());
    let o = t.stdin(std::process::Stdio::null()).output().expect("script");
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// A wait past 5 s: a human at a terminal sees `~ still working` (checked through a pty on macOS; the Linux and
/// Windows runners have no pty a test can open without `unsafe`); a pipe gets no byte of it (an agent's shell reads
/// stderr after the call, merged with the answer), and an MCP client that sent a `progressToken` gets
/// `notifications/progress` every 5 s with it. The local page answers after 6 s (plain, no key).
#[test]
fn a_long_wait_says_it_works_only_where_someone_watches() {
    use std::io::{BufRead, Write};
    let late = || {
        serve(|_, c| {
            std::thread::sleep(std::time::Duration::from_secs(6));
            let body = page("Late Page", &"<p>A page that comes after six seconds, whole.</p>".repeat(8));
            let h = format!("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\n\r\n", body.len());
            c.write_all(h.as_bytes()).and_then(|_| c.write_all(body.as_bytes()))
        })
    };
    let (cli, mcp) = (late(), late());
    #[cfg(target_os = "macos")]
    let tty = {
        let url = late();
        let s = Sandbox::new("still-tty");
        std::thread::spawn(move || in_terminal(s.cmd(&url).env_remove("FATS_OFFLINE").env("FATS_ALLOW_PRIVATE", "1")))
    };
    let piped = std::thread::spawn(move || {
        let s = Sandbox::new("still-cli");
        let o = s.cmd(&cli).env_remove("FATS_OFFLINE").env("FATS_ALLOW_PRIVATE", "1").output().expect("run");
        (String::from_utf8_lossy(&o.stdout).into_owned(), o.stderr)
    });
    let s = Sandbox::new("still-mcp");
    let mut child = s
        .cmd("mcp")
        .env_remove("FATS_OFFLINE")
        .env("FATS_ALLOW_PRIVATE", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn fats mcp");
    let mut stdin = child.stdin.take().expect("stdin");
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"it","version":"0"}}}"#;
    let call = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "fats", "arguments": {"cmd": mcp}, "_meta": {"progressToken": "tok7"}}});
    let reqs = format!("{init}\n{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}}\n{call}\n");
    stdin.write_all(reqs.as_bytes()).expect("write requests");
    // the answer first, then EOF: the server ends with its client
    let lines: Vec<serde_json::Value> = std::io::BufReader::new(child.stdout.take().expect("stdout"))
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .take_while(|v: &serde_json::Value| v["id"] != 2)
        .collect();
    drop(stdin);
    child.wait().expect("fats mcp exits");
    let ticks: Vec<_> =
        lines.iter().filter(|v| v["method"] == "notifications/progress").map(|v| &v["params"]).collect();
    let first = serde_json::json!({"progressToken": "tok7", "progress": 5.0, "message": "still working · 5 s"});
    assert_eq!(ticks.first(), Some(&&first), "{lines:?}");
    let (out, err) = piped.join().expect("cli");
    assert!(out.starts_with("= j1.u1 Late Page"), "{out}");
    assert_eq!(String::from_utf8_lossy(&err), "", "a piped stderr gets no byte");
    #[cfg(target_os = "macos")]
    {
        let seen = tty.join().expect("tty");
        // one tick in a 6 s wait: at 5 s, naming the tier that runs
        let line = "~ still working · plain running · 5 s\r\n";
        assert!(seen.contains(line) && seen.matches("still working").count() == 1, "{seen}");
    }
}

/// A local page's picture by its path, `file:///p/x.jpg`, as one text on every OS: Windows names the drive
/// the page is on (`file:///C:/p/x.jpg`, the URL standard's join).
fn no_drive(out: &str) -> String {
    regex::Regex::new("file:///[A-Za-z]:/").expect("regex").replace_all(out, "file:///").into_owned()
}

fn online(s: &Sandbox, cmd: &str) -> String {
    let o = s.cmd(cmd).env_remove("FATS_OFFLINE").env("FATS_ALLOW_PRIVATE", "1").output().expect("run");
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// consent.google's `http_400` once offered `fresh:true` forever: a page that failed the same way along the same path
/// a second time in the session is a count with no arrow; a site's 4xx with no keyed tier left gets no retry at all.
#[test]
fn a_repeated_page_failure_is_a_count() {
    use std::io::Write;
    let addr = serve(|head, c| {
        let status = match head {
            _ if head.starts_with("GET /bad") => "400 Bad Request",
            _ if head.starts_with("GET /akamai") => "200 OK",
            _ => "500 Internal Server Error",
        };
        let body = if status.starts_with("200") {
            "<html><head><title>Site Maintenance</title></head><body><h1>Oops! Something went wrong.</h1>\
             <p>Reference #18.5f3c1102.1760500000.1a2b3c</p></body></html>"
        } else {
            "<html><head><title>Oops</title></head><body>no</body></html>"
        };
        let h = format!("HTTP/1.1 {status}\r\ncontent-type: text/html\r\ncontent-length: {}\r\n\r\n", body.len());
        c.write_all(h.as_bytes()).and_then(|_| c.write_all(body.as_bytes()))
    });
    let s = Sandbox::new("again");
    let row =
        |out: &str| out.lines().find(|l| l.starts_with("u") && l.contains(" ! http_")).unwrap_or_default().to_string();
    let first = row(&online(&s, &format!("{addr}/flaky")));
    assert!(first.ends_with("→ fats u1 fresh:true") && !first.contains("failed"), "a 5xx retries once: {first}");
    let again = row(&online(&s, "u1 fresh:true"));
    // no key here: a count, and a key could serve it, never the call that failed twice
    let setup = " · failed 2/2 here → fats setup";
    assert!(again.ends_with(setup), "{again}");
    assert!(row(&s.run("u1").0).ends_with(setup), "the older copy says the count too");
    let bad = row(&online(&s, &format!("{addr}/bad")));
    assert!(bad.contains("! http_400") && bad.ends_with("→ fats setup"), "no keyed tier: no retry of a 4xx: {bad}");
    // Plain is no provider: its error page names none; a row with no arrow is a count from the first copy
    let page = |out: &str| out.lines().find(|l| l.contains(" ! error_page 1")).unwrap_or_default().to_string();
    let first = page(&online(&s, &format!("{addr}/akamai")));
    assert!(
        first.ends_with("· tried plain:error_page✗ · showing the best copy · failed 1/1 here → fats setup"),
        "{first}"
    );
    let again = page(&online(&s, "u4 fresh:true"));
    assert!(again.ends_with("showing the best copy · failed 2/2 here → fats setup"), "the error page again: {again}");
}

/// The command's own cap stopping the paid try is no failed command: the best copy is shown, its `budget_hit` row
/// carries the paid move, rc 0 (rc 1 reads as "the command failed", and a weak model stops there). The same wall
/// with no key to pay is a failed fetch, rc 1; so is a capped page with no copy to show.
#[test]
fn a_capped_page_with_a_copy_exits_zero() {
    use std::io::Write;
    let addr = serve(|_, c| {
        let body = "<html><head><title>Access denied</title></head><body><div id=\"px-captcha\"></div>\
                    <p>Please verify you are a human to read the news today.</p></body></html>";
        let h = format!("HTTP/1.1 403 Forbidden\r\ncontent-type: text/html\r\ncontent-length: {}\r\n\r\n", body.len());
        c.write_all(h.as_bytes()).and_then(|_| c.write_all(body.as_bytes()))
    });
    let run = |s: &Sandbox, cmd: &str| {
        let o = s.cmd(cmd).env_remove("FATS_OFFLINE").env("FATS_ALLOW_PRIVATE", "1").output().expect("run");
        (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code())
    };
    let s = Sandbox::new("capzero");
    s.keys();
    let (out, rc) = run(&s, &format!("{addr}/wall cost:0"));
    assert!(out.starts_with("= j1.u1 Access denied · ") && out.contains("\nPlease verify you are a human"), "{out}");
    let row = format!("\nu1 ! budget_hit oxy bd oxyjs not tried · $0.0040 > cost:0 → {PAY_ROW}\n");
    assert_eq!((out.contains(&row), rc), (true, Some(0)), "{out}");
    // a batch counts it as the cap's stop, still rc 0
    let (out, rc) = run(&s, &format!("{addr}/wall {addr}/wall2 cost:0"));
    assert!(out.starts_with("= j2 2 inputs · 0 ok · 2 ! budget_hit · ") && rc == Some(0), "{out}");
    // one paid key: the provider ask would repeat the row's own move
    let bd = Sandbox::new("capzero-bd");
    bd.keys();
    std::fs::remove_file(bd.dir.join("keys/oxylabs.key")).expect("oxy key");
    let (out, rc) = run(&bd, &format!("{addr}/wall cost:0"));
    let one = " → fats u1 fresh:true cost:0.0015 ≤$0.0015\n";
    assert!(out.contains(one) && !out.contains("said pay") && rc == Some(0), "{out}");
    let bare = Sandbox::new("capzero-nokey");
    let (out, rc) = run(&bare, &format!("{addr}/wall"));
    assert_eq!((out.contains(" ! captcha_wall"), rc), (true, Some(1)), "no key to pay: a failed fetch: {out}");
    // the cap stopped it and there is no copy to show (nothing listens on the port): a failed command
    let gone = Sandbox::new("capzero-gone");
    gone.keys();
    let closed = std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).expect("port");
    let (out, rc) = run(&gone, &format!("http://{closed}/gone cost:0"));
    assert!(out.starts_with("= j1.u1 ! budget_hit ") && rc == Some(1), "no copy: rc 1: {out}");
}

/// A `budget_hit` row's moves with both keys: the raised cap, then the provider ask for a user who said pay.
const PAY_ROW: &str =
    "fats u1 fresh:true cost:0.004 ≤$0.0040 · if the user said pay, fats u1 fresh:true provider:oxy ≤$0.0025";

/// ONE price glyph. No output prints `~$`: every fixture served keyless (its `no_key` rows), and the
/// dry, config, account and search rows in every key mode.
#[test]
fn every_price_is_one_glyph() {
    use std::io::Write;
    let addr = serve(|head, c| {
        let name = head.split(' ').nth(1).unwrap_or_default().trim_start_matches('/').to_string();
        let body = std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap_or_default();
        let h = format!("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\n\r\n", body.len());
        c.write_all(h.as_bytes()).and_then(|_| c.write_all(&body))
    });
    let s = Sandbox::new("keyprice");
    let mut outs = Vec::new();
    for f in std::fs::read_dir("tests/fixtures").expect("fixtures").flatten() {
        let name = f.file_name().to_string_lossy().into_owned();
        if name.ends_with(".html") {
            outs.push(online(&s, &format!("{addr}/{name}")));
        }
    }
    let cmds =
        ["config", "account", "https://1.1.1.1/ dry:true", "https://1.1.1.1/ js:true dry:true", "rust news dry:true"];
    for drop in [&["oxylabs.key", "brightdata.key"][..], &["oxylabs.key"], &["brightdata.key"], &[]] {
        s.keys();
        drop.iter().for_each(|k| std::fs::remove_file(s.dir.join("keys").join(k)).expect("key"));
        outs.extend(cmds.map(|c| {
            String::from_utf8_lossy(&s.cmd(c).env_remove("FATS_OFFLINE").output().expect("run").stdout).into_owned()
        }));
    }
    let all = outs.concat();
    assert!(all.contains("! no_key") && all.contains("up to $") && all.contains("≤$"), "{all}");
    assert!(!all.contains("~$"), "{}", all.lines().filter(|l| l.contains("~$")).collect::<Vec<_>>().join("\n"));
}

/// A chunked body (no Content-Length) that never ends is cut at the cap: `! too_big`, nothing read past it.
#[test]
fn endless_body_stops_at_the_cap() {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    static SENT: AtomicU64 = AtomicU64::new(0);
    let addr = serve(|_, c| {
        c.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ntransfer-encoding: chunked\r\n\r\n")?;
        let chunk = vec![b'a'; 1 << 20];
        for _ in 0..1024 {
            c.write_all(format!("{:x}\r\n", chunk.len()).as_bytes())?;
            c.write_all(&chunk)?;
            c.write_all(b"\r\n")?;
            SENT.fetch_add(1, Relaxed);
        }
        c.write_all(b"0\r\n\r\n")
    });
    let s = Sandbox::new("endless");
    let out = online(&s, &format!("{addr}/ provider:plain"));
    assert!(out.contains("too_big"), "{out}");
    std::thread::sleep(std::time::Duration::from_millis(200));
    let mib = SENT.load(Relaxed);
    assert!(mib <= fats::cli::MAX_BYTES / (1 << 20) + 16, "read {mib} MiB of an endless body");
    assert!(!s.dir.join("cache/t/u1/raw.html").exists(), "nothing stored");
}

/// A wrong product slug that the site redirects to its listing (a card shop) is served, but the
/// `=` line says it is another page; a trailing slash or `www.` alone is the same page. So is a short id made of
/// numbers (`/eli/reg/2021/821`) landing on a front page that keeps none of them; a short alias (`/releases/latest/`)
/// landing on the post it names is the page asked for.
#[test]
fn wrong_page_says_redirected() {
    use std::io::Write;
    let addr = serve(|head, c| {
        let listing = "/en/Pokemon/Products/Singles/Prismatic-Evolutions";
        let to = [
            ("Umbreon-ex-wrong", listing),
            ("GET /same ", "/same/"),
            ("/eli/reg/2021/821 ", "/TodayOJ/index.html"),
            ("/releases/latest/ ", "/2026/10/15/Rust-1.99.1/"),
        ];
        if let Some((_, to)) = to.iter().find(|(k, _)| head.contains(k)) {
            return c.write_all(
                format!("HTTP/1.1 302 Found\r\nlocation: {to}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .as_bytes(),
            );
        }
        let body = page("Prismatic Evolutions - Pokémon Singles", &"<p>Umbreon ex Espeon ex Sylveon ex</p>".repeat(20));
        let h = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        c.write_all(h.as_bytes()).and_then(|_| c.write_all(body.as_bytes()))
    });
    let s = Sandbox::new("redirected");
    let out = online(&s, &format!("{addr}/en/Pokemon/Products/Singles/Prismatic-Evolutions sylveon"));
    assert!(
        out.starts_with("= j1.u1 ") && out.contains("Sylveon") && out.contains("match:"),
        "a page + words = read then filter: {out}"
    );
    let out = online(&s, &format!("{addr}/en/Pokemon/Products/Singles/Prismatic-Evolutions/Umbreon-ex-wrong"));
    let head = out.lines().next().unwrap_or_default();
    let want = " · ! redirected /en/Pokemon/Products/Singles/Prismatic-Evolutions";
    assert!(head.starts_with("= j2.u2 Prismatic Evolutions ") && head.ends_with(want), "{out}");
    // every read of the page says it on its `=` line (`fats u10 €` once did not)
    for read in ["u2", "u2 sylveon", "u2#1", "u2 links", "u2 all", "u2 full"] {
        let head = s.run(read).0.lines().next().unwrap_or_default().to_string();
        assert!(head.starts_with("= j2.u2") && head.contains(" · ! redirected /en/"), "{read}: {head}");
    }
    let head = s.run("u1 sylveon").0;
    assert!(!head.contains("redirected"), "the page asked for has no flag: {head}");
    let out = online(&s, &format!("{addr}/same"));
    assert!(!out.contains("redirected"), "a trailing slash is the same page: {out}");
    let out = online(&s, &format!("{addr}/eli/reg/2021/821"));
    assert!(out.lines().next().is_some_and(|h| h.ends_with(" · ! redirected /TodayOJ/index.html")), "{out}");
    let out = online(&s, &format!("{addr}/releases/latest/"));
    assert!(!out.contains("redirected"), "an alias landing on its post is the page: {out}");
}

/// A detached fetch whose process died (no `done`, its lock free) reads as its failed copy, `! abandoned`, in the id,
/// `stats` and status; `fresh:true` is the fix. Its call in flight is a call we cut: `≤$X (unverified)` on the `=` line
/// and in `stats`, whose total adds it apart (a killed bd run was once missing from `stats`).
#[test]
fn dead_run_is_abandoned() {
    let s = Sandbox::new("abandoned");
    let cache = s.dir.join("cache");
    let file = |p: &std::path::Path, body: &str| {
        std::fs::create_dir_all(p.parent().expect("dir")).expect("mkdir");
        std::fs::write(p, body).expect("write");
    };
    let t = fats::cache::now() - 30.0;
    file(&cache.join("jobs.json"), &format!(r#"[{{"session":"t","cmd":"https://ex.com/a","t":{t}}}]"#));
    file(&cache.join("t/state.json"), r#"{"u":[{"url":"https://ex.com/a","dir":"","job":1}]}"#);
    let run = format!(
        r#"{{"url":"https://ex.com/a","cmd":"https://ex.com/a","job":1,"spent":0,"t":{t},"pid":4242,"status":"oxy running","done":null}}"#
    );
    file(&cache.join("t/run/u1.json"), &run);
    let (out, rc) = s.run("u1");
    assert_eq!(rc, 1, "{out}");
    // its read = its failed copy's (`Detached::cut`), the same before and after the sweep stores it
    let cut = "= j1.u1 ! abandoned · ex.com · tried oxy:abandoned✗≤$0.0010 (unverified) · 0ms spent $0.0000 + ≤$0.0010 unverified\n";
    assert!(out.starts_with(cut) && out.contains("\nu1 ! abandoned ex.com/a → fats u1 fresh:true"), "{out}");
    assert!(s.run("status").0.contains("\nrunning u1 ! abandoned 3"), "{}", s.run("status").0);
    let stats = s.run("stats").0;
    // and its `stats` row is that `!` row, what it ran before the move
    let row = "\nu1 ex.com · ! abandoned ex.com/a · oxylabs 0ms $0.0000 · oxy:abandoned✗≤$0.0010 (unverified) → fats u1 fresh:true";
    assert!(
        stats.contains(row) && stats.lines().next().is_some_and(|l| l.contains(" + ≤$0.0010 unverified")),
        "{stats}"
    );
    shape(&out);
}

/// A search result is an id with a URL and no body: `stats` names it `search→u1`, `fats u1` fetches it through
/// the tiers into the same id (the job stays the search's), and the search itself is one stats row.
#[test]
fn search_result_fills_its_own_id() {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    static HITS: AtomicU64 = AtomicU64::new(0);
    let addr = serve(|_, c| {
        HITS.fetch_add(1, Relaxed);
        let body = page("Result page", "<p>the page behind a search result</p><script src=/app.js></script>");
        write!(c, "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\n\r\n{body}", body.len())
    });
    let s = Sandbox::new("serp");
    let c = s.dir.join("cache");
    std::fs::create_dir_all(c.join("t/j1")).expect("dirs");
    std::fs::write(c.join(".fatsearch-cache"), "").expect("a root fats made: status sizes it");
    let url = format!("{addr}/a");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("now").as_secs_f64();
    let state = serde_json::json!({"u": [{"url": url, "dir": "", "job": 1, "serp": true}], "t": now - 5.0});
    std::fs::write(c.join("t/state.json"), state.to_string()).expect("state");
    std::fs::write(c.join("jobs.json"), format!(r#"[{{"session":"t","cmd":"result page","t":{now}}}]"#)).expect("jobs");
    let hop = r#"{"provider":"oxylabs","code":"ok","ms":900,"cost":0.001}"#;
    let hits = format!(r#"[{{"url":"{url}","title":"Result page","snippet":"behind"}}]"#);
    let meta = format!(
        r#"{{"url":"search:result page","provider":"oxylabs","ok":true,"ms":900,"cost":0.001,"t":1,"kind":"serp","hops":[{hop}],"first":1,"hits":{hits}}}"#
    );
    std::fs::write(c.join("t/j1/meta.json"), meta).expect("meta");
    std::fs::create_dir_all(s.dir.join("keys")).expect("keys");
    let row = serde_json::json!([now, [0.001, 0], c.join("t/j1")]).to_string(); // the store's ledger row
    std::fs::write(s.dir.join("keys/spend.log"), row).expect("ledger");
    let (out, _) = s.run("stats");
    assert!(out.starts_with("= stats s:t · 1 searches · 0 fetches · 900ms spent $0.0010"), "{out}");
    let rows = ["\nj1 search \"result page\" · 1 results · oxylabs 900ms $0.0010\n", "/a · not fetched → fats u1\n"];
    assert!(rows.iter().all(|r| out.contains(r)) && s.run("stats full").0.contains("$0.0010 · oxy✓"), "{out}");
    let status = s.run("").0;
    assert!(status.starts_with("= s:t 1 ids (1 search results not fetched) · last u1 127.0.0.1"), "{status}");
    // status counts the search apart, and health counts its call under oxy
    assert!(status.contains("· 0 pages · 1 searches ·") && status.contains("oxy 1/1 900ms ✓"), "{status}");
    assert!(s.run("j1").0.contains("\nsearch→j1.u1 127.0.0.1:"));
    let out = online(&s, "u1 provider:plain");
    // provider:plain is free, nothing to hedge
    assert!(out.starts_with("= j1.u1 ") && out.contains("Result page") && !out.contains("hedge:off"), "{out}");
    shape(&out);
    let st: serde_json::Value =
        serde_json::from_slice(&std::fs::read(c.join("t/state.json")).expect("state")).expect("json");
    assert_eq!(st["u"].as_array().map(Vec::len), Some(1), "filled, not appended: {st}");
    assert_eq!((st["u"][0]["dir"].as_str(), st["u"][0]["job"].as_u64()), (Some("t/u1"), Some(1)));
    let (out, _) = s.run("stats");
    assert!(
        out.contains("1 searches · 1 fetches") && out.contains("\nsearch→u1 127.0.0.1 · Result page · plain "),
        "{out}"
    );
    let o = s.run("u1 behind").0;
    // a plain-only fetch of a thin page keeps its `js_shell` flag on every read, the filter too
    let head = o.lines().next().unwrap_or_default();
    assert!(head.starts_with("= j1.u1 filter \"behind\"") && head.ends_with(" · ! js_shell"), "from the cache: {o}");
    // `fresh:true` on a search result fetches again into the same id (a fetched page would get a new one)
    let dry = online(&s, "u1 fresh:true provider:plain dry:true");
    assert!(dry.contains("fetch plain → fills u1") && !dry.contains("hedge:off"), "{dry}");
    assert_eq!(HITS.load(Relaxed), 1);
    let out = online(&s, "u1 fresh:true provider:plain");
    let head = out.lines().next().unwrap_or_default();
    assert!(head.starts_with("= j1.u1 Result page") && head.ends_with(" · ! js_shell"), "a failed fetch: {out}");
    assert!(!out.contains("cache"), "a failed fetch: {out}");
    assert_eq!(HITS.load(Relaxed), 2, "fetched again, not read from the cache");
    let st: serde_json::Value =
        serde_json::from_slice(&std::fs::read(c.join("t/state.json")).expect("state")).expect("json");
    assert_eq!((st["u"].as_array().map(Vec::len), st["u"][0]["serp"].as_bool()), (Some(1), Some(true)), "{st}");
}

/// A search with no network, no key or a bad mod is a record with its fix; `dry:true` shows the plan and the
/// most it can cost; `site:` / `fats <host> <words>` and `since:` shape the query.
#[test]
fn search_without_network_is_a_record() {
    let s = Sandbox::new("search");
    let (out, rc) = s.run("cheapest proxies de");
    assert_eq!(rc, 1);
    assert!(out.starts_with("= ! offline search \"cheapest proxies de\""), "{out}");
    shape(&out);
    let keyless = |cmd: &str| {
        let o = s.cmd(cmd).env_remove("FATS_OFFLINE").output().expect("run");
        (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code())
    };
    let (out, rc) = keyless("cheapest proxies");
    assert_eq!(rc, Some(1));
    // pages need no key, search needs one: the default (cheapest) key's row first
    let eq = "= ! no_key search \"cheapest proxies\" · web search needs one key (pages work without) · 0ms\n";
    assert!(out.starts_with(eq), "{out}");
    assert!(out.contains("0ms\n! no_key oxy · would try web search ≤$0.0010 → run fats setup oxy, give the user its 3 lines\n! no_key bd"), "{out}");
    shape(&out);
    let (out, _) = keyless("https://x.com/a/b.pdf preis dry:true");
    assert!(
        out.contains("x.com/a/b.pdf fetch plain → new u1") && !out.contains("search \""),
        "a page + words reads it: {out}"
    );
    let (out, _) = keyless("uni-heidelberg.de studium since:2026-01 country:FR limit:50 dry:true");
    assert!(
        out.contains(
            "search \"site:uni-heidelberg.de studium after:2026-01-01\" · nothing can search · $0 · ! no_key oxy"
        ),
        "{out}"
    );
    shape(&out);
    assert!(s.run("rust since:soon").0.starts_with("? since:soon"));
    assert!(s.run("rust provider:plain").0.starts_with("? provider:plain cannot search"));
    let (out, _) = s.run("rust news more");
    assert!(out.contains("search \"rust news more\""), "free text keeps reserved words as text: {out}");
    assert!(s.run("config").0.contains("hedge search 2.5 s · fetch 5 s · default (both billed when both run)"));
    // The search row names only the providers a key is present for
    let search =
        |s: &Sandbox| s.run("config").0.lines().find(|l| l.starts_with("search ")).unwrap_or_default().to_string();
    assert!(search(&s).starts_with("search oxy $0.0010, then bd (Google via the zone) $0.0015 · "));
    std::fs::create_dir_all(s.dir.join("keys")).expect("key dir");
    std::fs::write(s.dir.join("keys/oxylabs.key"), "oxyK3y-cli").expect("oxy key");
    assert!(search(&s).starts_with("search oxy $0.0010 · 10 results"), "{}", search(&s));
}

/// A keyed search `dry:true`: the route with the 2.5 s search hedge (search p50 ~2 s), bd beside oxy
/// only inside `wait:`, the hedge row with its source, and `cost:` dropping bd.
#[test]
fn search_dry_shows_the_hedge() {
    let s = Sandbox::new("search-dry");
    let keyed = |cmd: &str| {
        let o = s.cmd(cmd).env_remove("FATS_OFFLINE").env("BRIGHTDATA_UNLOCKER_ZONE", "z").output().expect("run");
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    s.auth(&[("oxy", "x"), ("bd", "y")]);
    // the fetch router's driver: Bright Data starts beside Oxylabs after the hedge when `wait:` leaves time for
    // it, `cost:` drops it
    let out = keyed("rust news site:blog.rust-lang.org country:fr dry:true wait:30");
    let want = "search \"site:blog.rust-lang.org rust news\" via oxy ∥ bd after 2.5s · limit:10 · country:FR · wait:30s → new u1-u10 · up to $0.0025";
    assert!(out.contains(want), "{out}");
    // the search hedge is 2.5 s (search p50 ~2 s), so bd starts beside oxy inside the default wait: 5 s
    let out = keyed("rust news dry:true");
    assert!(out.contains("via oxy ∥ bd after 2.5s · limit:10"), "{out}");
    assert!(out.contains("\nhedge search 2.5 s · fetch 5 s · default\n"), "{out}");
    assert!(keyed("rust news wait:2 dry:true").contains("via oxy → bd · limit:10"), "hedged past wait: never beside");
    let out = keyed("rust news hedge:0 dry:true");
    assert!(out.contains("via oxy ∥ bd after 0s") && out.contains("\nhedge 0 s · flag hedge:0\n"), "{out}");
    assert!(
        keyed("rust news cost:0.001 dry:true").contains("via oxy · limit:10 · wait:30s → new u1-u10 · up to $0.0010")
    );
    // Bright Data alone leads (~10 s answers): inside the default 30 s it runs inline and keeps its retry (S6);
    // past a `wait:5` it runs detached, once, under `resume`'s patience (one call at the retry's time); the row
    // echoes the asked `wait:` (`wait:3` once printed `wait:30s`)
    s.auth(&[("bd", "y")]);
    let bd = |q: &str| {
        let mut bd = s.cmd(q);
        let o = bd.env_remove("FATS_OFFLINE").env("BRIGHTDATA_UNLOCKER_ZONE", "z");
        String::from_utf8_lossy(&o.output().expect("run").stdout).into_owned()
    };
    let out = bd("rust news dry:true");
    assert!(out.contains("via bd → bd retry · limit:10 · wait:30s → new u1-u10 · up to $0.0030"), "{out}");
    let out = bd("rust news dry:true wait:5");
    assert!(out.contains("via bd · limit:10 · wait:5s → new u1-u10 · up to $0.0015"), "{out}");
}

/// A bare name is a host only with a real TLD; a TLD that is also a file extension (`.rs`) only leading the command;
/// a closing bare `json` on a search is a word. Each case's dry plan: the query searched, or the page fetched.
#[test]
fn a_host_needs_a_real_tld_and_json_ends_a_search_as_a_word() {
    let s = Sandbox::new("search-words");
    s.auth(&[("oxy", "x")]);
    let keyed = |cmd: &str| {
        let mut run = s.cmd(&format!("{cmd} dry:true"));
        let o = run.env_remove("FATS_OFFLINE").output().expect("run");
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    for (cmd, query) in [
        ("opencode mcp config opencode.yaml", "opencode mcp config opencode.yaml"),
        ("pyproject.toml build backend options", "pyproject.toml build backend options"),
        ("nextjs next.config.mjs", "nextjs next.config.mjs"),
        ("main.go error", "main.go error"),
        ("logo.png size", "logo.png size"),
        ("cargo build main.rs", "cargo build main.rs"),
        ("build lib.so", "build lib.so"),
        // a bare name mid-query, or written as a name, is a word: no one-site search
        ("rust x.ai", "rust x.ai"),
        ("Purchasing.NYC Lakewood New Jersey registered agent", "Purchasing.NYC Lakewood New Jersey registered agent"),
        ("contact at acmebuilders.com phone", "contact at acmebuilders.com phone"),
        ("Booking.com hotels Lisbon", "Booking.com hotels Lisbon"),
        ("site:x.com y", "site:x.com y"),
        ("vite config json", "vite config json"),
        ("docs.rs tokio spawn", "site:docs.rs tokio spawn"),
        ("main.rs error handling", "site:main.rs error handling"),
        ("example.com rust json", "site:example.com rust json"),
        ("rust blog.rust-lang.org", "rust blog.rust-lang.org"),
        ("serde site:docs.rs/serde", "site:docs.rs/serde serde"),
    ] {
        let out = keyed(cmd);
        assert!(out.contains(&format!("\nsearch \"{query}\" via oxy")), "{cmd}: {out}");
    }
    for (cmd, host) in [
        ("https://docs.rs", "docs.rs"),
        ("example.com", "example.com"),
        ("x.co", "x.co"),
        ("a.io", "a.io"),
        ("Booking.com", "Booking.com"),
    ] {
        let out = keyed(cmd);
        assert!(
            out.starts_with(&format!("= dry https://{host} ")) && out.contains(&format!("\n{host} fetch ")),
            "{out}"
        );
    }
    assert!(keyed("vite config json:true").starts_with('{'), "json:true is the output mod");
}

/// Beside search words a path must exist and `-` be the whole command; a search engine's query key is a search
/// word as typed, while an unknown key of ours still asks.
#[test]
fn search_words_that_look_like_paths_or_operators_search() {
    let s = Sandbox::new("path-words");
    s.file("notes.md", "# Notes\nhello world\n");
    s.auth(&[("oxy", "x")]);
    let keyed = |cmd: &str| {
        let mut run = s.cmd(&format!("{cmd} dry:true"));
        let o = run.env_remove("FATS_OFFLINE").output().expect("run");
        (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code())
    };
    for q in [
        "NORTHROP GRUMMAN CORP /DE/ company business",
        "irs liaison committee - tei svc company",
        "rust filetype:pdf",
        "before:2026-01 rust",
        "intitle:fatsearch",
        "rust inurl:docs intext:spawn after:2025 allintitle:tokio allinurl:x",
    ] {
        let (out, rc) = keyed(q);
        assert!(out.contains(&format!("\nsearch \"{q}\" via oxy")) && rc == Some(0), "{q}: {out}");
    }
    for (cmd, row) in
        [("./notes.md", "\n./notes.md 20B local"), ("notes.md hello", "\nnotes.md 20B local"), ("-", "\n- 0B local")]
    {
        let (out, _) = keyed(cmd);
        assert!(out.contains(row) && !out.contains("\nsearch "), "{cmd}: {out}");
    }
    let (out, rc) = keyed("rust countr:lt");
    assert!(out.starts_with("? countr: unknown") && rc == Some(2), "{out}");
    let mut run = s.cmd("-");
    let mut child = run.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().expect("run");
    std::io::Write::write_all(&mut child.stdin.take().expect("stdin"), b"hello from a pipe\n").expect("write");
    let out = String::from_utf8_lossy(&child.wait_with_output().expect("wait").stdout).into_owned();
    assert!(out.starts_with("= j1.u1 stdin:") && out.contains("hello from a pipe"), "{out}");
}

/// A local path never reaches a provider: a token with a `./ ../ ~/ file:` prefix, a drive, a `/` path of 2+
/// segments or a document extension is a file whatever stands beside it, missing or not (`no_file`), and no search
/// row carries it, dry or real. A missing name that is also a host (`notes.md`) asks which, $0. Offline: a search
/// would still print its `search "…"` row there.
#[test]
fn a_local_path_never_reaches_a_search() {
    let s = Sandbox::new("path-leak");
    s.auth(&[("oxy", "x")]);
    let keyed = |cmd: &str| {
        let o = s.cmd(cmd).output().expect("run");
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    // a file URL names this OS's path (Windows: a drive; `file:///tmp/…` has no Windows path, an ask)
    let url = if cfg!(windows) { "file:///C:/fats-nope/a" } else { "file:///tmp/fats-nope/a" };
    let paths = ["./secret.txt", "secret.txt", "../keyring", "~/notes/plan", url, "b.pdf"];
    let windows =
        ["C:\\nope\\a", "C:/nope/a", ".\\secret\\plan", "..\\..\\hr", "~\\x", "\\\\corp-fs\\hr\\pay", "Documents\\pay"];
    let shaped = ["~alice/secret", "$HOME/secret/plan", "internal/hr/salaries", "Documents/plan.key", "id.pem"];
    let rest = ["/tmp/fats-nope/a.pdf", "/tmp/fats-nope/no-ext"].into_iter().chain(windows).chain(shaped);
    for path in paths.into_iter().chain(rest) {
        for cmd in [format!("acme phone {path}"), format!("{path} acme phone"), format!("search {path} acme")] {
            let dry = keyed(&format!("{cmd} dry:true"));
            assert!(!dry.contains("search \"") && !dry.contains(" via "), "{cmd}: {dry}");
            let real = keyed(&cmd);
            assert!(real.starts_with("= ! no_file ") && !real.contains("search \""), "{cmd}: {real}");
        }
    }
    // a dry row of a missing file says the run's verdict, $0, and plans nothing
    let dry = keyed("./secret.txt dry:true");
    assert!(dry.contains("\n! no_file ./secret.txt · $0") && !dry.contains("run it:"), "{dry}");
    let o = s.cmd("notes.md hello").output().expect("run");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.starts_with("? notes.md: a missing file or a site · type ./notes.md or https://notes.md"), "{out}");
    assert_eq!(o.status.code(), Some(2));
    // two segments, a one-dot name beside words, a TLD after a dot are words, missing here: the search is planned
    for cmd in ["TCP/IP tuning", "rust and/or tokio", "AC/DC rust", "cargo build main.rs", ".NET runtime"] {
        let dry = keyed(&format!("{cmd} dry:true"));
        assert!(dry.contains(&format!("search \"{cmd}\"")), "{cmd}: {dry}");
    }
}

/// A dot-led name is a path wherever it stands, a bare name that is a file here is that file, a directory asks and a
/// lone `main.rs` that is no file asks file or site: none of them is searched, CLI or MCP. A key or credential file
/// (`.env`, `.env.*`, `id_rsa`, `*.key`) is refused by its name, never read or stored; missing alone, `no_file`.
#[test]
fn dotfiles_bare_files_and_directories_stay_local() {
    let s = Sandbox::new("dotfiles");
    s.auth(&[("oxy", "x")]);
    let keyed = |cmd: &str| {
        let o = s.cmd(cmd).output().expect("run");
        (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code())
    };
    for cmd in [".env", "summary .env", "id_rsa", "id_ed25519", "authorized_keys"] {
        let f = cmd.split(' ').next_back().unwrap_or_default();
        assert!(keyed(cmd).0.starts_with(&format!("= ! no_file {f} ")), "{cmd}: {:?}", keyed(cmd));
        assert!(!keyed(&format!("{cmd} dry:true")).0.contains("search \""), "{cmd}");
    }
    assert!(keyed("id_rsa backup dry:true").0.contains("search \"id_rsa backup\""), "a missing bare name is a word");
    std::fs::create_dir_all(s.dir.join("work/src")).expect("a dir here");
    let (out, rc) = keyed("acme phone ./src");
    assert!(out.starts_with("? ./src is a directory · name a file in it") && rc == Some(2), "{out}");
    let (out, rc) = keyed("main.rs");
    assert!(out.starts_with("? main.rs: a missing file or a site · type ./main.rs") && rc == Some(2), "{out}");
    let secrets = [".env", "id_rsa", ".env.local", "server.key"];
    for f in secrets {
        s.file(f, "FATS_TEST_SECRET=1\n");
    }
    s.file("main.rs", "FATS_TEST_PLAIN=1\n");
    for cmd in secrets.iter().map(|f| f.to_string()).chain(["summary .env".into(), "id_rsa backup".into()]) {
        let f = cmd.split(' ').find(|w| secrets.contains(w)).unwrap_or_default();
        let (out, rc) = keyed(&cmd);
        let row = format!("= ! secret_file {f} · a key or credential file is not read · $0\n");
        assert!(out.starts_with(&row) && rc == Some(1), "{cmd}: {out}");
        let (dry, _) = keyed(&format!("{cmd} dry:true"));
        assert!(dry.contains(&format!("\n! secret_file {f} · $0")) && !dry.contains("search \""), "{cmd}: {dry}");
    }
    let dry = keyed("main.rs dry:true").0;
    assert!(dry.contains("B local") && !dry.contains("search \""), "{dry}");
    assert!(keyed("main.rs").0.contains("FATS_TEST_PLAIN"), "a plain file here is read");
    let fats = |cmd: &str| serde_json::json!({"name": "fats", "arguments": {"cmd": cmd}});
    let cmds = ["main.rs dry:true", ".env", "id_ed25519", "acme phone /etc", "/etc/passwd"];
    let want = [
        "B local", "= ! secret_file .env ", "= ! no_file id_ed25519 ", "= ! not_allowed /etc ",
        "= ! not_allowed /etc/passwd ",
    ];
    let by_id = mcp_session(&s, &cmds.iter().map(|c| fats(c)).collect::<Vec<_>>());
    for (i, (cmd, want)) in cmds.iter().zip(want).enumerate() {
        let text = by_id[&(i as u64 + 3)]["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
        assert!(text.contains(want) && !text.contains("search \""), "mcp {cmd}: {text}");
    }
    // nothing of a refused file reached the cache; the plain file read above did
    fn walk(p: &std::path::Path, out: &mut Vec<u8>) {
        for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                walk(&e.path(), out);
            } else {
                out.extend(std::fs::read(e.path()).unwrap_or_default());
            }
        }
    }
    let mut stored = Vec::new();
    walk(&s.dir.join("cache"), &mut stored);
    let has = |needle: &[u8]| stored.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"FATS_TEST_PLAIN") && !has(b"FATS_TEST_SECRET"), "the cache holds a refused file's bytes");
}

/// `dry:true` spends nothing, the network included: a host is judged by its name alone (a lookup here gave
/// `dns_timeout` with no network and `no_host` for a name nobody serves). A name that resolves nowhere gets its route;
/// no real TLD is `no_host`, a loopback name `private_address`; under `FATS_OFFLINE=1` a dry is `offline`.
#[test]
fn a_dry_run_resolves_nothing() {
    let s = Sandbox::new("dry-nolookup");
    let dry = |cmd: &str, offline: bool| {
        let mut c = s.cmd(&format!("{cmd} dry:true dns_wait:1"));
        let o = if offline { c.output() } else { c.env_remove("FATS_OFFLINE").output() }.expect("run");
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let out = dry("https://fats-dry-nowhere-zz9.com/a", false);
    assert!(out.contains("\nfats-dry-nowhere-zz9.com/a fetch plain → new u1 · up to $"), "{out}");
    let out = dry("https://dead.invalid/a", false);
    assert!(out.contains("\ndead.invalid/a ! no_host dead.invalid · new u1 · $0 → "), "{out}");
    // a label no resolver can ask for (> 63 chars, empty, a hyphen at an end) is `no_host` by its shape, never a
    // lookup blamed on the network
    let long = "a".repeat(64);
    for host in [format!("{long}.com"), "a..com".into(), "-a.com".into(), "a-.com".into()] {
        let out = dry(&format!("https://{host}/a"), false);
        assert!(out.contains(&format!(" ! no_host {host} · new u1 · $0 → ")) && !out.contains(" fetch "), "{out}");
    }
    let out = dry("http://localhost:8080/a", false);
    assert!(out.contains(" ! private_address → ") && !out.contains(" fetch "), "{out}");
    let out = dry("https://fats-dry-nowhere-zz9.com/a", true);
    assert!(out.contains(" not cached → ! offline (FATS_OFFLINE) · $0") && !out.contains("dns_timeout"), "{out}");
}

/// One real fetch, run by hand: `FATS_LIVE=1 cargo test --locked -- --ignored live_plain`.
#[test]
#[ignore = "network; run with FATS_LIVE=1"]
fn live_plain() {
    if std::env::var("FATS_LIVE").as_deref() != Ok("1") {
        return;
    }
    let s = Sandbox::new("live");
    let o = s.cmd("https://example.com/ provider:plain fresh:true").env_remove("FATS_OFFLINE").output().expect("run");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.starts_with("= j1.u1 Example Domain") && out.contains("plain "), "{out}");
    shape(&out);
}

/// One real search (~$0.001), run by hand with the keys in the env:
/// `FATS_LIVE=1 cargo test --locked -- --ignored live_search`.
#[test]
#[ignore = "network; run with FATS_LIVE=1"]
fn live_search() {
    if std::env::var("FATS_LIVE").as_deref() != Ok("1") {
        return;
    }
    let s = Sandbox::new("live-search");
    let o = s.cmd("heidelberg university limit:3").env_remove("FATS_OFFLINE").output().expect("run");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.starts_with("= j1 search \"heidelberg university\" · 3 results · "), "{out}");
    assert!(out.lines().nth(1).is_some_and(|l| l.starts_with("u1  ") && l.contains("uni-heidelberg.de")), "{out}");
    shape(&out);
}

/// Loopback, private and link-local hosts are refused before any request (a prompt-injected metadata URL reads
/// nothing); `FATS_ALLOW_PRIVATE=1` lets a local dev server through, and `config` shows which is in effect.
#[test]
fn private_hosts_are_refused() {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    static HITS: AtomicU64 = AtomicU64::new(0);
    let addr = serve(|_, c| {
        HITS.fetch_add(1, Relaxed);
        let body = "<title>Local</title><p>a page from the dev server</p>";
        c.write_all(
            format!("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\n\r\n{body}", body.len())
                .as_bytes(),
        )
    });
    let s = Sandbox::new("private");
    let shut = |cmd: &str| {
        let o = s.cmd(cmd).env_remove("FATS_OFFLINE").env_remove("FATS_ALLOW_PRIVATE").output().expect("run");
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let local = addr.replace("127.0.0.1", "localhost");
    for url in [addr.as_str(), &local, "http://10.0.0.1/", "http://169.254.169.254/latest/meta-data/", "http://[::1]/"]
    {
        let out = shut(url);
        assert!(out.starts_with("= ! private_address "), "{out}");
        assert!(out.contains("! private_address → private address refused · see fats config"), "{out}");
        assert!(!out.contains("FATS_ALLOW_PRIVATE"), "the refusal never prints its bypass: {out}");
    }
    let dry = shut(&format!("{addr} dry:true"));
    assert!(dry.contains("! private_address → private address refused") && !dry.contains("FATS_ALLOW_PRIVATE"));
    assert!(!dry.contains("run it:"), "nothing to run: {dry}");
    let rc = s.cmd(&format!("{addr} dry:true")).env_remove("FATS_ALLOW_PRIVATE").output().expect("run").status;
    assert_eq!(rc.code(), Some(1), "a refused plan fails");
    assert_eq!(HITS.load(Relaxed), 0, "no request reached the server");
    assert!(shut("config").contains("allow_private false · default"));
    assert!(online(&s, &addr).starts_with("= j"), "allowed when set");
    assert_eq!(HITS.load(Relaxed), 1);
}

/// Several inputs = one job: a `=` summary first, then each input's block; json = one record per input, in
/// input order with `i`; a partial failure exits 0, a total one 1.
#[test]
fn several_inputs_are_one_job() {
    let s = Sandbox::new("multi");
    s.file("a.html", &page("Alpha", &"<p>alpha words</p>".repeat(20)));
    s.file("b.html", &page("Beta", &"<p>beta words</p>".repeat(20)));
    let (out, rc) = s.run("./a.html ./b.html ./nope.html");
    assert_eq!(rc, 0, "{out}");
    let head = out.lines().next().unwrap_or_default();
    assert!(head.starts_with("= j1 3 inputs · 2 ok · 1 ! no_file · ") && head.ends_with("ms"), "free: {head}");
    assert!(
        out.contains("\n= j1.u1 Alpha") && out.contains("\n= j1.u2 Beta") && out.contains("\n= ! no_file ./nope.html")
    );
    assert!(s.run("j1").0.starts_with("= j1 2 pages"), "one job holds both pages");
    let (out, rc) = s.run("./a.html ./nope.html ./b.html json:true");
    assert_eq!(rc, 0);
    let recs: Vec<serde_json::Value> = out.lines().map(|l| serde_json::from_str(l).expect("json line")).collect();
    let got: Vec<(u64, &str, &str)> = recs
        .iter()
        .map(|r| (r["i"].as_u64().unwrap_or(9), r["id"].as_str().unwrap_or("-"), r["error"].as_str().unwrap_or("")))
        .collect();
    assert_eq!(got, [(0, "j2.u1", ""), (1, "-", "no_file"), (2, "j2.u2", "")], "input order, a hit reads its id");
    assert!(recs[0]["lines"][0].as_str().unwrap_or_default().contains("Alpha"), "each record carries its own lines");
    assert!(recs.iter().all(|r| r["url"].is_string() && r["ms"].is_u64() && r["cost"].is_f64()));
    let (out, rc) = s.run("./nope.html ./gone.html");
    assert_eq!(rc, 1);
    assert!(out.starts_with("= 2 inputs · 0 ok · 2 ! no_file · "), "{out}");
}

/// `stats` is the session index: the last 20 rows and what was cut, `limit:N`, `all`, a word over host and title
/// (never a web search), `json` every row whole.
#[test]
fn stats_is_a_session_index() {
    let s = Sandbox::new("stats-index");
    let names: Vec<String> = (1..=26).map(|i| format!("./p{i}.html")).collect();
    for i in 1..=26 {
        let title = if i % 5 == 0 { format!("Kamado grill {i}") } else { format!("Page {i}") };
        s.file(&format!("p{i}.html"), &page(&title, "<p>Some text that makes a short page.</p>"));
    }
    assert_eq!(s.run(&names.join(" ")).1, 0);
    let rows = |out: &str| out.lines().filter(|l| l.starts_with('u')).map(String::from).collect::<Vec<_>>();
    let (out, rc) = s.run("stats");
    let got = rows(&out);
    assert!(rc == 0 && out.starts_with("= stats s:t · 0 searches · 26 fetches · "), "{out}");
    assert!(got.len() == 20 && got[0].starts_with("u7 p7.html · Page 7 · local ") && got[19].starts_with("u26 "));
    assert!(got.iter().all(|r| r.ends_with("tok") && r.chars().count() <= 90), "{out}");
    assert!(out.contains("\n… 6 more · fats stats all\n"), "{out}");
    let out = s.run("stats limit:5").0;
    assert!(rows(&out).len() == 5 && out.contains("\nu22 ") && out.contains("\n… 21 more · fats stats all\n"));
    let out = s.run("stats all").0;
    assert!(rows(&out).len() == 26 && out.contains("\nu1 p1.html · Page 1 · ") && !out.contains(" more · "));
    let out = s.run("stats kamado").0;
    let got = rows(&out);
    assert!(got.len() == 5 && got.iter().all(|r| r.contains(" · Kamado grill ")) && !out.contains(" more"), "{out}");
    let out = s.run("stats KAMADO limit:2").0;
    assert!(rows(&out).len() == 2 && out.contains("\n… 3 more · fats stats KAMADO all\n"), "{out}");
    assert_eq!(rows(&s.run("stats KAMADO all").0).len(), 5, "the arrow's own command lists the rest");
    assert!(s.run("stats nowhere").0.contains("\n0 of 26 rows hold \"nowhere\" · all: fats stats all\n"));
    let rec: serde_json::Value = serde_json::from_str(&s.run("stats kamado json").0).expect("one record");
    let lines = rec["lines"]
        .as_array()
        .map_or(0, |l| l.iter().filter(|l| l.as_str().is_some_and(|l| l.contains(" raw "))).count());
    assert_eq!(lines, 5, "json = the matching rows whole: {rec}");
}

/// The same file read again is the stored snapshot under the same id (a hit is a read: a new job, the same u), not
/// a second copy; changed bytes are a new id.
#[test]
fn identical_reingest_is_one_snapshot() {
    let s = Sandbox::new("dedupe");
    s.file("a.html", SECTIONS);
    let (first, _) = s.run("./a.html");
    let (again, rc) = s.run("./a.html");
    assert_eq!(rc, 0, "{again}");
    assert!(first.lines().next().is_some_and(|l| l.contains(" · local ") && !l.contains("spent")), "{first}");
    assert!(again.starts_with("= j2.u1 ") && again.lines().next().is_some_and(|l| l.contains("· cache ")), "{again}");
    s.file("a.html", &format!("{SECTIONS}<p>one more line</p>"));
    let (changed, _) = s.run("./a.html");
    assert!(changed.starts_with("= j3.u2 ") && changed.contains(" · local "), "a new read: {changed}");
    let snaps = |d: &str| std::fs::read_dir(s.dir.join(d)).map(|r| r.flatten().filter(|e| e.path().is_dir()).count());
    assert_eq!(snaps("cache/t").ok(), Some(2), "u1 and u2 stored");
    let (stats, _) = s.run("stats");
    assert!(stats.contains("\nu2 ") && !stats.contains("\nu3 "), "{stats}");
    assert!(s.run("j2").0.starts_with("= j2 1 pages · "), "the job that read it lists it");
    let (fresh, _) = s.run("./a.html fresh:true");
    assert!(fresh.starts_with("= j4.u3 ") && fresh.contains(" · local "), "a refetch is a new id: {fresh}");
}

/// The cache root is created readable by its owner only: it keeps every command (queries too), the sites fetched
/// and account balances. `fats` shows the mode.
#[cfg(unix)]
#[test]
fn cache_root_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let s = Sandbox::new("mode");
    s.file("a.html", &page("Private", "<p>mine</p>"));
    let (out, rc) = s.run("./a.html");
    assert_eq!(rc, 0, "{out}");
    let mode = std::fs::metadata(s.dir.join("cache")).expect("cache root").permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    assert!(s.run("status").0.contains(" (mode 700) · "), "status shows it");
}

/// A text document (plain text, CSV, Markdown, JSON, XML, RSS, a sitemap) is served as it is from plain: `lN`
/// rows, the kind and size on the `=` line, an outline row for JSON and XML, $0, no provider, no route; a long
/// one shows 300 lines and names the next section, which reads on; the filter finds lines by id.
#[test]
fn text_documents_are_served_as_they_are() {
    use std::io::Write;
    let long: String = (1..=700).map(|i| format!("row {i}\n")).collect();
    let addr = serve(move |head, c| {
        let path = head.split_whitespace().nth(1).unwrap_or("/");
        let (ct, body) = match path {
            "/a.lua" => ("text/plain; charset=utf-8", "local x = 1\nreturn x\n".to_string()),
            "/c.csv" => ("text/csv", "name,price\nbread,2\n".into()),
            // raw Markdown as GitHub serves it; a `text/markdown` answer is a page (`text::markdown`)
            "/r.md" => ("text/plain", "# Notes\n\nkeep this\n".into()),
            "/d" => ("application/json", r#"{"ok":true,"data":[1,2,3],"meta":{"page":1}}"#.into()),
            "/feed" => {
                ("application/rss+xml", "<rss><channel><item><title>First post</title></item></channel></rss>".into())
            }
            "/map" => (
                "text/plain",
                "<?xml version=\"1.0\"?>\n<urlset><url><loc>a</loc></url><url><loc>b</loc></url></urlset>".into(),
            ),
            _ => ("text/plain", long.clone()),
        };
        write!(c, "HTTP/1.1 200 OK\r\ncontent-type: {ct}\r\ncontent-length: {}\r\n\r\n{body}", body.len())
    });
    let s = Sandbox::new("text");
    let rows = [
        ("/a.lua", "text", "2 lines", "\nlocal x = 1\n", ""),
        ("/c.csv", "text", "2 lines", "\nbread,2\n", ""),
        ("/r.md", "text", "3 lines", "\nkeep this\n", ""),
        ("/d", "json", "1 line", "\n{\"ok\"", "#0 info | keys: ok, data[3], meta{1}"),
        ("/feed", "xml", "1 line", "\n<rss>", "#0 info | 1 items: First post"),
        ("/map", "xml", "2 lines", "\n<urlset>", "#0 info | <urlset> url ×2"),
    ];
    for (i, (path, kind, lines, row, info)) in rows.iter().enumerate() {
        let out = online(&s, &format!("{addr}{path}"));
        let id = format!("u{}", i + 1);
        let head = out.lines().next().unwrap_or_default();
        assert!(head.contains(&format!(" · {kind} 1 KB {lines} · ")) && head.contains("plain "), "{out}");
        assert!(!head.contains("spent"), "a free fetch names no spend: {out}");
        assert!(out.contains(row) && out.contains("(whole document shown)"), "{out}");
        assert!(info.is_empty() || out.contains(&format!("{id}{info}")), "{out}");
        assert!(!out.contains(" ! "), "no provider, no row: {out}");
        shape(&out);
    }
    let dir = s.dir.join("cache/t");
    assert!(dir.join("u4/raw.json").exists() && dir.join("u1/raw.txt").exists() && dir.join("u6/raw.xml").exists());
    let out = online(&s, &format!("{addr}/long.txt"));
    // 300 lines a section, read to the 1,000-token cap (`clean::tok`); a line carries no id: `lN` is a link
    assert!(out.contains(" · text 6 KB 700 lines · ") && out.contains("\nrow 1\n") && !out.contains("l1 "), "{out}");
    assert!(out.contains(" more → fats more · filter: fats u7 <words>"), "{out}");
    let next = s.run("u7#2").0;
    assert!(next.contains("\nrow 301\n") && next.contains("lines 301-600"), "{next}");
    let hit = s.run("u7 650 match:words").0;
    assert!(hit.contains("u7#3 row 650"), "{hit}");
    shape(&hit);
}

/// Every id a read prints opens: an island's lines carry no `lN` (that name is the page's links), so a line a
/// model copies from the read never answers `? lN unknown`.
#[test]
fn a_printed_line_id_opens() {
    let s = Sandbox::new("lineids");
    let rows: Vec<String> = (0..1000).map(|i| format!(r#"{{"id":{i},"name":"item {i}"}}"#)).collect();
    let body = format!(
        "<p>Lead of the <a href=\"https://ex.com/w\">widget</a>.</p>\
         <script id=\"__NEXT_DATA__\" type=\"application/json\">{{\"items\":[{}]}}</script>",
        rows.join(",")
    );
    s.file("p.html", &page("Widget", &body));
    let first = s.run("./p.html").0;
    let read = s.run("u1#2").0 + &s.run("u1 item 7").0;
    assert!(first.contains("island script") && read.contains(r#""name": "item 7""#), "{first}{read}");
    let words = read.split(|c: char| !c.is_ascii_alphanumeric());
    for l in words.filter(|w| w.len() > 1 && w.starts_with('l') && w[1..].bytes().all(|b| b.is_ascii_digit())) {
        let (out, rc) = s.run(&format!("u1 {l} dry:true"));
        assert!(rc == 0 && !out.contains(" unknown"), "{l} printed, not addressable: {out}");
    }
}

/// Islands on five framework shapes (Next, Nuxt 3, Nuxt 2, Shopify, JSON-LD) plus a deferred
/// `<template>`: big inert blocks are listed after the page's sections and open with the existing readers;
/// small ones, a module script and the page filter leave them alone.
#[test]
fn islands_list_and_open() {
    let s = Sandbox::new("islands");
    let rows = |n: usize| (0..n).map(|i| format!(r#"{{"id":{i},"name":"item {i} blue widget"}}"#)).collect::<Vec<_>>();
    let json = rows(600).join(",");
    let small = rows(3).join(",");
    let comments: String = (0..40)
        .map(|i| format!("<article><p>Comment {i} says the hinge broke after a week of use.</p><a href=\"https://ex.com/u/{i}\">user{i}</a></article>"))
        .collect();
    let body = format!(
        "<p>Short lead of the <a href=\"https://ex.com/w\">blue widget</a>.</p>\
         <script id=\"__NEXT_DATA__\" type=\"application/json\">{{\"props\":{{\"items\":[{json}]}}}}</script>\
         <script type=\"application/json\" id=\"__NUXT_DATA__\" data-ssr=\"true\">[{json}]</script>\
         <script id=\"__NUXT__\">window.__NUXT__=(function(a){{return {{items:[{json}]}}}}(1))</script>\
         <script type=\"application/json\" data-product-json>{{\"variants\":[{json}]}}</script>\
         <script type=\"application/ld+json\">{{\"@type\":\"ItemList\",\"itemListElement\":[{json}]}}</script>\
         <script type=\"application/json\">[{small}]</script><script type=\"module\">var x = [{json}];</script>\
         <template id=\"tiny\"><p>x</p></template><template id=\"deferred-comments\">{comments}</template>"
    );
    s.file("i.html", &page("Widget", &body));
    let (out, rc) = s.run("./i.html");
    assert_eq!(rc, 0, "{out}");
    shape(&out);
    let isl: Vec<&str> = out.lines().filter(|l| l.contains("] island ")).collect();
    let want = [
        "[u1#2] island template#deferred-comments 3 KB · ",
        "[u1#3] island script#__NEXT_DATA__ 24 KB · ",
        "[u1#4] island script#__NUXT_DATA__ 24 KB · ",
        "[u1#5] island script#__NUXT__ 24 KB · ",
        "[u1#6] island script[application/json] 24 KB · ",
        "[u1#7] island script[application/ld+json] 24 KB · ",
    ];
    assert_eq!(isl.len(), want.len(), "{out}");
    for (row, w) in isl.iter().zip(want) {
        assert!(row.starts_with(w) && row.ends_with(&format!("tok → fats {}", &w[1..5])), "{row}");
    }
    assert!(out.contains("· 1 sections +6 island ·") && !out.contains("Comment 3"), "served page only: {out}");
    // the template opens rendered, its links numbered on from the page's
    let (open, rc) = s.run("u1#2");
    assert_eq!(rc, 0, "{open}");
    // a thin page (18 tok served of the ~25k its islands hold) says so on every read
    let head = open.lines().next().unwrap_or_default();
    assert!(head.starts_with("= j1.u1#2 island template#deferred-comments 3 KB · part 1/2"), "{open}");
    assert!(head.ends_with(" · thin 18 of ~25.0k visible") && !head.contains('!'), "a count, last: {head}");
    let open = open + &s.run("u1#2b").0; // 40 comments = two 1,000-token parts
    assert_eq!(open.lines().filter(|l| l.starts_with("Comment ")).count(), 40, "{open}");
    assert!(open.contains("user0 [l2]") && open.contains("user39 [l41]"), "{open}");
    let links = s.run("u1 links").0;
    assert!(links.contains("l1 ex.com/w\n") && links.contains("l2 ex.com/u/0\n"), "{links}");
    // JSON opens as a text document: pretty, no id on a line; a JS blob as it is
    let next = s.run("u1#3").0;
    assert!(next.contains("\n{\n") && next.lines().any(|l| l.contains(r#""name": "item 0 blue widget""#)), "{next}");
    assert!(s.run("u1#5").0.contains("\nwindow.__NUXT__="));
    // the page filter reads the served page, then (no hit there) the islands, and says so; an island filters
    // by its id
    let hinge = s.run("u1 hinge").0;
    assert!(
        hinge.contains(" filter \"hinge\" · 40 hit · match:exact · in islands") && hinge.contains("\nu1#2 "),
        "{hinge}"
    );
    // a thin page: one search over the served text and the islands, the served hits first
    let both = s.run("u1 blue widget").0;
    let (own, isl) = (both.find("\nu1#1 "), both.find("\nu1#3 "));
    assert!(own.is_some() && own < isl && !both.contains("in islands"), "served first: {both}");
    assert!(s.run("u1#2 hinge").0.contains("u1#2 "));
    let doc = std::fs::read_to_string(s.dir.join("cache/t/u1/doc.json")).expect("doc.json");
    let doc: serde_json::Value = serde_json::from_str(&doc).expect("doc json");
    assert_eq!(doc["islands"], 6, "stored in the snapshot");
}

/// A long page: the island row comes after its section rows, before the read hint.
#[test]
fn island_rows_follow_the_sections() {
    let s = Sandbox::new("islands-long");
    let json: Vec<String> = (0..900).map(|i| format!(r#"{{"id":{i},"name":"item {i}"}}"#)).collect();
    let json = json.join(",");
    let long: String =
        (0..200).map(|i| format!("<h2>Part {i}</h2><p>{}</p>", "words of the page ".repeat(4))).collect();
    let next = format!("<script id=\"__NEXT_DATA__\" type=\"application/json\">[{json}]</script>");
    s.file("l.html", &page("Long", &format!("{long}{next}")));
    let out = s.run("./l.html").0;
    let at = |p: &str| out.lines().position(|l| l.starts_with(p));
    assert!(
        at("u1#3 ").is_some()
            && at("u1#3 ") < at("[u1#202] island script#__NEXT_DATA__")
            && at("[u1#202]") < at("read:"),
        "{out}"
    );
}

/// An island whose first part is an encoded blob (base64, raw or URL-safe in a JSON string) prints its size and
/// no way in: reading it gives an agent 1,000 tokens of noise a part. An island of words keeps its arrow.
#[test]
fn an_encoded_island_has_no_arrow() {
    let s = Sandbox::new("islands-blob");
    let abc = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let blob: String = (0..24_000).map(|i: usize| char::from(abc.as_bytes()[(i * 7 + i / 64) % 62])).collect();
    let words: Vec<String> = (0..600).map(|i| format!(r#"{{"id":{i},"name":"item {i} blue widget"}}"#)).collect();
    let body = format!(
        "<p>Short lead of the page.</p><script id=\"__FILE_URLS__\">{blob}+/==</script>\
         <script id=\"__STATE__\" type=\"application/json\">\"{blob}-_\"</script>\
         <script id=\"__NEXT_DATA__\" type=\"application/json\">[{}]</script>",
        words.join(",")
    );
    s.file("b.html", &page("Track", &body));
    let out = s.run("./b.html").0;
    let isl: Vec<&str> = out.lines().filter(|l| l.contains("] island ")).collect();
    assert_eq!(isl.len(), 3, "{out}");
    for (row, title) in isl.iter().zip(["script#__FILE_URLS__", "script#__STATE__"]) {
        assert!(row.contains(&format!(" island {title} ")) && row.contains(" (encoded) · "), "{row}");
        assert!(row.ends_with(" tok") && !row.contains('→'), "{row}");
    }
    assert!(isl[2].contains("__NEXT_DATA__") && isl[2].ends_with(" tok → fats u1#4"), "{out}");
}

/// `all`'s `#0 info` row is sized by what `uN#0` prints (its `key: value` lines), not by their JSON form.
#[test]
fn the_info_row_is_sized_by_its_read() {
    let s = Sandbox::new("infosize");
    let head = r#"<meta name="description" content="A tent "for" two — light, dry, "warm""><meta name="author" content="Ann">"#;
    s.file("i.html", &format!("<html><head><title>Tent</title>{head}</head><body><p>A tent.</p></body></html>"));
    assert_eq!(s.run("./i.html").1, 0);
    let read: usize = s.run("u1#0").0.lines().skip(1).map(fats::clean::tok).sum();
    let all = s.run("u1 all").0;
    assert!(all.contains(&format!("\nu1#0 info ({read})\n")), "{read}: {all}");
}

/// `json:true` records byte for byte (`tests/golden/`; times zeroed, the sandbox path named): key order of the
/// record, of the page's `info` (document order: meta, then JSON-LD, a repeated key in its first place with its last
/// value) and of a JSON island (keys sorted) stays put whatever map type holds them.
/// `FATS_GOLDEN=write` rewrites the file.
#[test]
fn json_records_are_golden() {
    let s = Sandbox::new("golden");
    let ld = r#"{"@type":"Product","name":"Tent","offers":{"priceCurrency":"EUR","price":"12.50"},
        "aggregateRating":{"reviewCount":7,"ratingValue":4.5,"ratingValue":4.75},
        "author":{"name":"Ann"},"dateModified":"2026-10-02","datePublished":"2026-10-01"}"#;
    let head =
        format!(r#"<meta name="description" content="A tent."><script type="application/ld+json">{ld}</script>"#);
    s.file("info.html", &format!("<html><head><title>Tent</title>{head}</head><body><p>A tent.</p></body></html>"));
    let zeta = "a reply that says the same thing again ".repeat(600);
    let data =
        format!(r#"<p>Does it work?</p><script type=application/json>{{"zeta":"{zeta}","alpha":[3,1]}}</script>"#);
    s.file("island.html", &page("Data", &data));
    s.file("media-honest.html", &std::fs::read_to_string("tests/fixtures/media-honest.html").expect("fixture"));
    let mut got = String::new();
    for cmd in ["./info.html", "u1#0", "./island.html", "u2#2", "./media-honest.html", "u3 media"] {
        got += &s.run(&format!("{cmd} json:true")).0;
    }
    // the run's own dir as `<dir>`, `/` between parts, a file URL's drive gone (`file:///C:/img` → `file:///img`):
    // one golden file on every OS
    let dirs = [s.dir.canonicalize().expect("dir").display().to_string(), s.dir.display().to_string()];
    let local = |t: &str| {
        let t = dirs.iter().fold(t.to_string(), |t, d| t.replace(d, "<dir>"));
        no_drive(&if cfg!(windows) { t.replace('\\', "/") } else { t })
    };
    // every number of milliseconds or seconds of age → 0 (a debug build's first media page can take 10 ms)
    let zero = |text: &str| {
        let mut out = String::new();
        for part in text.split_inclusive(|c: char| !c.is_ascii_digit()) {
            let (num, rest) = part.split_at(part.trim_end_matches(|c: char| !c.is_ascii_digit()).len());
            let timed = num.chars().all(|c| c.is_ascii_digit()) && !num.is_empty();
            let age = rest == "s" && out.ends_with("cache ") || out.ends_with("\"age\":");
            out += if timed && (age || rest == "m" || out.ends_with("\"ms\":")) { "0" } else { num };
            out += rest;
        }
        out
    };
    // `bytes_out` stays exact: it counts the printed lines, so it loses only the digits zeroed in them
    let mut out = String::new();
    let tok = regex::Regex::new(r"\d+tok").expect("regex");
    for rec in got.lines() {
        let mut v: serde_json::Value = serde_json::from_str(rec).expect("record");
        let mut lines: Vec<String> =
            v["lines"].as_array().expect("lines").iter().filter_map(|l| l.as_str()).map(String::from).collect();
        assert_eq!(v["bytes_out"], lines.join("\n").len(), "bytes_out counts the lines: {rec}");
        lines = lines.into_iter().map(|l| local(&l)).collect();
        for k in ["path", "url"] {
            if let Some(t) = v[k].as_str().map(local) {
                v[k] = t.into();
            }
        }
        // the `=` line's Ntok counts the printed text, its own `local Nms` too: a slow run's 12ms is one more token
        if let Some(head) = lines.first_mut().filter(|l| l.starts_with("= ")) {
            *head = tok.replace(head, "0tok").into_owned();
        }
        let text = lines.join("\n");
        v["lines"] = lines.into();
        v["bytes_out"] = zero(&text).len().into();
        out += &(zero(&v.to_string()) + "\n");
    }
    let path = "tests/golden/records.jsonl";
    if std::env::var("FATS_GOLDEN").as_deref() == Ok("write") {
        std::fs::create_dir_all("tests/golden").expect("dir");
        std::fs::write(path, &out).expect("write golden");
    }
    assert_eq!(out, std::fs::read_to_string(path).expect("golden"), "json records moved");
}

/// `fats uN mK` (pixels on demand): the picture fetched plain once into the page's `assets/`, its real size and
/// view cost on the `=` line before any pixel; a re-run is a cache hit; each failure is a `!` row with its next
/// step (`picture_failures`); `pictures_through_mcp` for the MCP side.
#[test]
fn pictures_on_demand() {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    static PNG_GETS: AtomicU64 = AtomicU64::new(0);
    static MD_ASKED: AtomicU64 = AtomicU64::new(0);
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(3000, 2000).write_to(&mut png, image::ImageFormat::Png).expect("png");
    let png = png.into_inner();
    // the longest alt ranks first: a.png is m1
    let alt = |f: &str| if f == "a.png" { "the night sky over the deck, a wide shot" } else { "a shot" };
    let imgs = ["a.png", "big.jpg", "viewer.jpg", "gone.jpg", "c.avif"]
        .map(|f| format!("<p><img src=\"/{f}\" alt=\"{}\"></p>", alt(f)));
    let prose = "<p>Four pictures of the same night sky, taken from the deck, each with its own short story.</p>";
    let html = page("Pics", &(prose.repeat(8) + &imgs.concat()));
    let addr = serve(move |head, c| {
        let (ctype, body, len) = match head.split_whitespace().nth(1).unwrap_or_default() {
            "/a.png" => {
                PNG_GETS.fetch_add(1, Relaxed);
                MD_ASKED.fetch_add(u64::from(head.to_lowercase().contains("text/markdown")), Relaxed);
                ("image/png", png.clone(), png.len())
            }
            "/big.jpg" => ("image/jpeg", Vec::new(), 14_000_000),
            "/c.avif" => ("image/avif", b"\0\0\0\x1cftypavif".to_vec(), 12),
            "/viewer.jpg" => ("text/html", b"<p>viewer</p>".to_vec(), 13),
            "/gone.jpg" => return c.write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n"),
            _ => ("text/html", html.clone().into_bytes(), html.len()),
        };
        write!(c, "HTTP/1.1 200 OK\r\ncontent-type: {ctype}\r\ncontent-length: {len}\r\nconnection: close\r\n\r\n")?;
        c.write_all(&body)
    });
    let s = Sandbox::new("pictures");
    let out = online(&s, &format!("{addr}/p provider:plain"));
    assert!(out.starts_with("= j1.u1 Pics") && !out.contains("tok if viewed"), "no pixels by default: {out}");
    // `fats u1 media` order: m<K> of each file
    let media = s.run("u1 media").0;
    let m =
        |f: &str| media.lines().find(|l| l.ends_with(f)).and_then(|l| l.split(' ').next()).unwrap_or("m9").to_string();
    let (png_m, first) = (m("a.png"), online(&s, &format!("u1 {}", m("a.png"))));
    let line = first.lines().next().unwrap_or_default();
    let saved = s.dir.join("cache").join("t").join("u1").join("assets").join(format!("{png_m}.png"));
    let size = fats::lang::ksize(std::fs::read(&saved).expect("saved").len());
    let want = format!(
        "= j1.u1 {png_m} photo 3000x2000 png {size}B · saved {} · ~1.5–4.7k tok if viewed · plain ",
        saved.display()
    );
    assert!(line.starts_with(&want) && line.ends_with("ms $0"), "{first}\nwant {want}");
    let again = online(&s, &format!("u1 {png_m}"));
    assert!(again.lines().next().unwrap_or_default().contains("~1.5–4.7k tok if viewed · cache "), "{again}");
    assert_eq!(PNG_GETS.load(Relaxed), 1, "a re-run reads the cache");
    // `fresh:true` on a picture downloads it again into the same page, never the page
    let fresh = online(&s, &format!("u1 {png_m} fresh:true"));
    assert!(fresh.starts_with(&format!("= j1.u1 {png_m} photo ")) && fresh.contains(" · plain "), "{fresh}");
    assert_eq!(PNG_GETS.load(Relaxed), 2, "fresh:true downloads again");
    assert_eq!(MD_ASKED.load(Relaxed), 0, "a picture download never asks for Markdown (review 2026-10-14 #12)");
    picture_failures(&s, &addr, &m);
    assert!(s.run("stats full").0.contains(" · pictures 1 plain "), "stats shows the download");
    assert!(s.run("config").0.contains("\npictures fats uN mK: plain only, image/* at most 10.0MB, 15s, $0"));
    let status = s.run("").0;
    assert!(status.contains("B pictures) · 1 pages") && !status.contains("(0B pictures)"), "{status}");
    pictures_through_mcp(&s, &png_m, &m("c.avif"));
}

/// A picture on a loopback host is fenced like a page: without `FATS_ALLOW_PRIVATE` the download is refused as
/// `! private_address` by address or by name, and nothing reaches the host.
#[test]
fn picture_on_a_private_host_is_refused() {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    static HITS: AtomicU64 = AtomicU64::new(0);
    let addr = serve(|_, c| {
        HITS.fetch_add(1, Relaxed);
        c.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: image/png\r\ncontent-length: 0\r\n\r\n")
    });
    let s = Sandbox::new("private-pic");
    let port = addr.rsplit(':').next().unwrap_or_default();
    let imgs =
        format!("<img src='{addr}/a.png' alt='a loopback picture'><img src='http://localhost:{port}/b.png' alt='b'>");
    s.file("p.html", &format!("<html><head><title>P</title></head><body><p>Text here.</p>{imgs}</body></html>"));
    assert!(s.run("p.html").0.starts_with("= j1.u1 P"));
    for k in [1, 2] {
        let o = s.cmd(&format!("u1 m{k}")).env_remove("FATS_OFFLINE").env_remove("FATS_ALLOW_PRIVATE").output();
        let out = String::from_utf8_lossy(&o.expect("run").stdout).into_owned();
        assert!(out.lines().nth(1).unwrap_or_default().contains(" ! private_address "), "m{k}: {out}");
    }
    assert_eq!(HITS.load(Relaxed), 0, "nothing reached the host");
}

/// Each picture that cannot be saved is a `!` row with its next step.
fn picture_failures(s: &Sandbox, addr: &str, m: &dyn Fn(&str) -> String) {
    let row = |f: &str| online(s, &format!("u1 {}", m(f))).lines().nth(1).unwrap_or_default().replace(&m(f), "mK");
    assert_eq!(
        row("big.jpg"),
        format!("u1 mK ! too_big 14.0MB (>10.0MB) → nothing left to try · open {addr}/big.jpg in a browser")
    );
    assert_eq!(row("viewer.jpg"), format!("u1 mK ! not_image text/html → fats {addr}/viewer.jpg"));
    let gone = "u1 mK ! http_403 · signed link expired or hotlink-blocked → fats u1 fresh:true, then mK of";
    assert_eq!(row("gone.jpg"), format!("{gone} the new id, a fresh link · or open {addr}/gone.jpg in a browser"));
    assert_eq!(row("none"), "u1 mK ! no_media · the page lists 5 → fats u1 media");
}

/// `fats mcp` attaches the pixels only for `mK` (downscaled as Claude would), never beside `structuredContent`;
/// a saved picture it cannot send is a `! not_viewable` block.
fn pictures_through_mcp(s: &Sandbox, png_m: &str, avif_m: &str) {
    let avif = online(s, &format!("u1 {avif_m}"));
    assert!(avif.contains(" avif 12B · saved ") && avif.contains("view cost unknown"), "{avif}");
    let fats = |cmd: &str| serde_json::json!({"name": "fats", "arguments": {"cmd": cmd}});
    let by_id = mcp_session(s, &[fats(&format!("u1 {png_m}")), fats("u1"), fats(&format!("u1 {avif_m}"))]);
    let content = |id: u64| by_id[&id]["result"]["content"].as_array().cloned().unwrap_or_default();
    let c = content(3);
    assert_eq!(c.len(), 2, "text + image: {c:?}");
    assert_eq!((c[1]["type"].as_str(), c[1]["mimeType"].as_str()), (Some("image"), Some("image/jpeg")));
    assert!(c[0]["text"].as_str().unwrap_or_default().contains("m1 photo 3000x2000 png"), "cost first");
    use base64::Engine;
    let jpg = base64::engine::general_purpose::STANDARD.decode(c[1]["data"].as_str().unwrap_or_default());
    let sent = image::load_from_memory(&jpg.expect("base64")).expect("jpeg");
    assert_eq!((sent.width(), sent.height()), (1344, 896), "downscaled as Claude would");
    assert_eq!(content(4).len(), 1, "a page read carries no pixels");
    assert!((3..=4).all(|id| by_id[&id]["result"].get("structuredContent").is_none()), "never structuredContent");
    // a saved picture Claude cannot read as pixels is said, never silently dropped
    let avif = content(5);
    let said = avif.get(1).and_then(|c| c["text"].as_str()).unwrap_or_default();
    assert!(
        said.starts_with("! not_viewable · Claude reads png, jpeg, gif, webp → open ") && said.ends_with(".avif"),
        "{avif:?}"
    );
}

/// Stored copies of `url` as session `t`'s u1, u2, … : each its meta.json and raw.html. A new session each time: a
/// stale doc.json would stand for the new raw.html.
fn store(s: &Sandbox, url: &str, copies: &[(serde_json::Value, String)]) {
    std::fs::remove_dir_all(s.dir.join("cache/t")).ok();
    let mut u = Vec::new();
    for (i, (meta, raw)) in copies.iter().enumerate() {
        let d = s.dir.join(format!("cache/t/u{}", i + 1));
        std::fs::create_dir_all(&d).expect("dir");
        std::fs::write(d.join("raw.html"), raw).expect("raw");
        std::fs::write(d.join("meta.json"), meta.to_string()).expect("meta");
        u.push(serde_json::json!({"url": url, "dir": format!("t/u{}", i + 1)}));
    }
    std::fs::write(s.dir.join("cache/t/state.json"), serde_json::json!({ "u": u }).to_string()).expect("state");
}

/// A thin Reddit copy never loops for money. Stored copies of one URL (bd's thin auto copy u1,
/// a paid ask, its own meta): (b) a paid copy with the same served text (bd again, 958 tok) = no
/// `js:true` on either; (a) a stored render with other text = `→ fats u2 · rendered`, no `js:true`; (c) comments in a
/// `<template>` with ≥ 5 paragraphs = the island first, $0. The control: u1 alone asks for the render.
#[test]
fn a_thin_copy_never_loops_for_money() {
    let url = "https://www.reddit.com/r/rust/comments/63wts9/why_are_there_so_many_linear_algebra_crates/";
    let page = |n: usize| {
        let c = |i| {
            format!("<p>{}</p>", format!("Comment {i}: ndarray, nalgebra and cgmath aim at other jobs. ").repeat(50))
        };
        include_str!("fixtures/reddit-thin-template.html").replace("{comments}", &(1..=n).map(c).collect::<String>())
    };
    let copy = |(tier, hedge, raw): &(&str, &str, String)| {
        let hops = serde_json::json!([{"provider": "plain", "code": "js_shell"}, {"provider": tier, "code": "ok", "cost": 0.0015}]);
        let meta = serde_json::json!({"url": url, "provider": tier, "ok": true, "code": "ok", "t": 1, "cost": 0.0015, "hops": hops, "hedge": hedge});
        (meta, raw.clone())
    };
    let store = |s: &Sandbox, rows: &[(&str, &str, String)]| store(s, url, &rows.iter().map(copy).collect::<Vec<_>>());
    let (bd, oxyjs) = ("brightdata", "oxylabs-js");
    let s = Sandbox::new("thinloop");
    s.keys();
    store(&s, &[(bd, "", page(4))]);
    let alone = s.run("u1").0;
    assert!(alone.contains(" ! thin ") && alone.contains("→ fats u1 js:true"), "the control: {alone}");
    // Airbnb: u2 = that `js:true` ask, its render cut at 60 s: u1 never offers `js:true` again
    let hop = serde_json::json!([{"provider": oxyjs, "code": "provider_timeout", "upto": 0.0015, "timeout_s": 60}]);
    let cut = serde_json::json!({"url": url, "provider": oxyjs, "ok": false, "code": "provider_timeout", "t": 2, "hedge": "off", "hops": hop});
    crate::store(&s, url, &[copy(&(bd, "", page(4))), (cut, String::new())]);
    let out = s.run("u1").0;
    assert!(out.contains(" · thin ") && !out.contains("js:true"), "the cut render again: {out}");
    store(&s, &[(bd, "", page(4)), (bd, "off", page(4))]);
    for id in ["u1", "u2"] {
        let out = s.run(id).0;
        assert!(out.contains(" · thin ") && !out.contains("js:true"), "(b) same text, paid: {out}");
    }
    let it = include_str!("fixtures/geo-reddit-it.html").to_string();
    store(&s, &[(bd, "", page(4)), (oxyjs, "off", it)]);
    let out = s.run("u1").0;
    assert!(out.contains("→ fats u2 · rendered") && !out.contains("js:true"), "(a) {out}");
    store(&s, &[(bd, "", page(6))]);
    let out = s.run("u1").0;
    assert!(out.contains("→ fats u1#") && out.contains(" tok · $0") && !out.contains("js:true"), "(c) {out}");
    // The `=` arrow walks the island's parts, never back at a part read in this session
    let arrow = |out: &str| {
        let head = out.lines().next().unwrap_or_default();
        head.rsplit_once("→ fats ").map(|r| r.1.split(' ').next().unwrap_or_default().to_string()).unwrap_or_default()
    };
    let (mut at, mut read) = (arrow(&out), Vec::new());
    while !at.is_empty() {
        read.push(at.clone());
        let out = s.run(&at).0;
        at = arrow(&out);
        assert!(!read.contains(&at) && read.len() < 20, "points back at {at} after {read:?}: {out}");
    }
    assert!(read.len() >= 2 && read[1] == format!("{}b", read[0]), "the next part first: {read:?}");
    let out = s.run("u1").0;
    assert!(!out.lines().next().unwrap_or_default().contains('→'), "all read: no arrow, no render: {out}");
    // A cache hit is a new id of the same copy, with its parts read: never back at `#5`
    let idx = serde_json::json!({ url: {"dir": "t/u1", "t": 1} });
    std::fs::write(s.dir.join("cache/index.json"), idx.to_string()).expect("index");
    let out = s.run(url).0;
    assert!(out.starts_with("= ") && !out.lines().next().unwrap_or_default().contains('→'), "the cache hit: {out}");
}

/// A TikTok video: the 1st visit's oxyjs shell held the caption (`page
/// content shown → u1#…`); the 2nd visit, plain alone, got a shell with none of the page's own words in its head and
/// said `nothing left to try`. A copy without them points at the stored copy's part holding them, $0, the failed
/// row too; the copy holding them still points into itself.
#[test]
fn a_shell_points_at_the_stored_copy_of_the_page() {
    let url = "https://www.tiktok.com/@example_creator/video/7000000000000000001";
    let pad = |html: &str| {
        let row = |i| format!(r#""k{i}":"Upload a video, follow creators and log in to like and comment. ""#);
        html.replace("{pad}", &format!("{{{}}}", (1..=400).map(row).collect::<Vec<_>>().join(",")))
    };
    let s = Sandbox::new("ownwords");
    s.keys();
    let rows = [
        ("oxylabs-js", "no_copy", include_str!("fixtures/tiktok-render.html")),
        ("plain", "js_shell", include_str!("fixtures/tiktok-shell-head.html")),
    ];
    let copy = |(tier, code, raw): &(&str, &str, &str)| {
        let hops =
            serde_json::json!([{"provider": "plain", "code": "js_shell"}, {"provider": tier, "code": "js_shell"}]);
        let meta = serde_json::json!({"url": url, "provider": tier, "ok": false, "code": code, "t": 1, "hops": hops});
        (meta, pad(raw))
    };
    store(&s, url, &rows.iter().map(copy).collect::<Vec<_>>());
    let first = s.run("u1").0;
    assert!(first.contains("page content shown") && first.contains("→ fats u1#"), "the copy holding it: {first}");
    let second = s.run("u2").0;
    let row = second.lines().find(|l| l.starts_with("u2 ! js_shell")).unwrap_or_default();
    assert!(row.contains("→ fats u1#") && row.ends_with(" · page content"), "the failed row: {second}");
    assert!(!second.contains("nothing left"), "{second}");
}

/// A TikTok video: a plain shell with no og tags and the site's name as
/// its title, every keyed tier failed, said `nothing left to try · open … in a browser` 7 times while its app state
/// held the caption. The page record = the island part with the URL's id as a value (the canonical link holding it
/// first is a link), at its wordiest sibling: `→ fats u1#2… · page record · N tok · $0`, at any chunk boundary;
/// "nothing left" only once that part was read.
#[test]
fn a_shell_points_at_its_page_record() {
    let url = "https://www.tiktok.com/@example_creator/video/7000000000000000001";
    let s = Sandbox::new("pagerecord");
    s.keys();
    let hops = serde_json::json!([{"provider": "plain", "code": "js_shell"}, {"provider": "brightdata", "code": "provider_timeout"}, {"provider": "oxylabs-js", "code": "provider_timeout"}]);
    let meta =
        serde_json::json!({"url": url, "provider": "plain", "ok": false, "code": "no_copy", "t": 1, "hops": hops});
    let shell = |n: usize| {
        let row = |i| format!(r#""k{i}":"Upload a video, follow creators and log in to like and comment. ""#);
        let pad = format!("{{{}}}", (1..=n).map(row).collect::<Vec<_>>().join(","));
        include_str!("fixtures/tiktok-shell-app-state.html").replace("{pad}", &pad)
    };
    for n in [300, 340, 380, 420] {
        store(&s, url, &[(meta.clone(), shell(n))]);
        let out = s.run("u1").0;
        let row = out.lines().find(|l| l.starts_with("u1 ! no_copy")).unwrap_or_default();
        let part = row.split("→ fats ").nth(1).and_then(|r| r.strip_suffix(" tok · $0")).unwrap_or_default();
        assert!(part.contains(" · page record · ") && !out.contains("nothing left"), "pad {n}: {out}");
        let read = s.run(part.split(' ').next().unwrap_or_default()).0;
        assert!(read.contains(r#""desc": "Did you know a moon suit"#), "pad {n}, the caption's part: {read}");
        assert!(s.run("u1").0.contains("nothing left to try"), "pad {n}: only after the record was read");
    }
    // ONE own-words rule (`fetch::words_of`): a wall's title has 1 word outside the stop list, a 404
    // (its title and the URL's id echoed) shows none: never `page content shown`, never a page record
    let wall = "<title>Access to this page has been denied</title><h1>Access to this page has been denied</h1>";
    let wall = shell(0).replace("<title>TikTok - Make Your Day</title>", wall);
    let gone = shell(300).replace("TikTok - Make Your Day", "Moon suits tested by example_creator");
    for (code, raw) in [("captcha_wall", wall), ("not_found", gone)] {
        let meta = serde_json::json!({"url": url, "provider": "plain", "ok": false, "code": code, "t": 1, "hops": [{"provider": "plain", "code": code}]});
        store(&s, url, &[(meta, raw)]);
        let out = s.run("u1").0;
        let heads = out.lines().filter(|l| l.starts_with("= ") || l.starts_with("u1 !")).collect::<String>();
        assert!(!heads.contains("page content shown") && !heads.contains("→ fats u1#"), "{code}: {out}");
    }
}

/// eBay, and bd's case on `geo-booking-es.html`: a geo copy's arrow ends once its fix ran
/// in this session (a newer copy asked another way → `→ fats u2`, $0), and a country the user named (the run's
/// command, not the country recorded: an older rule sent one too) is no provider's geo copy: `served in en (asked
/// DE)`, nothing to buy.
#[test]
fn a_geo_copy_points_at_its_fix() {
    let s = Sandbox::new("geoend");
    s.keys();
    let copy = |url: &str, tier: &str, country: &str, why: &str, raw: &str| {
        let hops = serde_json::json!([{"provider": "plain", "code": "blocked_403"}, {"provider": tier, "code": "ok", "why": why}]);
        let meta = serde_json::json!({"url": url, "provider": tier, "country": country, "ok": true, "code": "ok", "t": 1, "hops": hops});
        (meta, raw.to_string())
    };
    let (ebay, html) = ("https://www.ebay.com/itm/100000000001", include_str!("fixtures/ebay-served-en.html"));
    let de = copy(ebay, "oxylabs", "DE", "lang:en", html);
    // the run that fetched u1 (`run/u1.json`, as a run kept it): its command named the country
    let asked = |s: &Sandbox| {
        let run = serde_json::json!({"url": ebay, "cmd": format!("{ebay} country:de fresh:true"), "prov": null, "job": 1,
            "spent": 0.0, "t": 1.0, "pid": 1, "status": "", "done": {"ok": true, "code": "ok", "cost": 0.001, "warn": [], "fail": null}, "told": true});
        std::fs::create_dir_all(s.dir.join("cache/t/run")).expect("run dir");
        std::fs::write(s.dir.join("cache/t/run/u1.json"), run.to_string()).expect("run");
    };
    store(&s, ebay, std::slice::from_ref(&de));
    let out = s.run("u1").0;
    assert!(
        out.contains("u1 ! lang:en · served in en, sent DE · the provider's geo copy → fats u1 country:us"),
        "{out}"
    );
    asked(&s);
    let out = s.run("u1").0;
    let head = out.lines().next().unwrap_or_default();
    assert!(
        head.contains("lang:en · served in en (asked DE)") && !out.contains("geo copy") && !out.contains("fresh:"),
        "{out}"
    );
    store(&s, ebay, &[de, copy(ebay, "oxylabs", "US", "", html)]);
    asked(&s);
    assert!(s.run("u1").0.contains("u1 ! lang:en · served in en (asked DE) → fats u2\n"), "the fix ran");
    let (bk, es) =
        ("https://www.booking.com/hotel/fr/example-hotel.html", include_str!("fixtures/geo-booking-es.html"));
    let none = copy(bk, "brightdata", "", "lang:es", es);
    store(&s, bk, std::slice::from_ref(&none));
    let row = "u1 ! lang:es · served in es, sent none · the provider's geo copy → fats u1 country:us fresh:true";
    assert!(s.run("u1").0.contains(row), "bd sent none: the fix");
    // the US copy: an English page, not the eBay listing's bytes
    let us = include_str!("fixtures/geo-reddit-en.html");
    store(&s, bk, &[none, copy(bk, "brightdata", "US", "", us)]);
    let out = s.run("u1").0;
    assert!(out.contains("sent none · the provider's geo copy → fats u2\n"), "the fix ran: $0: {out}");
}

/// TikTok, oxy key alone: oxyjs cut at 40 s → the page record → then the end: the row names the cut and no
/// longer call, the agent's `fresh:true` copy cut the same way counts `failed 2/2 here`; with both keys bd
/// is the move, beside the count when the same lap comes again. A part read under one copy is read under
/// every copy.
#[test]
fn a_cut_render_ends_the_walk() {
    let url = "https://www.tiktok.com/@example_creator/video/7000000000000000002";
    let s = Sandbox::new("cutend");
    let cut = || {
        let hops = serde_json::json!([{"provider": "plain", "code": "js_shell", "timeout_s": 10},
            {"provider": "oxylabs-js", "code": "provider_timeout", "timeout_s": 40, "why": "took > 40 s here", "upto": 0.0015}]);
        let meta = serde_json::json!({"url": url, "provider": "plain", "ok": false, "code": "provider_timeout", "t": 1, "hops": hops});
        let pad =
            (1..=300).map(|i| format!(r#""k{i}":"Upload a video, follow creators and log in to like and comment. ""#));
        let pad = format!("{{{}}}", pad.collect::<Vec<_>>().join(","));
        let raw = include_str!("fixtures/tiktok-shell-app-state.html").replace("{pad}", &pad);
        (meta, raw.replace("7000000000000000001", "7000000000000000002"))
    };
    let row =
        |out: &str, id: &str| out.lines().find(|l| l.starts_with(&format!("{id} !"))).unwrap_or_default().to_string();
    for (keys, after) in [("oxy", ""), ("both", " → fats u1 fresh:true provider:bd ≤$0.0015")] {
        s.keys();
        if keys == "oxy" {
            std::fs::remove_file(s.dir.join("keys/brightdata.key")).expect("bd key");
        }
        store(&s, url, &[cut()]);
        let out = s.run("u1").0;
        let part = out.split("→ fats ").nth(1).and_then(|r| r.split(" · page record").next()).unwrap_or_default();
        assert!(part.starts_with("u1#2") && !out.contains("timeout:"), "{keys}: the record first: {out}");
        // TikTok `u1#2fw`, Airbnb `u1#3bb`: the part read is marked before its `=` line's next move
        let head = s.run(part).0.lines().next().unwrap_or_default().to_string();
        assert!(!head.contains(&format!("fats {part} ")), "{keys}: never → the part being read: {head}");
        // A failed copy with a doc says why, as one without (`fail_detail`): our cut with its age, then the end
        let first = row(&s.run("u1").0, "u1");
        let end = if keys == "oxy" { " ago) · failed 1/1 here".to_string() } else { format!(" ago){after}") };
        assert!(first.contains("· showing the best copy · oxyjs cut at 40 s (") && first.ends_with(&end), "{first}");
        // the walk goes on in the same session: u2 = the agent's `fresh:true`, cut the same way (read log kept)
        let state = s.dir.join("cache/t/state.json");
        let mut st: serde_json::Value = serde_json::from_slice(&std::fs::read(&state).expect("state")).expect("json");
        store(&s, url, &[cut(), cut()]);
        st["u"].as_array_mut().expect("u").push(serde_json::json!({"url": url, "dir": "t/u2"}));
        std::fs::write(&state, st.to_string()).expect("state");
        for id in ["u2", "u1"] {
            let out = s.run(id).0;
            let r = row(&out, id);
            assert!(!out.contains("page record") && !out.contains("timeout:"), "{keys} {id}: {out}");
            // the same lap again (its last tier and code repeat u1's): a count; with both keys bd, never asked, stays
            // the move beside it
            let end = if keys == "oxy" {
                " · failed 2/2 here".into()
            } else {
                format!(" · failed 2/2 here{}", after.replace("u1", id))
            };
            assert!(r.contains(" · oxyjs cut at 40 s (") && r.ends_with(&end), "{keys} {id}: {r}");
        }
    }
}

/// Oxy-only Zara (the paid ask after the cut served in 16.1 s): the older copy points at the copy that served;
/// the same with no body stored (`no_doc`), never `failed 1/2` beside it.
#[test]
fn a_served_longer_call_is_the_move() {
    let url = "https://www.zara.com/es/en/jacket-p1.html";
    let s = Sandbox::new("longerserved");
    s.keys();
    let hops = serde_json::json!([{"provider": "plain", "code": "js_shell", "timeout_s": 10},
        {"provider": "oxylabs-js", "code": "provider_timeout", "timeout_s": 60, "why": "took > 60 s here"}]);
    let cut = serde_json::json!({"url": url, "provider": "plain", "ok": false, "code": "provider_timeout", "t": 1, "hops": hops});
    let hops = serde_json::json!([{"provider": "plain", "code": "js_shell", "timeout_s": 10},
        {"provider": "oxylabs-js", "code": "ok", "timeout_s": 60, "ms": 16100, "cost": 0.0015}]);
    let meta = serde_json::json!({"url": url, "provider": "oxylabs-js", "ok": true, "code": "ok", "t": 2, "hops": hops,
        "hedge": "off"});
    let words = "A faux fur jacket with a belt, two pockets and a high collar, lined in the same soft fabric. ";
    let page = format!("<html><body><main><h1>Jacket</h1><p>{}</p></main></body></html>", words.repeat(8));
    store(&s, url, &[(cut, "<html><body><div id=app></div></body></html>".into()), (meta, page)]);
    for f in ["", "raw.html", "doc.json"] {
        std::fs::remove_file(s.dir.join("cache/t/u1").join(f)).ok();
        let out = s.run("u1").0;
        let r = out.lines().find(|l| l.starts_with("u1 !")).unwrap_or_default();
        assert!(r.contains("oxyjs cut at 60 s (") && r.ends_with(" ago) → fats u2 · rendered"), "{r}");
        assert!(!r.contains("failed"), "{r}");
    }
}

/// A listings site, bd key alone, 4 hops: the walk ends in 3. Hop 1 reads u1 (bd 502 at 60 s) → one
/// `fresh:true`; hop 2's copy u2 (the 150 s call, 502 again, same tiers and country) is the same failure: hop 3
/// reads `failed 2/2 here` and no second `fresh:true` (the time each call had is no other way).
#[test]
fn a_longer_call_failing_the_same_way_ends_the_walk() {
    let url = "https://m.listings.example/flats-1-1";
    let s = Sandbox::new("samefail");
    s.keys();
    std::fs::remove_file(s.dir.join("keys/oxylabs.key")).expect("oxy key");
    let copy = |t: u64| {
        let hops = serde_json::json!([{"provider": "plain", "code": "captcha_wall", "timeout_s": 10},
            {"provider": "brightdata-unlocker", "code": "http_502", "status": 502, "timeout_s": t, "country": "FR", "cost": 0.0015}]);
        let meta =
            serde_json::json!({"url": url, "provider": "plain", "ok": false, "code": "http_502", "t": t, "hops": hops});
        (meta, "<html><head><title>Captcha</title></head><body><p>Ar jus ne robotas?</p></body></html>".to_string())
    };
    let row =
        |id: &str| s.run(id).0.lines().find(|l| l.starts_with(&format!("{id} !"))).unwrap_or_default().to_string();
    store(&s, url, &[copy(60)]);
    let first = row("u1");
    assert!(first.contains("→ fats u1 fresh:true") && !first.contains("failed"), "hop 1: one more call: {first}");
    store(&s, url, &[copy(60), copy(150)]);
    for id in ["u2", "u1"] {
        let r = row(id);
        assert!(r.ends_with(" · failed 2/2 here") && !r.contains("fresh:true"), "hop 3, {id}: the end: {r}");
    }
}

/// Both keys: 0.1.6 stored both renders cut at 60 s as `no_copy`; 0.1.7 reads it by its hops, the row of the
/// `provider_timeout` copy apart from the code word (never `js:true`, both renders again), and with no keyed tier left
/// it is the end: `failed 1/1 here`, a second copy cut too `failed 2/2 here`.
#[test]
fn an_old_both_cut_copy_is_a_timeout() {
    let url = "https://www.tiktok.com/@example_creator/video/7000000000000000002";
    let s = Sandbox::new("bothcut");
    s.keys();
    let cut = |p: &str, t: u64| {
        let why = format!("took > {t} s here");
        serde_json::json!({"provider": p, "code": "provider_timeout", "timeout_s": t, "upto": 0.0015, "why": why})
    };
    let meta = |code: &str, hops: serde_json::Value| serde_json::json!({"url": url, "provider": "oxylabs-js", "ok": false, "code": code, "t": 1, "hops": hops});
    let shell = serde_json::json!({"provider": "plain", "code": "js_shell", "timeout_s": 10});
    let both = serde_json::json!([shell, cut("oxylabs-js", 60), cut("brightdata", 60)]);
    let raw = include_str!("fixtures/tiktok-shell-app-state.html").replace("{pad}", "{}");
    let rows = |code: &str, with: bool| {
        let long = (meta("provider_timeout", serde_json::json!([cut("brightdata", 150)])), raw.clone());
        store(&s, url, &[(meta(code, both.clone()), raw.clone()), long][..if with { 2 } else { 1 }]);
        let out = s.run("u1").0;
        out.lines().find(|l| l.starts_with("u1 !")).unwrap_or_default().replacen(code, "CODE", 1)
    };
    for with in [false, true] {
        assert_eq!(rows("no_copy", with), rows("provider_timeout", with), "stored code, same hops");
    }
    let (once, end) = (rows("no_copy", false), rows("no_copy", true));
    assert!(once.contains(" · bd cut at 60 s (") && once.ends_with(" ago) · failed 1/1 here"), "{once}");
    assert!(end.contains(" · bd cut at 60 s (") && end.ends_with(" ago) · failed 2/2 here"), "{end}");
}

/// An oxy-only session re-read with both keys: the failed run's stored `no_key bd`
/// row said `→ fats setup bd` under an arrow that buys bd. A stored `no_key` row is said only while that key
/// is still missing, with the reading binary's name.
#[test]
fn a_stored_no_key_row_ends_with_the_key() {
    let url = "https://www.ebay.com/itm/100000000001";
    let s = Sandbox::new("nokeyrow");
    let bd = "! no_key bd · would try unblock+JS ≤$0.0015 → \
              fats-run17 setup · bd https://brightdata.com/cp/start";
    let cap = "! budget_hit bd not tried · $0.0030 > cost:0.002 → fats-run23 u1 fresh:true cost:0.003";
    let hops = serde_json::json!([{"provider": "plain", "code": "blocked_403"}, {"provider": "oxylabs-js", "code": "js_shell", "cost": 0.0015}]);
    let meta = serde_json::json!({"url": url, "provider": "oxylabs-js", "ok": false, "code": "js_shell", "t": 1,
        "hops": hops, "skipped": [bd, cap]});
    store(&s, url, &[(meta, page("eBay", "<p>Please enable JavaScript.</p>"))]);
    s.keys();
    std::fs::remove_file(s.dir.join("keys/brightdata.key")).expect("bd key");
    // Its fix is run by the binary reading it, never the one that wrote it (`fats-run17`); one glyph
    let now = bd.replace("fats-run17 setup", "run fats setup bd, give the user its 3 lines");
    let (now, out) = (format!("\nu1 {now}\n"), s.run("u1").0);
    assert!(out.contains(&now), "bd still missing: the row");
    // a failed copy's `stats` row keeps its `!` code, never a served page's look
    let stats = s.run("stats").0;
    assert!(stats.contains("\nu1 ebay.com · eBay · oxylabs-js 0ms $0.0000 ! js_shell · "), "{stats}");
    // A 0.1.6 `budget_hit` row's arrow is priced by the reader as `dry:true` prices it
    let dry = s.cmd("u1 fresh:true cost:0.003 dry:true").env_remove("FATS_OFFLINE").output().expect("dry").stdout;
    let dry = String::from_utf8_lossy(&dry).into_owned();
    let up = dry.split("up to $").nth(1).and_then(|r| r.split([' ', ',', '\n']).next()).unwrap_or("?");
    // one paid key (bd's is gone): no provider ask beside the cap, it would be the same move
    assert!(out.contains(&format!("u1 {} ≤${up}\n", cap.replace("fats-run23", "fats"))), "{dry}\n{out}");
    s.keys();
    let out = s.run("u1").0;
    // its cap stopped bd: the budget row is the one move, the copy's row offers no second one
    let one = !out.contains("no_key") && !out.contains("provider:bd") && out.contains("u1 ! budget_hit bd not tried");
    assert!(one, "bd set: no row, the budget row's arrow: {out}");
}

/// A failed search's arrow moved the mods in front of the query (`fats wait:30 rust html …
/// fresh:true`). The canonical command keeps the words where the user typed them, the mods after them.
#[test]
fn a_failed_search_keeps_the_word_order() {
    let s = Sandbox::new("searchwords");
    let c = s.dir.join("cache");
    std::fs::create_dir_all(c.join("t/j1")).expect("dirs");
    std::fs::write(c.join("t/state.json"), r#"{"u": []}"#).expect("state");
    let job = r#"[{"session":"t","cmd":"rust html to markdown crate wait:30","t":1}]"#;
    std::fs::write(c.join("jobs.json"), job).expect("jobs");
    let meta = r#"{"url":"search:rust html to markdown crate","provider":"","ok":false,"code":"provider_error","ms":900,
        "t":1,"kind":"serp","hops":[{"provider":"brightdata","code":"provider_error","ms":900}]}"#;
    std::fs::write(c.join("t/j1/meta.json"), meta).expect("meta");
    let out = s.run("j1").0;
    assert!(out.contains("→ fats rust html to markdown crate wait:30 fresh:true"), "{out}");
}

/// A TikTok video, oxy: u1 = a shell whose app state holds the caption record; u2 = oxyjs's 536 B
/// error page (`Site Maintenance … error code: …`), no page words, no record. u2's row showed "the best copy" (the
/// error page) and offered another paid call; a copy with neither the page's words nor a record points at the
/// stored copy's record, $0, until it was read.
#[test]
fn an_error_page_points_at_the_stored_record() {
    let url = "https://www.tiktok.com/@example_creator/video/7000000000000000001";
    let s = Sandbox::new("errpage");
    s.keys();
    let row = |i| format!(r#""k{i}":"Upload a video, follow creators and log in to like and comment. ""#);
    let pad = format!("{{{}}}", (1..=300).map(row).collect::<Vec<_>>().join(","));
    let shell = include_str!("fixtures/tiktok-shell-app-state.html").replace("{pad}", &pad);
    let error = "<html><head><title>Site Maintenance</title></head><body><h1>Oops! Something went wrong</h1>\
                 <p>error code: 0.efb52e17.1792028320.2e2fcfc9</p></body></html>";
    let copy = |code: &str, raw: &str| {
        let hops = serde_json::json!([{"provider": "plain", "code": "js_shell"}, {"provider": "oxylabs-js", "code": code, "cost": 0.0015}]);
        let meta =
            serde_json::json!({"url": url, "provider": "oxylabs-js", "ok": false, "code": code, "t": 1, "hops": hops});
        (meta, raw.to_string())
    };
    store(&s, url, &[copy("no_copy", &shell), copy("js_shell", error)]);
    let out = s.run("u2").0;
    let r = out.lines().find(|l| l.starts_with("u2 ! js_shell")).unwrap_or_default();
    let part = r.split("→ fats ").nth(1).and_then(|r| r.strip_suffix(" tok · $0")).unwrap_or_default();
    assert!(part.starts_with("u1#2") && part.contains(" · page record · "), "the stored record: {out}");
    s.run(part.split(' ').next().unwrap_or_default());
    let out = s.run("u2").0;
    assert!(!out.contains("page record") && out.contains("→ fats u2 fresh:true provider:bd"), "read: {out}");
}

/// A fix's `done` (`fetch::spent`) is the session's, not the copy's. bd refused u1 → `provider:oxy`;
/// u2 = that oxy copy, refused too; u3 (`fresh:true`, the ladder again) must not offer `provider:oxy` once more,
/// with a body (`page_flags`) or without one (`no_doc`). The control: u3 alone offers it.
#[test]
fn a_tier_another_copy_tried_is_no_fix() {
    let url = "https://www.example.de/anzeigen/wohnung-12345678";
    let s = Sandbox::new("seam");
    s.keys();
    let page = "<html><head><title>Access denied</title></head><body><p>Access denied.</p></body></html>";
    let copy = |hops: serde_json::Value| {
        let meta = serde_json::json!({"url": url, "provider": "brightdata", "ok": false, "code": "provider_refused", "t": 1, "hops": hops});
        (meta, page.to_string())
    };
    let ladder = serde_json::json!([{"provider": "plain", "code": "blocked_403"}, {"provider": "brightdata", "code": "provider_refused"},
        {"provider": "oxylabs-js", "code": "blocked_403", "cost": 0.0015}]);
    let oxy = serde_json::json!([{"provider": "oxylabs", "code": "provider_refused"}]);
    let row = |id: &str| {
        let out = s.run(id).0;
        out.lines().find(|l| l.starts_with(&format!("{id} ! provider_refused"))).unwrap_or_default().to_string()
    };
    store(&s, url, &[copy(ladder.clone())]);
    assert!(row("u1").contains("→ fats u1 fresh:true provider:oxy"), "control: {}", row("u1"));
    let copies = [copy(ladder.clone()), copy(oxy), copy(ladder)];
    store(&s, url, &copies);
    let with_body = row("u3");
    store(&s, url, &copies);
    std::fs::remove_file(s.dir.join("cache/t/u3/raw.html")).expect("raw");
    let no_body = row("u3");
    assert!(!no_body.contains("showing the best copy"), "no body: {no_body}");
    // U3 failed as u1 did along the same path: a count, no arrow (never `→ nothing left to try · open …`)
    for r in [&with_body, &no_body] {
        assert!(r.ends_with(" · failed 2/3 here") && !r.contains("provider:oxy"), "oxy ran on u2: {r}");
    }
}

/// Make `p` look `secs` old (its mtime): the lifetime tests' clock. By path, a file or a folder: Windows sets a
/// folder's time only through a handle `File::open` does not ask for.
fn age_by(p: &std::path::Path, secs: u64) {
    let t = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
    let times = std::fs::FileTimes::new().set_modified(t);
    std::fs::set_times(p, times).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
}

/// The cache lifetime: a page unread for 30 min, or fetched 60 min ago, is gone (its id says so and names the
/// fetch again), a read keeps it; this session is swept after every answer, every session at most once per 5 min;
/// a session's state stays (its ids never restart); a dir that is no session (no state.json) is left.
#[test]
fn the_cache_forgets_within_the_hour() {
    let s = Sandbox::new("ttl");
    let (cache, t) = (s.dir.join("cache"), s.dir.join("cache/t"));
    for n in ["a", "b", "c"] {
        s.file(&format!("{n}.html"), &page(&format!("Page {n}"), "<p>some text here</p>"));
        assert_eq!(s.run(&format!("./{n}.html")).1, 0);
    }
    age_by(&t.join("u1"), 31 * 60); // unread 31 min
    age_by(&t.join("u2/meta.json"), 61 * 60); // fetched 61 min ago, read since
    age_by(&t.join("u3"), 20 * 60); // unread 20 min: kept, and the read below starts its 30 min again
    let (out, rc) = s.run("u1");
    assert_eq!(rc, 1, "{out}");
    assert!(
        out.starts_with("= j1.u1 ! snapshot_missing") && out.contains("a.html") && out.contains("→ fats "),
        "{out}"
    );
    assert!(!t.join("u1").exists() && !t.join("u2").exists(), "swept after the answer");
    assert!(s.run("u2").0.starts_with("= j2.u2 ! snapshot_missing"));
    assert_eq!(s.run("u3").1, 0);
    let since = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).expect("mtime").elapsed();
    assert!(since(&t.join("u3")).expect("now").as_secs() < 60, "a read touches its page");
    let status = s.run("").0;
    assert!(status.contains(" · 1 pages · "), "{status}");
    // another session, idle: kept while the last sweep of every session is under 5 min old, then its pages go
    s.file("d.html", &page("Page d", "<p>other text</p>"));
    assert_eq!(s.cmd("./d.html").env("FATS_SESSION", "old").output().expect("run").status.code(), Some(0));
    let (old, foreign) = (cache.join("old"), cache.join("mine/u1"));
    std::fs::create_dir_all(&foreign).expect("a dir that is no session");
    for p in [&old.join("u1"), &old.join("state.json"), &foreign] {
        age_by(p, 2 * 3600);
    }
    s.run("u3");
    assert!(old.join("state.json").exists(), "every session at most once per 5 min");
    age_by(&cache.join("swept"), 301);
    s.run("u3");
    assert!(!old.join("u1").exists() && old.join("state.json").exists(), "its pages went, its ids stay");
    assert!(foreign.exists() && t.join("u3").exists(), "nothing else");
}

/// A swept page's way back is priced like every arrow that can spend: `≤$X` = `dry:true`'s `up to`.
#[test]
fn a_swept_page_names_its_price() {
    let s = Sandbox::new("sweptprice");
    s.keys();
    std::fs::create_dir_all(s.dir.join("cache/t")).expect("session");
    let st = r#"{"u":[{"url":"https://www.reddit.com/r/rust/","dir":"t/u1","job":1}]}"#;
    std::fs::write(s.dir.join("cache/t/state.json"), st).expect("state");
    let (out, rc) = s.run("u1");
    assert_eq!(rc, 1, "{out}");
    let arrow = out.lines().last().and_then(|l| l.split(" → fats ").nth(1)).unwrap_or_default();
    let (cmd, price) = arrow.split_once(" ≤$").unwrap_or_default();
    assert_eq!(cmd, "https://www.reddit.com/r/rust/ fresh:true", "{out}");
    let o = s.cmd(&format!("{cmd} dry:true")).env_remove("FATS_OFFLINE").output().expect("run fats");
    let dry = String::from_utf8_lossy(&o.stdout);
    assert!(dry.contains(&format!("up to ${price}")), "{price} vs {dry}");
}

/// The URL index once pointed at a dir that now holds another page (`example.com` → serde's copy, `cache 0s $0`).
/// A hit is the page asked for or none; a swept page takes its index row along, an hour-old job its command.
#[test]
fn a_cache_hit_is_the_page_asked_for() {
    let s = Sandbox::new("hit");
    s.file("a.html", &page("Page a", "<p>some text here</p>"));
    assert_eq!(s.run("./a.html").1, 0);
    let c = s.dir.join("cache");
    let meta = r#"{"url":"https://docs.rs/serde/","provider":"plain","ok":true,"code":"ok","t":1,"kind":"html"}"#;
    std::fs::create_dir_all(c.join("t/u2")).expect("dir");
    std::fs::write(c.join("t/u2/meta.json"), meta).expect("meta");
    std::fs::write(c.join("t/u2/raw.html"), page("serde - Rust", "<p>serde text</p>")).expect("raw");
    let rows = r#"{"https://example.com":{"dir":"t/u2","t":1},"https://ex.com/gone":{"dir":"t/u1","t":1}}"#;
    std::fs::write(c.join("index.json"), rows).expect("index");
    let (out, rc) = s.run("https://example.com");
    assert!(rc != 0 && out.contains("! offline") && !out.contains("serde"), "a miss, never serde: {out}");
    age_by(&c.join("t/u1"), 31 * 60);
    let jobs = std::fs::read_to_string(c.join("jobs.json")).expect("jobs");
    let t = jobs.split("\"t\": ").nth(1).and_then(|r| r.split(['\n', ',', '}']).next()).expect("t");
    std::fs::write(c.join("jobs.json"), jobs.replacen(t, "1.0", 1)).expect("an hour-old job");
    age_by(&c.join("swept"), 301);
    s.run("config");
    let idx = std::fs::read_to_string(c.join("index.json")).expect("index");
    assert!(!idx.contains("ex.com/gone") && idx.contains("example.com"), "the row went with its page: {idx}");
    let jobs = std::fs::read_to_string(c.join("jobs.json")).expect("jobs");
    assert!(!jobs.contains("a.html") && jobs.contains("\"session\": \"t\""), "its command went, the row stays: {jobs}");
}

/// The default cache stays fats's own dir in the user cache dir (no temp dir: MCP hosts and `env -i` change
/// those); one an older build made is marked and swept by the same rule, nothing else in it touched, nothing said.
#[test]
fn the_default_cache_is_swept() {
    let s = Sandbox::new("defroot");
    let caches: &[&str] = match () {
        _ if cfg!(target_os = "macos") => &["Library", "Caches"],
        _ if cfg!(windows) => &["AppData", "Local"],
        _ => &[".cache"],
    };
    let root = caches.iter().fold(s.dir.join("home"), |p, c| p.join(c)).join("fatsearch");
    std::fs::create_dir_all(root.join("default/u1")).expect("an older build's cache");
    std::fs::create_dir_all(root.join("default/notes")).expect("not ours");
    std::fs::write(root.join("default/state.json"), "{}").expect("state");
    std::fs::write(root.join("default/u1/meta.json"), "{}").expect("meta");
    [root.join("default/u1/meta.json"), root.join("default/u1"), root.join("default/notes")]
        .iter()
        .for_each(|p| age_by(p, 2 * 3600));
    let o = s.cmd("config").env_remove("FATS_CACHE").env("FATS_SESSION", "default").output().expect("run fats");
    let out = String::from_utf8_lossy(&o.stdout);
    let row = out.lines().find(|l| l.starts_with("cache ")).unwrap_or_default();
    assert_eq!(row, format!("cache {} · default · swept", root.display()));
    assert!(o.stderr.is_empty() && root.join(".fatsearch-cache").exists(), "marked, nothing said");
    assert!(!root.join("default/u1").exists() && root.join("default/notes").exists(), "only its own page went");
}

/// The default key dir is the user's config dir: on Windows `%APPDATA%` as the install guide says.
#[test]
fn the_default_key_dir_is_the_users_config_dir() {
    let s = Sandbox::new("defkeys");
    let config: &[&str] = match () {
        _ if cfg!(target_os = "macos") => &["Library", "Application Support"],
        _ if cfg!(windows) => &["AppData", "Roaming"],
        _ => &[".config"],
    };
    let dir = config.iter().fold(s.dir.join("home"), |p, c| p.join(c)).join("fatsearch");
    let o = s.cmd("config").env_remove("FATS_KEY_DIR").output().expect("run fats");
    let out = String::from_utf8_lossy(&o.stdout);
    let row = out.lines().find(|l| l.starts_with("key_dir ")).unwrap_or_default();
    assert!(row.starts_with(&format!("key_dir {} · default", dir.display())), "{row}");
}

/// A dir with a page's `meta.json` and its parent's `state.json`, both written two hours ago.
fn stale_page(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).expect("dir");
    std::fs::write(dir.join("meta.json"), "{}").expect("meta");
    std::fs::write(dir.parent().expect("parent").join("state.json"), "{}").expect("state");
    age_by(dir, 2 * 3600);
}

/// A cache root fats made, holding one page of session `t` two hours old: the next run that is not
/// `FATS_SESSION=t` sweeps it (the last sweep is aged past 5 min).
fn swept_root(s: &Sandbox) -> PathBuf {
    s.file("a.html", &page("Page a", "<p>some text here</p>"));
    assert_eq!(s.run("./a.html").1, 0);
    let c = s.dir.join("cache");
    age_by(&c.join("t/u1"), 2 * 3600);
    age_by(&c.join("swept"), 301);
    c
}

/// `FATS_CACHE` at a dir fats did not make (another app's `otherapp/state.json`) is never swept, and says
/// so; in a root fats made, only a session's `u<N>`/`j<N>` with a meta.json (and `run/`) go, never a project's
/// `src/`.
#[test]
fn the_sweep_deletes_only_its_own() {
    let s = Sandbox::new("foreign");
    let shared = s.dir.join("shared");
    let notes = shared.join("otherapp/projects/important");
    std::fs::create_dir_all(&notes).expect("dir");
    std::fs::write(notes.join("notes.txt"), "keep").expect("notes");
    std::fs::write(shared.join("otherapp/state.json"), "{}").expect("state");
    let foreign = [notes.as_path(), &shared.join("otherapp/projects"), &shared.join("otherapp")];
    foreign.iter().for_each(|p| age_by(p, 2 * 3600));
    let o = s.cmd("config").env("FATS_CACHE", &shared).output().expect("run fats");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains(" · env FATS_CACHE · not swept (dir not created by fats)"), "{out}");
    assert!(notes.join("notes.txt").exists() && !shared.join("swept").exists(), "not ours: untouched");
    let o = s.cmd("").env("FATS_CACHE", &shared).output().expect("run fats");
    let out = String::from_utf8_lossy(&o.stdout);
    let walked = " · 0 pages · 0 searches · not swept (dir not created by fats) · 0 jobs\n";
    assert!(out.contains(walked), "another app's dirs never walked: {out}");
    // a root fats made, with a project that has a state.json
    let c = swept_root(&s);
    stale_page(&c.join("proj/src"));
    assert!(s.run("config").0.contains(" · env FATS_CACHE · swept\n"));
    assert!(!c.join("t/u1").exists(), "ours went");
    assert!(c.join("proj/src/meta.json").exists(), "nothing else");
}

/// A cache dir made before fats ran (`mkdir cache`) is not swept, but the status still counts the pages fats
/// stored there and their calls: size, pages, the provider health row.
#[test]
fn a_cache_dir_fats_did_not_make_still_counts_its_pages() {
    use std::io::Write;
    let s = Sandbox::new("premade");
    std::fs::create_dir_all(s.dir.join("cache")).expect("mkdir cache");
    let addr = serve(|_, c| {
        let body = page("Made Page", &"<p>A page stored in a cache dir another hand made.</p>".repeat(40));
        let h = format!("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\n\r\n", body.len());
        c.write_all(h.as_bytes()).and_then(|_| c.write_all(body.as_bytes()))
    });
    let first = online(&s, &addr);
    assert!(first.starts_with("= j1.u1 Made Page · "), "{first}");
    assert!(!s.dir.join("cache").join(fats::cache::MINE).exists(), "still not ours");
    let status = s.run("").0;
    assert!(status.contains(" · 1 pages · 0 searches · not swept (dir not created by fats) · 1 jobs\n"), "{status}");
    assert!(status.contains("\nplain 1/1 ") && !status.contains("plain - no calls yet"), "{first}{status}");
}

/// The sweep never follows a session that is a symlink out of the cache: not when it walks every session, not
/// when that session is this run's own.
#[cfg(unix)]
#[test]
fn the_sweep_never_follows_a_linked_session() {
    let s = Sandbox::new("linked");
    let c = swept_root(&s);
    let victim = s.dir.join("victim/u1");
    stale_page(&victim);
    std::os::unix::fs::symlink(s.dir.join("victim"), c.join("linked")).expect("symlink");
    assert!(s.run("config").0.contains(" · env FATS_CACHE · swept\n"));
    assert!(!c.join("t/u1").exists() && victim.join("meta.json").exists(), "ours went, the link's stayed");
    s.cmd("").env("FATS_SESSION", "linked").output().expect("run fats");
    assert!(victim.join("meta.json").exists(), "a linked session is never swept");
}

/// An hour after its job, a history row goes with its page: the URL (a search result's too) leaves state.json, the
/// id stays (`! expired`, rc 1; ids never restart), a live page's row stays.
#[test]
fn the_history_forgets_within_the_hour() {
    let s = Sandbox::new("history");
    let c = s.dir.join("cache");
    for n in ["a", "b"] {
        s.file(&format!("{n}.html"), &page(&format!("Page {n}"), "<p>some text here</p>"));
        assert_eq!(s.run(&format!("./{n}.html")).1, 0);
    }
    let st = std::fs::read_to_string(c.join("t/state.json")).expect("state");
    let result = r#""u": [{"url": "https://r.example/q", "dir": "", "serp": true, "job": 1}, "#;
    std::fs::write(c.join("t/state.json"), st.replacen(r#""u": ["#, result, 1)).expect("a search result");
    let jobs = std::fs::read_to_string(c.join("jobs.json")).expect("jobs");
    let t = jobs.split("\"t\": ").nth(1).and_then(|r| r.split(['\n', ',', '}']).next()).expect("t");
    std::fs::write(c.join("jobs.json"), jobs.replacen(t, "1.0", 1)).expect("an hour-old job");
    std::fs::create_dir_all(c.join("t/j1")).expect("a search");
    std::fs::write(c.join("t/j1/meta.json"), r#"{"url":"search:q","kind":"serp","ok":true}"#).expect("meta");
    [c.join("t/u1"), c.join("t/j1")].iter().for_each(|d| age_by(d, 31 * 60));
    age_by(&c.join("swept"), 301);
    assert!(s.run("stats").0.starts_with("= stats s:t · 0 searches · "), "an unread search goes in 30 min too");
    let st = std::fs::read_to_string(c.join("t/state.json")).expect("state");
    assert!(!st.contains("r.example") && !st.contains("a.html") && st.contains("b.html"), "{st}");
    let stats = s.run("stats").0;
    assert!(stats.starts_with("= stats s:t · 0 searches · 2 fetches"), "{stats}");
    assert!(!stats.contains("u1 ") && stats.contains(" · swept 0ms "), "a result never fetched is no row: {stats}");
    for id in ["u1", "u2", "u1 fresh:true"] {
        let (out, rc) = s.run(id);
        assert!(rc == 1 && out.starts_with(&format!("= j1.{} ! expired", &id[..2])), "{out}");
    }
    assert!(s.run("j1").0.starts_with("= ! expired j1 · its command and pages went after an hour"));
    assert!(s.run("./a.html").0.starts_with("= j3.u4 Page a"), "ids never restart");
}

/// A wiped cache restarts the ids, and the ledger rows of the records wiped with it name the same dirs: a row counts
/// for a session only when written since it began (state.json `t`, its first write), so a $0 session never shows the
/// old money, not even when `stats` is its first command (no session yet: nothing counts).
#[test]
fn a_wiped_cache_never_bills_the_new_session() {
    let s = Sandbox::new("wiped");
    let c = s.dir.join("cache");
    let old = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("now").as_secs() - 10;
    let row = |d: &str| serde_json::json!([old, [0.001, 0], c.join(d)]).to_string() + "\n";
    let rows = row("t/u1") + &row("t/j2");
    std::fs::create_dir_all(s.dir.join("keys")).expect("keys");
    std::fs::write(s.dir.join("keys").join("spend.log"), rows).expect("the wiped records' rows");
    let first = s.run("stats").0;
    assert!(first.starts_with("= stats s:t · 0 searches · 0 fetches · 0ms spent $0.0000"), "{first}");
    std::thread::sleep(std::time::Duration::from_millis(1100)); // the new session begins after the old rows
    s.file("a.html", &page("Page a", "<p>some text here</p>"));
    assert_eq!(s.run("./a.html").1, 0);
    std::fs::remove_dir_all(c.join("t/u1")).expect("swept");
    let out = s.run("stats").0;
    assert!(out.starts_with("= stats s:t · 0 searches · 1 fetches · 0ms spent $0.0000"), "{out}");
    assert!(out.contains(" · swept 0ms $0.0000") && !out.contains("j2 search"), "{out}");
    assert!(out.contains("spend all sessions: 24h $0.0020 · all $0.0020 · 2 paid records"), "{out}");
}

/// A dead run's calls in flight have no ledger row while its run file lives (`stats` reads them there); the sweep
/// that deletes `run/` stores them as the id's failed copy (its ledger row), so the session total keeps them and the
/// id still reads `! abandoned` for $0, its way back priced.
#[test]
fn a_dead_runs_cost_outlives_its_run_dir() {
    let s = Sandbox::new("deadrun");
    let c = s.dir.join("cache");
    s.file("a.html", &page("Page a", "<p>some text here</p>"));
    assert_eq!(s.run("./a.html").1, 0);
    let st = std::fs::read_to_string(c.join("t/state.json")).expect("state");
    let run = r#""u": [{"url": "https://ex.com/a", "dir": "", "job": 1}, "#;
    std::fs::write(c.join("t/state.json"), st.replacen(r#""u": ["#, run, 1)).expect("a fetch that died");
    let dead = r#"{"url":"https://ex.com/a","cmd":"","job":1,"spent":0,"t":1,"pid":0,"status":"bd running"}"#;
    std::fs::create_dir_all(c.join("t/run")).expect("run dir");
    std::fs::write(c.join("t/run/u1.json"), dead).expect("its run file");
    let spent = |out: &str| out.lines().next().and_then(|l| l.split(" spent ").nth(1)).unwrap_or_default().to_string();
    // a run that died after its store (u2 holds its page, the store wrote the row) is no dead run's money
    std::fs::write(c.join("t/run/u2.json"), dead.replace("/a", "/b")).expect("a run that stored");
    let before = s.run("stats").0; // its end is told: the run dir is written, then left 31 min
    assert!(s.run("u1 dry:true").0.contains("\nu1 local read · $0\n"), "dry = the `! abandoned` read");
    // a run whose lock is held lives: its run dir stays, no row
    std::fs::write(c.join("t/run/u3.json"), dead.replace("bd running", "")).expect("a live run");
    let lock = std::fs::File::create(c.join("t/run/u3.lock")).expect("lock");
    lock.try_lock().expect("held");
    age_by(&c.join("t/run"), 31 * 60);
    assert!((s.run("guide"), c.join("t/run").exists()).1, "a live run keeps run/");
    drop(lock);
    age_by(&c.join("t/run"), 31 * 60);
    let sweeps: Vec<_> = (0..8).map(|_| s.cmd("guide").spawn().expect("spawn")).collect();
    sweeps.into_iter().for_each(|mut p| assert!(p.wait().is_ok_and(|w| w.success())));
    // which of the 8 takes run/ is a race: one that met another's lock probe, or a footer's write under a run's lock
    // (a fresh mtime), leaves it, and one may take it at once; a later sweep of the idle run/ takes what is left
    for _ in 0..5 {
        if !c.join("t/run").exists() {
            break;
        }
        age_by(&c.join("t/run"), 31 * 60);
        s.run("guide");
    }
    s.auth(&[("oxy", "x")]);
    let after = s.cmd("stats").output().expect("run").stdout;
    let after = String::from_utf8_lossy(&after);
    assert!(spent(&before).contains("≤$0.0015") && spent(&after) == spent(&before), "{before}\n{after}");
    assert!(!c.join("t/run").exists() && after.contains(" · 1 paid records · "), "8 sweeps, one row: {after}");
    // the id keeps the dead run as its failed copy: a bare read is `! abandoned` at $0, the priced fix on its row
    let run = |cmd: &str| s.cmd(cmd).output().expect("run");
    let (o, row) = (run("u1"), "\nu1 ! abandoned ex.com/a → fats u1 fresh:true ≤$0.0025\n");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.code() == Some(1) && out.starts_with("= j1.u1 ! abandoned ") && out.contains(row), "{out}");
    assert!(String::from_utf8_lossy(&run("u1 dry:true").stdout).contains("\nu1 local read · $0\n"));
    // its `stats` row = the `!` row its read prints, what it ran before the move
    // (its time = the sweep's own disk writes, 0 to a few ms under load: not part of the row's rule)
    let row =
        after.lines().find(|l| l.starts_with("u1 ex.com · ! abandoned ex.com/a · brightdata ")).unwrap_or_default();
    let tail = "ms $0.0000 · bd:abandoned✗≤$0.0015 (unverified) → fats u1 fresh:true ≤$0.0025";
    assert!(row.ends_with(tail), "{after}");
    assert!(s.run("stats").0.contains(" · 1 paid records · "), "the read spent nothing");
}

/// A ledger row fats cannot write is said in `stats`, and its total is `?`, never a silent $0.
#[test]
fn an_unwritable_ledger_is_a_row() {
    let s = Sandbox::new("noledger");
    s.file("a.html", &page("Page a", "<p>some text here</p>"));
    std::fs::write(s.dir.join("keys"), "a file, not a dir").expect("block the key dir");
    assert_eq!(s.run("./a.html").1, 0);
    let out = s.run("stats").0;
    assert!(out.lines().next().is_some_and(|l| l.ends_with(" spent ? (ledger not written)")), "never $0: {out}");
    assert!(
        out.contains("\n! cache_write ledger not written: ") && out.contains(" → fats config · FATS_KEY_DIR"),
        "{out}"
    );
}

/// A flag fats lacks is a usage error, never a search: `--login` is not a query. Two other words make it a
/// search's text (`git rebase --onto`), as does the `search` verb; a `-word` after a word is the engine's
/// exclusion. A flag named like a mod points to the mod. `--help` / `-h` anywhere is the card.
#[test]
fn an_unknown_flag_never_searches() {
    let s = Sandbox::new("flags");
    // a flag alone, or beside URLs and other flags only (no free word to make it text): a paid search else
    for f in ["--login", "-x", "--login --x", "--dry https://ex.com"] {
        let (out, rc) = s.run(f);
        let flag = f.split(' ').find(|w| w.starts_with('-')).unwrap_or_default();
        let fix = if ["--verbose", "--dry"].contains(&flag) { "mod: dry:true" } else { "keys: fats setup" };
        let fix = fix.replace("dry", flag.trim_start_matches('-'));
        assert_eq!(out, format!("! unknown flag {flag} · {fix} · help: fats --help\n"), "{f}");
        assert_eq!(rc, 2, "{f}");
    }
    // beside a page (an id or a file) the words are a free filter: `u1 -n` looks for the option in a man page
    let page = Sandbox::new("flag-filter");
    std::fs::write(page.dir.join("man.html"), "<p>Use -n to show what would run, then run it for real.</p>").unwrap();
    let man = page.dir.join("man.html").display().to_string();
    for f in [format!("{man} -n"), format!("--verbose {man}"), format!("{man} -x -y"), "u1 -n".into()] {
        let (out, rc) = page.run(&f);
        assert!(rc == 0 && out.starts_with("= j") && out.contains(" filter \"-"), "{f}: {out}");
    }
    let card = s.run("--help").0;
    assert!(card.starts_with("fats <url|file.pdf>"));
    for f in ["-h", "rust --help", "login --help", "u1 -h", "help login", "mcp --help"] {
        assert_eq!(s.run(f).0, card, "{f}");
    }
    assert!(s.run("--version").0.starts_with("= fats "));
    // a command that names nothing (mods and a verb aside) is the status; a mod's row points nowhere then, never
    // at a bare verb or at another mod alone
    let status = s.run("").0;
    for f in ["search", "read", "-v", "fetch:3", "-v zone:x", "open -v", "search fetch:3", "read zone:x"] {
        let (out, rc) = s.run(f);
        let rest: Vec<&str> = out.lines().filter(|l| !l.starts_with("! ")).collect();
        assert!(rc == 0 && rest == status.lines().collect::<Vec<_>>(), "{f}: not the status: {out}");
        assert!(out.lines().all(|l| !l.starts_with("! ") || !l.contains(" → ")), "{f}: a dead-end arrow: {out}");
    }
    assert!(s.run("-v").0.contains("\n! not_in_v1 verbose:true · no diagnostics in this build\n"));
    assert!(s.run("fetch:3 dry:true").0.contains("\n! ignored fetch:3 · only a web search prefetches\n"));
    let (beside, _) = s.run("u1 -v");
    assert!(beside.ends_with("\n! not_in_v1 verbose:true · no diagnostics in this build → fats u1\n"), "{beside}");
    assert!(s.run("stats").0.contains(" · 0 searches · "), "a flag searched");
    let searches = ["rust -tokio", "git rebase --onto", "gcc --std=c++20 error", "search -tokio rust", "rust --fast"];
    for q in searches.into_iter().chain(["-5 degrees celsius", "-x rust", "rust — tokio", "1990 – 2000 census"]) {
        let words = q.trim_start_matches("search ");
        assert!(s.run(q).0.contains(&format!("search \"{words}\"")), "{q} is a search's text");
    }
    // a dash in a filter's words is the text: the exact quote is matched as typed
    let book = "<p>Rust — the book about borrowing and lifetimes, read it twice.</p><p>Rust the book club.</p>";
    std::fs::write(s.dir.join("work/p.html"), format!("<title>Books</title>{book}")).expect("page");
    let out = s.run("p.html Rust — the book").0;
    assert!(out.contains(" filter \"Rust — the book\" · ") && out.contains(" match:exact"), "{out}");
    assert!(
        out.lines()
            .nth(1)
            .is_some_and(|l| l.ends_with("Rust — the book about borrowing and lifetimes, read it twice.")),
        "{out}"
    );
    // a file named like a flag is the file (the file test runs before the flag one)
    std::fs::write(s.dir.join("work/-notes.pdf"), include_bytes!("fixtures/two-pages.pdf")).expect("pdf");
    let pdf = s.run("-notes.pdf").0;
    assert!(pdf.starts_with("= j") && pdf.contains(" Tiny Test Doc · pdf · "), "a file: {pdf}");
}

/// What people type for a command (`words.json` `aliases`) is that command when it is the whole command, mods and
/// `json` aside; words that name no command (`elsewhere`) get one `!` line. None ever searches.
#[test]
fn command_synonyms_are_free() {
    let s = Sandbox::new("synonyms");
    let card = s.run("--help").0;
    let install = format!("! install: see docs/installation.md in the repo · {}\n", to_guide());
    let other = |w: &str| format!("! {w} is not a fats command · keys: fats setup · status: fats · {}\n", to_guide());
    for (w, want) in [
        ("login", "= dry setup no key · pages free".to_string()),
        ("Sign-In", "= dry setup no key · pages free".into()),
        ("keys limit:5", "= dry setup no key · pages free".into()),
        ("login json", "{\"bytes_in\":0,".into()),
        ("balance", "= ! offline account ".into()),
        ("usage json", "{\"bytes_in\":0,".into()),
        ("commands", card.clone()),
        ("-V", "= fats ".into()),
        ("version json", "{\"bytes_in\":0,".into()),
        ("install", install.clone()),
        ("upgrade", install.replacen("install", "upgrade", 1)),
        ("logout", other("logout")),
        ("whoami json", "{\"bytes_in\":0,".into()),
        ("doctor wait:5", other("doctor")),
        ("quit", other("quit")),
    ] {
        let out = s.run(w).0;
        assert!(out.starts_with(&want) && !out.contains("search \""), "{w}: {out}");
    }
    assert!(s.run("login json").0.contains("\"= dry setup no key · pages free"), "login json = setup");
    for q in ["login page design", "init rust project", "exit code 137"] {
        assert!(s.run(q).0.contains(&format!("search \"{q}\"")), "{q} is a search");
    }
    assert!(s.run("u1 key").0.starts_with("? u1 unknown"), "a page's filter");
    assert!(s.run("stats").0.contains(" · 0 searches · 0 fetches · 0ms spent $0.0000"), "a synonym spent");
}

/// A stored search of 25 results, run as `cmd` (jobs.json), the cursor at its start: what `more` lists.
fn listing(s: &Sandbox, cmd: &str) -> Vec<String> {
    let c = s.dir.join("cache");
    std::fs::create_dir_all(c.join("t/j1")).expect("dirs");
    let urls: Vec<String> = (1..=25).map(|i| format!("https://ex{i}.example/result-page-number-{i}")).collect();
    let u: Vec<_> = urls.iter().map(|u| serde_json::json!({"url": u, "dir": "", "job": 1, "serp": true})).collect();
    let state = serde_json::json!({ "u": u, "cursor": {"u": 1, "serp": [1, 0]} });
    std::fs::write(c.join("t/state.json"), state.to_string()).expect("state");
    std::fs::write(c.join("jobs.json"), serde_json::json!([{"session": "t", "cmd": cmd, "t": 1}]).to_string())
        .expect("jobs");
    let snip = "a result whose snippet is long enough to fill the list cap in a few rows of text ".repeat(2);
    let hits: Vec<_> =
        urls.iter().map(|u| serde_json::json!({"url": u, "title": "Result page", "snippet": snip})).collect();
    let hop = serde_json::json!({"provider": "oxylabs", "code": "ok", "ms": 900, "cost": 0.001});
    let meta = serde_json::json!({"url": "search:result page", "provider": "oxylabs", "ms": 900, "cost": 0.001,
        "ok": true, "code": "ok", "t": 1, "kind": "serp", "hops": [hop], "first": 1, "hits": hits});
    std::fs::write(c.join("t/j1/meta.json"), meta.to_string()).expect("meta");
    s.run("more").0.lines().map(String::from).collect()
}

/// A search's own `limit:N` is the rows it shows (`more` goes on by N); without it every row a read's cap holds.
#[test]
fn a_search_limit_is_the_rows_shown() {
    let s = Sandbox::new("limit");
    let rows = |l: &[String]| l.iter().filter(|r| r.starts_with('u') && r.contains("  ")).count();
    let default = listing(&s, "result page");
    assert!(default[0].contains(" · 25 results · 17 shown u1-u17 · "), "the read cap: {default:?}");
    // `limit:N` = at most N rows (20 at most) within a read's 1000-token cap, a cut by tokens said on the `=` line
    let toks = |l: &[String]| l.iter().filter(|r| r.starts_with('u')).map(|r| fats::clean::tok(r)).sum::<usize>();
    for (n, want, said) in [(12, 12, false), (20, 17, true), (50, 17, true), (5, 5, false)] {
        let l = listing(&s, &format!("result page limit:{n}"));
        assert_eq!((rows(&l), toks(&l) <= 1000), (want, true), "limit:{n} {l:?}");
        let head = format!(" · {want} shown u1-u{want} of limit:{} (1000 tok cap) · ", n.min(20));
        assert_eq!(l[0].contains(&head), said, "limit:{n} {}", l[0]);
    }
    let more = s.run("more").0;
    assert!(more.contains(" · 5 shown u6-u10 · ") && more.contains("\nu10  "), "more goes on by 5: {more}");
}

/// `site:` alone is a web search on that site (Oxylabs answered `site:x.com/<path> after:<date>` with 20
/// results, live 2026-10-16): no `?`, no `! ignored site:`.
#[test]
fn a_site_alone_is_a_search() {
    let s = Sandbox::new("siteonly");
    let (out, rc) = s.run("site:ex.com/news/status since:2026-10-01 limit:20 dry:true");
    assert_eq!(rc, 0, "{out}");
    assert!(out.contains("\nsearch \"site:ex.com/news/status after:2026-10-01\" · "), "{out}");
    assert!(!out.contains("ignored") && !out.starts_with('?'), "{out}");
    let (out, _) = s.run("site:ex.com/news/status");
    assert!(out.starts_with("= ! offline search \"site:ex.com/news/status\" "), "{out}");
}

/// A part of a page with no sections names none, never a range `#1-#0`, for a read and a filter.
#[test]
fn no_sections_is_said() {
    let s = Sandbox::new("nosecs");
    let page = s.dir.join("empty.html");
    std::fs::write(&page, "<html><head><title>Empty shell page</title></head><body><div id=root></div></body></html>")
        .expect("page");
    assert!(s.run(&page.to_string_lossy()).0.contains(" · 0 sections · "));
    for q in ["u1#1", "u1#2 word"] {
        let k = &q[3..4];
        assert_eq!(s.run(q).0, format!("? u1#{k} unknown · u1 has no sections · outline: fats u1\n"), "{q}");
    }
}

/// A search hit whose URL is in the cache says so in the list, `stats`, the job view and the status count, and opens
/// from it for $0.
#[test]
fn a_cached_hit_says_so() {
    let s = Sandbox::new("cachedhit");
    let url = "https://ex3.example/result-page-number-3";
    let d = s.dir.join("cache/t/u13");
    std::fs::create_dir_all(&d).expect("dir");
    std::fs::write(d.join("raw.html"), page("Result three", &"<p>the third result, read before</p>".repeat(20)))
        .expect("raw");
    let meta = serde_json::json!({"url": url, "provider": "plain", "ok": true, "code": "ok", "t": 1, "hops": []});
    std::fs::write(d.join("meta.json"), meta.to_string()).expect("meta");
    let idx = serde_json::json!({ url: {"dir": "t/u13", "t": 1} });
    std::fs::write(s.dir.join("cache/index.json"), idx.to_string()).expect("index");
    // u3's and u4's prefetches failed (plain only): u3's URL is cached, so its row says cached, never `no copy`;
    // neither row names a render the prefetch never tried (an unpriced `js:true` arrow)
    std::fs::create_dir_all(s.dir.join("cache/t/run")).expect("run dir");
    let done = serde_json::json!({"ok": false, "code": "login_wall", "cost": 0.0, "warn": [], "fail": null});
    for n in [3, 4] {
        let url = format!("https://ex{n}.example/result-page-number-{n}");
        let run = serde_json::json!({"url": url, "cmd": "result page", "job": 1, "spent": 0, "t": 1, "pid": 4242, "status": "", "done": done});
        std::fs::write(s.dir.join(format!("cache/t/run/u{n}.json")), run.to_string()).expect("run");
    }
    let list = listing(&s, "result page limit:5");
    let row = |n: usize| list.iter().find(|l| l.starts_with(&format!("u{n}  "))).cloned().unwrap_or_default();
    assert!(row(3).ends_with(" · cached $0") && !row(3).contains("no copy") && !row(2).contains("cached"), "{list:?}");
    assert!(row(4).ends_with("…\"") && !list.iter().any(|l| l.contains("js:true")), "{list:?}");
    let stats = s.run("stats all").0;
    assert!(stats.contains("\nsearch→u3 ex3.example/") && stats.contains("· cached → fats u3\n"), "{stats}");
    assert!(stats.contains("· not fetched → fats u2\n"), "{stats}");
    let job = s.run("j1").0;
    assert!(
        job.contains("\nsearch→j1.u3 ex3.example/") && job.contains(" · cached → fats j1.u3\n") && job.contains("u2 "),
        "{job}"
    );
    assert!(s.run("").0.contains(" (24 search results not fetched) "), "the status count leaves the cached one out");
    let stats = s.run("stats all").0;
    assert!(stats.contains("\nsearch→u3 ex3.example/") && stats.contains("· cached → fats u3\n"), "{stats}");
    assert!(stats.contains("· not fetched → fats u2\n"), "{stats}");
    let out = s.run("u3").0;
    assert!(out.starts_with("= j1.u3 Result three") && out.contains(" cache"), "opens from the cache: {out}");
}

/// The background line names a search's prefetch, why a failed run failed and what it cost: a billed call its
/// `spent $`, nothing billed no spend (as every `=` line, `provider::spent`).
#[test]
fn a_failed_prefetch_says_what_it_cost() {
    let s = Sandbox::new("prefetchfoot");
    let cache = s.dir.join("cache");
    let t = fats::cache::now() - 30.0;
    let urls = ["https://ex.com/a", "https://ex.com/b", "https://ex.com/c"];
    let state: Vec<_> = urls
        .iter()
        .enumerate()
        .map(|(i, u)| serde_json::json!({"url": u, "dir": format!("t/u{}", i + 1), "job": 1, "serp": i < 2}))
        .collect();
    std::fs::create_dir_all(cache.join("t/run")).expect("mkdir");
    std::fs::write(cache.join("t/state.json"), serde_json::json!({ "u": state }).to_string()).expect("state");
    for (i, (u, cost, cmd, ok, code)) in [
        (urls[0], 0.0, "rust tokio", false, "login_wall"),
        (urls[1], 0.001, "rust tokio", false, "captcha_wall"),
        (urls[2], 0.0, "https://ex.com/c", true, "ok"),
    ]
    .into_iter()
    .enumerate()
    {
        let n = i + 1;
        std::fs::create_dir_all(cache.join(format!("t/u{n}"))).expect("dir");
        // u2's billed answer was never stored (a failed cache write): the run's own cost bills it
        let meta = serde_json::json!({"url": u, "provider": "oxy", "ok": ok, "code": code, "cost": cost, "t": t});
        if n != 2 {
            std::fs::write(cache.join(format!("t/u{n}/meta.json")), meta.to_string()).expect("meta");
        }
        let done = serde_json::json!({"ok": ok, "code": code, "cost": cost, "warn": [], "fail": null});
        let run = serde_json::json!({"url": u, "cmd": cmd, "job": 1, "spent": 0, "t": t, "pid": 4242, "status": "", "done": done});
        std::fs::write(cache.join(format!("t/run/u{n}.json")), run.to_string()).expect("run");
    }
    let out = s.run("config").0;
    let foot = "~ prefetch u1 failed · login_wall · prefetch u2 failed · captcha_wall · spent $0.0010 · u3 done\n";
    assert!(out.ends_with(foot), "{out}");
    assert!(!s.run("config").0.contains("\n~ "), "told once");
}

/// A lone command word with the output mod is that command (`fats version json` once searched "version", paid);
/// with a real word it stays a search.
#[test]
fn a_command_word_with_json_is_the_command() {
    let s = Sandbox::new("wordjson");
    for (cmd, head) in [("version json", "= fats "), ("-V json", "= fats "), ("status json", "= s:t ")] {
        let (out, rc) = s.run(cmd);
        let rec: serde_json::Value = serde_json::from_str(out.trim()).expect("one record");
        let first = rec["lines"][0].as_str().unwrap_or_default().to_string();
        assert!(rc == 0 && first.starts_with(head) && !first.contains("search \""), "{cmd}: {out}");
    }
    assert!(s.run("version control").0.starts_with("= ! offline search \"version control\""));
}

/// Same binary, two audiences: piped (and `FATS_HUMAN=0`, `json`) the bytes agents read today; `FATS_HUMAN=1` (a
/// terminal) the human screens: home, help and its topics, no hole left, no colour under `NO_COLOR`.
#[test]
fn a_terminal_gets_the_screens_a_pipe_the_lines() {
    let s = Sandbox::new("screens");
    let with = |args: &str, human: &str, colour: &str| {
        let o = s.cmd(args).env("FATS_HUMAN", human).env("NO_COLOR", colour).output().expect("run");
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let hole = |out: &str| out.split('{').skip(1).any(|r| r.split_once('}').is_some_and(|(h, _)| !h.contains(' ')));
    for (args, head) in [
        ("", "READY?"),
        ("help", "USAGE"),
        ("--help", "USAGE"),
        ("help mcp", "claude mcp add"),
        ("help setup", "OXYLABS_WEB_API_KEY"),
        ("help search", "site:github.com"),
    ] {
        let out = with(args, "1", "1");
        assert!(out.contains(head) && !hole(&out) && !out.contains('\x1b'), "{args}: {out}");
        assert!(with(args, "1", "").contains("\x1b[1m"), "{args}: colour on a terminal");
    }
    let home = with("", "1", "1");
    assert!(home.contains("✓ Free fetch") && home.contains("○ Oxylabs") && !home.contains("YOUR STUFF"), "{home}");
    let status = s.run("status").0;
    // no key: pages are free, web search needs one key, the default named; an agent runs its setup for the user
    let off = "= s:t no pages · pages free, no key · web search off: needs one key → run fats setup oxy, give the user its 3 lines\n";
    assert!(status.starts_with(off), "{status}");
    assert_eq!(s.run("").0, status, "bare fats piped = the status");
    assert_eq!(with("", "0", ""), status, "FATS_HUMAN=0 = the status");
    let human = status.replace("run fats setup oxy, give the user its 3 lines", "fats setup oxy");
    assert_eq!(with("status", "1", ""), human, "status = the agent's screen on a terminal, the human runs the setup");
    let lines = |j: String| serde_json::from_str::<serde_json::Value>(&j).map(|v| v["lines"].clone()).ok();
    let json = lines(with("json:true", "1", ""));
    assert!(json.is_some() && json == lines(s.run("json:true").0), "json is the agent's, on a terminal too");
    let card = s.run("help").0;
    assert_eq!(s.run("--help").0, card, "the agent's help = the card");
    assert_eq!(with("guide", "1", ""), s.run("guide").0, "guide stays the guide on a terminal");
    assert_eq!(card, format!("{}\n{}\n", fats::lang::CARD, to_guide()));
}

/// A walled page and a search with no provider key: piped, the `! no_key` rows; on a terminal, the tip to add one.
#[test]
fn a_keyless_failure_on_a_terminal_is_a_tip() {
    use std::io::Write;
    let addr = serve(|_, c| {
        let body = "<html><head><title>Just a moment...</title></head><body>checking your browser</body></html>";
        let h = format!("HTTP/1.1 403 Forbidden\r\ncontent-type: text/html\r\ncontent-length: {}\r\n\r\n", body.len());
        c.write_all(h.as_bytes()).and_then(|_| c.write_all(body.as_bytes()))
    });
    let s = Sandbox::new("tip");
    let run = |args: &str, human: &str| {
        let mut c = s.cmd(args);
        c.env_remove("FATS_OFFLINE").env("FATS_ALLOW_PRIVATE", "1").env("FATS_HUMAN", human).env("NO_COLOR", "1");
        String::from_utf8_lossy(&c.output().expect("run").stdout).into_owned()
    };
    let piped = run(&format!("{addr}/wall"), "0");
    assert!(piped.contains("! cloudflare_wall") && piped.contains("! no_key oxy"), "{piped}");
    let tip = run(&format!("{addr}/wall fresh:true"), "1");
    assert!(tip.starts_with("✗ 127.0.0.1 blocks the free fetch ("), "the site, no port: {tip}");
    assert!(tip.contains("→ tip: add a provider (2 min): fats setup"), "{tip}");
    assert!(!tip.contains("no_key") && !tip.contains('{'), "{tip}");
    // a search: no provider can run, so nothing is called
    assert!(run("rust http client", "0").starts_with("= ! no_key search"));
    assert!(run("rust http client", "1").starts_with("○ Web search needs a provider key"));
    assert!(run("rust http client dry:true", "1").contains("no_key"), "dry is the agent's language");
}

/// Every `→ fats …` the guide and the MCP instructions teach, and a `budget_hit` row's moves, paste into bash and
/// zsh (the macOS default) whole, as printed, with `dry:true` after it: rc 0, never a shell error (`~25s` was zsh's
/// "no such user", `pay?` its "no matches found").
#[test]
fn the_guide_arrows_paste_in_every_shell() {
    let s = Sandbox::new("guide-paste");
    s.file("s.html", "<h1>T</h1><p>one</p><h2>A</h2><p>two</p><h2>B</h2><p>three</p>");
    assert_eq!(s.run("./s.html").1, 0);
    let text = [fats::lang::GUIDE.to_string(), fats::mcp::instructions()].join("\n");
    let arrows: Vec<&str> = text.split("→ ").skip(1).filter(|a| a.starts_with("fats ")).collect();
    let arrows: Vec<&str> = arrows.iter().map(|a| a.split(['`', '\n']).next().unwrap_or_default().trim()).collect();
    assert!(arrows.len() >= 3 && arrows.iter().any(|a| a.contains("≈25s")), "{arrows:?}");
    // and a `budget_hit` row's two moves, as printed (`a_capped_page_with_a_copy_exits_zero`)
    arrows.iter().chain([&PAY_ROW]).for_each(|a| pastes(&s, a));
}

/// `arrow` pasted into bash and zsh (the macOS default; `?`, `~`, `(` are its globs) whole, as printed, unquoted,
/// with `dry:true` after it, in `s`'s work dir: rc 0, never a shell error.
fn pastes(s: &Sandbox, arrow: &str) {
    let bin = std::path::Path::new(env!("CARGO_BIN_EXE_fats")).parent().expect("bin dir").display().to_string();
    for sh in ["/bin/bash", "/bin/zsh"].into_iter().filter(|sh| std::path::Path::new(sh).exists()) {
        let mut c = Command::new(sh);
        c.env_clear().env("PATH", format!("{bin}:/usr/bin:/bin")).env("HOME", s.dir.join("home")).envs(s.env());
        let o = c.current_dir(s.dir.join("work")).arg("-c").arg(format!("{arrow} dry:true")).output().expect("sh");
        let (out, err) = (String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        assert!(o.status.success() && err.is_empty(), "{sh} -c '{arrow} dry:true': {out}{err}");
    }
}

/// A pasted arrow never searches nor filters on its decorations: every arrow shape the binary prints (the error
/// table's fixes, priced and marked as `lang::fix` prints them; a JS render's tiers, price and time; a held or
/// stored part's size and `$0`; a stored render; a pending run's poll; the screens' setup moves), pasted whole
/// with `dry:true`, reads as its command alone (an older build's `(…)` groups too; a printed arrow has none, and
/// survives an unquoted zsh: no `(`, `~`, `;`, its `$0` expanded). A search keeps its parentheses.
#[test]
fn a_pasted_arrow_never_searches() {
    let s = Sandbox::new("paste");
    let mut shapes: Vec<String> = [
        "fats u1 js:true (oxyjs) ≤$0.0015 ~25s",
        "fats u1 js:true (bd, then oxyjs) ≤$0.0030 ~25s",
        "fats https://example.com/ js:true (bd, then oxyjs) ≤$0.0030 ~25s",
        "fats u1#3 · 1.5k tok · $0",
        "fats u1#2fv (page record) · 995 tok · $0",
        "fats u2 (rendered)",
        "fats u1#3fy (page content)",
        "fats u3 wait:30 (free, usually ready in ~11s)",
        "fats j5 wait:30 (free; check again in 30 s)",
        "fats u1 fresh:true provider:oxy ≤$0.0025",
        "fats u1 fresh:true timeout:150 (≤$0.0015 unverified)",
        "fats u1 ~25s ≤$0.0030 (oxyjs) js:true",
        "fats u1 js:true · bd, then oxyjs ≤zsh.0030 ≈25s",
        "fats u1 fresh:true ≤bash.0015",
        PAY_ROW,
    ]
    .map(String::from)
    .to_vec();
    let arg = |code: &str| match code {
        "budget_hit" => "0.004",
        "no_zone" => "my_zone",
        "off" => "oxy",
        c if c.starts_with("http_4") => "l4",
        _ => "js:true",
    };
    for (code, t) in fats::lang::FIX.iter().filter(|r| r.1.starts_with("{b} ")) {
        let holes = [("b", "fats"), ("id", "u1"), ("url", "https://ex.com/a"), ("arg", arg(code))];
        shapes.push(format!("{} ≤$0.0015 · last try not charged", fats::lang::draw(t, &holes)));
    }
    // what the binary prints now: no `(…)` group, no shell-special character in any arrow
    let printed =
        [fats::fetch::JS_QUOTE.to_string()].into_iter().chain(fats::lang::FIX.iter().map(|r| r.1.to_string()));
    assert!(printed.clone().all(|f| !f.contains(['(', ')', '~', ';'])), "{:?}", printed.collect::<Vec<_>>());
    for (_, text) in fats::lang::SCREENS {
        let moves = text.split("→ ").skip(1).filter(|m| m.starts_with("fats "));
        shapes.extend(moves.map(|m| m.split(['|', '{', '\n']).next().unwrap_or_default().trim().to_string()));
    }
    assert!(shapes.len() > 40 && shapes.iter().any(|m| m == "fats setup free on"), "{shapes:?}");
    for shape in &shapes {
        let c = fats::lang::parse(std::slice::from_ref(shape));
        assert!(c.words.is_empty() || c.ask.is_some(), "{shape} → words {:?}", c.words);
        let (out, _) = s.run(&format!("{shape} dry:true"));
        assert!(!out.contains("search \"") && !out.contains("≤$0.0015 (") && !out.contains(" tok "), "{shape}: {out}");
    }
    s.file("a.html", SECTIONS);
    s.run("./a.html");
    let (out, _) = s.run("fats u1#3 · 1.5k tok · $0 dry:true");
    assert!(out.starts_with("= dry u1#3 dry:true · spends nothing now"), "the stripped canonical form: {out}");
    let (out, _) = s.run("rust (programming language) dry:true");
    assert!(out.contains("search \"rust (programming language)\""), "a search keeps its parentheses: {out}");
}

/// A JS render served once is the answer to every later `js:true` of its URL in the session, from the og card's
/// id or the render's own (`cache`, $0, no new id; its dry `= u2 (rendered, cache)`); only `fresh:true` renders
/// again. The card's re-read points at the render (`→ fats u2 · rendered`), a paid ask's and the auto ladder's
/// alike, never at a second `js:true`.
#[test]
fn a_served_render_answers_js_true_again() {
    let url = "https://social.example/a/status/1";
    let s = Sandbox::new("rerender");
    s.keys();
    let card = serde_json::json!({"url": url, "provider": "plain", "ok": true, "code": "ok", "t": 1, "kind": "og",
        "hops": [{"provider": "plain", "code": "ok"}]});
    let prose = "The crew photographed the far side of the Moon through the capsule window. ".repeat(8);
    let render = format!(
        "<html><head><title>Space agency</title></head><body><h1>Space agency</h1><h2>Post</h2><p>{prose}</p>\
         <h2>Replies</h2><p>{prose}</p></body></html>"
    );
    let served = |tier: &str, hedge: &str| {
        let hops = serde_json::json!([{"provider": "plain", "code": "js_shell"}, {"provider": tier, "code": "ok", "cost": 0.0015}]);
        serde_json::json!({"url": url, "provider": tier, "ok": true, "code": "ok", "t": 2, "cost": 0.0015,
            "hedge": hedge, "hops": hops})
    };
    let raw = include_str!("fixtures/og-card.html").to_string();
    store(&s, url, &[(card.clone(), raw.clone())]);
    let alone = s.run("u1").0;
    assert!(alone.contains("→ fats u1 js:true · bd, then oxyjs"), "the control: {alone}");
    // the paid ask's render (`hedge:off`) and the auto ladder's (bd after plain's shell, no `hedge`)
    for (tier, hedge) in [("oxylabs-js", "off"), ("brightdata", "")] {
        store(&s, url, &[(card.clone(), raw.clone()), (served(tier, hedge), render.clone())]);
        let again = s.run("u1").0;
        let head = again.lines().next().unwrap_or_default();
        assert!(head.contains("→ fats u2 · rendered") && !again.contains("js:true"), "{tier}: {again}");
        for id in ["u1", "u2"] {
            let dry = s.run(&format!("{id} js:true dry:true")).0;
            assert!(dry.contains(" = u2 (rendered, cache) · $0") && !dry.contains("new u3"), "{tier} {id}: {dry}");
            let (out, rc) = s.run(&format!("{id} js:true"));
            let head = out.lines().next().unwrap_or_default();
            assert!(rc == 0 && head.starts_with("= ") && head.contains("u2 ") && head.contains(" cache "), "{out}");
        }
        let state = std::fs::read_to_string(s.dir.join("cache/t/state.json")).expect("state");
        assert!(!state.contains("u3"), "no new id: {state}");
        let fresh = s.run("u1 js:true fresh:true dry:true").0;
        assert!(!fresh.contains("rendered, cache"), "fresh:true renders again: {fresh}");
    }
}
