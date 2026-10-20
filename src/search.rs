//! FILTER inside a page: the match ladder's local rungs: exact phrase → all words (stemmed)
//! → best partial. `near`/`topic` need a model and are not in this build (a ! row says so).

use crate::cache::Cursor;
use crate::clean::{CARD, collapse, fold, tok};
use crate::lang::{Match, bin, cut};
use crate::outline::{Page, Shown, cap_rows, info_line, is_thin, lead, span};

fn terms_of(words: &[String]) -> Vec<String> {
    let trim = |w: &String| fold(w).trim_matches(|c| ".,;:!?\"'()".contains(c)).to_string();
    let keep = |f: &String| f.chars().count() >= 2 && !crate::lang::WORDS.stop.contains(&f.as_str());
    let t = crate::lang::uniq(words.iter().map(trim).filter(keep), String::clone);
    if t.is_empty() { words.iter().filter(|w| !w.is_empty()).map(|w| fold(w)).collect() } else { t }
}

fn stem(t: &str) -> &str {
    let n = t.chars().count();
    if n >= 6 { cut(t, (n - 2).max(4)) } else { t }
}

/// A ladder's answer: (candidate index, char position of the first hit) rows, the label, what was tried when
/// nothing hit.
type Rungs = (Vec<(usize, usize)>, String, &'static str);

/// Char index of `needle` in `hay`, like Python `str.find` on str.
fn cfind(hay: &str, needle: &str) -> Option<usize> {
    hay.find(needle).map(|b| hay[..b].chars().count())
}

/// `sec` = filter only section k and its subsections (`u3#2 <words>`); 0 = the info block.
/// `limit` = at most this many hit rows (`limit:`), inside the GREP_CAP token cap.
pub fn grep(
    p: &Page,
    words: &[String],
    (rung, pinned): (Match, bool),
    sec: Option<usize>,
    limit: Option<usize>,
) -> Result<Shown, String> {
    let (doc, b, pid) = (p.doc, bin(), &p.id);
    if let Some(k) = sec.filter(|&k| k > doc.sections.len()) {
        let has = if doc.sections.is_empty() { "no sections".into() } else { format!("#1-#{}", doc.sections.len()) };
        return Err(format!("? {pid}#{k} unknown · {pid} has {has} · outline: {b} {pid}"));
    }
    let scope = sec.map(|k| format!("#{k}")).unwrap_or_default();
    let (id, eq) = (&format!("{pid}{scope}"), format!("{}{scope}", p.eq));
    let q = words.join(" ");
    let cursor = Some(Cursor::default());
    if matches!(rung, Match::Near | Match::Topic) {
        let m = rung.name();
        let head = format!("= {eq} filter \"{q}\" · 0 hit · match:{m}");
        let row = format!("{id} ! not_in_v1 match:{m} needs a local model (later) → {b} {id} {q} match:words");
        return Ok((vec![head, row], cursor));
    }
    let cands = candidates(p, sec);
    let folded: Vec<String> = cands.iter().map(|(_, l, _)| fold(l)).collect();
    // islands are always searchable, after the served text: its hits first, else theirs (also when the served
    // rows are only the most words, k/n, and the islands hold all), said to be there; a thin page's text sits in its
    // islands (`outline::thin`): one search over both
    let thin = is_thin(doc); // once: a thin page's anchor reads the island parts
    let own = cands.iter().take_while(|c| c.0 <= doc.served() || thin).count();
    let (served, isl) = (ladder(&folded[..own], words, rung, pinned), ladder(&folded[own..], words, rung, pinned));
    let island = served.0.is_empty() || served.1.contains('/') && !isl.1.contains('/'); // k/n labels hold a `/`
    let (cands, (mut rows, label, tried)) = if island { (&cands[own..], isl) } else { (&cands[..own], served) };
    // one row a hit: a short line's row carries the next line (`candidates`), a hit past its own text is that
    // line's row; a title (a section's first row) hit is its lines' hit when they hit too (a thread row's title is
    // its first words)
    let first = |i: usize| i == 0 || cands[i - 1].0 != cands[i].0;
    let lined: std::collections::HashSet<usize> = rows.iter().filter(|r| !first(r.0)).map(|r| cands[r.0].0).collect();
    rows.retain(|&(i, pos)| pos < cands[i].2 && !(first(i) && lined.contains(&cands[i].0)));
    let label = label + if rows.iter().all(|r| cands[r.0].0 > doc.served()) { " · in islands" } else { "" };
    if rows.is_empty() {
        let line = format!("= {eq} filter \"{q}\" · 0 hit · tried {tried} · outline: {b} {pid} · all: {b} {pid} all");
        let media = words.iter().all(|w| crate::lang::WORDS.media.contains(&w.to_lowercase().as_str()));
        return Ok((std::iter::once(line).chain(media.then(|| format!("→ {b} {pid} media"))).collect(), cursor));
    }
    let hits: Vec<String> = rows.iter().map(|&(i, pos)| hit_row(pid, &cands[i], pos)).collect();
    let head = format!("= {eq} filter \"{q}\" · {} hit · {label}", rows.len());
    let (used, mut lines) = (tok(&head), vec![head]);
    let shown = cap_rows(&mut lines, &hits, used, (crate::settings::limit::<usize>("grep_cap"), 0, limit));
    if shown < rows.len() {
        lines.push(format!("… +{} hits · narrow: {b} {id} {q} <more words>", rows.len() - shown));
    }
    lines.push(format!("read: {b} {pid}#{}", cands[rows[0].0].0));
    Ok((lines, cursor))
}

/// THE list filter (`links`, `cards`, `stats`): every word is in the text, case and accents folded (`fold`).
pub fn holds(text: &str, words: &[String]) -> bool {
    words.iter().all(|w| fold(text).contains(&fold(w)))
}

/// The lines a filter searches, with their section: the info line (#0), then every title and line in scope. A
/// short line (a label) carries the next line as context: "Price Trend" → "318,67 €".
fn candidates(p: &Page, sec: Option<usize>) -> Vec<(usize, String, usize)> {
    let doc = p.doc;
    let mut raw: Vec<(usize, &str)> = Vec::new();
    let il = info_line(&doc.info);
    if !il.is_empty() && sec.is_none_or(|k| k == 0) {
        raw.push((0, &il));
    }
    let ks: Vec<usize> = match sec {
        Some(0) => vec![],
        Some(k) => span(doc, k),
        None => (1..=doc.sections.len()).collect(),
    };
    for k in ks {
        let s = &doc.sections[k - 1];
        raw.push((k, &s.title));
        raw.extend(s.lines.iter().map(|l| (k, l.as_str())));
    }
    let joined = |(i, &(k, ln)): (usize, &(usize, &str))| match raw.get(i + 1) {
        Some(&(k2, nx)) if ln.chars().count() < 60 && k2 == k => {
            (k, format!("{ln} → {}", cut(nx, 120)), fold(ln).chars().count())
        }
        _ => (k, ln.to_string(), usize::MAX),
    };
    raw.iter().enumerate().map(joined).collect()
}

/// THE match ladder over folded lines: exact phrase (pinned `match:exact`, or first when unpinned, the all-words
/// rows appended after it: an exact hit never hides an inflected one, `raided` → `raids`) → all words (stemmed) →
/// the most words, labelled `k/n` (never dropped: the one word that matched may be the rare, decisive one).
fn ladder(folded: &[String], words: &[String], rung: Match, pinned: bool) -> Rungs {
    let mut exact: Vec<(usize, usize)> = Vec::new();
    if rung == Match::Exact || !pinned {
        let phrase = collapse(&fold(&words.join(" ")));
        exact = folded.iter().enumerate().filter_map(|(i, f)| cfind(f, &phrase).map(|at| (i, at))).collect();
        if rung == Match::Exact {
            return (exact, "match:exact".into(), "exact");
        }
    }
    let terms = terms_of(words);
    let auto = if pinned { "" } else { " auto" };
    // (terms found, candidate index, first position)
    let score = |(i, f): (usize, &String)| {
        let pos: Vec<usize> = terms.iter().filter_map(|t| cfind(f, t).or_else(|| cfind(f, stem(t)))).collect();
        (!pos.is_empty()).then(|| (pos.len(), i, pos.into_iter().min().unwrap_or(0)))
    };
    let scored: Vec<(usize, usize, usize)> = folded.iter().enumerate().filter_map(score).collect();
    let all = |x: &&(usize, usize, usize)| x.0 == terms.len() && !exact.iter().any(|e| e.0 == x.1);
    let full: Vec<(usize, usize)> = scored.iter().filter(all).map(|x| (x.1, x.2)).collect();
    if !exact.is_empty() || !full.is_empty() {
        let ex = if exact.is_empty() { "" } else { "exact" };
        let label = if full.is_empty() { format!("match:{ex}") } else { format!("match:{ex}+all{auto}") };
        return ([exact, full].concat(), label.replace(":+", ":"), "");
    }
    let (best, n) = (scored.iter().map(|x| x.0).max().unwrap_or(0), terms.len());
    let rows = scored.iter().filter(|x| x.0 == best).map(|x| (x.1, x.2)).collect();
    let tried = if pinned { "all words, any word" } else { "exact, all words, any word" };
    (rows, format!("match:{best}/{n} words auto"), tried)
}

/// One hit row: up to 200 chars of the line from 70 before the hit, `…` where it was cut; a card row
/// (`clean::CARD`) starts with its `outline::lead` (title, link, price): the hit is the whole card, the window
/// after the lead (a teaser), never repeating it.
fn hit_row(pid: &str, (k, ln, _): &(usize, String, usize), pos: usize) -> String {
    let lead = if ln.starts_with(CARD) { lead(ln) + " " } else { String::new() };
    let start = pos.saturating_sub(70).max(lead.chars().count()); // the window goes on after the lead
    let snip: String = ln.chars().skip(start).take(200).collect();
    let head = if start > 0 { "…" } else { "" };
    let tail = if start + 200 < ln.chars().count() { "…" } else { "" };
    format!("{pid}#{k} {lead}{head}{}{tail}", crate::html::py_strip(&snip))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_and_stems() {
        let w = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(terms_of(&w("What is the PRICE, preis?")), ["price", "preis"]);
        assert_eq!(terms_of(&w("the a")), ["the", "a"]);
        assert_eq!(stem("pricing"), "prici");
        assert_eq!(stem("prices"), "pric");
        assert_eq!(stem("price"), "price");
        assert_eq!(cfind("gałązka drzewo", "drzewo"), Some(8));
    }
}
