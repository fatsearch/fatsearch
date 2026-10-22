//! OUTLINE + READ: lines → sections; outline-top, `all`, section parts, `full`, `links`.

use crate::cache::{Cursor, HopRow, Meta};
use crate::clean::{
    CARD, COMMENT, Doc, Info, LISTS, LREF, Line, PIECE, Section, collapse, fold, host_of, is_word, paragraph, tok,
    tok_cut,
};
use crate::fetch::{JS_QUOTE, KEY_ROWS, budget_row, clear, fail_row, move_code, no_key_left, skips, spent, title_head};
use crate::html::py_strip;
use crate::lang::{Flag, Id, WORDS, bin, cut, ksize, letters, priced, restamp, short, uniq};
use crate::provider::{Tier, domain_country};
use crate::settings::limit;
use std::cmp::Reverse;

/// Media rows: in the outline, in `fats uN media`.
const MEDIA_ROWS: usize = 3;
/// Every heading (≤ 120 chars) opens a section, the text before the first one = `(top)` (level 0, never a
/// parent); < 2 headings → `chunk`-token blocks `part N` (settings, default 300). Returns (sections, top level):
/// the shallowest heading level used ≥ 3 times, else the deepest one.
pub fn sections_of(lines: &[Line]) -> (Vec<Section>, usize) {
    let is_head = |l: &Line| matches!(l, Line::H(_, t) if t.chars().count() <= 120);
    let heads = lines.iter().filter(|l| is_head(l)).count() >= 2;
    let (cap, mut n) = (crate::settings::get().chunk.value, 0);
    let first = if heads { "(top)" } else { "part 1" };
    let mut secs = vec![Section { level: usize::from(!heads), title: first.into(), lines: vec![] }];
    for ln in lines {
        let (t, k) = (ln.text(), secs.len());
        match ln {
            Line::H(lv, h) if heads && is_head(ln) => {
                secs.push(Section { level: *lv, title: h.clone(), lines: vec![] })
            }
            _ if !heads && n + tok(t) > cap && !secs[k - 1].lines.is_empty() => {
                secs.push(Section { level: 1, title: format!("part {}", k + 1), lines: vec![t.to_string()] });
                n = tok(t);
            }
            _ => {
                secs[k - 1].lines.push(t.to_string());
                n += tok(t);
            }
        }
    }
    if secs[0].lines.is_empty() {
        secs.remove(0);
    }
    let levels: std::collections::BTreeSet<usize> = secs.iter().map(|s| s.level).filter(|&l| l > 0).collect();
    let count = |l: usize| secs.iter().filter(|s| s.level == l).count();
    let top = levels.iter().copied().find(|&l| count(l) >= 3).or(levels.last().copied()).unwrap_or(1);
    (secs, top)
}

/// Section k (1-based) + its subsections.
pub fn span(doc: &Doc, k: usize) -> Vec<usize> {
    let lv = doc.sections[k - 1].level;
    let subs = doc.sections[k..].iter().take_while(|s| lv > 0 && s.level > lv).count();
    (k..=k + subs).collect()
}

/// `, 3 sub, 30 cards` (`1 card`) after a row's size: its `sub` subsections and the card rows (`clean::CARD`) in
/// sections `js` (the row's own and those no other row shows: each card counted once), the counts that are not 0.
fn counts(doc: &Doc, js: &[usize], sub: usize) -> String {
    let cards = js.iter().flat_map(|&j| &doc.sections[j - 1].lines).filter(|l| l.starts_with(CARD)).count();
    let card = if cards == 1 { "card" } else { "cards" };
    [(sub, "sub"), (cards, card)].iter().filter(|x| x.0 > 0).map(|(n, w)| format!(", {n} {w}")).collect()
}

fn sec_tokens(doc: &Doc, k: usize) -> usize {
    span(doc, k).iter().map(|&j| tok(&doc.sections[j - 1].lines.join("\n")) + tok(&doc.sections[j - 1].title)).sum()
}

/// ≥ 60% of the section's own text sits on link-only lines (menus, related) or it is empty with no subs; never a
/// comment (a short one under its byline links is no menu).
fn is_link_list(doc: &Doc, k: usize) -> bool {
    let s = &doc.sections[k - 1];
    if s.lines.is_empty() || s.level >= COMMENT {
        return span(doc, k).len() == 1 && s.level < COMMENT;
    }
    let len = |t: &String| t.chars().count();
    let tot: usize = s.lines.iter().map(len).sum();
    let link: Vec<&String> = s.lines.iter().filter(|t| link_line(t)).collect();
    !link.is_empty() && link.iter().map(|t| len(t)).sum::<usize>() as f64 >= 0.6 * tot as f64
}

/// A line that is mostly links: ≤ 140 chars of text per `[lN]`, and no `prose_words` words past its last link
/// (a comment under its author and age links is prose).
fn link_line(t: &str) -> bool {
    if t.starts_with(CARD) {
        return false; // a card row (a price, specs) is content
    }
    let (n, rest) = (LREF.find_iter(t).count(), LREF.replace_all(t, ""));
    let tail = LREF.find_iter(t).last().map_or("", |m| &t[m.end()..]);
    let prose = tail.split_whitespace().filter(|w| w.chars().any(char::is_alphabetic)).count() >= limit("prose_words");
    !prose && n > 0 && py_strip(rest.trim_matches(|c| matches!(c, '-' | ' ' | '|'))).chars().count() <= 140 * n
}

/// The section to read first: the first heading that repeats the page title, else the first top-level section
/// holding ≥ 100 ~tok of prose (lines that are not link rows, its subsections included), else the biggest
/// top-level section; on a page with no headings (chunks of one flow: `part 1`…) the first (HN's story 1).
/// Never a folded link list (`lists`: nav, breadcrumbs, related).
fn read_first(doc: &Doc, lists: &[usize]) -> usize {
    let (secs, tw) = (&doc.sections, words3(&doc.title));
    let titled = |s: &Section| {
        let hw = words3(&s.title);
        let both = hw.iter().filter(|w| tw.contains(w)).count();
        !tw.is_empty() && s.title != "(top)" && both as f64 >= (0.5 * hw.len() as f64).max(1.0)
    };
    let prose = |k| {
        span(doc, k).iter().flat_map(|&j| &secs[j - 1].lines).filter(|l| !link_line(l)).map(|l| tok(l)).sum::<usize>()
    };
    let cands: Vec<usize> = (1..=doc.served()).filter(|k| !lists.contains(k)).collect();
    let tops = || cands.iter().copied().filter(|&k| secs[k - 1].level <= doc.top);
    (cands.iter().copied().find(|&k| titled(&secs[k - 1]) && sec_tokens(doc, k) >= 50))
        .or_else(|| tops().find(|&k| prose(k) >= 100))
        .or_else(|| tops().rev().max_by_key(|&k| sec_tokens(doc, k)).filter(|_| secs[0].title != "part 1"))
        .unwrap_or(1)
}

/// A row budget: (`cap` tokens, at least `min` rows, at most `limit` rows).
pub type Cap = (usize, usize, Option<usize>);

/// The first of `rows` onto `lines`: the ones that fit under a `cap`-token budget stacked on the `used` tokens
/// already on screen, always at least `min` of them and at most `limit`; returns how many. The one row-capping loop
/// behind every list (outline-top, `all`, links, media, filter hits, and the SERP/routes lists in cli/stats).
pub fn cap_rows(lines: &mut Vec<String>, rows: &[String], mut used: usize, (cap, min, limit): Cap) -> usize {
    // a row fits while the rows so far stay under the cap (the first `min` always), up to `limit`
    let shown = rows.iter().enumerate().take_while(|&(i, r)| {
        used += tok(r);
        (used <= cap || i < min) && limit.is_none_or(|n| i < n)
    });
    let shown = shown.count();
    lines.extend(rows[..shown].iter().cloned());
    shown
}

pub fn preview(text: &str, n: usize) -> String {
    let t = collapse(text);
    if t.chars().count() <= n {
        return t;
    }
    let c = cut(&t, n);
    let keep = c.rfind(' ').filter(|&sp| c[..sp].chars().count() as f64 > n as f64 * 0.6).map_or(c, |sp| &c[..sp]);
    format!("{keep}…")
}

/// Distinct words of ≥ 3 word chars, folded, in order of appearance.
fn words3(s: &str) -> Vec<String> {
    let f = fold(s);
    uniq(f.split(|c: char| !is_word(c)).filter(|w| w.chars().count() >= 3).map(String::from), String::clone)
}

/// A section's outline preview. Clean and hard renders pruned the chrome, so their first n chars are content.
/// A light render kept it and its sections open with menus: there the preview is the filter's densest window
/// with the page title standing in for the query an outline lacks (`title`, empty unless light): the title words
/// the section heading does not already show; the first n chars when they hold one (a lead), else the window
/// starting at one that holds the most distinct (ties → earliest); none → the first n chars.
fn sec_preview(s: &Section, title: &[String], n: usize) -> String {
    // A layout row (a table row with an empty cell and a link: HN's stories, its comments) shows its first
    // piece of > 3 words between cells, links and marks, else its wordiest (the first of a tie): the title or the
    // comment, never a rank or a byline (`user [l] 28 days ago [l] | next [–]`: ≤ 3 words, a number is none); a
    // comment's row needs no link (its byline's are the page's repeats, kept once)
    let piece = |c: &str| c.rsplit(']').next().unwrap_or_default().trim().to_string();
    let word = |w: &&str| w.contains(char::is_alphabetic) && !w.contains(|d: char| d.is_ascii_digit());
    let words = |c: &String| c.split_whitespace().filter(word).count().min(4);
    let wordiest = |l: &str| l.split(['|', '[']).map(piece).rev().max_by_key(words);
    let lay = |l: &String| l.starts_with('|') && l.contains("|  |") && (s.level >= COMMENT || LREF.is_match(l));
    let row = |l: &String| lay(l).then(|| wordiest(l)).flatten();
    // a row of ≥ 3 links with ≤ 3 words a link (a menu, tags, a byline: author · age · comments) never starts it
    let alnum =
        |l: &str| LREF.replace_all(l, "").split_whitespace().filter(|w| w.contains(char::is_alphanumeric)).count();
    let link_row = |l: &&String| LREF.find_iter(l).nth(2).is_some() && alnum(l) <= 3 * LREF.find_iter(l).count();
    // a comment starts at its text, past its byline (`clean::Render::byline`: one short line); a spacer row's
    // comment is its row (no `<time>`: its byline is a cell)
    let by = s.lines.first().is_some_and(|l| !paragraph(l) && !lay(l));
    let from = usize::from(s.level >= COMMENT && by && s.lines.len() > 1);
    let t = s.lines[from..].iter().filter(|l| !link_row(l)).map(|l| row(l).map_or_else(|| l.clone(), |c| c + " |"));
    let t = collapse(&t.collect::<Vec<_>>().join("\n"));
    let head = words3(&s.title);
    let title: Vec<&String> = title.iter().filter(|w| !head.contains(w)).collect();
    let toks: Vec<&str> = t.split(' ').collect();
    let bits =
        |f: String| title.iter().enumerate().filter(|(_, q)| f.contains(q.as_str())).fold(0, |m, (i, _)| m | 1 << i);
    let has: Vec<u64> = toks.iter().map(|w| bits(fold(w))).collect();
    // title words in the tokens a preview starting at token i shows whole: `preview` drops the token it cuts
    // (so a token must end before char n), and a start after 0 spends 1 char on `…`
    let held = |i: usize| {
        let (mut seen, mut len, mut j, room) = (0u64, 0, i, if i == 0 { n } else { n - 1 });
        while j < toks.len() && (j == i || len + toks[j].chars().count() < room) {
            seen |= has[j];
            len += toks[j].chars().count() + 1;
            j += 1;
        }
        seen.count_ones()
    };
    // the start holding the most (a strict `>` keeps the earliest of a tie); none when the lead holds one
    let pick = |b: (u32, usize), i| if held(i) > b.0 { (held(i), i) } else { b };
    let best = if held(0) > 0 { (0, 0) } else { (0..toks.len()).filter(|&i| has[i] != 0).fold((0, 0), pick) };
    if best.1 == 0 { preview(&t, n) } else { preview(&format!("…{}", toks[best.1..].join(" ")), n) }
}

/// Title words a light render's previews look for (≤ 64): `words3` minus the filter's stopwords.
fn title_query(doc: &Doc) -> Vec<String> {
    if doc.mode != "light" {
        return Vec::new();
    }
    words3(&doc.title).into_iter().filter(|w| !WORDS.stop.contains(&w.as_str())).take(64).collect()
}

pub fn info_line(info: &Info) -> String {
    let get = |k: &str| info.get(k).map(String::as_str).filter(|v| !v.is_empty());
    let mut parts: Vec<String> = get("outline").map(String::from).into_iter().collect();
    // the post's own counts after the page's facts (`clean::post`): `comments 296 · score 26 · posted 2025-12-11`
    let fields = ["author", "published", "modified", "price", "rating"].into_iter().chain(crate::clean::post_keys());
    parts.extend(fields.filter_map(|k| get(k).map(|v| format!("{k} {v}"))));
    if let Some(d) = get("description")
        && parts.len() < 3
    {
        parts.push(preview(d, 90));
    }
    parts.join(" · ")
}

/// A title cut to `n` chars at a word, marked `…`. When the cut-off words hold a number (a year, a model, an
/// issue), the first of them end the line: `Volkswagen Golf Automobile for sale in parts … 2000-02, | A28920205`.
pub fn title_cut(t: &str, n: usize) -> String {
    const TAIL: usize = 24;
    if t.chars().count() <= n {
        return t.to_string();
    }
    // the most words that fit in `room` chars, trailing punctuation dropped; and how many words that took
    let fit = |ws: &[&str], room: usize| {
        let (mut len, mut k) = (0, 0);
        while k < ws.len() && len + ws[k].chars().count() + usize::from(k > 0) <= room {
            len += ws[k].chars().count() + usize::from(k > 0);
            k += 1;
        }
        (k, ws[..k].join(" ").trim_end_matches(|c: char| !(c.is_alphanumeric() || c == '_')).to_string())
    };
    let ws: Vec<&str> = t.split_whitespace().collect();
    let (k, head) = fit(&ws, n - 1);
    let tail = ws[k..].iter().position(|w| w.bytes().any(|b| b.is_ascii_digit())).map(|j| fit(&ws[k + j..], TAIL).1);
    match tail.filter(|s| !s.is_empty()) {
        Some(tail) => format!("{} … {tail}", fit(&ws, n - 3 - tail.chars().count()).1).trim_start().to_string(),
        None if !head.is_empty() => format!("{head}…"),
        None => format!("{}…", cut(t, n - 1)),
    }
}

/// Host for web pages; file name for local documents. `landed <host>` when the page is off the asked site (bd's
/// Airbnb `.com` ask served from `.com.pe`, in Spanish): neither host (no `www.`, no root dot) is
/// the other or under it; `landed http://<host>` when an https ask was served over http (`provider::plain`).
pub fn place(asked: &str, url: &str) -> String {
    let [a, h] = [asked, url].map(|u| format!(".{}", host_of(u).trim_start_matches("www.").trim_end_matches('.')));
    match host_of(url) {
        f if f.is_empty() => url.rsplit(['/', '\\']).next().unwrap_or(url).to_string(),
        f if asked.starts_with("https:") && url.starts_with("http:") => format!("landed http://{f}"),
        f if a == "." || a.ends_with(&h) || h.ends_with(&a) => f,
        _ => format!("landed {}", &h[1..]),
    }
}

/// `42s`, `15m`, `3h`: whole units, the biggest that fits.
pub fn fmt_age(s: f64) -> String {
    let (div, unit) = [(3600.0, "h"), (60.0, "m")].into_iter().find(|(d, _)| s >= *d).unwrap_or((1.0, "s"));
    format!("{}{unit}", (s / div) as i64)
}

/// A page as the reader sees it: `id` = what hints print (`u3` or `j8.u3`), `eq` = the `=` line id.
pub struct Page<'a> {
    pub id: String,
    pub eq: String,
    pub doc: &'a Doc,
    pub meta: &'a Meta,
    /// Seconds since the fetch when the page is served from the cache; None = fetched by this command.
    pub age: Option<f64>,
    /// Where the read stands: the parts read, this one first (`#5b`, "" = the page), then the ones read before in this
    /// session (`Entry::read`): a pointer to one is no next move; and the URL's other stored copies
    /// (`cli::copies`): the newest paid ask's (`js:true`) copy that served other text, whether a paid ask
    /// brought back this same text; the URL's other copies (the timeouts its fix ran: `fetch::fix_arg`; the failure
    /// count).
    pub at: Seen,
}

/// `Page::at`: the parts read, the render, a paid same-text copy, the geo fix, the other copies' meta.
pub type Seen = (Vec<String>, Option<String>, bool, (Option<String>, bool), Vec<Meta>);

/// Part `part` of section `k` (its `lines`) was read in this session: under this copy's id, or its text
/// under any copy of the URL (`key_of`: the same page record was once offered per copy).
fn read(p: &Page, k: usize, part: usize, lines: &[String]) -> bool {
    [Id { sec: Some(k), part, ..Id::default() }.sec_tail(), key_of(lines)].iter().any(|r| p.at.0.contains(r))
}

/// The read log's key of part `part` of section `k` (`Entry::read`): `=` + 16 hex of its text.
pub fn part_key(doc: &Doc, k: usize, part: usize) -> Option<String> {
    doc.sections.get(k.checked_sub(1)?)?;
    parts_of(doc, k).get(part).map(|lines| key_of(lines))
}

fn key_of(lines: &[String]) -> String {
    format!("={}", &blake3::hash(lines.concat().as_bytes()).to_hex()[..16])
}

pub type Shown = (Vec<String>, Option<Cursor>);

/// The outline footer's moves: a thread names its view first (no cold reader guessed `comments` from a count alone;
/// `list:` = what it prints), then filter and links, and the cards list where it exists (the footer teaches it: cold
/// agents found it here, not in --help).
fn moves(doc: &Doc, id: &str) -> String {
    let b = bin();
    let talk = listed(doc, id, &said(doc)).map(|t| t + " · ").unwrap_or_default();
    let cards = (1..=doc.served()).any(|k| !counts(doc, &[k], 0).is_empty());
    let cards = if cards { format!(" · cards: {b} {id} cards") } else { String::new() };
    format!("{talk}filter: {b} {id} <words> · links: {b} {id} links{cards}")
}

/// Outline-top: a cached page says `cache 2h` where a fetch says `1.2s spent $.001`.
pub fn outline(p: &Page, prov: Option<&str>) -> Shown {
    let (doc, id, b) = (p.doc, &p.id, bin());
    let mut lines = vec![head_line(p, prov, doc.tokens.clean, None)];
    lines.extend(page_flags(p).iter().filter(|f| f.row).map(|f| f.bang(id)));
    // a failed run's skipped tiers (`fetch::skipped`: `no_key`, `budget_hit`), kept with it: every read says them,
    // a `no_key` row only while that key is still missing (`→ fats setup bd` under an arrow that buys bd),
    // a `budget_hit` row only while no newer copy served (the page's row points there), its arrow the reader's
    // (`lang::restamp`)
    let missing = no_key_left(p.meta);
    let kept = |r: &&str| !KEY_ROWS.iter().chain(&["! budget_hit "]).any(|k| r.starts_with(k)) || missing.contains(r);
    let rows = skips(p.meta).filter(kept).map(|r| restamp(r, id, &p.meta.url));
    let rows = rows.chain(budget_row(p.meta, id, &p.at.4).filter(|_| p.at.1.is_none()));
    lines.extend(rows.map(|r| format!("{id} {r}")));
    let il = Some(info_line(&doc.info)).filter(|il| !il.is_empty());
    lines.extend(il.map(|il| format!("{id}#0 info | {}", preview(&il, 150))));
    // the page plain's meta refresh came from (`provider::refresh`): shown, never judged
    lines.extend(p.meta.refreshed_from.as_ref().map(|f| format!("{id}#0 info | refreshed from: {f}")));
    // content media only (`Media::demoted` rows come last), unless the page has nothing else
    let content = doc.media.iter().filter(|m| !m.demoted).count();
    let rows = doc.media.len().min(if content == 0 { MEDIA_ROWS } else { content.min(MEDIA_ROWS) });
    let more = (doc.media.len() > rows).then(|| format!("… +{} media → {b} {id} media", doc.media.len() - rows));
    let media: Vec<String> = media_rows(doc, rows).chain(more).collect();
    // the list cap counts the `=` line as printed (with the flags `cli::page_cmd` adds), not the media count and rows
    let head = lines[0].replace(&photos(doc, p.meta), "") + &flags(p);
    let used = tok(&head) + lines[1..].iter().map(|x| tok(x)).sum::<usize>();
    lines.extend(media);
    if LISTS.text.contains(&doc.mode.as_str()) {
        return text_outline(p, lines);
    }
    let cursor = Some(Cursor { u: 0, ..Cursor::default() });
    // islands: openable sections after the page's own, whatever its size; one holding the page's own
    // words when they are served repeats the page (Discourse's posts again, escaped)
    let own = doc.islands > 0 && anchor(doc, 1, false).is_some_and(|a| a.sec <= Some(doc.served()));
    let islands = (doc.served() + 1..=doc.sections.len()).map(|k| {
        let size = ksize(sec_tokens(doc, k));
        let dup = if own && anchor(doc, k, true).is_some_and(|a| a.sec == Some(k)) { " (dup)" } else { "" };
        // an encoded island has no way in: its size, no arrow
        let (mark, to) =
            if encoded(doc, k) { (" (encoded)", String::new()) } else { (dup, format!(" → {b} {id}#{k}")) };
        format!("[{id}#{k}] {}{mark} · {size} tok{to}", doc.sections[k - 1].title)
    });
    let moves = moves(doc, id);
    if whole_read(doc) {
        lines.extend(whole(doc, id, &(1..=doc.served()).collect::<Vec<_>>()));
        lines.extend(islands);
        // never a claim of the whole page when its `=` line says some of it is elsewhere; a card says it is one
        let all = page_flags(p).is_empty() && doc.islands == 0 && pruned(doc).is_none();
        let claim = match (doc.mode == "og", all) {
            (true, _) => "(card only) · ",
            (_, true) => "(whole page shown) · ",
            _ => "",
        };
        lines.push(format!("{claim}{moves}"));
        lines[0] = head_line(p, prov, size(p, &lines), None);
        return (lines, cursor);
    }
    let tops: Vec<usize> = (1..=doc.served()).filter(|&k| doc.sections[k - 1].level <= doc.top).collect();
    // PDF pages carry no links here; an empty page is a no_text page, not a menu
    let lists: Vec<usize> = tops.iter().copied().filter(|&k| doc.mode != "pdf" && is_link_list(doc, k)).collect();
    // only link lists (SERP, index): the lists are the content
    let lists = if lists.len() == tops.len() { Vec::new() } else { lists };
    let rows = top_rows(doc, id, &lists);
    let shown = cap_rows(&mut lines, &rows, used, (limit::<usize>("list_cap"), 3, None));
    lines.extend((shown < rows.len()).then(|| format!("… +{} sections → {b} {id} all", rows.len() - shown)));
    lines.extend(islands);
    if !lists.is_empty() {
        lines.push(format!("{id} {} link-list sections folded (menus, related) → {b} {id} all", lists.len()));
    }
    let first = read_first(doc, &lists);
    lines.push(format!("read: {b} {id}#{first} · {moves} · all: {b} {id} all"));
    (lines, cursor)
}

/// An island no reader can use: its first part is an encoded blob (base64 or its URL-safe form: ≥ `blob_share`
/// of its chars, no space in it). Its row prints the size and no way in.
fn encoded(doc: &Doc, k: usize) -> bool {
    let text = parts_of(doc, k).into_iter().next().unwrap_or_default().concat();
    let coded = text.chars().filter(|c| c.is_ascii_alphanumeric() || "+/=-_".contains(*c)).count();
    let share: f64 = limit("blob_share");
    !text.is_empty() && !text.contains(char::is_whitespace) && coded as f64 >= share * text.chars().count() as f64
}

/// A page its outline prints whole: an HTML page within `read_cap` (a text document reads by `text_outline`).
fn whole_read(doc: &Doc) -> bool {
    !LISTS.text.contains(&doc.mode.as_str()) && doc.tokens.clean <= limit::<usize>("read_cap")
}

/// THE `=` figure of a page's outline (its `=` line, `fats stats`): read whole, what it prints (`lines`: the `=`
/// line with its flags, no `←` and the HOW at `HOW_WIDTH`, then the rows); else the page (what `full` prints).
pub fn size(p: &Page, lines: &[String]) -> usize {
    if !whole_read(p.doc) {
        return p.doc.tokens.clean;
    }
    let head = head_line(p, None, p.doc.tokens.clean, Some(HOW_WIDTH)) + &flags(p);
    tok(&head) + lines.iter().skip(1).map(|x| tok(x)).sum::<usize>()
}

/// The HOW a whole read's `=` figure counts in place of its own: `local 2ms`, `cache 2h`, `bd 4321ms spent $0.0015`
/// differ per read of one page, so it counts a plain fetch's width and a fetch and its cache read print one size.
const HOW_WIDTH: &str = "plain 400ms";

/// How a page this command fetched came: `plain 301ms`, `bd 4321ms spent $0.0015`; the path when it was more than
/// one plain call (a second ask says why: `md:thin`).
pub fn fetched(meta: &Meta) -> String {
    let more = meta.hops.len() > 1 || meta.hops.iter().any(|h| h.why.starts_with("md:"));
    let by = if more { meta.tried() } else { meta.provider.clone() + &meta.geo() };
    format!("{by} {}ms{}{}", meta.ms, crate::provider::spent(" ", meta.cost, meta.upto()), meta.how())
}

/// The `=` line: id, a failed fetch's code, title, shape (sections, PDF pages, text lines), `size` tok, and how it
/// came (a cache read too: a paid run is read from the cache); `how` = a HOW in place of this read's (`HOW_WIDTH`).
fn head_line(p: &Page, prov: Option<&str>, size: usize, how: Option<&str>) -> String {
    let (doc, meta) = (p.doc, p.meta);
    let tail = match (how, p.age) {
        (Some(how), _) => how.to_string(),
        (None, Some(age)) => format!("cache {}{}", fmt_age(age), meta.how()),
        (None, None) => fetched(meta),
    };
    let n = doc.served();
    let shape = match doc.mode.as_str() {
        // a PDF: one section per page, and what reading its text cost per page
        "pdf" => {
            let ms = doc.info.get("read_ms").and_then(|v| v.parse::<usize>().ok());
            let pages = if n == 1 { "page" } else { "pages" };
            format!("pdf · {n} {pages}{}", ms.map(|m| format!(" · read {}ms/page", m / n.max(1))).unwrap_or_default())
        }
        kind if LISTS.text.contains(&kind) => {
            let lines = crate::text::lines(doc);
            format!("{kind} {} KB {lines} {}", meta.bytes.div_ceil(1024), if lines == 1 { "line" } else { "lines" })
        }
        _ => {
            // the islands count beside the served sections: `3 sections +1 island`
            let isl = if doc.islands > 0 { format!(" +{} island", doc.islands) } else { String::new() };
            let (place, pruned) = (place(&doc.url, &doc.final_url), pruned(doc).unwrap_or_default());
            format!("{place} · {n} sections{isl}{}{pruned}", photos(doc, meta))
        }
    };
    // the page's flags go last, as on every read (`cli::page_cmd`)
    let (title, size) = (title_cut(&doc.title, 70), ksize(size));
    let from = prov.map(|x| format!(" ← {x}")).unwrap_or_default();
    // HOW names a page that did not come through our CONVERT: its og card, the server's own Markdown
    let how = if matches!(doc.mode.as_str(), "og" | "md") { format!("{} ", doc.mode) } else { String::new() };
    format!("= {} {title} · {shape} · {size}tok · {how}{tail}{from}", p.eq)
}

/// THE page flags (`page_flags`) as ONE `! …` group, the LAST `·` group of the `=` line of EVERY read of the page
/// (the `=` line: `= id title · place · COUNTS · tok · HOW · ! FLAGS`; put there by `cli::page_cmd`), "" when none:
/// `· ! js_shell · thin 0 of ~4.4k → fats u3#5ae`. A flag with its own row names only its code here; the `→`
/// belongs to the last code with a next step. `thin` or a geo copy (`lang:it · sent US`) with no way out is a
/// COUNT before it, never a lone `!`.
pub fn flags(p: &Page) -> String {
    let mut all = page_flags(p);
    let lone = |f: &mut Flag| (f.code == "thin" || f.code.starts_with("lang:")) && !f.row && f.next.is_empty();
    let count: String = all.extract_if(.., lone).map(|f| format!(" · {}", f.body())).collect();
    let arrow = all.iter().rposition(|f| !f.row && !f.next.is_empty());
    all.iter_mut().enumerate().filter(|(i, _)| Some(*i) != arrow).for_each(|(_, f)| f.next.clear());
    let codes: Vec<String> = all.iter().map(|f| if f.row { f.code.clone() } else { f.body() }).collect();
    if codes.is_empty() { count } else { format!("{count} · ! {}", codes.join(" · ")) }
}

/// `! redirected /listing` when the site served another CLASS of page than the one asked for: the asked path's last
/// segment (its id or slug, without `.html`) is gone from the final URL (its path, a query value, a host label: the
/// same id moved to `?v=` or to a user subdomain is the same page), and it held 8+ letters or digits, or the path's
/// all-digit segments, 2 or more, are its id and the landing keeps none of them (`/eli/reg/2021/821` →
/// `/TodayOJ/index.html`). A short segment with no such id is an alias resolved (`/releases/latest/` → the latest
/// post); a URL with no path segment names no page (`?q=` alone). A site adding a slug, a locale, a query or
/// another host to the same id is the same page; a wrong product slug landing on its listing, a login or a consent
/// page is not, and passes every content check. So is a path of the same depth with the same numeric ids: a slug
/// fixed beside its id (`/questions/26946646/<slug>`). Pure (two URLs in, the landing as shown out): THE rule for a
/// fetch's landing (`fetch::check`) and a served page's alike.
pub fn lost_page(asked: &str, landed: &str) -> Option<String> {
    let (a, b) = (url::Url::parse(asked).ok()?, url::Url::parse(landed).ok()?);
    let segs = |u: &url::Url| u.path().split('/').filter(|s| !s.is_empty()).map(String::from).collect::<Vec<_>>();
    let (sa, sb, num) = (segs(&a), segs(&b), |s: &&String| s.bytes().all(|c| c.is_ascii_digit()));
    let nums: Vec<&String> = sa.iter().filter(num).collect();
    let gone = nums.len() >= 2 && !nums.iter().any(|n| sb.contains(n));
    let last = sa.last().filter(|l| gone || l.bytes().filter(u8::is_ascii_alphanumeric).count() >= 8)?;
    let seg = last.rsplit_once('.').map_or(last.as_str(), |(stem, _)| stem).to_lowercase();
    let id = seg.trim_start_matches('@');
    let host = b.host_str().is_some_and(|h| h.split('.').any(|l| l == id));
    let kept = b.path().to_lowercase().contains(&seg) || b.query_pairs().any(|(_, v)| v.to_lowercase() == id) || host;
    let fixed = !nums.is_empty() && sa.len() == sb.len() && sa.iter().zip(&sb).all(|(x, y)| !num(&x) || x == y);
    if kept || fixed {
        return None;
    }
    let host = |u: &url::Url| u.host_str().unwrap_or_default().trim_start_matches("www.").to_string();
    let shown = if host(&a) == host(&b) { String::new() } else { host(&b) };
    let q = b.query().map(|q| format!("?{q}")).unwrap_or_default();
    Some(short(&format!("{shown}{}{q}", b.path().trim_end_matches('/')), 60))
}

/// `! thin`: the served text (its chars) is under `thin_share` of the text in the bytes (`loss.visible_chars`),
/// of at least `thin_min_chars` (measured on 114 pages).
/// Never on markdown (the page as its site serves it), nor when the page's own words (`anchor`) are served: the
/// islands holding them repeat the page (Discourse's posts again, escaped).
pub fn is_thin(doc: &Doc) -> bool {
    let seen = doc.loss.visible_chars;
    let served: usize = doc.sections[..doc.served()].iter().flat_map(|s| &s.lines).map(|l| l.chars().count()).sum();
    let (min, part): (usize, f64) = (limit("thin_min_chars"), limit("thin_share"));
    let share = seen >= min && (served as f64) < part * seen as f64 && doc.mode != "md";
    share && anchor(doc, 1, false).is_none_or(|a| a.sec > Some(doc.served()))
}

/// `! thin 290 of ~2.3k visible → fats u24#5ae`: the loss guard counts numbers, this counts text; cleaning served a
/// small part of what the bytes hold, so every read says so. The way out (`next`) is ANCHORED, never sized: the
/// part holding the page's own words (`anchor`), else a served `feed`, else `way_out`. On a social app's page the
/// biggest island is its app state; the one with the most prose, an i18n string table; its longest prose string, a
/// moderation policy or a legal notice.
fn thin(doc: &Doc, next: Option<String>) -> Option<Flag> {
    let doc = Some(doc).filter(|d| is_thin(d))?;
    // the visible text is a char count: ~2 chars a token (printed reads, median 1.97)
    let what = format!("{} of ~{} visible", ksize(doc.tokens.clean), ksize(doc.loss.visible_chars / 2));
    Some(Flag { code: "thin".into(), what, next: next.unwrap_or_default(), row: false })
}

/// A thin page's feed, when no part holds its own words (`held`) and no island holds a thread (`threads`: the
/// posts' own text wins over their titles): the served section holding the most link rows titled in ≥ 2 words (a
/// profile's videos by their captions), at ≥ `feed_links` of them and most of the served text (never a related list
/// beside the page), as its section. `thin` weighs the served text against the islands' strings too (an app's
/// state), so a feed of captions stays under its bar; the thin arrow points at the posts, the rule stays.
fn feed(p: &Page) -> Option<usize> {
    let doc = Some(p.doc).filter(|d| is_thin(d) && held(p).1.is_none() && threads(d).is_empty())?;
    let words =
        |l: &str| LREF.replace_all(l, "").split_whitespace().filter(|w| w.contains(char::is_alphabetic)).count();
    let titled = |l: &&String| LREF.is_match(l) && words(l) >= 2;
    let rows = |k: usize| doc.sections[k - 1].lines.iter().filter(titled).count();
    let most = |k: usize| 2 * tok(&doc.sections[k - 1].lines.join("\n")) >= doc.tokens.clean;
    let (n, k) = (1..=doc.served()).map(|k| (rows(k), k)).max()?;
    (n >= limit("feed_links") && most(k)).then_some(k)
}

/// The way to the page from a copy without it, free first: a render of it already stored (`→ fats u2
/// · rendered`), else the `<template>` island with the most paragraphs (texts of ≥ `paragraph_words` words) when it
/// holds ≥ `island_paragraphs` (a thread's comments: `→ fats u1#5 · 3.3k tok · $0`), at its first part not read in
/// this session (`#5` read → `#5b`, never back at a read part; all read → the next island, then none),
/// else a JS render (`js_next`). A data island is never one: its long strings are an app's i18n table.
fn way_out(p: &Page) -> Option<String> {
    let islands = threads(p.doc);
    let island = islands.iter().find_map(|&k| unread(p, k));
    // every part of the islands read: no arrow, never a render of what was read
    p.at.1.as_ref().map(|r| format!("{} {r}", bin())).or(island).or_else(|| js_next(p).filter(|_| islands.is_empty()))
}

/// The arrow to section `k` at its first part not read in this session (`#5` read → `#5b · 1.2k tok · $0`, the
/// rest's size), none when every part is read.
fn unread(p: &Page, k: usize) -> Option<String> {
    let parts = parts_of(p.doc, k);
    let i = (0..parts.len()).find(|&i| !read(p, k, i, &parts[i]))?;
    let n = if i == 0 { sec_tokens(p.doc, k) } else { parts[i..].iter().flatten().map(|l| tok(l)).sum() };
    let at = Id { sec: Some(k), part: i, ..Id::default() }.sec_tail();
    Some(format!("{} {}{at} · {} tok · $0", bin(), p.id, ksize(n)))
}

/// The `<template>` islands holding ≥ `island_paragraphs` paragraphs (texts of ≥ `paragraph_words` words: a
/// thread's comments), the most paragraphs first, the last of a tie first.
fn threads(doc: &Doc) -> Vec<usize> {
    let paras = |k: &usize| doc.sections[k - 1].lines.iter().filter(|l| paragraph(l)).count();
    let template = |k: &usize| doc.sections[k - 1].title.starts_with("island template");
    let mut islands: Vec<usize> = (doc.served() + 1..=doc.sections.len()).filter(template).collect();
    islands.retain(|k| paras(k) >= limit("island_paragraphs"));
    islands.sort_by_key(|&k| Reverse((paras(&k), k)));
    islands
}

/// The part holding the page, as an id tail: its own words (`anchor`, `#3fy`), else a thread island (`#5`):
/// the copy shows the page, and a run that served it taught its tier (`cli` `holds`). Never a missing page's
/// (`not_found`: a 404 echoes the URL's words, as in `fetch::check`).
pub fn shown(doc: &Doc, meta: &Meta) -> Option<String> {
    let page = anchor(doc, 1, false).map(|a| a.sec_tail()).or_else(|| threads(doc).first().map(|k| format!("#{k}")));
    page.filter(|_| meta.code != "not_found")
}

/// The anchor's length: the first chars of the page's own words an island part must hold.
const ANCHOR: usize = 25;

/// The id (section, part) of the first part of section `from` or after, served or island, with a line holding the
/// first `ANCHOR` folded chars of the page's description, else of its title when that is not the site's name, else
/// of its first content picture's alt (an app shell's card: TikTok's caption); each only with ≥ 2 of the
/// page's own words (`fetch::words_of`, the title up to its site suffix: a wall's `Access to this page has been
/// denied` has 1); None = no such words (shorter ones match anywhere) or no part holds them. `any` =
/// any island part holding them (a repeat, `(dup)`), else only one with a line of words (a way in).
fn anchor(doc: &Doc, from: usize, any: bool) -> Option<Id> {
    let info = |k: &str| doc.info.get(k).map(|v| fold(v));
    let own = |t: &str| crate::fetch::words_of(t).len() >= 2;
    let head = own(title_head(&doc.title));
    let title = Some(fold(&doc.title)).filter(|t| head && info("site").as_ref() != Some(t));
    let alt = doc.media.iter().find(|m| !m.demoted).map(|m| fold(&m.alt));
    // a description too short to anchor falls back to the title
    let mut texts = [info("description"), title.clone(), alt].into_iter().flatten().filter(|t| own(t));
    let text = texts.find(|w| w.chars().count() >= ANCHOR)?;
    let words: String = text.chars().take(ANCHOR).collect();
    // The title's words count in a line of `paragraph_words` words only, never in a name or a heading alone
    // (Reddit's title over a thread whose comments sit in an island)
    let line = |l: &String| fold(l).contains(&words) && (any || title.as_ref() != Some(&text) || paragraph(l));
    let holds = |part: &Vec<String>| part.iter().any(line);
    // An island part holding them beside image links only (no line of `paragraph_words` words) yields to a
    // later one holding a line of words too (Airbnb's listing name: its description); else it stays (a short post)
    let prose = |k: usize, p: &[String]| k <= doc.served() || p.iter().any(|l| paragraph(l));
    // Then its section's part holding the longest text of ≥ `prose_run` words with ≥ 2 of their own words (the
    // first of a tie; never a policy or a legal notice): Airbnb's name sits beside a 26-word teaser, its
    // 139-word listing text 36 parts earlier
    let mine = |l: &str| crate::fetch::words_of(&words).iter().filter(|w| fold(l).contains(w.as_str())).count() >= 2;
    let run = |p: &Vec<String>| p.iter().map(|l| l.split_whitespace().count() * usize::from(mine(l))).max();
    let part = |k| parts_of(doc, k).iter().position(|p| holds(p) && (any || prose(k, p)));
    let a = (from..=doc.sections.len()).find_map(|k| part(k).map(|part| Id { sec: Some(k), part, ..Id::default() }))?;
    // An island anchor's from its section on (a bd copy: the name and a teaser in #4, the text in #5)
    let end = if any || a.sec <= Some(doc.served()) { a.sec? } else { doc.sections.len() };
    let runs = |k| parts_of(doc, k).into_iter().enumerate().map(move |(i, p)| (run(&p), Reverse(k), Reverse(i)));
    let long = (a.sec?..=end).flat_map(runs).max().filter(|&(n, ..)| n >= Some(limit("prose_run")));
    Some(long.map_or(a, |(_, k, i)| Id { sec: Some(k.0), part: i.0, ..a }))
}

/// A URL id's length: the fewest chars of a path segment that may name the page's record (`record`).
const RECORD_ID: usize = 8;

/// The page record, when no words anchor the page: in the first island with a line whose value is the URL's
/// id (its last path segment of ≥ `RECORD_ID` letters and digits, an extension cut, else its longest segment of as
/// many; a URL holding the id is a link, not the record), the part holding the wordiest line at that line's depth
/// around it (a tie: the first upward, then down): the record's own text (`"desc": "Did you know…"` beside
/// `"id": "7399…"` in an app shell's state). (its id, its tokens); None = no such segment or line.
fn record(doc: &Doc) -> Option<(Id, usize)> {
    let u = url::Url::parse(&doc.url).ok()?;
    let segs: Vec<&str> = u.path_segments()?.filter(|s| s.len() >= RECORD_ID).collect();
    let last = segs.last().map(|s| s.rsplit_once('.').map_or(*s, |(stem, _)| stem));
    let last = last.filter(|s| s.len() >= RECORD_ID && s.bytes().all(|c| c.is_ascii_alphanumeric()));
    let id = last.or_else(|| segs.iter().max_by_key(|s| s.len()).copied())?;
    let value = |l: &str| l.rsplit(':').next().is_some_and(|v| v.trim_matches([' ', '"', ',']) == id);
    (doc.served() + 1..=doc.sections.len()).find_map(|k| {
        let parts = parts_of(doc, k);
        let lines: Vec<(usize, &str)> =
            parts.iter().enumerate().flat_map(|(i, p)| p.iter().map(move |l| (i, l.as_str()))).collect();
        let at = lines.iter().position(|(_, l)| value(l))?;
        // the record = the lines around it at its depth or deeper; its siblings = those at its depth
        let indent = |l: &str| l.split_once(' ').map_or(0, |(_, r)| r.len() - r.trim_start().len()); // after `l42`
        let depth = indent(lines[at].1);
        let inside = |l: &&(usize, &str)| indent(l.1) >= depth;
        let around = lines[..at].iter().rev().take_while(inside).chain(lines[at..].iter().take_while(inside));
        let (part, _) =
            around.filter(|l| indent(l.1) == depth).min_by_key(|l| Reverse(l.1.split_whitespace().count()))?;
        let (text, n) = (parts[*part].iter().any(|l| paragraph(l)), parts[*part].iter().map(|l| tok(l)));
        Some(text.then_some((Id { sec: Some(k), part: *part, ..Id::default() }, n.sum())))
    })?
}

/// The way into a copy's page record (`record`) when no words anchor it, as an id tail: `#2fv · page record · 995
/// tok · $0`; None once the read log `log` holds its text (`key_of`, under any copy) or on a missing page.
pub fn record_in(doc: &Doc, meta: &Meta, log: &[String]) -> Option<String> {
    let (at, n) = record(doc).filter(|_| anchor(doc, 1, false).is_none() && meta.code != "not_found")?;
    let key = key_of(parts_of(doc, at.sec?).get(at.part)?);
    (!log.contains(&key)).then(|| format!("{} · page record · {} tok · $0", at.sec_tail(), ksize(n)))
}

/// `fats u1 js:true · oxyjs ≤$0.0015 ≈25s`: a JS render, the tiers its `dry:true` would call (`bd, then oxyjs`),
/// its worst case (`fetch::worst`) and a render's time (`fetch::JS_QUOTE`); None for a file (nothing to render),
/// when none of those tiers is left untried (`fetch::spent` with the URL's other copies: nor one another copy
/// called, cut or not: a cut's one move is the longer call on that copy's row, `fetch::fix_arg`), when one is
/// settled for the URL (`fetch::settled`: the same ask is refused again), or a paid ask brought this copy
/// back. An app page (`no_copy`) gets it too: every `!` has a next step, the agent decides on the price.
fn js_next(p: &Page) -> Option<String> {
    let cuts: Vec<HopRow> = p.at.4.iter().flat_map(|o| o.hops.clone()).collect();
    let done = spent(p.meta, &cuts, &crate::fetch::keyed());
    let cmd = format!("{} {} js:true", bin(), p.id);
    let (tiers, c) = crate::fetch::worst(&cmd, &p.meta.url).filter(|_| p.meta.url.starts_with("http") && !p.at.2)?;
    let names: Vec<&str> = tiers.iter().map(|t| t.0.spec().short).collect();
    let open = names.iter().any(|n| !done.contains(n)) && clear(&cmd, p.meta, &p.at.4);
    open.then(|| format!("{cmd} · {} ≤${c:.4} {JS_QUOTE}", names.join(", then ")))
}

/// ` · 56 of 224 numbers pruned` (a COUNT group of the `=` line): numbers the light render holds that the served one
/// does not, when ≥ `pruned_share` of them and ≥ `pruned_min` (fewer = TOC numbering, related-item ratings).
fn pruned(doc: &Doc) -> Option<String> {
    let (n, all) = (doc.loss.vs_light_lost, doc.loss.light_facts);
    let (min, part): (usize, f64) = (limit("pruned_min"), limit("pruned_share"));
    (n >= min && n as f64 >= part * all as f64).then(|| format!(" · {n} of {all} numbers pruned"))
}

/// Whether the page's own words are shown (`anchor`), and the free way to them: their part, never one read;
/// else, when no stored copy shows the page, its record: never "nothing left" nor a browser while unread. A
/// missing page shows none. A way out only where a flag prints it, one next move an answer: a failed copy's
/// row takes it (never `js:true` beside "nothing left").
fn held(p: &Page) -> (bool, Option<String>) {
    let (doc, id, b, gone) = (p.doc, &p.id, bin(), p.meta.code == "not_found");
    let anchored = anchor(doc, 1, false).filter(|_| !gone);
    // its part unread, priced like every free arrow
    let unread = |k, i| parts_of(doc, k).get(i).filter(|l| !read(p, k, i, l)).map(|l| ksize(tok(&l.concat())));
    let held = anchored.and_then(|a| Some(format!("{b} {id}{} · {} tok · $0", a.sec_tail(), unread(a.sec?, a.part)?)));
    let rec = record_in(doc, p.meta, &p.at.0).map(|r| format!("{b} {id}{r}"));
    (anchored.is_some(), held.or(rec.filter(|_| p.at.1.is_none())))
}

/// Every `!` of a page, in print order: a failed fetch's code (`thin_content`, `pdf_truncated`, … its best copy
/// shown), `thin`, `redirected`, a parse cut short, PDF pages with no text layer; the first and the last two also get
/// their own `{id} ! …` row under the outline's `=` line.
fn page_flags(p: &Page) -> Vec<Flag> {
    let (doc, meta, id, b) = (p.doc, p.meta, &p.id, bin());
    let n = doc.sections.len();
    let flag = |code: &str, what: String, next: String| Flag { code: code.into(), what, next, row: true };
    let (anchored, held) = held(p);
    let (mut v, mut way) = (Vec::new(), (!meta.ok || doc.mode == "og" || is_thin(doc)).then(|| way_out(p)).flatten());
    if !meta.ok {
        // a cut PDF says how much of it was read
        let read =
            (doc.mode == "pdf").then(|| format!("{} of {n} pages read · ", n - crate::pdf::empty_pages(doc).len()));
        let (url, tried) = (short(&doc.final_url, 50), meta.tried());
        let (why, again, fix, _) = fail_row(meta, &p.at.4, "", id, p.at.1.is_some());
        // a copy holding the page's own words says so and points at them, never "nothing left"
        let (shown, copy) = if anchored { (" · page content shown", "") } else { ("", " · showing the best copy") };
        // why the run failed, as on a failed copy without a doc (`bd cut at 40 s (3m ago)`); the failure count, never
        // beside a newer copy that served (its row points there)
        let why = if why.is_empty() { why } else { format!(" · {why}") };
        // an app page's way out, or a stored copy of the page (never "nothing left" with the page in the cache)
        let next = held.clone().or(way.take().filter(|_| move_code(meta) == "no_copy" || p.at.1.is_some()));
        let again = if next.is_some() && fix.is_some() { "" } else { &again };
        let what = format!("{}{url} · tried {tried}{copy}{why}{again}", read.unwrap_or_default());
        v.push(flag(&format!("{}{shown}", meta.code), what, next.or(fix).unwrap_or_default()));
    }
    // a card is all the site serves without JavaScript (`fetch::og_card`): never thin against its shell; `! js_shell`
    // only while a way out is left
    let card = way.clone().map(|next| Flag { code: "js_shell".into(), next, ..Flag::default() });
    let landed = lost_page(&doc.url, meta.landed.as_deref().unwrap_or(&doc.final_url));
    let moved = landed.map(|what| Flag { code: "redirected".into(), what, ..Flag::default() });
    // a feed's section is the way to the posts before an island guess (`feed`)
    let feed = feed(p).and_then(|k| unread(p, k));
    v.extend(if doc.mode == "og" { card } else { thin(doc, held.or(feed).or(way)) }.into_iter().chain(moved));
    if doc.cut > 0 && doc.mode == "pdf" {
        let (all, secs) = (n + doc.cut, limit::<u64>("pdf_budget_s"));
        let next = format!("the rest: qpdf <file>.pdf --pages . {}-z -- rest.pdf, then {b} ./rest.pdf", n + 1);
        v.push(flag("parse_cut", format!("pages {}-{all} of {all} not read ({secs}s budget)", n + 1), next));
    } else if doc.cut > 0 {
        let (cut, bytes, secs) = (ksize(doc.cut), ksize(meta.bytes as usize), limit::<u64>("parse_budget_s"));
        let what = format!("last {cut}B of {bytes}B not parsed ({secs}s budget) · this outline is the start");
        v.push(flag("parse_cut", what, format!("{b} {id} all")));
    }
    v.extend(geo_flags(p));
    // the empty pages of a cut PDF are the cut, said by its `pdf_truncated` row, not a missing text layer
    let empty =
        if doc.mode == "pdf" && meta.code != "pdf_truncated" { crate::pdf::empty_pages(doc) } else { Vec::new() };
    if !empty.is_empty() {
        let list = empty.iter().take(10).map(|k| k.to_string()).collect::<Vec<_>>().join(" ");
        let more = if empty.len() > 10 { format!(" +{}", empty.len() - 10) } else { String::new() };
        let next = match empty.len() == n {
            true => format!("OCR it (e.g. ocrmypdf in.pdf out.pdf), then {b} ./out.pdf"),
            false => format!("{b} {id} all"),
        };
        let pages = format!("{} of {n} pages have no text layer (pages {list}{more})", empty.len());
        v.push(flag("no_text", pages + " · no OCR in this build", next));
    }
    v
}

/// A provider's geo copy (`fetch::geo`) on every read, a cache read too, with the country its call sent. Its next
/// move: a newer copy of the URL asked another way (`cli::copies`: the fix ran) → `fats u2`, $0; else, unless
/// the user named that country (`served in en (asked DE)`: no blame, nothing to buy), `geo_fix`; else a COUNT.
fn geo_flags(p: &Page) -> Vec<Flag> {
    let (meta, b) = (p.meta, bin());
    let sent = if meta.country.is_empty() { "none" } else { meta.country.as_str() };
    let geo = meta.hops.iter().filter(|h| h.code == "ok");
    let geo = geo.filter_map(|h| Some((h.why.split_once("lang:")?.1, Tier::parse(&h.provider)?)));
    let flag = |(lang, tier): (&str, Tier)| {
        let asked = p.at.3.1 && !meta.country.is_empty();
        let fix = geo_fix(&meta.url, &meta.country, tier, &crate::fetch::keyed()).filter(|_| !asked);
        let fix = fix.map(|f| priced(&format!("{b} {} {f}", p.id), &meta.url)).filter(|a| clear(a, meta, &p.at.4));
        let next = p.at.3.0.clone().map(|n| format!("{b} {n}")).or(fix);
        let what = match (asked, next.is_some()) {
            (true, _) => format!("· served in {lang} (asked {sent})"),
            (false, true) => format!("· served in {lang}, sent {sent} · the provider's geo copy"),
            (false, false) => format!("· sent {sent}"),
        };
        Flag { code: format!("lang:{lang}"), what, row: next.is_some(), next: next.unwrap_or_default() }
    };
    geo.map(flag).collect()
}

/// The fix of a provider's geo copy: one that changes the request, since the same request brings the same copy
/// : the domain's country, else `FATS_COUNTRY` (`us`), when another (or none) was sent; else the other
/// provider when `keys` hold it; else none (the copy is what the providers give: a COUNT, `flags`).
fn geo_fix(url: &str, sent: &str, served: Tier, keys: &[Tier]) -> Option<String> {
    let cc = domain_country(url).unwrap_or_else(crate::provider::home);
    if cc != sent {
        return Some(format!("country:{} fresh:true", cc.to_lowercase()));
    }
    let other = keys.iter().map(|t| t.owner()).find(|t| *t != Tier::Plain && *t != served.owner())?;
    Some(format!("provider:{} fresh:true", other.spec().short))
}

/// THE media count (a COUNT of the `=` line) as ` · N photos`: `0 photos` too, since an agent asked for the photo
/// would invent one; videos only when there are any; content only (`Media::demoted` rows are listed, not counted).
/// Measured only: a failed answer (a js shell, a wall) or an empty one with no content picture prints no count (its
/// `0` would be the shell's, not the page's); one that lists some (a shell's own og card) counts them. Every
/// outline, `all` and `media` `=` line, and `stats`.
pub fn photos(doc: &Doc, meta: &Meta) -> String {
    let (v, all) = doc.media.iter().filter(|m| !m.demoted).fold((0, 0), |(v, n), m| (v + usize::from(m.video), n + 1));
    if all == 0 && (!meta.ok || doc.tokens.clean == 0) {
        return String::new();
    }
    let n = |n: usize, w: &str| format!(" · {n} {w}{}", if n == 1 { "" } else { "s" });
    format!("{}{}", n(all - v, "photo"), if v == 0 { String::new() } else { n(v, "video") })
}

/// `m1 photo 5955x3969 webp | <alt> → <url>` for the first `k` media (`clean::rank` order); a demoted row says
/// `(not counted)`, so a wrong demotion is visible.
fn media_rows(doc: &Doc, k: usize) -> impl Iterator<Item = String> + '_ {
    doc.media.iter().take(k).enumerate().map(|(i, m)| {
        let size = if m.w.min(m.h) > 0 { format!("{}x{}", m.w, m.h) } else { String::new() };
        let not = if m.demoted { "(not counted)" } else { "" };
        let kind = [if m.video { "video" } else { "photo" }, &size, &m.ty, not].into_iter().filter(|s| !s.is_empty());
        let alt = if m.alt.is_empty() { "(no alt)".into() } else { preview(&m.alt, 200) };
        format!("m{} {} | {alt} → {}", i + 1, kind.collect::<Vec<_>>().join(" "), m.src)
    })
}

/// Claude's (long-edge px, visual tokens) limits per resolution tier: a picture over either is downscaled
/// before it is seen. Standard = most models (and what `fats mcp` sends); high-resolution = Claude 4.7 and later.
pub const STD: (u32, u32) = (1568, 1568);
pub const HI_RES: (u32, u32) = (2576, 4784);

/// A picture as Claude views it on a `tier` (`STD`, `HI_RES`): (size, visual tokens). The rule and its reference
/// code: platform.claude.com docs, vision-coordinates "How Claude resizes and pads images": one token per 28x28
/// patch; a picture whose padded sides pass the edge or whose patches pass the token limit shrinks, aspect kept,
/// to the largest long edge that fits (the reference's binary search; fitting only gets harder as the edge grows).
pub fn viewed(w: u32, h: u32, (edge, most): (u32, u32)) -> ((u32, u32), u32) {
    let patches = |(w, h): (u32, u32)| w.div_ceil(28) * h.div_ceil(28);
    let fits = |s: (u32, u32)| s.0.div_ceil(28) * 28 <= edge && s.1.div_ceil(28) * 28 <= edge && patches(s) <= most;
    let aspect = f64::from(w.max(h)) / f64::from(w.min(h).max(1));
    let at = |l: u32| (l, ((f64::from(l) / aspect).round_ties_even() as u32).max(1));
    let at = |l: u32| if w >= h { at(l) } else { (at(l).1, l) };
    let size = if fits((w, h)) { (w, h) } else { (1..w.max(h)).rev().map(at).find(|s| fits(*s)).unwrap_or((1, 1)) };
    (size, patches(size))
}

/// `fats uN media`: every picture and video of the page, a `read_cap` page at a time from `off` (`media from:N`).
pub fn media(p: &Page, off: usize, limit: Option<usize>) -> Shown {
    let rows: Vec<String> = media_rows(p.doc, usize::MAX).collect();
    // `pixels` only when there is an `m1` to look at (docs-2)
    let px = if rows.is_empty() { String::new() } else { format!(" · pixels {} {} m1", bin(), p.id) };
    let head = format!("= {} media{} · up {} {}{px}", p.eq, photos(p.doc, p.meta), bin(), p.id);
    paged(&p.id, head, &rows, (off, limit, crate::settings::limit::<usize>("read_cap"), "media"))
}

/// A text document as it is: its first lines up to the read cap; `more` goes on.
fn text_outline(p: &Page, mut lines: Vec<String>) -> Shown {
    let (doc, id, b) = (p.doc, &p.id, bin());
    let first = doc.sections.first().map_or(&[][..], |s| &s.lines);
    let parts = chunks_of(first);
    lines.extend(parts[0].iter().cloned());
    let rest = crate::text::lines(doc) - first.len();
    let filter = format!("filter: {b} {id} <words>");
    lines.push(match (parts.len(), rest) {
        (1, 0) => format!("(whole document shown) · {filter}"),
        (1, _) => format!("… +{rest} lines → {b} {id}#2 · {filter}"),
        _ => more(&parts, 0, &format!("{b} more · {filter}")).unwrap_or_default(),
    });
    (lines, Some(Cursor { k: Some(1), ..Cursor::default() }))
}

/// One outline row per top-level section that is not a folded link list: `u3#2 Title (1.2k, 3 sub) | preview`.
fn top_rows(doc: &Doc, id: &str, lists: &[usize]) -> Vec<String> {
    let (secs, tq) = (&doc.sections, title_query(doc));
    let tops = secs[..doc.served()].iter().enumerate().map(|(i, s)| (i + 1, s));
    let row = |(k, s): (usize, &Section)| {
        let sp = span(doc, k);
        let sub = sp.len() - 1;
        // the page's top part (text before its first heading, level 0) whole up to `top_part_tok`: a state banner
        // there ("listing expired", 96 tok on a live listing) is never cut from the first answer
        let n = if s.level == 0 && sec_tokens(doc, k) <= limit("top_part_tok") { usize::MAX } else { 80 };
        let mut pv = sec_preview(s, &tq, n);
        // a heading over its parts: their titles, else (comments: untitled) the first one's preview
        let titles: Vec<&str> = sp[1..sp.len().min(4)].iter().map(|&j| secs[j - 1].title.as_str()).collect();
        if pv.is_empty() && sub > 0 {
            pv = format!("→ {}", if titles[0].is_empty() { sec_preview(&secs[k], &tq, n) } else { titles.join(" · ") });
        }
        let own: Vec<usize> = sp.iter().copied().filter(|&j| j == k || secs[j - 1].level > doc.top).collect();
        let size = ksize(sec_tokens(doc, k)) + &counts(doc, &own, sub);
        format!("{} ({size}) | {pv}", format!("{id}#{k} {}", cut(&s.title, 60)).trim_end())
    };
    tops.filter(|(k, s)| s.level <= doc.top && !lists.contains(k)).map(row).collect()
}

pub fn outline_all(p: &Page) -> Shown {
    let (doc, id, b) = (p.doc, &p.id, bin());
    let (n, tok_n, tq) = (doc.sections.len(), ksize(doc.tokens.clean), title_query(doc));
    let mut lines = vec![format!("= {} all {n} sections{} · {tok_n}tok", p.eq, photos(doc, p.meta))];
    let used0 = tok(&lines[0]) + tok(&flags(p));
    // its size = what `uN#0` prints (`read_sec`): one `key: value` line each
    let info = (!doc.info.is_empty()).then(|| ksize(doc.info.iter().map(|(a, v)| tok(&format!("{a}: {v}"))).sum()));
    lines.extend(info.map(|n| format!("{id}#0 info ({n})")));
    let row = |(i, s): (usize, &Section)| {
        let (pad, size) = ("  ".repeat(s.level.saturating_sub(doc.top)), ksize(sec_tokens(doc, i + 1)));
        let head = format!("{id}#{} {}", i + 1, cut(&s.title, 60)); // a comment: untitled
        format!("{pad}{} ({size}) | {}", head.trim_end(), sec_preview(s, &tq, 50))
    };
    let rows: Vec<String> = doc.sections.iter().enumerate().map(row).collect();
    let shown = cap_rows(&mut lines, &rows, used0, (limit::<usize>("read_cap"), 0, None));
    if shown < doc.sections.len() {
        lines.push(format!("… +{} sections · read one: {b} {id}#{}", doc.sections.len() - shown, shown + 1));
    }
    (lines, Some(Cursor::default()))
}

pub fn chunks_of(text: &[String]) -> Vec<Vec<String>> {
    let (mut parts, mut cur, mut n) = (Vec::new(), Vec::new(), 0);
    for ln in text {
        let mut ln = ln.as_str();
        // one huge line: parts of 95% of the cap, each cut found in one pass (a 660k-char JSON island line)
        while tok_cut(ln, limit::<usize>("read_cap")) < ln.len() {
            let h = ln[..tok_cut(ln, limit::<usize>("read_cap") - 50)].to_string();
            ln = &ln[h.len()..];
            parts.extend((!cur.is_empty()).then(|| std::mem::take(&mut cur)));
            parts.push(vec![h]);
            n = 0;
        }
        if n + tok(ln) > limit::<usize>("read_cap") && !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
            n = 0;
        }
        n += tok(ln);
        cur.push(ln.to_string());
    }
    // the last part; a document with no text is one empty part
    if !cur.is_empty() || parts.is_empty() {
        parts.push(cur);
    }
    parts
}

pub fn read_sec(p: &Page, k: usize, part: usize) -> Result<Shown, String> {
    let (doc, secs, id, b) = (p.doc, &p.doc.sections, &p.id, bin());
    if k == 0 {
        let mut lines = vec![format!("= {}#0 info · up {b} {id}", p.eq)];
        lines.extend(doc.info.iter().map(|(a, v)| format!("{a}: {v}")));
        return Ok((lines, Some(Cursor::default())));
    }
    if k > secs.len() {
        let has = if secs.is_empty() { "no sections".into() } else { format!("#1-#{}", secs.len()) };
        return Err(format!("? {id}#{k} unknown · {id} has {has} · outline: {b} {id}"));
    }
    let (sp, parts) = (span(doc, k), parts_of(doc, k));
    if part >= parts.len() {
        return Ok((vec![format!("= {}#{k} complete · {} parts · up {b} {id}", p.eq, parts.len())], None));
    }
    let nk = sp[sp.len() - 1] + 1;
    let nxt = secs.get(nk - 1).map(|s| format!(" · next {b} {id}#{nk} {}", cut(&s.title, 40))).unwrap_or_default();
    let pid = if part == 0 { format!("{}#{k}", p.eq) } else { format!("{}#{k}{}", p.eq, letters(part)) };
    // A part of a page off the asked site says so, as its outline does (`place`)
    let off = Some(place(&doc.url, &doc.final_url)).filter(|s| s.starts_with("landed ")).map(|s| format!(" · {s}"));
    // a comment: `replies N`, its direct replies first (part 1), then its own text
    let (one, off) = (secs[k - 1].level >= COMMENT, off.unwrap_or_default());
    let title = if one { format!("replies {}", sp.len() - 1) } else { cut(&secs[k - 1].title, 60).into() };
    let head = format!("= {pid} {title} · part {}/{}{off} · up {b} {id}{nxt}", part + 1, parts.len());
    let mut lines = vec![head.trim_end().to_string()]; // a comment's next one is untitled
    lines.extend(comments(p, Some(k), 0, None).0.into_iter().skip(1).filter(|_| one && part == 0));
    lines.extend(listed(doc, id, &sp).filter(|_| !one && part == 0)); // a `Comments` heading's read
    lines.extend(parts[part].iter().cloned());
    lines.extend(more(&parts, part, &format!("{b} {id}#{k}{}", letters(part + 1))));
    Ok((lines, Some(Cursor { k: Some(k), part, ..Cursor::default() })))
}

/// The `… 1.2k more → <next>` row when parts follow `part`.
fn more(parts: &[Vec<String>], part: usize, next: &str) -> Option<String> {
    let rest = || ksize(parts[part + 1..].iter().flatten().map(|x| tok(x)).sum());
    (part + 1 < parts.len()).then(|| format!("… {} more → {next}", rest()))
}

/// Sections `ks` as read: each one's lines under its `## Title` head, the head led by `tag` (None = no head, the
/// section a read opens).
fn body(doc: &Doc, ks: &[usize], tag: impl Fn(usize) -> Option<String>) -> Vec<String> {
    let part = |&k: &usize| {
        let s = &doc.sections[k - 1];
        // a comment's head is its size, never its first words again (they follow), indented by its depth
        let (pad, n) = ("  ".repeat(s.level.saturating_sub(COMMENT)), span(doc, k).len() - 1);
        let head = if s.level < COMMENT { "#".repeat(s.level.max(1)) + " " + &s.title } else { format!("replies {n}") };
        tag(k).map(|t| format!("{pad}{t}{head}")).into_iter().chain(s.lines.iter().cloned())
    };
    ks.iter().flat_map(part).collect()
}

/// The parts `fats uN#k<part>` reads: section k with its subsections (a comment: its own text, its replies are
/// rows: `read_sec`), cut at `read_cap`.
fn parts_of(doc: &Doc, k: usize) -> Vec<Vec<String>> {
    let ks = if doc.sections[k - 1].level >= COMMENT { vec![k] } else { span(doc, k) };
    let id = |j: usize| if doc.sections[j - 1].level < COMMENT { String::new() } else { format!("#{j} ") };
    chunks_of(&body(doc, &ks, |j| (j != k).then(|| id(j)))) // a reply's id (a heading's is in the outline)
}

/// Every section with its `[u3#2] ## Title` head: the whole page as served, islands not opened (outline of a
/// short page, `full`).
fn whole(doc: &Doc, id: &str, ks: &[usize]) -> Vec<String> {
    body(doc, ks, |k| Some(format!("[{id}#{k}] ")))
}

/// `full`: the whole page as served, or (`uN#k full`, k a section: cli `read`) section k with all of its
/// subsections (a comment: its tree).
pub fn read_full(p: &Page, k: Option<usize>, part: usize) -> Shown {
    let (doc, id, b, at) = (p.doc, &p.id, bin(), k.map(|k| format!("#{k}")).unwrap_or_default());
    let parts = chunks_of(&whole(doc, id, &k.map_or_else(|| (1..=doc.served()).collect(), |k| span(doc, k))));
    if part >= parts.len() {
        return (vec![format!("= {}{at} full complete · {} parts · up {b} {id}", p.eq, parts.len())], None);
    }
    let mut lines = vec![format!("= {}{at} full · part {}/{} · up {b} {id}", p.eq, part + 1, parts.len())];
    lines.extend(parts[part].iter().cloned());
    lines.extend(more(&parts, part, &format!("{b} more")));
    (lines, Some(Cursor { full: true, part, k, ..Cursor::default() }))
}

/// The served sections that are comments (`clean::COMMENT`), in page order (`clean::build_doc` keeps a thread).
fn said(doc: &Doc) -> Vec<usize> {
    (1..=doc.served()).filter(|&k| doc.sections[k - 1].level >= COMMENT).collect()
}

/// The thread's line (the outline's footer, a read of a section holding comments): what is served, the verb.
fn listed(doc: &Doc, id: &str, ks: &[usize]) -> Option<String> {
    let n = ks.iter().filter(|&&k| doc.sections[k - 1].level >= COMMENT).count();
    (n > 0).then(|| format!("{} · list: {} {id} comments", tally(doc, n, n == said(doc).len()), bin()))
}

/// `n` served comments, against the size the page states when it is more (`clean::declared`): never a bare
/// `25` on a thread of 301 (`whole`: the page's thread, not one comment's replies).
fn tally(doc: &Doc, n: usize, whole: bool) -> String {
    let of = doc.info.get("comments").and_then(|d| d.parse().ok()).filter(|&d: &usize| d > n && whole);
    of.map_or_else(|| format!("{n} comments served"), |d| format!("{n} of {d} comments served (rest not in this copy)"))
}

/// `fats uN comments`: the top-level comments (`uN#k comments`: comment k's direct replies, also the head of its
/// read), most replied first (`replies` = the comments under it), one row each: `u3#12 · replies 42 · its first
/// words`; `from:N` goes on. No thread recognised (under `thread_rows`, flat posts, a thin copy) says so and points
/// at the page's sections, never that no comment is served (a short thread's comments are its sections' text);
/// a comment with none under it says `no replies`.
pub fn comments(p: &Page, k: Option<usize>, off: usize, limit: Option<usize>) -> Shown {
    let (doc, secs, id, at) = (p.doc, &p.doc.sections, &p.id, k.map(|k| format!("#{k}")).unwrap_or_default());
    let all = k.map_or_else(|| said(doc), |k| span(doc, k)[1..].to_vec());
    // a reply is direct when no comment before it in `all` is shallower: a dropped parent's reply moves up
    let (mut low, b, word) = (usize::MAX, bin(), if k.is_some() { "direct" } else { "top-level" });
    let mut tops: Vec<(usize, usize)> = all.iter().map(|&j| (j, span(doc, j).len() - 1)).collect();
    tops.retain(|&(j, _)| (secs[j - 1].level <= low, low = low.min(secs[j - 1].level)).0);
    tops.sort_by_key(|t| Reverse(t.1));
    let row = |&(j, n): &(usize, usize)| format!("{id}#{j} · replies {n} · {}", sec_preview(&secs[j - 1], &[], 120));
    let head = match all.len() {
        0 if said(doc).is_empty() => {
            format!("= {}{at} comments · no thread found · {} sections: {b} {id}", p.eq, doc.served())
        }
        0 => format!("= {}{at} comments · no replies · list: {b} {id} comments", p.eq),
        n => format!("= {}{at} {} · {} {word} · open one: {b} {id}#<k>", p.eq, tally(doc, n, k.is_none()), tops.len()),
    };
    let (rows, cap) = (tops.iter().map(row).collect::<Vec<_>>(), crate::settings::limit::<usize>("list_cap"));
    paged(&format!("{id}{at}"), head, &rows, (off, limit, cap, "comments"))
}

/// `fats uN links|cards [<words>]` from `off` up to the list cap (or `limit:` rows), `from:N` goes on: every link
/// (with its anchor text when filtered), or every card row (`lead`) with its part; with words, the ones whose URL
/// (percent-decoded) and anchor, or row, hold every word (folded).
pub fn list(p: &Page, off: usize, limit: Option<usize>, words: &[String], kind: &str) -> Shown {
    let (doc, id, b, q) = (p.doc, &p.id, bin(), words.join(" "));
    let all: Vec<(String, String)> = if kind == "links" {
        let a = |i: usize| doc.anchors.get(i).filter(|_| !q.is_empty()).map_or(String::new(), |a| format!(" · {a}"));
        let decoded = |u: &str| percent_encoding::percent_decode_str(u).decode_utf8_lossy().into_owned(); // `%C3%B1` = ñ
        let row = |(i, u): (usize, &String)| (decoded(u) + &a(i), format!("l{} {}{}", i + 1, short(u, 90), a(i)));
        doc.links.iter().enumerate().map(row).collect()
    } else {
        let card = |k: usize| doc.sections[k - 1].lines.iter().filter(|l| l.starts_with(CARD)).map(move |l| (k, l));
        (1..=doc.served()).flat_map(card).map(|(k, l)| (l.clone(), format!("{id}#{k} {}", lead(l)))).collect()
    };
    let rows: Vec<String> = all.iter().filter(|r| crate::search::holds(&r.0, words)).map(|r| r.1.clone()).collect();
    let of = if q.is_empty() { String::new() } else { format!(" of {} \"{q}\"", all.len()) };
    let open =
        (kind == "links" || rows.iter().any(|r| LREF.is_match(r))).then(|| format!(" · open one: {b} {id} l<N>"));
    let head = format!("= {} {kind} {}{of}{}", p.eq, rows.len(), open.unwrap_or_default());
    let cap = crate::settings::limit(if kind == "links" && q.is_empty() { "list_cap" } else { "read_cap" });
    paged(&p.id, head, &rows, (off, limit, cap, format!("{kind} {q}").trim_end()))
}

/// A card row (`clean::CARD`) as listed: up to its first paragraph and 200 chars (title, link, price, specs; a
/// teaser is filtered, never listed).
pub fn lead(l: &str) -> String {
    let pieces = l.split(PIECE).enumerate().take_while(|(i, p)| *i == 0 || !paragraph(p));
    short(&pieces.map(|x| x.1).collect::<Vec<_>>().join(PIECE), 200)
}

type Pager<'a> = (usize, Option<usize>, usize, &'a str);

/// THE paged list (`links`, `cards`, `media`, `comments`): the rows from `off` under the `cap` tokens (or `limit`
/// rows), then `+N → … <word> from:K` (the next page named in the hint, no session cursor: it works after any other
/// command), else `(end of <word>, N)`: the last page says so.
fn paged(at: &str, head: String, rows: &[String], (off, limit, cap, word): Pager) -> Shown {
    let rest = &rows[off.min(rows.len())..];
    // `limit:` = at most the rows asked, `list_rows` at most, under the cap too; a bigger one is named
    let max = crate::settings::limit("list_rows");
    let (mut lines, n) = (vec![head], limit.map(|n| n.min(max)));
    lines.extend(limit.filter(|&n| n > max).map(|n| format!("! ignored limit:{n} · max {max}")));
    let shown = cap_rows(&mut lines, rest, 0, (cap, 1, n));
    match rest.len() - shown {
        0 if rows.is_empty() => {}
        0 => lines.push(format!("(end of {word}, {})", rows.len())),
        n => lines.push(format!("+{n} → {} {at} {word} from:{}", bin(), off + shown + 1)),
    }
    (lines, Some(Cursor::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pg<'a>(doc: &'a Doc, meta: &'a Meta) -> Page<'a> {
        Page { id: "u1".into(), eq: "u1".into(), doc, meta, age: None, at: Default::default() }
    }

    /// Claude's resize rule (platform.claude.com vision docs): the docs' A4 example, a phone photo shrunk to 1568
    /// patches (measured: a vision model billed 1,572 input tokens for the 2048x1365 one), a small one as is; then the
    /// docs' size table ("Resolution and token cost"), both tiers. 2000x1500 on the standard tier: the table says
    /// 1269x952, the docs' own reference code (round half to even) 1270x952; the tokens agree (1564).
    #[test]
    fn viewed_as_claude_resizes() {
        assert_eq!(viewed(1075, 1520, STD), ((924, 1307), 1551));
        assert_eq!(viewed(1075, 1520, HI_RES), ((1075, 1520), 2145));
        assert_eq!(viewed(2048, 1365, STD), ((1345, 896), 1568));
        assert_eq!(viewed(5955, 3969, STD), ((1345, 896), 1568));
        assert_eq!(viewed(1000, 600, STD), ((1000, 600), 792));
        let table = [
            ((200, 200), ((200, 200), 64), ((200, 200), 64)),
            ((1000, 1000), ((1000, 1000), 1296), ((1000, 1000), 1296)),
            ((1092, 1092), ((1092, 1092), 1521), ((1092, 1092), 1521)),
            ((1920, 1080), ((1456, 819), 1560), ((1920, 1080), 2691)),
            ((2000, 1500), ((1270, 952), 1564), ((2000, 1500), 3888)),
            ((3840, 2160), ((1456, 819), 1560), ((2576, 1449), 4784)),
        ];
        for ((w, h), std, hi) in table {
            assert_eq!((viewed(w, h, STD), viewed(w, h, HI_RES)), (std, hi), "{w}x{h}");
        }
        // a picture's `~lo–hi tok if viewed` (`cli`): low = STD as `fats mcp` sends it, high = unscaled on HI_RES
        let ends = |w, h| [STD, HI_RES].map(|t| crate::lang::ksize(viewed(w, h, t).1 as usize));
        assert_eq!(
            (ends(3000, 2000), ends(2048, 1365)),
            (["1.5k".into(), "4.7k".into()], ["1.6k".into(), "3.6k".into()])
        );
    }

    /// A page off the asked site (bd's Airbnb) says where it landed on the `=` line; a subdomain or
    /// `www.` is the same site.
    #[test]
    fn landed_names_another_site() {
        let (com, pe) = ("https://www.airbnb.com/rooms/1", "https://www.airbnb.com.pe/rooms/1");
        assert_eq!(place(com, pe), "landed airbnb.com.pe");
        assert_eq!(place("https://www.google.de/", "https://www.google.com/"), "landed google.com");
        assert_eq!(place("https://www.cnn.com/a", "https://edition.cnn.com/a"), "edition.cnn.com");
        assert_eq!(place("https://medium.com/@k", "https://k.medium.com/"), "k.medium.com");
        assert_eq!(place("/tmp/a.pdf", "/tmp/a.pdf"), "a.pdf");
        let win = r"file://C:\Users\a\b.pdf"; // a Windows file names its file, never its whole path
        assert_eq!(place(win, win), "b.pdf");
        assert_eq!(place("https://ex.com/a", "http://ex.com/a"), "landed http://ex.com", "served over http");
        assert_eq!(place("https://ex.com./a", "https://www.ex.com/a"), "www.ex.com", "the root dot");
        // A part read says it too
        let sections = vec![Section { level: 1, title: "Rooms".into(), lines: vec!["Lima".into()] }];
        let doc = Doc { url: com.into(), final_url: pe.into(), sections, ..Doc::default() };
        let head = read_sec(&pg(&doc, &Meta::default()), 1, 0).map(|r| r.0[0].clone()).unwrap_or_default();
        assert!(head.contains(" · part 1/1 · landed airbnb.com.pe · up "), "{head}");
        // a section row counts its card rows, one card in the singular
        let lines = vec![format!("{CARD}Flat [l1] · 850 €"), "Lima".into()];
        let doc = Doc { sections: vec![Section { level: 1, title: "Rooms".into(), lines }], ..Doc::default() };
        assert_eq!((counts(&doc, &[1], 0), counts(&doc, &[1], 2)), (", 1 card".into(), ", 2 sub, 1 card".into()));
    }

    /// The flags of a page fetched (ok) from `url` that ended at `fin`.
    fn redirected(url: &str, fin: &str) -> String {
        let (doc, meta) =
            (Doc { url: url.into(), final_url: fin.into(), ..Doc::default() }, Meta { ok: true, ..Meta::default() });
        flags(&pg(&doc, &meta))
    }

    /// Parity pages: a site's own canonical URL for the same id is no redirect; a login, consent or other page is.
    /// A failed fetch's code comes first, on a redirected page and on an unparsable URL alike.
    #[test]
    fn redirected_only_when_the_id_is_gone() {
        let failed = |url: &str| {
            let doc = Doc { url: url.into(), final_url: "https://x.org/".into(), ..Doc::default() };
            let meta = Meta { code: "thin_content".into(), ..Meta::default() };
            flags(&pg(&doc, &meta))
        };
        assert_eq!(failed("https://x.org/a-long-slug"), " · ! thin_content · redirected");
        assert_eq!(failed("stdin:"), " · ! thin_content");
        // a bd landing by canonical (`Meta::landed`) is judged; the doc's base stays where the bytes came from
        let u = "https://a.example/i/9-charizard";
        let doc = Doc { url: u.into(), final_url: u.into(), ..Doc::default() };
        let meta = Meta { landed: Some("https://a.example/singles".into()), ok: true, ..Meta::default() };
        assert_eq!(flags(&pg(&doc, &meta)), " · ! redirected /singles");
        let same = [
            ("https://apps.apple.com/app/id68", "https://apps.apple.com/us/app/catchhour/id68"),
            ("https://vinted.fr/items/53", "https://www.vinted.fr/items/53-camisa-mango"),
            ("https://www.cnn.com/2026/politics/a-b", "https://edition.cnn.com/2026/politics/a-b"),
            ("https://data.gov.uk/datasets/1287/", "https://data.gov.uk/datasets/1287/?resource_version=52"),
            ("https://duckduckgo.com/?q=rust+async", "https://duckduckgo.com/?q=rust+async&ia=web"),
            ("https://x.org/sfc/bik/79.html", "https://x.org/view/79"),
            // canonical moves: the id to a query value, a handle to a host label, scheme, www, slash, utm
            ("https://www.youtube.com/shorts/aB3xYz9", "https://www.youtube.com/watch?v=aB3xYz9"),
            ("https://medium.com/@karpathy", "https://karpathy.medium.com/"),
            ("http://x.org/blog/post-1", "https://www.x.org/blog/post-1/"),
            ("https://x.org/?utm_source=hn", "https://x.org/"),
            // a segment under 8 letters or digits and no numeric id: an alias resolved (the latest post, the
            // current docs), a short path, a search's own canonical; no segment: a bare search
            ("https://blog.rust-lang.org/releases/latest/", "https://blog.rust-lang.org/2026/10/15/Rust-1.99.1"),
            ("https://x.org/docs/latest", "https://x.org/docs/v2.1"),
            ("https://x.org/a", "https://x.org/"),
            ("https://html.duckduckgo.com/html/?q=rust", "https://duckduckgo.com/"),
            ("https://duckduckgo.com/?q=rust", "https://duckduckgo.com/"),
            // A slug fixed beside its numeric id, at the same depth
            (
                "https://stackoverflow.com/questions/26946646/wrong-slug",
                "https://stackoverflow.com/questions/26946646/right",
            ),
        ];
        for (url, fin) in same {
            assert_eq!(redirected(url, fin), "", "{url} → {fin}");
        }
        let other = [
            ("https://eur-lex.europa.eu/eli/reg/2021/821-annex", "https://eur-lex.europa.eu/TodayOJ/", "/TodayOJ"),
            // a short segment whose numeric id (2+ all-digit segments) the landing drops: the page is lost
            ("https://eur-lex.europa.eu/eli/reg/2021/821", "https://eur-lex.europa.eu/TodayOJ/index.html", "/TodayOJ/"),
            ("https://x.org/reg/2021/821", "https://x.org/b", "/b"),
            (
                "https://old.reddit.com/r/fr/comments/1d/food_trucks/",
                "https://old.reddit.com/login/?dest=https%3A%2F%2Fold.reddit.com%2Fr%2Ffr%2Fcomments%2F1d%2Ffood%2F",
                "/login?dest",
            ),
            (
                "https://www.youtube.com/@BrianWilson",
                "https://consent.youtube.com/m?continue=x",
                "consent.youtube.com/m?",
            ),
            (
                "https://cards.example/en/Singles/Base-Set/Charizard-V9",
                "https://cards.example/en/Singles/Base-Set",
                "/en/",
            ),
            // another id at the same depth, the same id one level up: another page
            ("https://x.org/questions/26946646/async-await", "https://x.org/questions/31/a-c", "/questions/31/a-c"),
            ("https://x.org/q/26946646/async-await", "https://x.org/q/26946646", "/q/26946646"),
        ];
        for (url, fin, shown) in other {
            assert!(redirected(url, fin).contains(shown), "{url} → {fin}: {}", redirected(url, fin));
        }
    }

    /// A thin page (10 ~tok served of ~2.5k visible) with two islands: #2 the biggest (app state), #3 holding the
    /// post in its second part (after ~1.2k ~tok of strings), as the FB NASA post.
    fn thin_doc(info: &[(&str, &str)], title: &str) -> Doc {
        let sec = |title: &str, lines: Vec<String>| Section { level: 0, title: title.into(), lines };
        let filler = |n: usize| vec!["word ".repeat(200); n];
        let mut post = filler(5);
        post.push("Hello, Moon. It’s great to be back. Artemis II flew around the far side.".into());
        let sections = vec![
            Section { level: 1, ..sec("(top)", vec!["Log in".into()]) },
            sec("island json", filler(12)),
            sec("island json", post),
        ];
        let mut doc = Doc {
            url: "https://x.io/p".into(),
            final_url: "https://x.io/p".into(),
            sections,
            islands: 2,
            ..Doc::default()
        };
        (doc.title, doc.tokens.clean, doc.loss.visible_chars) = (title.into(), 10, 10_000);
        doc.info = info.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        doc
    }

    /// 5 Reddit threads: the title alone as a heading line is not the page's words served: the copy
    /// stays thin and points at the island; in a line of words it is the page (a post that quotes its title)
    #[test]
    fn a_title_heading_is_not_the_page() {
        let meta = Meta { url: "https://x.io/p".into(), ok: true, ..Meta::default() };
        let title = "Honest question, how important is safe code to you?";
        let mut doc = thin_doc(&[], title);
        (doc.sections[0].lines, doc.sections[2].title) = (vec![title.into()], "island template 9 KB".into());
        let f = flags(&pg(&doc, &meta));
        assert!(f.starts_with(" · ! thin 10 of ~5.0k visible → ") && f.ends_with(" u1#3 · 1.5k tok · $0"), "{f}");
        doc.sections[0].lines.push(format!("{title} I ask because my team keeps arguing about it."));
        assert_eq!(flags(&pg(&doc, &meta)), "");
    }

    /// 6 saved bd Airbnb copies: a page record of image names beside the id, no line of
    /// `paragraph_words` words, is no way in, nor a later record (Instagram's date formats); with its text it is
    /// (a TikTok caption)
    #[test]
    fn a_page_record_needs_its_text() {
        let mut doc = thin_doc(&[], "x");
        doc.url = "https://x.io/rooms/29055015".into();
        let text = format!(r#"l3   "desc": "{}""#, "a listing text of words ".repeat(3));
        doc.sections[1].lines = vec![r#"l1   "id": "29055015","#.into(), r#"l2   "name": "Living_room","#.into()];
        doc.sections[2].lines = vec![r#"l1   "id": "29055015","#.into(), text.clone()];
        assert_eq!(record_in(&doc, &Meta::default(), &[]), None);
        doc.sections[1].lines.push(text);
        let rec = record_in(&doc, &Meta::default(), &[]).unwrap_or_default();
        assert!(rec.starts_with("#2 · page record · "), "{rec}");
    }

    /// A thin page whose served section is a feed (≥ `feed_links` link rows titled in ≥ 2 words, most of the served
    /// text) points there before an island guess; a thread in a template island wins over it; bare counts, a few
    /// rows or a list beside the page's text are no feed.
    #[test]
    fn a_thin_feed_points_at_its_posts() {
        let (mut doc, b) = (thin_doc(&[], "Profile"), bin());
        let meta = Meta { url: doc.url.clone(), ok: true, ..Meta::default() };
        let to = |doc: &Doc, k: &str| flags(&pg(doc, &meta)).contains(&format!("→ {b} u1#{k} · "));
        let mut rows: Vec<String> = (1..=12).map(|i| format!("1.{i}M · Crew {i} reaches the station [l{i}]")).collect();
        rows.extend((13..=20).map(|i| format!("{i}.5K [l{i}]")));
        (doc.sections[0].lines, doc.tokens.clean) = (rows.clone(), tok(&rows.join("\n")));
        assert!(to(&doc, "1"), "{}", flags(&pg(&doc, &meta)));
        doc.sections[2].title = "island template 9 KB".into(); // the comments of a thread
        assert!(to(&doc, "3") && !to(&doc, "1"), "{}", flags(&pg(&doc, &meta)));
        doc.sections[2].title = "island json".into();
        doc.tokens.clean *= 3; // the rows beside a page's text twice their size: a related list
        assert!(!to(&doc, "1"));
        doc.tokens.clean /= 3;
        doc.sections[0].lines.drain(..3); // 9 titled rows
        assert!(!to(&doc, "1"));
    }

    /// The thin pointer is the island PART holding the description's first 25 chars, never the biggest
    /// island; no anchor = a JS render. One `!` group, the `→` on the last code with a next step.
    #[test]
    fn thin_points_at_the_anchored_part() {
        // 5 saved Airbnb copies: the listing name beside image links yields to the island part
        // holding it with a line of words; with no such part, never a way in (0 prose lines)
        let name = "Luxury Panoramic Seville Apartment";
        let mut doc = thin_doc(&[("description", name)], "Rental unit");
        doc.sections[1].lines = vec![r#""uri": "https://a0.x/1.jpg""#.into(), format!(r#""name": "{name}""#)];
        let meta = Meta { url: doc.url.clone(), ..Meta::default() };
        assert!(!flags(&pg(&doc, &meta)).contains("u1#2"), "{}", flags(&pg(&doc, &meta)));
        doc.sections[2].lines.push(format!("{name}: a penthouse near the Old Town, panoramic views over the city"));
        assert!(flags(&pg(&doc, &meta)).ends_with(&format!("→ {} u1#3b · 651 tok · $0", bin())));
        // Then the part of that island with the longest text of ≥ `prose_run` words holding ≥ 2 of
        // the name's words (Airbnb's listing text, 36 parts before its name); the 200-word filler holds none
        doc.sections[2].lines.insert(0, format!("A penthouse in Seville, {}", "panoramic views ".repeat(50)));
        assert!(
            flags(&pg(&doc, &meta)).ends_with(&format!("→ {} u1#3 · 861 tok · $0", bin())),
            "{}",
            flags(&pg(&doc, &meta))
        );
        // a bd copy: the name in a teaser of words in island #2, the listing text in #3: the text
        doc.sections[1].lines.push(format!("{name}: {}", "a teaser line ".repeat(5)));
        assert!(flags(&pg(&doc, &meta)).ends_with(&format!("→ {} u1#3 · 861 tok · $0", bin())));
        let (b, desc) = (bin(), ("description", "Hello, Moon. It’s great to be back. #Artemis"));
        let meta = Meta { url: "https://x.io/p".into(), code: "js_shell".into(), ..Meta::default() };
        let page = |doc: &Doc| flags(&pg(doc, &meta));
        let doc = thin_doc(&[("site", "Facebook"), desc], "Facebook");
        let shown = format!(" · ! js_shell · page content shown · thin 10 of ~5.0k visible → {b} u1#3b · 619 tok · $0");
        assert_eq!(page(&doc), shown);
        let held = pg(&doc, &meta);
        let row = page_flags(&held).remove(0).bang("u1");
        assert!(
            row.starts_with("u1 ! js_shell · page content shown")
                && row.ends_with(&format!(" → {b} u1#3b · 619 tok · $0")),
            "{row}"
        );
        let titled = thin_doc(&[("site", "Facebook")], "Hello, Moon. It’s great to be back.");
        assert_eq!(page(&titled), shown, "the title when it is not the site's name");
        let short = thin_doc(&[("description", "Moon")], "Hello, Moon. It’s great to be back.");
        assert_eq!(page(&short), shown, "a short description falls back to the title");
        for doc in [thin_doc(&[("site", "Facebook")], "Facebook"), thin_doc(&[("description", "Moon")], "x")] {
            // one next move an answer: the failed row's, never a second one on the `=` line
            let (f, row) = (page(&doc), page_flags(&pg(&doc, &meta)).remove(0).bang("u1"));
            assert!(
                row.contains(&format!("→ {b} u1 "))
                    && row.ends_with("js:true ≤$0.0015")
                    && !f.contains('→')
                    && !f.contains("u1#2"),
                "{f} {row}"
            );
        }
        // an app page (`no_copy`): every `!` has a next step, the render at its price
        let dead = Meta { code: "no_copy".into(), ..meta.clone() };
        let mut doc = thin_doc(&[("site", "Facebook")], "Facebook");
        let (f, row) = (flags(&pg(&doc, &dead)), page_flags(&pg(&doc, &dead)).remove(0).bang("u1"));
        // No own words, no island pointer (Instagram's longest prose string: a 107-char legal notice); the
        // render is the `no_copy` row's one next move, the `=` line has none
        assert_eq!(f, " · thin 10 of ~5.0k visible · ! no_copy");
        assert!(row.ends_with(&format!("→ {b} u1 js:true · bd, then oxyjs ≤$0.0030 ≈25s")), "{row}");
        // a render tier the arrow names refused the URL (`fetch::settled`): the same ask is refused again, no arrow
        let why = "400 url: value not supported".to_string();
        let hop = HopRow { provider: "oxylabs-js".into(), code: "provider_error".into(), why, ..HopRow::default() };
        let refused = Meta { hops: vec![hop], ..dead.clone() };
        let row = page_flags(&pg(&doc, &refused)).remove(0).bang("u1");
        assert!(!row.contains("js:true"), "{row}");
        // TikTok's card (`reg/u2` doc.json): no description, the site's title, the caption as the picture's
        // alt; the island part holding it, not the policy text (its longest prose string)
        doc.media = vec![crate::clean::Media::default()];
        doc.media[0].alt = "Hello, Moon. It’s great to be back. Artemis II".into();
        let f = flags(&pg(&doc, &dead));
        assert!(f.ends_with(&format!("→ {b} u1#3b · 619 tok · $0")), "{f}");
        let hits =
            crate::search::grep(&pg(&doc, &meta), &["flew".into()], (crate::lang::Match::Words, false), None, None);
        // No hit in the served text (nor the info line) → the islands', the `=` line says where they are
        let (l, _) = hits.unwrap_or_default();
        assert!(l[0].ends_with(" · in islands") && l[1].starts_with("u1#3 "), "the filter reads islands: {l:?}");
    }

    /// The page's own words served = its islands repeat it (Discourse's posts, escaped): not thin, the island
    /// holding them is `(dup)`, the other is not; markdown is never thin.
    #[test]
    fn served_words_are_not_thin() {
        let meta = Meta { url: "https://x.io/p".into(), ok: true, ..Meta::default() };
        let mut doc = thin_doc(&[("description", "Hello, Moon. It’s great to be back. #Artemis")], "T");
        doc.sections[0].lines.push("Hello, Moon. It’s great to be back.".into());
        let page = pg(&doc, &meta);
        let rows = outline(&page, None).0;
        assert!(!rows[0].contains("thin") && flags(&page).is_empty(), "{rows:?}");
        let isl: Vec<&String> = rows.iter().filter(|r| r.contains(" island json")).collect();
        assert!(!isl[0].contains("(dup)") && isl[1].starts_with("[u1#3] island json (dup) · "), "{isl:?}");
        let md = Doc { mode: "md".into(), ..thin_doc(&[], "T") };
        assert!(!is_thin(&md) && is_thin(&thin_doc(&[], "T")));
        // oxy-only Reddit: a thin copy whose thread island holds the comments shows the page (its run
        // taught its tier, `cli` `holds`); a data island never does
        let mut thread = thin_doc(&[], "T");
        assert_eq!(shown(&thread, &Meta::default()), None, "a data island holds no thread");
        thread.sections[2].title = "island template 9 KB".into();
        assert_eq!(shown(&thread, &Meta::default()), Some("#3".into()), "the thread island");
    }

    /// `N of M numbers pruned` is a COUNT on the `=` line at ≥ 20% and ≥ 10 numbers; the outline then
    /// never says `(whole page shown)`, nor on a thin page or one with an unread island.
    #[test]
    fn pruned_count_and_no_whole_page_claim() {
        let meta = Meta { ok: true, ..Meta::default() };
        let t = crate::html::parse("<title>T</title><p>text</p>");
        let mut doc = crate::clean::build_doc(&t, "https://x.io/", "https://x.io/", false);
        let mut show = |lost, all| {
            (doc.loss.vs_light_lost, doc.loss.light_facts) = (lost, all);
            let lines = outline(&pg(&doc, &meta), None).0;
            (lines[0].clone(), lines.iter().any(|l| l.starts_with("(whole page shown)")))
        };
        let (head, whole) = show(10, 50);
        assert!(head.contains(" · 0 photos · 10 of 50 numbers pruned · ") && !head.contains('!') && !whole, "{head}");
        for (lost, all) in [(9, 20), (10, 51), (0, 0)] {
            let (head, whole) = show(lost, all);
            assert!(!head.contains("pruned") && whole, "under the bar: {head}");
        }
        let mut islands = thin_doc(&[], "T");
        islands.loss.visible_chars = 0; // not thin, two islands unread
        for doc in [thin_doc(&[], "T"), islands] {
            let lines = outline(&pg(&doc, &meta), None).0;
            assert!(!lines.iter().any(|l| l.contains("whole page")), "{lines:?}");
        }
    }

    /// A `!` names its next step. A JS render already spent, `thin` is a COUNT (`thin 10 of ~2.5k visible`) and an og
    /// card is `og` + `(card only)` with no `! js_shell`; while the render is left, both keep their `!` and its `→`.
    #[test]
    fn no_bang_without_a_next_step() {
        let hop = |p: &str| crate::cache::HopRow { provider: p.into(), code: "js_shell".into(), ..Default::default() };
        let meta =
            |hops: Vec<crate::cache::HopRow>| Meta { url: "https://x.io/p".into(), ok: true, hops, ..Meta::default() };
        let (rendered, plain) =
            (meta(vec![hop("plain"), hop("brightdata"), hop("oxylabs-js")]), meta(vec![hop("plain")]));
        let page = |doc: &Doc, meta: &Meta| flags(&pg(doc, meta));
        let thin = thin_doc(&[("site", "Facebook")], "Facebook");
        assert_eq!(page(&thin, &rendered), " · thin 10 of ~5.0k visible");
        assert!(page(&thin, &plain).starts_with(" · ! thin 10 of ~5.0k visible → "), "{}", page(&thin, &plain));
        let card = Doc { mode: "og".into(), ..thin_doc(&[], "NASA") };
        assert_eq!(page(&card, &rendered), "");
        assert!(page(&card, &plain).starts_with(" · ! js_shell → "), "{}", page(&card, &plain));
        let lines = outline(&pg(&card, &rendered), None).0;
        assert!(lines.iter().any(|l| l.starts_with("(card only)")) && !lines[0].contains('!'), "{lines:?}");
    }

    /// `(whole page shown)` only when the page has no flag at all: an HTML parse cut short, a failed fetch's best copy
    /// and a redirect never claim it.
    #[test]
    fn whole_page_claim_needs_no_flag() {
        let t = crate::html::parse("<title>T</title><p>text</p>");
        let doc = crate::clean::build_doc(&t, "https://x.io/a", "https://x.io/a", false);
        let claims = |doc: &Doc, meta: &Meta| {
            let lines = outline(&pg(doc, meta), None).0;
            lines.iter().any(|l| l.starts_with("(whole page shown)"))
        };
        let ok = Meta { ok: true, ..Meta::default() };
        assert!(claims(&doc, &ok));
        assert!(!claims(
            &Doc { cut: 4096, ..crate::clean::build_doc(&t, "https://x.io/a", "https://x.io/a", false) },
            &ok
        ));
        assert!(!claims(&doc, &Meta { code: "thin_content".into(), ..Meta::default() }));
        let moved = crate::clean::build_doc(&t, "https://x.io/a-long-slug", "https://x.io/login", false);
        assert!(!claims(&moved, &ok));
    }

    /// A car listing's date sat past char 70 of its title; a cut-off number now ends the line.
    #[test]
    fn title_keeps_a_cut_off_number() {
        let car = "Volkswagen Golf Automobile for sale in parts, we can offer various years 2000-02, | A28920205";
        assert_eq!(title_cut(car, 70), "Volkswagen Golf Automobile for sale in parts … 2000-02, | A28920205");
        let words = "Polizei in Lyon deckt Millionenbetrug mit Scheinfirmen auf: Großeinsatz in mehreren Ländern";
        assert_eq!(title_cut(words, 70), "Polizei in Lyon deckt Millionenbetrug mit Scheinfirmen auf…");
        assert_eq!(title_cut("short", 70), "short");
        assert_eq!(title_cut(&"x".repeat(80), 70).chars().count(), 70);
        for t in [car, words] {
            assert!(title_cut(t, 70).chars().count() <= 70);
        }
    }

    #[test]
    fn sections_and_top() {
        let l = |t: &str| Line::T(t.into());
        let h = |n, t: &str| Line::H(n, t.into());
        let (s, top) = sections_of(&[l("intro"), h(2, "A"), l("a"), h(3, "A1"), h(2, "B"), h(2, "C")]);
        assert_eq!(s.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["(top)", "A", "A1", "B", "C"]);
        assert_eq!(top, 2);
        let (s, _) = sections_of(&[l(&"x".repeat(1000)), l(&"y".repeat(400))]);
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].title, "part 2");
    }

    /// An HN thread (one comment's markup as saved, text trimmed): a comment row is an untitled section (its
    /// words follow), never folded as a link list; a filter hit is one row.
    #[test]
    fn a_thread_comment_is_prose() {
        let row = |i: usize| {
            let w = if i.is_multiple_of(3) { 40 } else { 0 };
            format!(
                r##"<tr class="athing comtr"><td><table border="0"><tr><td class="ind"><img src="s.gif" height="1" width="{w}"></td><td class="default"><div><span
                class="comhead"><a href="user?id=collinfunk" class="hnuser">collinfunk</a> <span class="age"><a
                href="item?id=4794426{i}">5 months ago</a></span> <span class="navs"> | <a href="#47944210"
                class="clicky">next</a> <a class="togg clicky">[–]</a></span></span></div><br><div class="comment"><div
                class="commtext c00">Hi, I am one of the maintainers of GNU Coreutils {i}. Thanks for the article.
                </div></div></td></tr></table></td></tr>"##
            )
        };
        let html = format!(
            "<title>T</title><table><tr><td><table>{}</table></td></tr></table>",
            (1..=8).map(row).collect::<String>()
        );
        let doc = crate::clean::build_doc(&crate::html::parse(&html), "https://x.io/i", "https://x.io/i", false);
        let lv: Vec<(usize, &str)> = doc.sections.iter().map(|s| (s.level - COMMENT, s.title.as_str())).collect();
        assert_eq!(
            lv,
            [(0, ""), (0, ""), (1, ""), (0, ""), (0, ""), (1, ""), (0, ""), (0, "")],
            "a depth a spacer width"
        );
        let meta = Meta { ok: true, ..Meta::default() };
        let p = pg(&doc, &meta);
        assert!(!outline(&p, None).0.iter().any(|l| l.contains("folded")), "{:?}", outline(&p, None).0);
        let words = ["coreutils".to_string()];
        let hits =
            crate::search::grep(&p, &words, (crate::lang::Match::Words, false), None, None).unwrap_or_default().0;
        assert!(hits[0].contains(" · 8 hit · ") && hits.len() == 10, "a row a comment, not its title too: {hits:?}");
    }

    /// TOP PART (a live listing's "listing expired" banner, 96 tok, was cut at 80 chars in every first answer): the
    /// text before the first heading prints whole up to `top_part_tok`; a longer one keeps its preview.
    #[test]
    fn a_short_top_part_prints_whole() {
        let meta = Meta { ok: true, ..Meta::default() };
        let body =
            |w: &str| format!("<p>{}</p>", "Three rooms, a balcony and a quiet yard near the river. ".repeat(40) + w);
        for (n, whole) in [(3, true), (12, false)] {
            let top = format!("<p>{} The listing has expired.</p>", "Notice from the site about this ad. ".repeat(n));
            let secs: String = (1..=4).map(|i| format!("<h2>Part {i}</h2>{}", body(""))).collect();
            let t = crate::html::parse(&format!("<title>Flat</title><body>{top}{secs}</body>"));
            let doc = crate::clean::build_doc(&t, "https://x.io/f", "https://x.io/f", false);
            let lines = outline(&pg(&doc, &meta), None).0;
            let row = lines.iter().find(|l| l.starts_with("u1#1 (top)")).cloned().unwrap_or_default();
            assert_eq!(row.ends_with("The listing has expired."), whole, "{n}: {lines:?}");
        }
    }

    /// The media count is on every outline and `all` `=` line, `0 photos` too, and no `!` comes with it; a page
    /// with pictures lists its first in the outline (`m1 … | alt → url`) and all of them under `media`.
    #[test]
    fn photo_count_and_media_rows() {
        let meta = Meta { ok: true, ..Meta::default() };
        let t = crate::html::parse("<title>Example Domain</title><h1>Example Domain</h1><p>For use in examples.</p>");
        let doc = crate::clean::build_doc(&t, "https://example.com/", "https://example.com/", false);
        let p = Page { eq: "j1.u1".into(), ..pg(&doc, &meta) };
        let head = outline(&p, None).0.remove(0);
        assert!(head.starts_with("= j1.u1 Example Domain · example.com · 1 sections · 0 photos · "), "{head}");
        assert_eq!(media(&p, 0, None).0[0], format!("= j1.u1 media · 0 photos · up {} u1", bin()), "no m1: no pixels");
        assert!(outline_all(&p).0[0].contains(" · 0 photos · ") && !head.contains('!') && flags(&p).is_empty());
        let card = r#"<meta property="og:image" content="/a.jpg"><meta property="og:image:alt" content="A cat">"#;
        let pics: String = (1..=4).map(|i| format!("<img src='/p{i}.jpg' alt='picture {i}'>")).collect();
        let t = crate::html::parse(&format!("<title>T</title>{card}<p>text</p>{pics}"));
        let doc = crate::clean::build_doc(&t, "https://x.io/", "https://x.io/", false);
        let p = pg(&doc, &meta);
        let lines = outline(&p, None).0;
        assert!(lines[0].contains(" · 5 photos · "), "{lines:?}");
        assert!(lines.contains(&"m1 photo jpg | A cat → https://x.io/a.jpg".to_string()), "{lines:?}");
        assert!(lines.contains(&format!("… +2 media → {} u1 media", bin())), "{lines:?}");
        let all = media(&p, 0, None).0;
        assert_eq!(
            (all[0].clone(), all.len()),
            (format!("= u1 media · 5 photos · up {b} u1 · pixels {b} u1 m1", b = bin()), 7),
            "{all:?}"
        );
        // measured only: a js shell's or a wall's `0` is the shell's, never printed; the
        // pictures it does list count (a shell's own og card)
        let shell = Meta { ok: false, code: "js_shell".into(), ..Meta::default() };
        let p = Page { meta: &shell, ..p };
        assert!(outline(&p, None).0[0].contains(" · 5 photos · ") && media(&p, 0, None).0[0].contains(" · 5 photos"));
        let bare = crate::clean::build_doc(&crate::html::parse("<p>text</p>"), "https://x.io/", "https://x.io/", false);
        let p = Page { doc: &bare, ..p };
        assert!(!outline(&p, None).0[0].contains("photo") && !media(&p, 0, None).0[0].contains("photo"));
    }

    /// `media` pages like `links`: the rows under the read cap, `+N → … media from:K`, and
    /// `K` starts the next page where this one ended; tracking parameters never reach a row.
    #[test]
    fn media_pages_on() {
        let meta = Meta { ok: true, ..Meta::default() };
        let pics: String =
            (1..=80).map(|i| format!("<img src='/p/photo-{i}a.jpg?utm_source=x&k=1' alt='p{i:02}'>")).collect();
        let doc = crate::clean::build_doc(
            &crate::html::parse(&format!("<p>t</p>{pics}")),
            "https://x.io/",
            "https://x.io/",
            false,
        );
        let p = pg(&doc, &meta);
        let first = media(&p, 0, None).0;
        let next = first.len() - 2;
        assert_eq!(first.last(), Some(&format!("+{} → {} u1 media from:{}", 80 - next, bin(), next + 1)), "{first:?}");
        assert!(first[1].ends_with("→ https://x.io/p/photo-1a.jpg?k=1"), "{first:?}");
        let (second, _) = media(&p, next, None);
        assert!(second[1].starts_with(&format!("m{} ", next + 1)), "{second:?}");
    }

    /// Demoted media (avatar size, a bare `@handle`, the author's or site's name as alt) are listed under
    /// `media`, not counted, and not in the outline rows while content exists; a photo alt'd with the page title
    /// is content (a shop's main photo), counted, its alt dropped (the title says nothing about the picture).
    #[test]
    fn demoted_media_listed_not_counted() {
        let meta = Meta { ok: true, ..Meta::default() };
        let head = "<title>Sky tonight</title><meta property='og:site_name' content='NASA'><p>text</p>";
        let low = "<img src='/me.jpg' alt='@nasa'><img src='/n.jpg' alt='nasa'><img src='/f.jpg' width=96 height=96>";
        let page =
            |body: &str| crate::clean::build_doc(&crate::html::parse(body), "https://x.io/", "https://x.io/", false);
        let doc = page(&format!(
            "{head}<meta property='og:image' content='/sky.jpg'><img src='/t.jpg' alt='Sky tonight'>{low}"
        ));
        let p = pg(&doc, &meta);
        let lines = outline(&p, None).0;
        let rows: Vec<&String> = lines.iter().filter(|l| l.starts_with('m')).collect();
        let want = ["m1 photo jpg | (no alt) → https://x.io/sky.jpg", "m2 photo jpg | (no alt) → https://x.io/t.jpg"];
        assert!(lines[0].contains(" · 2 photos · ") && rows == want, "{lines:?}");
        assert!(lines.contains(&format!("… +3 media → {} u1 media", bin())), "{lines:?}");
        let all = media(&p, 0, None).0;
        assert!(all[0].contains(" · 2 photos · ") && all.len() == 7, "every row listed: {all:?}");
        assert!(all[5].contains(" (not counted) |"), "{all:?}");
        let doc = page(&format!("{head}{low}"));
        let p = pg(&doc, &meta);
        let lines = outline(&p, None).0;
        let rows = lines.iter().filter(|l| l.starts_with('m')).count();
        assert!(lines[0].contains(" · 0 photos · ") && rows == 3, "nothing else: shown, not counted: {lines:?}");
    }

    /// A parse that ran out of time says so on the outline, with how much was left.
    #[test]
    fn cut_parse_is_a_row() {
        let t = crate::html::parse("<title>T</title><p>start</p>");
        let mut doc = crate::clean::build_doc(&t, "https://x.io/", "https://x.io/", false);
        doc.cut = 3_000_000;
        let meta = Meta { bytes: 9_000_000, ok: true, ..Meta::default() };
        let p = pg(&doc, &meta);
        let (lines, _) = outline(&p, None);
        assert!(lines[1].starts_with("u1 ! parse_cut last 3.0MB of 9.0MB not parsed (2s budget)"), "{lines:?}");
        assert!(lines[1].contains(&format!("→ {} u1 all", bin())));
        doc.mode = "pdf".into();
        doc.cut = 70;
        let p = pg(&doc, &meta);
        let (lines, _) = outline(&p, None);
        assert!(lines[1].starts_with("u1 ! parse_cut pages 2-71 of 71 not read (10s budget) → the rest: qpdf "));
        assert!(lines[1].contains("--pages . 2-z -- rest.pdf"), "{lines:?}");
    }

    /// The read hint skips a portal header that is mostly links (a listings portal's 160 ~tok header, read
    /// first before) for the first block of prose; a page of only link rows reads its biggest section, a flat one
    /// (no headings) its first part.
    #[test]
    fn read_hint_skips_chrome() {
        let sec = |title: &str, lines: Vec<String>| Section { level: 1, title: title.into(), lines };
        let nav: Vec<String> = (1..30).map(|i| format!("- Anzeigen {i} [l{i}]")).chain(["Anmelden".into()]).collect();
        let prose = vec!["4-Zimmer-Wohnung mit Panoramafenstern, hell und geräumig, neben Park und Schule. ".repeat(6)];
        let doc = |secs: Vec<Section>| Doc {
            title: "Wohnung in Köln, 4 Zimmer".into(),
            top: 1,
            sections: secs,
            ..Doc::default()
        };
        let d = doc(vec![sec("Nr.1 Immobilienportal", nav.clone()), sec("Köln, Deutz", prose.clone())]);
        assert!(sec_tokens(&d, 1) >= 50, "big enough for the old rule (first top-level section ≥ 50 ~tok)");
        assert_eq!(read_first(&d, &[]), 2);
        let thread =
            |n: usize| vec![(0..n).map(|i| format!("user{i} [l{i}] 2 days ago | reply [l{i}] ")).collect::<String>()];
        let d = doc(vec![sec("a", thread(3)), sec("b", thread(200)), sec("c", thread(4))]);
        assert_eq!(read_first(&d, &[]), 2, "no prose anywhere: the biggest section");
        // the HN front page: a page with no headings is chunks of one flow: part 1, never the fullest chunk
        let d = doc(vec![sec("part 1", thread(3)), sec("part 2", thread(200)), sec("part 3", thread(4))]);
        assert_eq!(read_first(&d, &[]), 1, "flat parts: the first");
        let d = doc(vec![sec("part 1", prose.clone()), sec("4 Zimmer Wohnung in Köln", prose)]);
        assert_eq!(read_first(&d, &[]), 2, "a heading that repeats the title wins");
    }

    #[test]
    fn preview_skips_menus_to_title_words() {
        let sec = |title: &str, text: &str| Section { level: 1, title: title.into(), lines: vec![text.into()] };
        let menu = "0 [l1]\nYour account\n- Classifieds [l6]\n- Support [l13]\nHeadlines\nNo.1\nID: A28920205\n2000\n\
                    Volkswagen Golf\nPrice on request\nBuild date\n2000-02";
        let q = |t: &str| {
            let doc = Doc { title: t.into(), mode: "light".into(), ..Doc::default() };
            title_query(&doc)
        };
        let title = q("Volkswagen Golf 2000 for parts | Autos.example");
        let part = sec("part 1", menu);
        assert_eq!(sec_preview(&part, &title, 80), "…2000 Volkswagen Golf Price on request Build date 2000-02");
        // no title word in the section, or not a light render → the first chars, as before
        assert_eq!(sec_preview(&part, &q("Nissan Note"), 30), preview(menu, 30));
        let clean = Doc { title: "Volkswagen Golf 2000".into(), mode: "clean".into(), ..Doc::default() };
        assert_eq!(sec_preview(&part, &title_query(&clean), 30), preview(menu, 30));
        // a title word in the first n chars keeps the lead, even when a later window holds more
        let lead = "Photo: Reuters. Volkswagen sells parts. Later: the 2000 Volkswagen Golf for parts";
        assert_eq!(sec_preview(&sec("part 2", lead), &title, 40), preview(lead, 40));
        // words the heading already shows are not looked for: the main block keeps its lead (byline, price)
        let main = "15 September 2026 Peter Hoskins. Volkswagen Golf for parts";
        assert_eq!(sec_preview(&sec("Volkswagen Golf 2000 for parts", main), &title, 30), preview(main, 30));
    }

    /// A part's preview starts at its text, never at a menu or a byline row (≥ 3 links, ≤ 3 words a link).
    /// The HN front page (a saved copy): part 1 opened on the menu, part 2 on the last story's byline.
    #[test]
    fn a_part_preview_skips_menu_and_byline_rows() {
        let part = |lines: &[&str]| Section {
            level: 1,
            title: "part 1".into(),
            lines: lines.iter().map(|l| l.to_string()).collect(),
        };
        let menu =
            "|  | Hacker News [l1] new [l2] | past [l3] | comments [l4] | ask [l5] | show [l6] | jobs [l7] | login |";
        let story = "| 1. |  | Things to know when you get cancer [l9] (jjmaxwell4.com [l10] ) |";
        let byline = "|  | 597 points by gmays [l21] 12 hours ago [l22] | hide [l23] | 230 comments [l22] |";
        assert_eq!(sec_preview(&part(&[menu, story, byline]), &[], 60), "Things to know when you get cancer |");
        assert_eq!(sec_preview(&part(&[byline, story]), &[], 60), "Things to know when you get cancer |");
        // prose with links is text
        let prose = "Lyon is at the confluence of the Rhône [l1] and Saône [l2] rivers [l3], in the east of France.";
        assert_eq!(sec_preview(&part(&[prose]), &[], 30), preview(prose, 30));
        // A layout row (an empty cell, a link) shows its first piece of > 3 words, else its wordiest:
        // each story's title (a 2-word one too), each comment's text past its byline (a number is no word)
        let short = "| 3. |  | OpenCode 2.0 [l19] (anoma.ly [l20] ) |";
        let titles = "Things to know when you get cancer | OpenCode 2.0 |";
        assert_eq!(sec_preview(&part(&[story, byline, short, byline]), &[], 80), titles);
        let comment =
            "|  |  | Stitch4223 [l15] 28 days ago [l16] | next [–] “Hi Codex, please find attached the specification”";
        assert_eq!(sec_preview(&part(&[comment]), &[], 40), "“Hi Codex, please find attached the…");
        let unlinked =
            "|  |  | alhirzel 24 days ago | prev | next [–] Reminds me a lot of the Microgrid Standard (aka [l56] TMS";
        assert_eq!(sec_preview(&part(&[unlinked]), &[], 30), "Reminds me a lot of the…");
    }

    /// A provider's geo copy is a page flag on every read (a cache read too, was the fetch's own `!`
    /// row only); the tier word names the exit country only when it is not the default one (`oxyjs·fr✓`).
    /// Its fix changes the request, never the country the call already sent (the same copy again, a paid loop).
    #[test]
    fn a_geo_copy_is_flagged_on_every_read() {
        use Tier::*;
        let hop = |p: &str, why: &str| crate::cache::HopRow {
            provider: p.into(),
            code: "ok".into(),
            why: why.into(),
            ..Default::default()
        };
        let url = "https://www.reddit.com/r/rust/".to_string();
        let mut meta = Meta { ok: true, url, hops: vec![hop(Bd.spec().name, "lang:it")], ..Meta::default() };
        let doc = thin_doc(&[], "Linear algebra crates");
        let read = Page { age: Some(3600.0), ..pg(&doc, &meta) };
        let row = page_flags(&read).iter().find(|f| f.code == "lang:it").map(|f| f.bang("u1")).unwrap_or_default();
        let want =
            "u1 ! lang:it · served in it, sent none · the provider's geo copy → {b} u1 country:us fresh:true ≤$0.0040";
        assert_eq!(row, want.replace("{b}", bin()), "bd sent none (its best pick): asking US changes the request");
        assert_eq!(meta.tried(), "bd✓lang:it", "no country sent: no tier word");
        // an Oxylabs call sends US where nothing names a country (`provider::sent`): the fix is the other provider
        (meta.country, meta.hops) = ("US".into(), vec![hop(OxyJs.spec().name, "lang:it")]);
        assert_eq!(meta.tried(), "oxyjs✓lang:it", "the home country: no tier word");
        let row = page_flags(&pg(&doc, &meta)).iter().find(|f| f.code == "lang:it").map(|f| f.bang("u1"));
        assert!(row.is_some_and(|r| r.contains("sent US") && r.ends_with(" u1 provider:bd fresh:true ≤$0.0015")));
        (meta.country, meta.hops) = ("FR".into(), vec![hop(OxyJs.spec().name, "")]);
        assert_eq!(meta.tried(), "oxyjs·fr✓");
        assert!(!page_flags(&pg(&doc, &meta)).iter().any(|f| f.code.starts_with("lang:")), "no geo copy, no flag");
        // the fix never repeats what was sent: the domain's (else the home) country when another or none went, else
        // the other provider, else no fix (a COUNT, `flags`)
        let fix = |url: &str, sent: &str, t: Tier, keys: &[Tier]| geo_fix(url, sent, t, keys);
        let (fr, com) = ("https://www.lemonde.fr/a", "https://x.com/");
        assert_eq!(fix(fr, "US", OxyJs, &[Plain, OxyJs]).as_deref(), Some("country:fr fresh:true"));
        assert_eq!(fix(fr, "FR", OxyJs, &Tier::ALL).as_deref(), Some("provider:bd fresh:true"));
        assert_eq!(fix(fr, "FR", Bd, &Tier::ALL).as_deref(), Some("provider:oxy fresh:true"));
        assert_eq!(fix(com, "", Bd, &[Plain, Bd]).as_deref(), Some("country:us fresh:true"));
        assert_eq!(fix(com, "US", Bd, &[Plain, Bd]), None, "one key, US already sent: no fix");
        assert_eq!(fix(com, "US", OxyJs, &[Plain, Oxy, OxyJs]), None);
    }

    #[test]
    fn a_comment_preview_skips_its_byline() {
        // `clean` folds a byline (an author, an age, a badge) into one line: the text starts on the next, short too
        let row = |lines: &[&str]| {
            let s =
                Section { level: COMMENT, title: String::new(), lines: lines.iter().map(|l| l.to_string()).collect() };
            sec_preview(&s, &[], 120)
        };
        assert_eq!(row(&["ann [l5] • 10y ago [l6] Top 1% Commenter", "No.", "A second line."]), "No. A second line.");
        assert_eq!(row(&["ann • 10y ago No."]), "ann • 10y ago No.", "a byline-only comment keeps it");
        let long = "the text of a comment that is a paragraph by itself without any byline above it";
        assert_eq!(row(&[long, "more"]), format!("{long} more"));
    }

    #[test]
    fn preview_and_link_lines() {
        assert_eq!(preview("short  text", 80), "short text");
        assert_eq!(preview(&"word ".repeat(30), 20), "word word word word…");
        assert!(link_line("- Home [l1] | News [l22] [lx]") && !link_line(&format!("- Home [l1] {}", "x".repeat(141))));
        // 9,000 letters = 3,334 tok: parts of 950 tok (2,565 letters), four of them
        let parts = chunks_of(&["z".repeat(9000)]);
        assert_eq!(parts.iter().map(|p| tok(&p[0])).collect::<Vec<_>>(), [950, 950, 950, 484]);
        // a 660k-char JSON island line (a Zillow page) is cut in one pass a part: was minutes, quadratic
        let t = std::time::Instant::now();
        assert_eq!(chunks_of(&["{\"a\":1}".repeat(94_000)]).len(), 518);
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    }
}
