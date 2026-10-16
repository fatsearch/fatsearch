//! Parity gate: the Rust binary vs the Python prototype (the oracle) on every cached fixture page.
//! Oracle = the prototype's parity script, JSONL (outline, u1#1, u1#2, u1 <words>, u1 links per page).
//! Needs `FATS_PARITY=<oracle.jsonl>` (the cached pages are not in this repository); without it the test skips
//! and says so.
//!
//! Two runs per page, each in a throwaway cache seeded with raw.html only (the doc is rebuilt by Rust):
//!   port      FATS_CHUNK=400 + `match:words`: the prototype's own rules → must be 0 diffs
//!   doctrine  defaults (300-token blocks, exact rung first): counted and reported
//! Normalised before diffing: whitespace, `jN.` id prefixes, ms/cost/age, a trailing `· cache` on the `=` line,
//! `tok` counts within ±10%, the media count and rows (newer than the prototype; their tokens come off a whole
//! read's `=` figure too, `no_media_tok`).

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

fn norm(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| {
            let mut out: Vec<String> = Vec::new();
            for w in l.split_whitespace() {
                let w = strip_job(w);
                let timing = w.ends_with("ms") && w[..w.len() - 2].bytes().all(|b| b.is_ascii_digit())
                    || w.starts_with('$')
                    || w.starts_with("~$")
                    // an arrow's worst case (`fetch::worst`) is the route of the keys present, like `~$`
                    || w.starts_with("≤$")
                    || (out.last().is_some_and(|p| p == "cache")
                        && w.ends_with(['s', 'm', 'h'])
                        && w[..w.len() - 1].bytes().all(|b| b.is_ascii_digit()));
                if !timing {
                    out.push(w.replace("~tok", "tok"));
                }
            }
            out.join(" ")
        })
        .enumerate()
        // every snapshot read ends its `=` line with `· cache 2h` ; the prototype did it on the outline only
        .map(|(i, l)| if i == 0 { media_count(l.trim_end_matches(" · cache")) } else { l })
        // the whole-page claim is held back on a page with text elsewhere (one honesty line)
        .map(|l| l.strip_prefix("(whole page shown) · ").map_or(l.clone(), String::from))
        // the media rows came after the prototype (media rows are new)
        .filter(|l| !is_media(l))
        .collect()
}

fn is_media(l: &str) -> bool {
    l.starts_with('m') && l[1..].starts_with(|c: char| c.is_ascii_digit()) || l.contains(" media → ")
}

/// A whole read's `=` figure is what it printed, media rows and count included: the prototype
/// had neither, so their tokens come off before the ±10% compare.
fn no_media_tok(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let (Some(head), Some(last)) = (lines.first(), lines.last()) else { return text.to_string() };
    if last.starts_with("read: ") {
        return text.to_string(); // an outline: the figure is the page's (what `full` prints)
    }
    let tok = fats::clean::tok;
    let media = lines.iter().filter(|l| is_media(l)).map(|l| tok(l)).sum::<usize>()
        + tok(head).saturating_sub(tok(&media_count(head)));
    let head: Vec<String> = head
        .split(' ')
        .map(|w| tok_of(w).map_or(w.to_string(), |n| format!("{}tok", (n as usize).saturating_sub(media))))
        .collect();
    std::iter::once(head.join(" ")).chain(lines[1..].iter().map(|l| l.to_string())).collect::<Vec<_>>().join("\n")
}

/// The `=` line without its media count (`· 3 photos`, `· 1 video`) and its pruned-numbers count (`· 95 of 424
/// numbers pruned`), which the prototype never printed.
fn media_count(l: &str) -> String {
    let groups = l.split(" · ").filter(|g| {
        let w: Vec<&str> = g.split(' ').collect();
        let pruned = w.len() == 5 && w[0].parse::<usize>().is_ok() && w[1..] == ["of", w[2], "numbers", "pruned"];
        !(pruned
            || w.len() == 2 && w[0].parse::<usize>().is_ok() && matches!(w[1], "photo" | "photos" | "video" | "videos"))
    });
    groups.collect::<Vec<_>>().join(" · ")
}

fn strip_job(w: &str) -> String {
    let b = w.as_bytes();
    if let Some(p) = w.find(".u")
        && let Some(j) = w[..p].rfind('j')
        && p > j + 1
        && b[j + 1..p].iter().all(u8::is_ascii_digit)
        && (j == 0 || !b[j - 1].is_ascii_alphanumeric())
    {
        return format!("{}{}", &w[..j], &w[p + 1..]);
    }
    w.to_string()
}

/// `1.2ktok` / `345tok` → tokens.
fn tok_of(w: &str) -> Option<f64> {
    let n = w.strip_suffix("tok")?;
    let (n, k) = n.strip_suffix('k').map_or((n, 1.0), |x| (x, 1000.0));
    n.parse::<f64>().ok().map(|x| x * k)
}

fn same_line(want: &str, got: &str) -> bool {
    if want == got {
        return true;
    }
    let (wants, gots): (Vec<&str>, Vec<&str>) = (want.split(' ').collect(), got.split(' ').collect());
    wants.len() == gots.len()
        && wants.iter().zip(&gots).all(|(p, q)| {
            p == q || matches!((tok_of(p), tok_of(q)), (Some(m), Some(n)) if (m - n).abs() <= 0.1 * m.max(n))
        })
}

fn seed(root: &Path, row: &Value) {
    let d = root.join("p/u1");
    std::fs::create_dir_all(&d).expect("seed dir");
    let src = Path::new(row["dir"].as_str().expect("oracle row dir"));
    std::os::unix::fs::symlink(src.join("raw.html"), d.join("raw.html")).expect("seed raw.html");
    let mut meta = row["meta"].clone();
    meta["final_url"] = row["final_url"].clone();
    // the prototype's rule: every Oxylabs call rendered (`run_js`), nothing else did
    meta["render"] = (meta["provider"] == "oxylabs").into();
    std::fs::write(d.join("meta.json"), meta.to_string()).expect("seed meta.json");
    let st = serde_json::json!({"u": [{"url": row["url"], "dir": "p/u1"}], "cursor": null});
    std::fs::write(root.join("p/state.json"), st.to_string()).expect("seed state.json");
}

fn fats(root: &Path, chunk: Option<&str>, cmd: &str) -> String {
    let mut c = Command::new(env!("CARGO_BIN_EXE_fats"));
    c.args(cmd.split(' ')).env("FATS_CACHE", root).env("FATS_SESSION", "p").env_remove("FATS_JSON");
    match chunk {
        Some(n) => c.env("FATS_CHUNK", n),
        None => c.env_remove("FATS_CHUNK"),
    };
    String::from_utf8_lossy(&c.output().expect("run fats").stdout).into_owned()
}

#[test]
fn parity_with_prototype() {
    let Ok(oracle) = std::env::var("FATS_PARITY") else {
        eprintln!("parity: skipped (FATS_PARITY=<oracle.jsonl> unset)");
        return;
    };
    let text = std::fs::read_to_string(&oracle).expect("oracle file");
    let tmp = std::env::temp_dir().join(format!("fats-parity-{}", std::process::id()));
    let mut count: BTreeMap<String, (usize, usize)> = BTreeMap::new(); // kind → (runs, diffs)
    let mut report = String::new();
    for (i, line) in text.lines().enumerate() {
        let row: Value = serde_json::from_str(line).expect("oracle row");
        for (mode, chunk) in [("port", Some("400")), ("doctrine", None)] {
            let root = tmp.join(format!("{i}-{mode}"));
            seed(&root, &row);
            for c in row["cmds"].as_array().unwrap() {
                let (kind, cmd) = (c["kind"].as_str().unwrap(), c["cmd"].as_str().unwrap());
                let run =
                    if mode == "port" && kind == "words" { format!("{cmd} match:words") } else { cmd.to_string() };
                let got = fats(&root, chunk, &run);
                let got = norm(&if kind == "outline" { no_media_tok(&got) } else { got });
                let want = norm(c["out"].as_str().unwrap());
                let e = count.entry(format!("{mode} {kind}")).or_default();
                e.0 += 1;
                let bad = got.len() != want.len() || got.iter().zip(&want).any(|(a, b)| !same_line(a, b));
                if bad {
                    e.1 += 1;
                    let at =
                        got.iter().zip(&want).position(|(a, b)| !same_line(a, b)).unwrap_or(got.len().min(want.len()));
                    report += &format!(
                        "## {mode} {} · {run}\n  line {at}\n  rust: {}\n  slce: {}\n",
                        row["dir"].as_str().unwrap_or("?"),
                        got.get(at).map_or("<none>", |s| s),
                        want.get(at).map_or("<none>", |s| s)
                    );
                }
            }
        }
    }
    std::fs::remove_dir_all(&tmp).ok();
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("parity-diffs.md");
    std::fs::write(&out, &report).unwrap();
    eprintln!("parity: {} pages · diffs per command (runs/diffs) · details {}", text.lines().count(), out.display());
    for (k, (n, d)) in &count {
        eprintln!("  {k:<18} {n:>4} runs {d:>4} diffs");
    }
    let port: usize = count.iter().filter(|(k, _)| k.starts_with("port")).map(|(_, v)| v.1).sum();
    assert_eq!(port, 0, "port diffs must be 0 (doctrine diffs are reported)");
}
