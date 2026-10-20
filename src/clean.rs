//! CLEAN: tree → lines in three prune modes + loss guard → Doc (sections, links, info, loss, tokens).
//! Links resolve with the WHATWG `url` crate.

use crate::html::{Kid, Node, Tree, Visit, py_space, py_strip, unescape};
use crate::settings::limit;
use crate::tables::{COMBINING, FOLD, ND_ZEROS};
use crate::{fetch::title_head, fetch::words_of, outline::sections_of};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, ser::PrettyFormatter};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

/// Doc schema version: a cached doc.json with another `v` is rebuilt from raw.html.
pub const DOC_V: u32 = 16;
/// A comment's section level: `COMMENT` + its depth in the thread (beyond `h6`, so no heading is one).
pub const COMMENT: usize = 7;

/// The renderer's tables (`src/data/clean.json`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields, bound(deserialize = "'de: 'static"))]
pub struct Lists {
    /// Elements that end a line of text.
    block: Vec<&'static str>,
    /// Elements whose text is never page text.
    drop_always: Vec<&'static str>,
    /// Elements and ARIA roles that open a scope (a cookie banner is judged inside its scope only).
    sectioning: Vec<&'static str>,
    sectioning_roles: Vec<&'static str>,
    /// Words in an id or class that mark a cookie or consent banner.
    cookie: Vec<&'static str>,
    /// Path words of links that are site chrome (log in, share, vote), never page links.
    skip_link: Vec<&'static str>,
    /// DETECT by magic bytes (`sniff`): the files this build has no text converter for (pdf, zip = docx/xlsx/pptx).
    magic: Vec<(String, &'static str)>,
    /// Text documents (`kind_of`): served as they are, a line per `lN` row (`text::build`), never cleaned as a
    /// page.
    pub text: Vec<&'static str>,
    /// Media (`body_media`, `add`, `rank`): words in a `data-*` attribute that holds the real source; in a GIF's
    /// name that make it a filler; picture file types; a link folder of a picture's own page (`links`); in an alt,
    /// class or file name that make it not content (or only in a short alt); in a URL of a generated share card;
    /// query keys that track.
    lazy_src: Vec<&'static str>,
    filler: Vec<&'static str>,
    image_ext: Vec<&'static str>,
    picture_page: Vec<&'static str>,
    not_content: Vec<&'static str>,
    not_content_alt: Vec<&'static str>,
    share_card: Vec<&'static str>,
    share_host: Vec<&'static str>,
    tracking: Vec<&'static str>,
    /// Whole words (or their plural) of a tag, class or id that name an ad slot (`ad_slot`).
    ad_slot: Vec<&'static str>,
    /// A thread's stated size (`declared`): a number before a first word (`726 comments`), or in an attribute
    /// whose name holds a first word and a second (`comment-count="301"`; `comment-id` is no size).
    said: [Vec<&'static str>; 2],
    /// `info` keys read from `<meta>` names, the first name present wins (`meta_info`).
    info: Vec<(&'static str, Vec<&'static str>)>,
    /// Currency marks a price number sits next to (`facts`, `Loss::price_pruned`).
    currency: Vec<&'static str>,
}

pub static LISTS: LazyLock<Lists> = LazyLock::new(|| crate::settings::table("clean"));

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Section {
    pub level: usize,
    pub title: String,
    pub lines: Vec<String>,
}

/// A doc.json missing a field (another build's) fails to read and is rebuilt from the raw body.
#[derive(Serialize, Deserialize, Default)]
pub struct Loss {
    pub vs_light_lost: usize,
    pub light_facts: usize,
    /// Body chars the chrome prune keeps (the served doc's, or the last prune's when light is served):
    /// the content check's "cleans to almost nothing" reads this, never the light fallback.
    pub pruned_chars: usize,
    /// Text chars in the bytes: the light render's, `<template>` contents and JSON-island prose (`inert_chars`);
    /// `outline::thin` compares the served text with it.
    pub visible_chars: usize,
    /// Of `vs_light_lost`, the numbers next to a currency mark (`fats stats` `price_nums_pruned`): measured
    /// before any rule acts on it.
    pub price_pruned: usize,
}

/// One picture or video of a page (`media`): its URL, alt text (a JSON-LD caption), declared size, file type.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
pub struct Media {
    pub src: String,
    pub alt: String,
    /// og:video, or a `<video>`'s poster; else a photo.
    pub video: bool,
    /// As the page declares it (attributes, og, JSON-LD), 0 = not declared: a size in the file name only filters
    /// (`add`), it would print as if declared (`m1 photo 1287x836`).
    pub w: u32,
    pub h: u32,
    /// `webp`: the declared type's subtype, else the URL's extension; "" = unknown.
    pub ty: String,
    /// Not content (`rank`): an avatar, a bare `@handle` alt, or the author's or site's name as alt; listed last,
    /// never counted (`outline::photos`).
    pub demoted: bool,
    /// The picture's identity and largest side while the page is built (`add`).
    #[serde(skip)]
    key: (String, u32),
    /// Links to its own page or file (`links`): a gallery's picture, never an avatar.
    #[serde(skip)]
    linked: bool,
}

/// A page's `info`, in document order: `<meta>` first, then JSON-LD; a key set again keeps its first place.
pub type Info = IndexMap<String, String>;

#[derive(Serialize, Deserialize, Default)]
pub struct Tokens {
    pub clean: usize,
}

/// As `Loss`: a doc.json missing a field is rebuilt; one with another `v` or `chunk` too.
#[derive(Serialize, Deserialize, Default)]
pub struct Doc {
    pub v: u32,
    /// Block size used for heading-less text (a doc built with another size is rebuilt).
    pub chunk: usize,
    pub url: String,
    pub final_url: String,
    pub title: String,
    pub info: Info,
    pub sections: Vec<Section>,
    pub top: usize,
    pub links: Vec<String>,
    /// Each link's longest anchor text (`fats uN links <words>` matches it).
    pub anchors: Vec<String>,
    pub mode: String,
    pub loss: Loss,
    pub tokens: Tokens,
    /// Left unread when the time budget ran out (0 = all of it): bytes of an HTML body, pages of a PDF.
    pub cut: usize,
    /// The last `islands` sections are islands: inert blocks in the bytes, opened on demand (`clean::islands`).
    pub islands: usize,
    /// Pictures and videos, best first (`clean::rank`).
    pub media: Vec<Media>,
}

impl Doc {
    /// A Doc carrying the schema version, the settings chunk, the URLs, the title, the clean-token count of
    /// `sections` and the given mode; `top` = 1 and everything else defaults. The three builders
    /// (`build_doc`, `text::build`, `pdf::build_pdf`) add the parts they alone have (info, loss, cut, links).
    pub fn new(url: &str, final_url: &str, title: String, sections: Vec<Section>, mode: &str) -> Doc {
        Doc {
            v: DOC_V,
            chunk: crate::settings::get().chunk.value,
            url: url.into(),
            final_url: final_url.into(),
            title,
            tokens: Tokens { clean: sections.iter().flat_map(|s| s.lines.iter()).map(|t| tok(t)).sum() },
            sections,
            top: 1,
            mode: mode.into(),
            ..Doc::default()
        }
    }

    /// Sections of the page as served, before its islands.
    pub fn served(&self) -> usize {
        self.sections.len().saturating_sub(self.islands)
    }
}

/// THE token figure (the `=` line's `Ntok`, every read and list cap): real tokens of `s` as printed, estimated as
/// ASCII letters / 2.7 + other non-space chars / 0.8, an ASCII punctuation mark after another one as a letter (JSON's
/// `":"`, `},{"`). Fitted on count_tokens of 42 printed reads (Claude 5.5 models; within ±10% on 37, worst 18%;
/// older tokenizers count ~1.2× fewer, o200k ~1.4×).
pub fn tok(s: &str) -> usize {
    weights(s).sum::<usize>().div_ceil(216)
}

/// The byte length of the longest start of `s` within `cap` tok, in one pass: where a huge line is cut.
pub fn tok_cut(s: &str, cap: usize) -> usize {
    let mut w = 0;
    s.char_indices().zip(weights(s)).find(|&(_, c)| (w += c, w > cap * 216).1).map_or(s.len(), |((i, _), _)| i)
}

/// Each char's share of a token, in 216ths (`tok`).
fn weights(s: &str) -> impl Iterator<Item = usize> + '_ {
    let mark = |c: char| c.is_ascii_punctuation();
    s.chars().scan(' ', move |prev, c| {
        let letter = c.is_ascii_alphabetic() || mark(*prev) && mark(c);
        *prev = c;
        Some(usize::from(!py_space(c)) * if letter { 80 } else { 270 })
    })
}

/// `re.sub(r"\s+", " ", s).strip()`
pub fn collapse(s: &str) -> String {
    s.split(py_space).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// A combining mark (`COMBINING`: sorted, disjoint code point ranges).
fn combining(cp: u32) -> bool {
    let at = COMBINING.partition_point(|&(_, hi)| hi < cp);
    COMBINING.get(at).is_some_and(|&(lo, _)| lo <= cp)
}

/// NFKD, drop combining marks, lowercase (tables.rs holds the decompositions).
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        if cp < 0x80 {
            out.push(c);
        } else if (0xAC00..=0xD7A3).contains(&cp) {
            // Hangul syllable → L V (T) jamo (Unicode §3.12)
            let syl = cp - 0xAC00;
            let jamo = [0x1100 + syl / 588, 0x1161 + (syl % 588) / 28, 0x11A7 + syl % 28];
            let len = if syl.is_multiple_of(28) { 2 } else { 3 };
            out.extend(jamo[..len].iter().filter_map(|&j| char::from_u32(j)));
        } else if !combining(cp) {
            let at = FOLD.binary_search_by_key(&cp, |&(k, _)| k);
            if let Ok(at) = at { out.push_str(FOLD[at].1) } else { out.push(c) }
        }
    }
    out.to_lowercase()
}

/// Bytes → UTF-8 in the WHATWG order: a BOM, the transport's charset (`Content-Type`), a `<meta>` charset in the
/// first 1024 bytes, else UTF-8. Returns the text and the encoding's name for meta.json (`windows-1250`; "" = UTF-8).
/// Valid UTF-8 comes back as it went in (no copy); invalid bytes become U+FFFD later, never an error.
pub fn decode(raw: Vec<u8>, header: Option<&str>) -> (Vec<u8>, &'static str) {
    use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252, X_USER_DEFINED};
    let meta = || match prescan(&raw[..raw.len().min(1024)]).and_then(Encoding::for_label) {
        // a page cannot declare UTF-16 in ASCII-compatible bytes; x-user-defined is a legacy alias (spec 13.2.3.3)
        Some(e) if e == UTF_16BE || e == UTF_16LE => Some(UTF_8),
        Some(e) if e == X_USER_DEFINED => Some(WINDOWS_1252),
        e => e,
    };
    let bom = Encoding::for_bom(&raw).map(|b| b.0);
    let sent = header.and_then(|h| Encoding::for_label(h.as_bytes()));
    let enc = bom.or(sent).or_else(meta).unwrap_or(UTF_8);
    let name = if enc == UTF_8 { "" } else { enc.name() };
    if enc == UTF_8 && !raw.starts_with(b"\xEF\xBB\xBF") {
        return (raw, name);
    }
    (enc.decode_with_bom_removal(&raw).0.into_owned().into_bytes(), name)
}

/// The label of the first `<meta charset=…>` or `<meta http-equiv … content="…; charset=…">` (ASCII only).
fn prescan(head: &[u8]) -> Option<&[u8]> {
    let low = head.to_ascii_lowercase();
    let mut at = 0;
    while let Some(i) = find(&low[at..], b"<meta").map(|i| i + at) {
        let end = find(&low[i..], b">").map_or(low.len(), |e| i + e);
        at = end;
        let Some(c) = find(&low[i..end], b"charset").map(|c| i + c + 7) else { continue };
        let rest = head[c..end].trim_ascii_start().strip_prefix(b"=").map(<[u8]>::trim_ascii_start);
        let Some(v) = rest.map(|r| r.strip_prefix(b"\"").or(r.strip_prefix(b"'")).unwrap_or(r)) else { continue };
        let n = v.iter().position(|b| b" \"';/>\t\n\r".contains(b)).unwrap_or(v.len());
        if n > 0 {
            return Some(&v[..n]);
        }
    }
    None
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// DETECT, THE rule: what a body is, given its content type ("" when none came: a provider's answer, a local
/// file). Magic bytes first (`sniff`), then a type that names a PDF or a zip; a body that opens like an HTML page
/// (`<!doctype html` or `<html` in its first 1 KB) or a type that says HTML is `html`. Then the text documents:
/// a JSON type, or a body that parses as JSON, is `json`; an XML type (RSS, Atom), or a body that opens with
/// `<?xml`, `<rss`, `<feed`, `<urlset`, `<sitemapindex`, is `xml`; a `text/markdown` type is `md` (a server's own
/// conversion of its page, `text::markdown`); any other `text/*` type (plain, CSV) is `text`. Anything else is `html`,
/// the page path.
pub fn kind_of(ctype: &str, body: &[u8]) -> &'static str {
    let ct = ctype.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    let head = body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body).trim_ascii_start();
    let low = head[..head.len().min(1024)].to_ascii_lowercase();
    let opens = |tags: &[&[u8]]| tags.iter().any(|t| low.starts_with(t));
    let bare = ct.is_empty() || ct == "text/plain";
    match () {
        _ if let Some(k) = sniff(body) => k,
        _ if ct.contains("application/pdf") => "pdf",
        _ if ct.contains("application/zip") || ct.contains("officedocument") => "zip",
        _ if ct.contains("html") || opens(&[b"<!doctype html"]) || find(&low, b"<html").is_some() => "html",
        _ if ct.contains("json") => "json",
        _ if bare && opens(&[b"{", b"["]) && serde_json::from_slice::<serde::de::IgnoredAny>(body).is_ok() => "json",
        _ if ct.contains("xml") || bare && opens(&[b"<?xml", b"<rss", b"<feed", b"<urlset", b"<sitemapindex"]) => "xml",
        _ if ct == "text/markdown" => "md",
        _ if ct.starts_with("text/") => "text",
        _ => "html",
    }
}

/// DETECT: a body this build has no text converter for (`pdf`, `zip` = docx/xlsx/pptx), by magic bytes.
pub fn sniff(raw: &[u8]) -> Option<&'static str> {
    LISTS.magic.iter().find(|m| raw.starts_with(m.0.as_bytes())).map(|m| m.1)
}

/// Regex `\d` on str = Unicode Nd.
pub fn is_digit(c: char) -> bool {
    let u = c as u32;
    c.is_ascii_digit() || (u >= 0x660 && ND_ZEROS.iter().any(|&z| u >= z && u < z + 10))
}

/// Regex `\w` on str.
pub fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A card row's mark (`Render::card`): the line is one card of a list.
pub const CARD: &str = "▸ ";
/// Between a card's pieces (`card`); not the page's text (`outline::is_thin`).
pub const PIECE: &str = " · ";
/// Our `[lN]` link ids in a line.
pub static LREF: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\[l[0-9]+\]").expect("a pattern"));

#[derive(Clone)]
pub enum Line {
    H(usize, String),
    T(String),
}

impl Line {
    pub fn text(&self) -> &str {
        match self {
            Line::H(_, t) | Line::T(t) => t,
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Light,
    Hard,
    Clean,
}

struct Render<'a> {
    tree: &'a Tree,
    tlen: &'a [usize],
    /// The page URL without its fragment: the base of every link, and the link a page makes to itself.
    base_url: Option<url::Url>,
    mode: Mode,
    js: bool,
    lines: Vec<Line>,
    /// Text pieces (Python list semantics: `a` checks the pieces added after its start index).
    buf: String,
    marks: Vec<usize>,
    /// Link URLs in first-seen order (`[lN]` = position + 1) → their longest anchor text (a card's link around its
    /// title, never a teaser link to the same page before it: `card_row`).
    links: IndexMap<String, String>,
    inline: usize,
    /// The `<img>` and `<video>` elements the render kept (`body_media`), in document order.
    media: Vec<usize>,
    /// A nested thread's comments → their depth (`thread`).
    thread: HashMap<usize, usize>,
    /// A comment's head line while its byline runs: the line its `<time>` lands on ends it (`flush`).
    by: Option<(usize, usize)>,
}

/// Text chars under `id` that a page shows; a `<template>` shows none, its contents (the fragment after it) are
/// measured for `islands`.
fn measure(t: &Tree, id: usize, tlen: &mut [usize]) -> usize {
    let mut s = 0;
    for k in &t.nodes[id].kids {
        match k {
            Kid::Text(x) => s += py_strip(x).chars().count(),
            Kid::Node(c) if t.nodes[*c].tag == "template" => tlen[*c] = measure(t, c + 1, tlen),
            Kid::Node(c) if !LISTS.drop_always.contains(&t.nodes[*c].tag.as_str()) => s += measure(t, *c, tlen),
            Kid::Node(_) => {}
        }
    }
    tlen[id] = s;
    s
}

/// `aria-hidden` hides from screen readers only: its text is on screen (star counts, ratings) and stays; an icon
/// glyph (`text` ≤ 1 char) under it is still dropped.
fn hidden(n: &Node, text: usize) -> bool {
    if n.attr("hidden").is_some() || n.attr("aria-hidden") == Some("true") && text <= 1 {
        return true;
    }
    let style = n.attr("style").unwrap_or("");
    // display\s*:\s*none | visibility\s*:\s*hidden (case-sensitive)
    [("display", "none"), ("visibility", "hidden")].iter().any(|(k, v)| {
        style.match_indices(k).any(|(i, _)| {
            let r = style[i + k.len()..].trim_start_matches(py_space);
            r.strip_prefix(':').is_some_and(|r| r.trim_start_matches(py_space).starts_with(v))
        })
    })
}

impl Render<'_> {
    fn chrome(&self, id: usize, scoped: bool) -> bool {
        let n = &self.tree.nodes[id];
        if self.tlen[id] as f64 > 0.5 * self.tlen[0].max(1) as f64 {
            return false;
        }
        let (role, tag) = (n.attr("role").unwrap_or("").to_lowercase(), n.tag.as_str());
        let nav = matches!(role.as_str(), "navigation" | "banner" | "contentinfo");
        if nav || matches!(tag, "nav" | "aside" | "form") || (!scoped && matches!(tag, "header" | "footer")) {
            return true;
        }
        let cid = format!("{} {}", n.attr("id").unwrap_or(""), n.attr("class").unwrap_or("")).to_lowercase();
        LISTS.cookie.iter().any(|w| cid.contains(w))
            || self.mode == Mode::Clean && (tag == "dialog" || role.ends_with("dialog") || hidden(n, self.tlen[id]))
    }

    /// A nested thread's comments → their depth, by structure. A block = `li`, `article` or a custom element (a `-`
    /// in its tag). The comments' tag = the commonest tag of the blocks that are the nearest block of a paragraph (a
    /// text of their own) above a served `<time>` (a sidebar card, the post: another tag). A comment = the nearest
    /// ancestor of that tag of a served `<time>`, a short one too; one inside a list item of another tag is that item
    /// (the replies beside it: `li > article + ol > li`); the top ones share their parent. Its depth = the comments
    /// around it, or a spacer row's (below), clamped at `thread_depth` when rendered (`walk`).
    fn comments(&self) -> HashMap<usize, usize> {
        let t = self.tree;
        let block = |i: &usize| matches!(t.nodes[*i].tag.as_str(), "li" | "article") || t.nodes[*i].tag.contains('-');
        let says = |i: &usize| t.nodes[*i].kids.iter().any(|k| matches!(k, Kid::Text(s) if paragraph(s)));
        let said: HashSet<usize> = (0..t.nodes.len()).filter(says).filter_map(|i| up(t, i).find(block)).collect();
        // served: not in a template, nor in what the render prunes (a sidebar's cards are no thread's tops)
        let pruned = |a: usize| self.mode != Mode::Light && self.chrome(a, true);
        let served = |i: &usize| up(t, *i).last() == Some(0) && !up(t, *i).any(pruned);
        let dated: Vec<usize> = (0..t.nodes.len()).filter(|i| t.nodes[*i].tag == "time" && served(i)).collect();
        let tags = dated.iter().filter_map(|&i| up(t, i).find(|b| said.contains(b))).map(|b| t.nodes[b].tag.as_str());
        let tag = commonest(tags.collect());
        let item = |c: usize| t.nodes[c].parent.filter(|&p| t.nodes[p].tag == "li" && tag != Some("li")).unwrap_or(c);
        let of = |i: &usize| up(t, *i).find(|&b| Some(t.nodes[b].tag.as_str()) == tag).map(item);
        let comments: HashSet<usize> = dated.iter().filter_map(of).collect();
        let mut depths: HashMap<usize, usize> =
            comments.iter().map(|&c| (c, up(t, c).skip(1).filter(|a| comments.contains(a)).count())).collect();
        // the top comments share their parent (a story, a sidebar card: alone elsewhere)
        let list = commonest(depths.iter().filter(|e| *e.1 == 0).map(|e| t.nodes[*e.0].parent).collect()).flatten();
        depths.retain(|&c, d| *d > 0 || t.nodes[c].parent == list);
        // the old way to indent a reply: a text-less spacer 1 px high in its row (`<img height=1 width=40>`); the
        // row's depth = the rank of its width; a row's first spacer is its indent (a second stands in for a vote arrow)
        let width =
            |n: &Node| n.attr("width").filter(|_| n.kids.is_empty() && n.attr("height") == Some("1"))?.parse().ok();
        let in_row = |i: usize| up(t, i).find(|&r| t.nodes[r].tag == "tr");
        let row = |i: usize| width(&t.nodes[i]).and_then(|w: usize| Some((in_row(i)?, w)));
        let mut rows: Vec<(usize, usize)> = (0..t.nodes.len()).filter_map(row).filter(|r| served(&r.0)).collect();
        rows.dedup_by_key(|r| r.0);
        let steps: Vec<usize> = rows.iter().map(|r| r.1).collect::<BTreeSet<_>>().into_iter().collect();
        // spacers are layout too (a gutter, a TOC or file tree indent): a reply ladder starts at 0 (a gutter never
        // does) and ≥ `thread_rows` of its rows say something untitled (a TOC entry, a file name: no paragraph; a
        // newsletter's story: its own heading)
        let prose: HashSet<usize> = (0..t.nodes.len()).filter(says).filter_map(in_row).collect();
        let head = |v: Visit| matches!(v, Visit::Node(n) if matches!(n.tag.as_bytes(), [b'h', b'1'..=b'6']));
        let post = |r: &&(usize, usize)| prose.contains(&r.0) && !t.walk(r.0, &[]).any(head);
        let talk = steps.first() == Some(&0) && rows.iter().filter(post).count() >= limit("thread_rows");
        depths.extend(rows.iter().filter(|_| talk).map(|&(r, w)| (r, steps.partition_point(|&s| s < w))));
        // a thread: ≥ `thread_rows` comments, ≥ 2 at the top and one deeper (dated flat posts are a feed)
        let tops = depths.values().filter(|&&d| d == 0).count();
        if depths.len() >= limit("thread_rows") && (2..depths.len()).contains(&tops) { depths } else { HashMap::new() }
    }

    /// The thread's size as the page states it (the story's own line, its `<meta>`, a count attribute): the count
    /// nearest the thread before it, shown, never in a list item (a related post's, a sidebar card's) nor on an
    /// element around the thread (its text may follow it); its scope = the deepest of those elements at or before
    /// it, and two counts in that scope claim none.
    fn declared(&self) -> Option<usize> {
        let (t, path) = (self.tree, up(self.tree, *self.thread.keys().min()?).collect::<Vec<_>>());
        let [words, counts] = &LISTS.said;
        let said = |s: &str| match s.split_whitespace().collect::<Vec<_>>()[..] {
            [n, w] if words.contains(&w.to_lowercase().as_str()) => n.replace(',', "").parse().ok(),
            _ => None,
        };
        let has = |list: &[&str], k: &str| list.iter().any(|w| k.contains(w));
        let named = |k: &str, v: &str| (has(words, k) && has(counts, k)).then(|| v.parse().ok()).flatten();
        let attr = |(k, v): &(String, String)| said(v).or_else(|| named(k, v));
        let text = |k: &Kid| if let Kid::Text(s) = k { said(s) } else { None };
        let (p, off) = (&path, |x: usize| t.nodes[x].tag == "li" || self.chrome(x, true));
        let shown = |i: &usize| !p.contains(i) && !up(t, *i).take_while(|a| !p.contains(a)).any(off);
        let found = |i: usize| t.nodes[i].attrs.iter().filter_map(attr).chain(t.nodes[i].kids.iter().filter_map(text));
        let mut near =
            (0..path[0]).rev().filter(shown).flat_map(|i| found(i).map(move |n| (p.iter().find(|&&a| a <= i), n)));
        let (at, n) = near.next()?;
        near.take_while(|f| f.0 == at).all(|f| f.1 == n).then_some(n)
    }

    /// Two numbers in two elements stay two: a space between a digit and a digit at an element boundary
    /// (`$109.72` `0.24%`, never `$109.720.24%`).
    fn push(&mut self, s: &str) {
        let gap = self.buf.ends_with(is_digit) && s.starts_with(is_digit);
        self.buf.push_str(if gap { " " } else { "" });
        self.marks.push(self.buf.len());
        self.buf.push_str(s);
    }

    fn flush(&mut self) {
        if self.inline > 0 {
            self.push(" ");
            return;
        }
        let t = collapse(&self.buf);
        self.buf.clear();
        self.marks.clear();
        self.lines.extend((!t.is_empty() && t != "-" && t != "|" && t != "- |").then_some(Line::T(t)));
    }

    fn link(&mut self, href: &str, text: &str) -> Option<usize> {
        let href = py_strip(href);
        if href.is_empty() || ["#", "javascript:", "mailto:", "tel:", "data:"].iter().any(|p| href.starts_with(p)) {
            return None;
        }
        let mut j = url::Url::options().base_url(self.base_url.as_ref()).parse(href).ok()?;
        j.set_fragment(None);
        let u = String::from(j);
        if !u.starts_with("http") || self.base_url.as_ref().is_some_and(|b| b.as_str() == u) || skip_link(&u) {
            return None;
        }
        let (i, a) = self.links.insert_full(u, collapse(text));
        if let Some(a) = a.filter(|a| a.len() > text.len()) {
            self.links[i] = a; // keep the longest
        }
        Some(i + 1)
    }

    fn text_of(&mut self, id: usize, scoped: bool) -> String {
        let save = (std::mem::take(&mut self.buf), std::mem::take(&mut self.marks));
        self.inline += 1;
        self.kids(id, scoped);
        self.inline -= 1;
        let t = collapse(&self.buf);
        (self.buf, self.marks) = save;
        t
    }

    fn kids(&mut self, id: usize, scoped: bool) {
        let kids = &self.tree.nodes[id].kids;
        let cards = if self.inline == 0 { self.groups(id) } else { HashSet::new() };
        let mut spans = vec![];
        for k in kids {
            match k {
                Kid::Text(s) => self.push(s),
                Kid::Node(c) if cards.contains(c) => {
                    self.flush();
                    let l0 = self.lines.len();
                    self.walk(*c, scoped);
                    self.flush();
                    spans.push(l0..self.lines.len());
                }
                Kid::Node(c) => self.walk(*c, scoped),
            }
        }
        self.cards(&spans);
    }

    /// The kids of `id` that are cards: a group of ≥ `card_repeats` siblings of one shape (tag and first class),
    /// ≥ 60% of the block kids that show text, four in five of them holding no element of their own shape (a thread's
    /// comment holds its replies), no code and no table (a shape, never a size; `cards` judges the rendered rows).
    fn groups(&self, id: usize) -> HashSet<usize> {
        let t = self.tree;
        let kids: Vec<usize> = t.nodes[id].kids.iter().filter_map(Kid::node).collect();
        let mut shapes: IndexMap<(&str, &str), Vec<usize>> = IndexMap::new();
        // a block or a custom element (`<x-item>`), never a table row (`row`) or inline markup (citations)
        let tag = |c: usize| t.nodes[c].tag.as_str();
        let boxed = |c: usize| tag(c) != "tr" && (LISTS.block.contains(&tag(c)) || tag(c).contains('-'));
        for &c in kids.iter().filter(|&&c| boxed(c)) {
            shapes.entry(shape(&t.nodes[c])).or_default().push(c);
        }
        // its own shape inside = a thread (replies); code or a table inside keeps its lines; a byline first = a comment
        let own = |c: usize| held(t, c, &|n| shape(n) == shape(&t.nodes[c]) || n.tag == "pre" || n.tag == "table");
        let card = |c: &&usize| !own(**c) && !byline(t, self.tlen, **c) && !self.thread.contains_key(*c);
        // one group decides for all its members: never half a list as cards, half as sections
        let shown = kids.iter().filter(|&&c| boxed(c) && self.tlen[c] > 0).count();
        let most = |v: &Vec<usize>| v.len() >= limit("card_repeats") && 5 * v.len() >= 3 * shown;
        let cards = shapes.into_values().filter(|v| most(v) && 5 * v.iter().filter(card).count() >= 4 * v.len());
        cards.flatten().collect()
    }

    /// A card group (`groups`), each member's lines at `spans`: every member ONE row (`card_row`) when four in five
    /// are one item, else none (never half a list as cards, half as sections): a filter hit inside it is the card.
    /// The row takes its member's first line, the rest are emptied (no rendered text line is empty: `flush`; a
    /// comment's heading is untitled, `COMMENT`, and stays), and the
    /// group's lines are rebuilt once (a splice per member moved every later line: O(n²) on 100k cards).
    /// A link in half the members or more (a "learn more" every card shares) names none of them (`card_row`).
    fn cards(&mut self, spans: &[std::ops::Range<usize>]) {
        let ids: Vec<Vec<(usize, usize)>> = spans.iter().map(|s| link_ids(&self.lines[s.clone()])).collect();
        let mut uses: IndexMap<usize, usize> = IndexMap::new();
        for i in ids.iter().flat_map(|v| v.iter().map(|i| i.1).collect::<HashSet<_>>()) {
            *uses.entry(i).or_default() += 1;
        }
        let own = |v: Vec<(usize, usize)>| v.into_iter().filter(|i| 2 * uses[&i.1] < spans.len()).collect::<Vec<_>>();
        let rows: Vec<_> = spans.iter().zip(ids).map(|(s, v)| self.card_row(&self.lines[s.clone()], &own(v))).collect();
        let ok = 5 * rows.iter().flatten().filter(|r| r.1).count() >= 4 * rows.len();
        for (s, (row, _)) in spans.iter().zip(rows).filter_map(|(s, r)| Some((s, r?))).filter(|_| ok) {
            self.lines[s.start] = Line::T(row);
            self.lines[s.start + 1..s.end].fill(Line::T(String::new()));
        }
        let tail: Vec<Line> = self.lines.drain(spans.first().map_or(self.lines.len(), |s| s.start)..).collect();
        self.lines.extend(tail.into_iter().filter(|l| !matches!(l, Line::T(t) if t.is_empty())));
    }

    /// `▸ title [lN] · the other pieces in page order`, and whether it is one item: its heading is a link's
    /// (`anchor` puts a link around a heading on it), else its main link (the one around the most text) is a title
    /// of ≥ 2 words (never a name: forum posts), and that link holds ≥ 2/5 of its links' text (else a box of links:
    /// filters, a menu, more stories). `ids` = its own links (`link_ids`), never one the group shares. The title =
    /// the heading, else the line holding the main link, else the first. Pieces are never reordered (a post's prose
    /// stays in its order); only a repeat of the title drops. One line (a paragraph, a reference) is already a row;
    /// one holding cards is their list.
    fn card_row(&self, lines: &[Line], ids: &[(usize, usize)]) -> Option<(String, bool)> {
        if lines.len() < 2 || lines.iter().any(|l| l.text().starts_with(CARD)) {
            return None;
        }
        let anchor = |i: usize| self.links.get_index(i - 1).map_or("", |(_, a)| a.as_str());
        let head = lines.iter().position(|l| matches!(l, Line::H(..)));
        // the heading's link, else the one around the most text
        let key = |i: &&(usize, usize)| (anchor(i.1).len(), lines[i.0].text().len()); // then its longest line
        let main = ids.iter().rev().filter(|i| head.is_none_or(|h| i.0 == h)).max_by_key(key);
        let all: usize = ids.iter().map(|i| i.1).collect::<HashSet<_>>().into_iter().map(|i| anchor(i).len()).sum();
        // titled: the link holds most of the heading (never a type named inside it), else ≥ 2 words (never a name)
        let titled = |m: usize| {
            head.map_or(anchor(m).split_whitespace().nth(1).is_some(), |h| 2 * anchor(m).len() >= lines[h].text().len())
        };
        // a second heading = an article (a post, a review, a version's notes), never one item
        let article = lines.iter().filter(|l| matches!(l, Line::H(..))).nth(1).is_some();
        let one = !article && main.is_some_and(|&(_, m)| titled(m) && 5 * anchor(m).len() >= 2 * all);
        let mark = main.map_or(String::new(), |i| format!("[l{}]", i.1));
        let mut ps: Vec<String> =
            lines.iter().map(|l| collapse(&l.text().trim_start_matches("- ").replace(&mark, ""))).collect();
        let title = ps.remove(head.or(main.map(|i| i.0)).unwrap_or(0));
        ps.retain(|p| !p.is_empty() && *p != title); // the title's alt or aria repeat; the rest in page order
        let title = collapse(&format!("{title} {mark}"));
        Some((format!("{CARD}{}", [vec![title], ps].concat().join(PIECE)), one))
    }

    fn walk(&mut self, id: usize, scoped: bool) {
        let tree = self.tree;
        let n = &tree.nodes[id];
        let tag = n.tag.as_str();
        if LISTS.drop_always.contains(&tag) || (tag == "noscript" && self.js) {
            return;
        }
        if self.mode != Mode::Light && self.chrome(id, scoped) {
            return;
        }
        self.media.extend(matches!(tag, "img" | "video").then_some(id));
        let role = n.attr("role").unwrap_or("").to_lowercase();
        let scoped = scoped || LISTS.sectioning.contains(&tag) || LISTS.sectioning_roles.contains(&role.as_str());
        let top = self.inline == 0;
        // a comment opens a section at `COMMENT` + its depth, untitled: its first line follows (`thread`)
        if let Some(&depth) = self.thread.get(&id).filter(|_| top) {
            self.flush();
            self.lines.push(Line::H(COMMENT + depth.min(limit("thread_depth")), String::new()));
        }
        // a heading inside a comment is its text: it never ends the comment's replies
        let quoted = || up(tree, id).any(|a| self.thread.contains_key(&a));
        // a comment's byline = the widest element around its `<time>` in it of ≤ `byline_chars` (author, age, a
        // badge), one line: its text starts on the next (`outline::sec_preview`), a short `No.` too
        if tag == "time" && quoted() {
            let short = |x: &usize| !self.thread.contains_key(x) && self.tlen[*x] <= limit("byline_chars");
            let head = self.lines.iter().rposition(|l| matches!(l, Line::H(lv, _) if *lv >= COMMENT));
            self.by = head.zip(up(tree, id).take_while(short).last());
        }
        match tag.as_bytes() {
            [b'h', d @ b'1'..=b'6'] if top && !quoted() => self.heading(id, scoped, usize::from(d - b'0')),
            b"tr" if top => self.row(id, scoped),
            b"pre" if top => self.pre(id),
            _ => self.inline_or_block(id, scoped, top),
        }
        if let Some((h, _)) = self.by.take_if(|b| b.1 == id).filter(|_| self.inline == 0) {
            self.flush();
            if self.lines[h + 1..].iter().all(|l| !paragraph(l.text())) && self.lines.len() > h + 2 {
                let by: Vec<String> = self.lines.drain(h + 1..).map(|l| l.text().to_string()).collect();
                self.lines.push(Line::T(by.join(" ")));
            }
        }
    }

    fn heading(&mut self, id: usize, scoped: bool, level: usize) {
        self.flush();
        let t = self.text_of(id, scoped);
        self.lines.extend((!t.is_empty()).then_some(Line::H(level, t)));
    }

    /// A table row: `| cell | cell |`. A layout row (a cell holds a table, shown) is its cells' blocks.
    fn row(&mut self, id: usize, scoped: bool) {
        self.flush();
        let tree = self.tree;
        if held(tree, id, &|n| n.tag == "table") {
            return self.kids(id, scoped);
        }
        let td = |&c: &usize| matches!(tree.nodes[c].tag.as_str(), "td" | "th");
        let td = tree.nodes[id].kids.iter().filter_map(Kid::node).filter(td);
        let cells: Vec<String> = td.map(|c| self.text_of(c, scoped)).collect();
        self.lines.extend(cells.iter().any(|c| !c.is_empty()).then(|| Line::T(format!("| {} |", cells.join(" | ")))));
    }

    /// Preformatted text: its own lines, trailing space dropped.
    fn pre(&mut self, id: usize) {
        self.flush();
        let t: String = self.tree.texts(id, &LISTS.drop_always).collect();
        let text = t.replace("\r\n", "\n");
        let kept = text.split(PY_NEWLINE).filter(|ln| !py_strip(ln).is_empty());
        self.lines.extend(kept.map(|ln| Line::T(ln.trim_end_matches(py_space).to_string())));
    }

    /// A link's text, then its `[lN]` when it has text and an href worth listing. A link around blocks (a card's
    /// heading or whole body) flushed them as lines: their text is its text, its id goes on their heading, else on
    /// the longest that is no paragraph (a title, never a teaser or a button), else the first.
    fn anchor(&mut self, id: usize, scoped: bool) {
        let blocks = self.inline == 0 && held(self.tree, id, &|n| LISTS.block.contains(&n.tag.as_str()));
        if blocks {
            self.flush();
        }
        let (b0, l0) = (self.marks.len(), self.lines.len());
        self.kids(id, scoped);
        if blocks {
            self.flush();
        }
        let tail = self.marks.get(b0).map_or(String::new(), |&m| self.buf[m..].to_string());
        let text = self.lines[l0..].iter().map(Line::text).chain([tail.as_str()]).collect::<Vec<_>>().join(" ");
        if py_strip(&text).is_empty() {
            return;
        }
        let Some(i) = self.link(self.tree.nodes[id].attr("href").unwrap_or(""), &text) else { return };
        if !blocks {
            return self.push(&format!(" [l{i}] "));
        }
        let ls = self.lines[l0..].iter().enumerate();
        let title = ls.clone().filter(|l| !paragraph(l.1.text())).max_by_key(|l| l.1.text().chars().count());
        let at = ls.clone().find(|l| matches!(l.1, Line::H(..))).or(title).map_or(0, |l| l.0);
        let (Line::T(t) | Line::H(_, t)) = &mut self.lines[l0 + at];
        t.push_str(&format!(" [l{i}]"));
    }

    fn inline_or_block(&mut self, id: usize, scoped: bool, top: bool) {
        let tree = self.tree;
        let n = &tree.nodes[id];
        let tag = n.tag.as_str();
        match tag {
            "br" => self.flush(),
            // digits-only <sup> shows as superscript: glued, `419<sup>00</sup> €` read as 41900 €
            "sup" if let Some(t) = sup_digits(n) => self.push(&t),
            "a" => self.anchor(id, scoped),
            "dt" if top => {
                self.flush();
                let t = self.text_of(id, scoped);
                if !t.is_empty() {
                    self.push(&format!("{t}: "));
                }
            }
            "dd" | "li" if top => {
                if tag == "li" {
                    self.flush();
                    self.push("- ");
                }
                self.kids(id, scoped);
                self.flush();
            }
            _ => {
                let block = LISTS.block.contains(&tag);
                if block {
                    self.flush();
                }
                self.kids(id, scoped);
                if block {
                    self.flush();
                } else if (tag == "span" || tag == "label") && !top {
                    self.push(" ");
                }
            }
        }
    }
}

/// The `[lN]` link ids in `lines`: (line index, N).
fn link_ids(lines: &[Line]) -> Vec<(usize, usize)> {
    let id = |(k, m): (usize, regex::Match)| Some((k, m.as_str()[2..m.len() - 1].parse::<usize>().ok()?));
    let ids = lines.iter().enumerate().flat_map(|(k, l)| LREF.find_iter(l.text()).map(move |m| (k, m)));
    ids.filter_map(id).collect()
}

/// A member that opens with a byline (its first line: the first block, else the inline run before one, holding
/// no block, so one line, and a `<time>`) and holds no heading: a comment, an author and its age, never a card
/// (a dated post's title is a heading; a link around the whole card is no line).
fn byline(t: &Tree, tlen: &[usize], id: usize) -> bool {
    let block = |n: &Node| LISTS.block.contains(&n.tag.as_str());
    let (line, time) = (|k: &usize| !held(t, *k, &block), |k: usize| held(t, k, &|n| n.tag == "time"));
    let mut shown = t.nodes[id].kids.iter().filter_map(Kid::node).filter(|&k| tlen[k] > 0).peekable();
    let first = shown.next_if(|&k| block(&t.nodes[k])).map(|k| vec![k]);
    let first = first.unwrap_or_else(|| shown.take_while(|&k| !block(&t.nodes[k]) && line(&k)).collect());
    let head = held(t, id, &|n| matches!(n.tag.as_bytes(), [b'h', b'1'..=b'6']));
    !head && first.iter().all(line) && first.into_iter().any(|k| t.nodes[k].tag == "time" || time(k))
}

/// An element's shape for `groups`: its tag and first class.
fn shape(n: &Node) -> (&str, &str) {
    (n.tag.as_str(), n.attr("class").and_then(|c| c.split_whitespace().next()).unwrap_or(""))
}

/// Some element below `id` (never `id` itself) is `f`.
fn held(t: &Tree, id: usize, f: &impl Fn(&Node) -> bool) -> bool {
    t.walk(id, &LISTS.drop_always).skip(1).any(|v| matches!(v, Visit::Node(n) if f(n)))
}

/// A `<sup>` holding only text of ASCII digits, as Unicode superscript digits.
fn sup_digits(n: &Node) -> Option<String> {
    let t = n.kids.iter().map(|k| if let Kid::Text(s) = k { Some(s.as_str()) } else { None });
    let t: String = t.collect::<Option<_>>()?;
    const SUP: [char; 10] = ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'];
    let sup = |c: char| SUP[c as usize - '0' as usize];
    (!t.is_empty() && t.chars().all(|c| c.is_ascii_digit())).then(|| t.chars().map(sup).collect())
}

/// A Python `str.splitlines()` boundary (`\r\n` counts once: fold it to `\n` first).
const PY_NEWLINE: [char; 10] = ['\n', '\r', '\x0b', '\x0c', '\x1c', '\x1d', '\x1e', '\u{85}', '\u{2028}', '\u{2029}'];

fn skip_link(u: &str) -> bool {
    u.match_indices('/').any(|(i, _)| {
        let r = &u[i + 1..];
        LISTS.skip_link.iter().any(|w| {
            r.len() >= w.len()
                && r.as_bytes()[..w.len()].eq_ignore_ascii_case(w.as_bytes())
                && r[w.len()..].chars().next().is_none_or(|c| !is_word(c))
        })
    })
}

/// Host of a URL, lowercased; "" when it has none (local files, `stdin:`).
pub fn host_of(url: &str) -> String {
    let host = url::Url::parse(url).ok();
    host.and_then(|u| u.host_str().map(|h| h.trim_matches(['[', ']']).to_lowercase())).unwrap_or_default()
}

// ------------------------------------------------------------------ facts + page info
/// Numbers, prices, dates: `\d+(?:[.,:/-]\d+)*` tokens with ≥ 2 digits; `priced`: only those with a currency mark
/// (`Lists::currency`) right before or after them, a space between allowed. Our own `[lN]` link ids are not the
/// page's numbers: never counted (they made the loss guard serve the nav).
pub fn facts(lines: &[Line], priced: bool) -> HashSet<String> {
    let mut s = HashSet::new();
    for ln in lines {
        let cs: Vec<char> = LREF.replace_all(ln.text(), " ").chars().collect();
        let digit = |k: usize| cs.get(k).is_some_and(|c| is_digit(*c));
        let mut from = 0;
        while let Some(st) = (from..cs.len()).find(|&k| digit(k)) {
            // a separator joins when a digit of the match is before it and a digit after it
            let joins =
                |k: usize| k > st && matches!(cs[k], '.' | ',' | ':' | '/' | '-') && digit(k - 1) && digit(k + 1);
            let end = (st..cs.len()).find(|&k| !digit(k) && !joins(k)).unwrap_or(cs.len());
            let (before, after): (String, String) =
                (cs[st.saturating_sub(4)..st].iter().collect(), cs[end..].iter().take(4).collect());
            let mark = |c: &&str| before.trim_end().ends_with(c) || after.trim_start().starts_with(c);
            let digits = cs[st..end].iter().filter(|c| is_digit(**c)).count();
            if digits >= 2 && (!priced || LISTS.currency.iter().any(mark)) {
                s.insert(cs[st..end].iter().collect());
            }
            from = end.max(st + 1);
        }
    }
    s
}

/// og_only's page (`fetch::og_card`, a stored `og` body: `cache::build`): the shell's own og card, its text the
/// title and info line; the shell's own text (enable JavaScript) is not the page.
pub fn card(d: Doc) -> Doc {
    let clean = tok(&d.title) + d.info.values().map(|v| tok(v)).sum::<usize>();
    Doc { mode: "og".into(), sections: Vec::new(), tokens: Tokens { clean }, ..d }
}

/// Text as HTML text or a quoted attribute value: `&` `"` `<` `>` escaped.
pub fn esc(v: &str) -> String {
    v.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

/// A server's Markdown (`kind_of` md: its own conversion of the page) → HTML (CommonMark, tables) cleaned like any
/// page (`build_doc`: sections, `[lN]` links, media, the loss guard), `mode` md. The frontmatter's `title`
/// `description` `image` become og tags, as the page's head has them; with no title there, the first `# ` heading.
/// A code block holding JSON-LD is the page's JSON-LD (author, dates), as a `<script>` in HTML is.
pub fn markdown(raw: &[u8], url: &str, fin: &str) -> Doc {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let text = String::from_utf8_lossy(raw);
    let front = text.strip_prefix("---").and_then(|r| r.split_once("\n---")).map_or("", |(f, _)| f);
    let og =
        front.lines().filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim(), v.trim().trim_matches(['"', '\''])));
    let og = og.filter(|(k, _)| matches!(*k, "title" | "description" | "image"));
    // the title: the frontmatter's, else the first `# ` heading
    let h1 = text.lines().find_map(|l| l.strip_prefix("# ")).unwrap_or_default();
    let og: String = og.map(|(k, v)| format!("<meta property=\"og:{k}\" content=\"{}\">", esc(v))).collect();
    let mut html = format!("<title>{}</title>{og}", esc(h1));
    let opts = Options::ENABLE_YAML_STYLE_METADATA_BLOCKS | Options::ENABLE_TABLES;
    let mut ev: Vec<Event> = Parser::new_ext(&text, opts).collect();
    // JSON-LD comes as a code block (Markdown has no script): the page's JSON-LD, as in its HTML, never text
    let ld = |w: &[Event]| matches!(w, [Event::Start(Tag::CodeBlock(_)), Event::Text(t), Event::End(TagEnd::CodeBlock)] if t.trim_start().starts_with("{\"@context\""));
    while let Some(i) = ev.windows(3).position(ld) {
        let Event::Text(t) = &ev[i + 1] else { break };
        let script = format!("<script type=\"application/ld+json\">{t}</script>");
        ev.splice(i..i + 3, [Event::Html(script.into())]);
    }
    pulldown_cmark::html::push_html(&mut html, ev.into_iter());
    Doc { mode: "md".into(), ..build_doc(&crate::html::parse(&html), url, fin, false) }
}

/// A key's value when truthy; a list there → its first item.
fn pick<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    match v.get(k).filter(|x| truthy(x))? {
        Value::Array(v) => v.first(),
        x => Some(x),
    }
}

/// Python's truth of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}

/// Text of a value: strings as-is, numbers as written (`330`, `12.5`, as Python's `str()`), a list → its first
/// item, an object → its `name`.
fn py_str(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(v) => v.first().map(py_str).unwrap_or_default(),
        Value::Object(_) => v.get("name").map(py_str).unwrap_or_default(),
    }
}

/// Chars of the strings holding a space (prose, not ids or URLs) in a JSON value; a string opening like JSON is read
/// as JSON (a store serialised twice).
fn prose_chars(j: &Value) -> usize {
    match j {
        Value::String(s) if s.starts_with(['{', '[']) => serde_json::from_str(s).map_or(0, |inner| prose_chars(&inner)),
        Value::String(s) if py_strip(s).contains(' ') => s.chars().count(),
        Value::Array(v) => v.iter().map(prose_chars).sum(),
        Value::Object(v) => v.values().map(prose_chars).sum(),
        _ => 0,
    }
}

/// Text no render shows: `<template>` contents (`measure` put them in the element's `tlen`) and the prose of the
/// data scripts.
fn inert_chars(t: &Tree, tlen: &[usize]) -> usize {
    let templates = (0..t.nodes.len()).filter(|&i| t.nodes[i].tag == "template").map(|i| tlen[i]);
    let json = t.json.iter().filter_map(|(_, j, _)| serde_json::from_str::<Value>(py_strip(j)).ok());
    templates.sum::<usize>() + json.map(|j| prose_chars(&j)).sum::<usize>()
}

fn walk_ld<'a>(j: &'a Value, depth: usize, out: &mut Vec<&'a Value>) {
    if depth > 6 {
        return;
    }
    match j {
        Value::Object(v) => {
            out.push(j);
            v.values().for_each(|x| walk_ld(x, depth + 1, out));
        }
        Value::Array(v) => v.iter().for_each(|x| walk_ld(x, depth + 1, out)),
        _ => {}
    }
}

/// The head: `info`, its media (og:image, twitter:image, og:video, each with its `:alt` `:width` `:height`
/// `:type`; then JSON-LD, `<link rel=image_src>`) and how many of them are those og/twitter cards.
fn page_info(t: &Tree, base: &str) -> (Info, Vec<Media>, usize) {
    let (mut info, mut media) = (meta_info(t), Vec::new());
    for key in ["og:image", "twitter:image", "og:video"] {
        let get = |k: &str| t.meta(&format!("{key}{k}")).unwrap_or_default();
        let src = ["", ":url", ":secure_url"].iter().map(|k| get(k)).find(|s| !s.is_empty()).unwrap_or_default();
        let (alt, w, h, ty) = (get(":alt").into(), px(get(":width")), px(get(":height")), get(":type").into());
        add(&mut media, base, Media { src: src.into(), alt, video: key == "og:video", w, h, ty, ..Media::default() });
    }
    let og = media.len();
    for (_, blob, _) in t.json.iter().filter(|j| j.2).take(20) {
        let Ok(json) = serde_json::from_str::<Value>(py_strip(blob)) else { continue };
        let mut objs = Vec::new();
        walk_ld(&json, 0, &mut objs);
        for obj in objs {
            ld_info(obj, &mut info);
            ld_media(obj, base, &mut media);
        }
    }
    let rel =
        |n: &&Node| n.attr("rel").is_some_and(|r| r.split_whitespace().any(|w| w.eq_ignore_ascii_case("image_src")));
    for n in t.nodes.iter().filter(|n| n.tag == "link").filter(rel) {
        add(&mut media, base, Media { src: n.attr("href").unwrap_or_default().into(), ..Media::default() });
    }
    info.extend(t.meta("og:site_name").map(|s| ("site".to_string(), s.to_string())));
    let info = info.into_iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (k, collapse(&unescape(&v))));
    (info.collect(), media, og)
}

/// A declared size (`1200`, `1200px`); 0 = none or not a number.
fn px(v: &str) -> u32 {
    v.trim().trim_end_matches("px").parse().unwrap_or(0)
}

/// A JSON-LD object's pictures: an `ImageObject` (`contentUrl` or `url`, its `caption` as the alt, its size), and
/// the URLs in `image` / `thumbnailUrl` (an object there is walked as its own too); its `logo` is not content (added
/// before the logo's own `ImageObject` is walked, so the picture keeps that verdict, `add`).
fn ld_media<'a>(obj: &'a Value, base: &str, out: &mut Vec<Media>) {
    let text = |k: &str| obj.get(k).map(py_str).unwrap_or_default();
    let list = |v: &'a Value| if let Value::Array(v) = v { v.as_slice() } else { std::slice::from_ref(v) };
    let url_of = |u: &Value| if let Value::String(s) = u.get("url").unwrap_or(u) { s.clone() } else { String::new() };
    for (k, demoted) in [("logo", true), ("image", false), ("thumbnailUrl", false)] {
        for u in obj.get(k).into_iter().flat_map(list) {
            add(out, base, Media { src: url_of(u), demoted, ..Media::default() });
        }
    }
    if text("@type") == "ImageObject" {
        let src = if text("contentUrl").is_empty() { text("url") } else { text("contentUrl") };
        let (alt, w, h) = (text("caption"), px(&text("width")), px(&text("height")));
        add(out, base, Media { src, alt, w, h, ..Media::default() });
    }
}

/// A served `<img>` or a `<video>`'s poster, its alt and declared size; aria-hidden and role=presentation are
/// decoration. The source is the tag's largest: a `srcset` candidate (its `<picture>`'s too), else a `data-*`
/// source a script swaps in (named by a `lazy_src` word: the lazy or hi-res copy where `src` is a placeholder or a
/// thumbnail), else `src`. Demoted: classed as a logo or an icon (`has_word`), or linked elsewhere.
fn body_media(t: &Tree, id: usize, base: &str) -> Option<Media> {
    let (n, page) = (&t.nodes[id], url::Url::parse(base).ok()?);
    if n.attr("aria-hidden") == Some("true") || n.attr("role") == Some("presentation") {
        return None;
    }
    let attr = |k: &str| n.attr(k).unwrap_or_default();
    let (w, h, alt) = (px(attr("width")), px(attr("height")), collapse(attr("alt")));
    if n.tag == "video" {
        return Some(Media { src: n.attr("poster")?.into(), video: true, w, h, ..Media::default() });
    }
    let pic = n.parent.filter(|&p| t.nodes[p].tag == "picture").map(|p| &t.nodes[p].kids[..]).unwrap_or_default();
    let source = |k: &Kid| k.node().and_then(|c| t.nodes[c].attr("srcset"));
    let set: Vec<&str> = pic.iter().filter_map(source).chain(n.attr("srcset")).chain(n.attr("data-srcset")).collect();
    let named = |k: &str| k.strip_prefix("data-").is_some_and(|k| LISTS.lazy_src.iter().any(|w| k.contains(w)));
    let data = n.attrs.iter().find(|(k, v)| named(k) && v.starts_with(['/', 'h']) && !v.contains(' '));
    let src = largest(&set.join(", ")).map(String::from).or(data.map(|d| d.1.clone()));
    let url = page.join(py_strip(&src.or(n.attr("src").map(String::from))?)).ok()?;
    let (away, linked) = links(t, id, &page);
    let demoted = has_word(attr("class"), &LISTS.not_content) || away || ad_slot(t, id);
    Some(Media { src: url.into(), alt, w, h, demoted, linked, ..Media::default() })
}

/// The links on a picture: an `<a href>` around it, or an empty one laid over it in its box (a card's overlay
/// link), but none to the page itself or below it (`/post/1/photo/1`). `.0` = one goes to another page: a related
/// story's, another product's, a feed item's picture. `.1` = the one around it goes to the picture's own: a path
/// with an image file type in it (a lightbox, the original, a `File:x.jpg` page) or a `picture_page` word
/// (`/photos/1`, `/image-article/…`, `/attachment/…`; a gallery's).
fn links(t: &Tree, id: usize, page: &url::Url) -> (bool, bool) {
    let above: Vec<usize> = up(t, id).skip(1).collect();
    let empty = |c: &usize| t.nodes[*c].tag == "a" && t.texts(*c, &[]).all(|s| s.trim().is_empty());
    let kids = above.iter().take(2).flat_map(|&p| &t.nodes[p].kids);
    let overlay = kids.filter_map(Kid::node).filter(empty);
    let below = |u: &url::Url| page.path().len() > 1 && u.path().starts_with(page.path());
    let href =
        |a: usize| page.join(t.nodes[a].attr("href")?).ok().filter(|u| u.scheme().starts_with("http") && !below(u));
    let own = |u: &url::Url| has_word(u.path(), &LISTS.image_ext) || has_word(u.path(), &LISTS.picture_page);
    let around: Vec<url::Url> = above.iter().filter(|&&p| t.nodes[p].tag == "a").filter_map(|&a| href(a)).collect();
    let away = around.iter().chain(&overlay.filter_map(href).collect::<Vec<_>>()).any(|u| !own(u));
    (away, around.iter().any(own))
}

/// A picture in an ad slot: it, or an element around it, is named an ad by a whole word (or its plural,
/// `has_word`) of its tag, class or id (`ad_slot`: `<x-ad-post>`, `class="ad-slot"`; never `advert`, `promoted`,
/// `sponsored`: classifieds sites name their real listings so) and holds under `ad_slot_words` words and no number
/// (`facts`: a listing's price, date or size keeps its photo). Demoted: listed, not counted.
fn ad_slot(t: &Tree, id: usize) -> bool {
    let max: usize = limit("ad_slot_words");
    let name = |n: &Node| format!("{} {} {}", n.tag, n.attr("class").unwrap_or(""), n.attr("id").unwrap_or(""));
    let small = |p: usize| {
        let text = || t.texts(p, &LISTS.drop_always).flat_map(str::split_whitespace);
        let few = text().filter(|w| w.chars().any(char::is_alphanumeric)).nth(max.saturating_sub(1)).is_none();
        few && facts(&[Line::T(text().collect::<Vec<_>>().join(" "))], false).is_empty()
    };
    up(t, id).any(|p| has_word(&name(&t.nodes[p]), &LISTS.ad_slot) && small(p))
}

/// The largest candidate of a `srcset` (by its `w` or `x` descriptor; none = 1x).
fn largest(set: &str) -> Option<&str> {
    let size = |d: Option<&str>| d.and_then(|d| d.get(..d.len() - 1)?.parse::<f64>().ok()).unwrap_or(1.0);
    let all =
        set.split(", ").filter_map(|c| c.split_whitespace().next().map(|u| (u, size(c.split_whitespace().nth(1)))));
    all.max_by(|a, b| a.1.total_cmp(&b.1)).map(|c| c.0)
}

/// A word of an alt, a class or a file name (lower case, or its singular) is in `list`: `not_content` says the
/// picture is a logo, an icon, a badge; `filler` that the file is a spacer.
fn has_word(text: &str, list: &[&str]) -> bool {
    words(text).iter().any(|w| list.contains(&w.strip_suffix('s').unwrap_or(w)))
}

/// The words of a text, lower case, split on anything not a letter or a digit.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// Size words in a picture URL (lower case): `250px-`, `-150x84` `_389x557` `/1200x630wa`, `@2x`, `=s1600`, `w_1456`.
static SIZE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\d{2,4}px-|(?:[-_]|\b)(\d{2,4})x(\d{2,4})|@[1-4]x|=s\d{2,4}|\b[wh]_\d{2,4}\b")
        .expect("a pattern")
});

/// Where a picture's file name ends: at its first image extension (`.jpg.webp`, `.jpg_960x960q75.jpg_.avif`) or
/// a transform chain after a `=` (`=w1060-fcrop64…`).
static NAME_END: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(&format!(r"\.(?:{})(?:[^a-z0-9]|$)|=", LISTS.image_ext.join("|"))).expect("a pattern")
});

/// One picture's identity and size by its URL (one picture at several sizes is one row): the file's own name
/// (`NAME_END`) without size words (`SIZE`), never its folders or query, where CDNs put sizes and crops, but with
/// a proxied original (a query value with a `/` and a `.`: `_next/image?url=`, an image proxy's `?url=`); a name under
/// 8 chars or of one word with no digit (`image`, `hqdefault`) keeps its folders. Two different file names are
/// never one picture (`IMG_2026.10.11.jpg`, `.12.jpg`). The size: the largest number of its size words, or a `w`
/// or `width` query.
fn key_of(u: &url::Url) -> (String, u32) {
    let path = u.path().to_lowercase().replace("%2f", "/");
    let (dir, file) = path.rsplit_once('/').unwrap_or_default();
    let name = SIZE.replace_all(NAME_END.find(file).map_or(file, |m| &file[..m.start()]), "");
    let word = !name.contains(|c: char| c.is_ascii_digit() || !c.is_alphanumeric());
    let query: Vec<_> = u.query_pairs().map(|(_, v)| v).filter(|v| v.contains('/') && v.contains('.')).collect();
    let dir = if name.len() < 8 || word { SIZE.replace_all(dir, "") } else { "".into() };
    let key = format!("{dir}{name}?{}", query.join("&"));
    let words = SIZE.find_iter(&path).flat_map(|m| m.as_str().split(|c: char| !c.is_ascii_digit()).map(str::parse));
    let width = u.query_pairs().filter(|(k, _)| k == "w" || k == "width").map(|(_, v)| v.parse());
    (key, words.chain(width).filter_map(Result::ok).max().unwrap_or(0))
}

/// One media row into `out`, its URL resolved against `base`, known tracking parameters (`tracking`) cut.
/// Refused: no URL, one that is not http(s) or file (data:), an svg, a declared side under `ICON`, a filler (a
/// GIF named as one, or any file named `placeholder`: `pxl.gif`, `grey-placeholder.png`). The same picture
/// (`key_of`) keeps its first place and verdict (`demoted`: a head card over a linked copy, a JSON-LD logo over
/// its own object, a share card: `og`, `share`, `card`… as a word of its file name, `share_card`, or an Open
/// Graph host label, `share_host`), gains the alt it lacked and takes the larger copy (declared side, else the
/// URL's size word).
fn add(out: &mut Vec<Media>, base: &str, m: Media) {
    let m = Media { alt: unjson(&m.alt), ..m };
    let url = url::Url::parse(base).and_then(|b| b.join(py_strip(&m.src))).ok().filter(|_| !m.src.trim().is_empty());
    let Some(mut url) = url.filter(|u| matches!(u.scheme(), "http" | "https" | "file")) else { return };
    let ext = url.path().rsplit_once('.').map(|e| e.1.to_ascii_lowercase()).filter(|e| (2..=4).contains(&e.len()));
    let ty = m.ty.rsplit('/').next().map(str::to_string).filter(|t| !t.is_empty()).or(ext).unwrap_or_default();
    let file = url.path_segments().and_then(|mut s| s.next_back()).unwrap_or_default();
    let filler = has_word(file, &["placeholder"]) || ty == "gif" && has_word(file, &LISTS.filler);
    // a share card: whole words of its own file name (a proxied one's last part), or a card host's first label
    let (host, last) = (url.host_str().and_then(|h| h.split('.').next()), file.to_lowercase().replace("%2f", "/"));
    let own = has_word(last.rsplit('/').next().unwrap_or_default(), &LISTS.share_card);
    let card = own || has_word(host.unwrap_or_default(), &LISTS.share_host);
    let side = |c: &regex::Captures, i| c.get(i).and_then(|n| n.as_str().parse().ok());
    let named = SIZE.captures_iter(&file.to_lowercase()).find_map(|c| side(&c, 1).zip(side(&c, 2)));
    // the file name's size filters (a strip, an icon) and picks the larger copy, never stored as declared
    let (w, h) = if m.w == 0 && m.h == 0 { named.unwrap_or_default() } else { (m.w, m.h) };
    // a strip (one side ≥ `strip_aspect` × the other), an icon or a spacer (a declared side under `icon_px`)
    let (aspect, icon): (u32, u32) = (limit("strip_aspect"), limit("icon_px"));
    let strip = w.min(h) > 0 && w.max(h) >= aspect * w.min(h);
    if filler || strip || ty.starts_with("svg") || (1..icon).contains(&w) || (1..icon).contains(&h) {
        return;
    }
    let tracking = |kv: &&str| LISTS.tracking.iter().any(|t| kv.to_lowercase().starts_with(t));
    let kept: Vec<&str> = url.query().into_iter().flat_map(|q| q.split('&')).filter(|kv| !tracking(kv)).collect();
    url.set_query(Some(kept.join("&")).filter(|q| !q.is_empty()).as_deref());
    let (key, by_url) = key_of(&url);
    let m = Media { key: (key, w.max(h).max(by_url)), src: url.into(), ty, demoted: m.demoted || card, ..m };
    let Some(x) = out.iter_mut().find(|x| x.key.0 == m.key.0) else { return out.push(m) };
    let alt = if x.alt.is_empty() { m.alt.clone() } else { std::mem::take(&mut x.alt) };
    if m.key.1 > x.key.1 {
        *x = Media { demoted: x.demoted, ..m };
    }
    x.alt = alt;
}

/// Alt text as a reader sees it: JSON escapes a page left in an attribute (`\u2019`, a surrogate pair) decoded,
/// `\n` `\t` `\r` and runs of whitespace one space.
fn unjson(s: &str) -> String {
    let (mut out, mut rest) = (Vec::new(), s);
    while let Some(c) = rest.chars().next() {
        let hex = rest.strip_prefix("\\u").and_then(|r| r.get(..4)).filter(|h| !h.starts_with('+'));
        let (units, len) = match hex.map(|h| u16::from_str_radix(h, 16)) {
            Some(Ok(u)) => (vec![u], 6),
            _ if ["\\n", "\\t", "\\r"].iter().any(|e| rest.starts_with(e)) => (vec![32], 2),
            _ => (c.encode_utf16(&mut [0; 2]).to_vec(), c.len_utf8()),
        };
        out.extend(units);
        rest = &rest[len..];
    }
    collapse(&String::from_utf16_lossy(&out))
}

/// MEDIA in rank order: the og/twitter cards as they came (`page_info`), then the rest of the head
/// and the served pictures (`body_media`, after the chrome prune) by alt length, then declared size, then
/// document order, one row per picture (`add`); the `demoted` go last: avatars (both sides ≤ `AVATAR`, square
/// within 0.8-1.25), a bare `@handle` alt, an alt that is the author's or the site's name (`info`), a logo or an
/// icon by its URL or an alt of ≤ 3 words (`has_word`: `/brand/site_logo.png`, an `avatars.` host). Never the title: a
/// shop or a listing alts its main photo with it, so that photo stays content, its alt dropped (`(no alt)`: the title
/// says nothing about the picture).
fn rank(cards: (Vec<Media>, usize), t: &Tree, body: &[usize], base: &str, info: &Info) -> Vec<Media> {
    let (mut out, head) = cards;
    body.iter().filter_map(|&id| body_media(t, id, base)).for_each(|m| add(&mut out, base, m));
    out[head..].sort_by_key(|m| std::cmp::Reverse((m.alt.chars().count(), u64::from(m.w) * u64::from(m.h))));
    let named = |a: &str, keys: &[&str]| info.iter().any(|(k, v)| keys.contains(&k.as_str()) && v.to_lowercase() == a);
    // the site's name as whole words in an alt of at most 2 more (site "X" never names "Red fox"); no name, none
    let site = words(info.get("site").map_or("", String::as_str));
    let has_site = |a: &str| words(a).windows(site.len().max(1)).any(|w| w == site) && words(a).len() <= site.len() + 2;
    // an avatar: both sides ≤ `avatar_px` and square-ish (last)
    let avatar_px: u32 = limit("avatar_px");
    for m in &mut out {
        let (alt, (small, big)) = (m.alt.to_lowercase(), (m.w.min(m.h), m.w.max(m.h)));
        let words = alt.split_whitespace().count();
        let avatar = small > 0 && big <= avatar_px && 4 * big <= 5 * small && words < 4 && !m.linked;
        let handle = alt.starts_with('@') && !alt.contains(' ');
        // a logo or a badge is named in a few words; a caption's "emoji" or "badge" describes a screenshot
        let few = words <= 3 && (has_word(&alt, &LISTS.not_content) || has_word(&alt, &LISTS.not_content_alt));
        let named_by = few || has_word(&m.src, &LISTS.not_content) || has_site(&alt);
        m.demoted |= avatar || handle || named(&alt, &["author"]) || named_by;
        if named(&alt, &["title"]) || alt == collapse(&t.title).to_lowercase() {
            m.alt.clear();
        }
    }
    out.sort_by_key(|m| m.demoted);
    out
}

/// The `<meta>` half of `info`: title, description, author, dates, price. The title = `og:title`, else the first
/// `h1` when `<title>` is only the site's name and its tagline (one own word before a suffix: THE own-words rule,
/// `fetch::words_of`; `Reddit - The heart of the internet` on every thread; none = a wall's, no suffix = maybe
/// an error's, `Site Maintenance`) and shares none of its words with the `h1` (its text outside controls:
/// `tokio - Rust` over `<h1>Crate tokio <button>Copy item path</button></h1>` is the page's own title; `Home -
/// Away - League` with `<h1>Home</h1>`), else `<title>`.
fn meta_info(t: &Tree) -> Info {
    let mut info = IndexMap::new();
    let site = matches!(title_head(&t.title), h if h.len() < t.title.len() && words_of(h).len() == 1);
    let text = |i| collapse(&t.texts(i, &["button", "input", "select"]).collect::<String>());
    let mine = |h: &String| site && !h.is_empty() && words_of(&t.title).iter().all(|w| !words_of(h).contains(w));
    let h1 = t.nodes.iter().position(|n| n.tag == "h1").map(text).filter(mine).unwrap_or_else(|| t.title.clone());
    info.insert("title".into(), t.meta("og:title").unwrap_or(&h1).to_string());
    for (key, names) in &LISTS.info {
        // an author that is a URL names no one; a description is cut to 300 chars
        let named = |v: &&str| *key != "author" || !v.starts_with("http");
        let Some(v) = names.iter().find_map(|k| t.meta(k)).filter(named) else { continue };
        let cut = if *key == "description" { 300 } else { usize::MAX };
        info.insert(key.to_string(), v.chars().take(cut).collect());
    }
    if let Some(price) = t.meta("product:price:amount").or(t.meta("og:price:amount")) {
        let cur = t.meta("product:price:currency").or(t.meta("og:price:currency")).unwrap_or("");
        info.insert("price".into(), format!("{price} {cur}"));
    }
    info
}

/// One JSON-LD object fills the `info` keys still missing: author, dates, price, rating.
fn ld_info(obj: &Value, info: &mut Info) {
    if !info.contains_key("author")
        && let Some(author @ (Value::Object(_) | Value::String(_))) = pick(obj, "author")
        && let Some(name) = author.get("name").or(Some(author).filter(|a| a.is_string()))
        && truthy(name)
        && !py_str(name).starts_with("http")
    {
        info.insert("author".into(), py_str(name));
    }
    for (key, ld) in [("published", "datePublished"), ("modified", "dateModified")] {
        if !info.contains_key(key)
            && let Some(date) = pick(obj, ld)
        {
            info.insert(key.into(), py_str(date));
        }
    }
    if !info.contains_key("price")
        && let Some(offer @ Value::Object(_)) = pick(obj, "offers")
        && let Some(price) = pick(offer, "price").or(pick(offer, "lowPrice"))
    {
        let cur = offer.get("priceCurrency").map(py_str).unwrap_or_default();
        info.insert("price".into(), py_strip(&format!("{} {cur}", py_str(price))).to_string());
    }
    if !info.contains_key("rating")
        && let Some(rating @ Value::Object(_)) = obj.get("aggregateRating")
        && let Some(value) = pick(rating, "ratingValue")
    {
        let count = pick(rating, "reviewCount").or(pick(rating, "ratingCount")).map_or("?".into(), py_str);
        info.insert("rating".into(), format!("{} ({count} reviews)", py_str(value)));
    }
}

fn render<'a>(t: &'a Tree, tlen: &'a [usize], base: &str, mode: Mode, js: bool) -> Render<'a> {
    let mut r = Render {
        tree: t,
        tlen,
        base_url: url::Url::parse(base.split('#').next().unwrap_or(base)).ok(),
        mode,
        js,
        lines: Vec::new(),
        buf: String::new(),
        marks: Vec::new(),
        links: IndexMap::new(),
        inline: 0,
        media: Vec::new(),
        thread: HashMap::new(),
        by: None,
    };
    r.thread = r.comments();
    r.kids(0, false);
    r.flush();
    r
}

/// JSON one value per line, nesting shown by one space (indent costs tokens); not JSON → as it is.
fn one_space(text: &str) -> Vec<u8> {
    let mut out = serde_json::Serializer::with_formatter(Vec::new(), PrettyFormatter::with_indent(b" "));
    // keys sorted (Python's `sort_keys=True` order)
    let sorted = |mut v: Value| {
        v.sort_all_objects();
        v.serialize(&mut out)
    };
    let ok = matches!(serde_json::from_str::<Value>(text).map(sorted), Ok(Ok(())));
    if ok { out.into_inner() } else { text.as_bytes().to_vec() }
}

/// Islands (what the served copy hides in templates and data scripts), after the served sections: `<template>` contents
/// (`measure` put their text in the element's `tlen`) holding ≥ 1 in `island_template_1_in` of the page's, rendered by
/// the served render (links numbered on), then data scripts ≥ `island_json_bytes` as text documents (`one_space` JSON,
/// `lN` lines).
fn islands(tree: &Tree, r: &mut Render, url: &str) -> Vec<Section> {
    let sec = |label: String, bytes: usize, lines| Section {
        level: 0,
        title: format!("island {label} {} KB", bytes.div_ceil(1024)),
        lines,
    };
    let mut out = Vec::new();
    r.lines.clear();
    // a `<template>` holding ≥ 1 in `island_template_1_in` of the page's text, a data script ≥ `island_json_bytes`
    let (part, json): (usize, usize) = (limit("island_template_1_in"), limit("island_json_bytes"));
    let big = |i: &usize| tree.nodes[*i].tag == "template" && r.tlen[*i] > 0 && r.tlen[*i] * part >= r.tlen[0];
    for f in (0..tree.nodes.len()).filter(big).map(|i| i + 1).collect::<Vec<_>>() {
        r.kids(f, false);
        r.flush();
        let lines = std::mem::take(&mut r.lines).into_iter().map(|l| match l {
            Line::H(lv, t) => format!("{} {t}", "#".repeat(lv)),
            Line::T(t) => t,
        });
        out.push(sec(tree.nodes[f - 1].label(), r.tlen[f - 1], lines.collect()));
    }
    for (label, text, _) in tree.json.iter().filter(|(_, t, _)| t.len() >= json) {
        let doc = crate::text::build(&one_space(text), "json", url, url);
        out.push(sec(label.clone(), text.len(), doc.sections.into_iter().flat_map(|s| s.lines).collect()));
    }
    out
}

/// The value `v` holds most often (a tie: the greatest, `max_by_key` keeps the last).
fn commonest<T: Ord + Copy>(mut v: Vec<T>) -> Option<T> {
    v.sort_unstable();
    v.chunk_by(|a, b| a == b).max_by_key(|g| g.len()).map(|g| g[0])
}

/// `i` and its ancestors, nearest first.
fn up(t: &Tree, i: usize) -> impl Iterator<Item = usize> + '_ {
    std::iter::successors(Some(i), |&p| t.nodes[p].parent)
}

/// `js` = the HTML came from a JS render: its `<noscript>` was never shown, so it is dropped. One parse per page:
/// the content check's tree.
pub fn build_doc(tree: &Tree, url: &str, final_url: &str, js: bool) -> Doc {
    let (mut info, cards, og) = page_info(tree, final_url);
    let mut tlen = vec![0; tree.nodes.len()];
    measure(tree, 0, &mut tlen);
    // LOSS GUARD: every prune is measured against light (nothing pruned), never against another prune: a
    // price inside a pruned <form> is gone from both clean and hard. Serve the first render that keeps
    // ≥ 70% of light's numbers: clean → hard → light.
    let light = render(tree, &tlen, final_url, Mode::Light, js);
    let (fl, priced) = (facts(&light.lines, false), facts(&light.lines, true));
    let lost = |lines: &[Line]| fl.difference(&facts(lines, false)).count();
    let loss_max: f64 = limit("loss_max");
    let keeps = |lines: &[Line]| lost(lines) as f64 <= loss_max * fl.len() as f64;
    let mut mode = "clean";
    let mut used = render(tree, &tlen, final_url, Mode::Clean, js);
    // the pictures the chrome prune keeps, whichever render is served
    let media = rank((cards, og), tree, &std::mem::take(&mut used.media), final_url, &info);
    if !keeps(&used.lines) {
        (mode, used) = ("hard", render(tree, &tlen, final_url, Mode::Hard, js));
    }
    let title = info.get("title").cloned().unwrap_or_default();
    let chars = |secs: &[Section]| secs.iter().flat_map(|s| s.lines.iter()).map(|l| l.chars().count()).sum();
    let mut pruned_chars = None;
    if !keeps(&used.lines) {
        pruned_chars = Some(chars(&sections_of(&used.lines).0));
        (mode, used) = ("light", light);
    }
    let (sections, top) = sections_of(&used.lines);
    let pruned_chars = pruned_chars.unwrap_or_else(|| chars(&sections));
    let visible_chars = tlen[0] + inert_chars(tree, &tlen);
    let (vs_light_lost, price_pruned) = (lost(&used.lines), priced.difference(&facts(&used.lines, false)).count());
    let loss = Loss { vs_light_lost, light_facts: fl.len(), pruned_chars, visible_chars, price_pruned };
    let title = if title.is_empty() { crate::lang::short(final_url, 60) } else { title };
    let islands = islands(tree, &mut used, final_url);
    // the size the page states for its thread (`outline::tally` prints `25 of 301`)
    info.extend(used.declared().map(|n| ("comments".into(), n.to_string())));
    let mut doc = Doc {
        info,
        top,
        links: used.links.keys().cloned().collect(),
        anchors: used.links.into_values().collect(),
        cut: tree.cut,
        loss,
        islands: islands.len(),
        media,
        ..Doc::new(url, final_url, title, sections, mode)
    };
    doc.sections.extend(islands);
    doc
}

/// A paragraph: a text of ≥ `paragraph_words` words (a thread's post, a description; never a name or a link).
pub fn paragraph(t: &str) -> bool {
    t.split_whitespace().count() >= limit("paragraph_words")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse;

    /// A `<title>` that is only the site's name and tagline (Reddit's on every thread) yields to the first `h1`;
    /// a title of the page's own words, a bare word, an error's, one that holds the `h1` already and `og:title` stay.
    #[test]
    fn a_site_name_title_yields_to_the_h1() {
        let title = |head: &str| {
            let h = format!("<head>{head}</head><h1>Visiting <b>Portugal</b> for a week</h1><h1>Other</h1>");
            page_info(&parse(&h), "https://a.example/").0.get("title").cloned().unwrap_or_default()
        };
        assert_eq!(title("<title>Reddit - The heart of the internet</title>"), "Visiting Portugal for a week");
        assert_eq!(title("<title>Reddit</title>"), "Reddit");
        assert_eq!(title("<title>Rust traits explained | Blog</title>"), "Rust traits explained | Blog");
        let held = "<title>Lisboa - Visiting Portugal for a week</title>";
        assert_eq!(title(held), "Lisboa - Visiting Portugal for a week");
        assert_eq!(title("<title>Site Maintenance</title>"), "Site Maintenance");
        let og = "<title>Reddit</title><meta property='og:title' content='From og'>";
        assert_eq!(title(og), "From og");
        // An API docs page whose h1 holds the title's own word (and a button label) keeps its `<title>`
        let docs = |h1: &str| {
            let h = format!("<head><title>widget - Lang</title></head><h1>{h1}</h1>");
            page_info(&parse(&h), "https://a.example/").0.get("title").cloned().unwrap_or_default()
        };
        assert_eq!(docs("Crate widget <button>Copy item path</button>"), "widget - Lang");
        assert_eq!(docs("Module <span>Widget</span>"), "widget - Lang");
        assert_eq!(docs("<button>Copy item path</button>"), "widget - Lang");
        assert_eq!(docs("Gadget parts for sale"), "Gadget parts for sale");
    }

    /// A TikTok rehydration island as saved (20 lines): Claude 5.5 count_tokens
    /// = 220; JSON's runs of marks (`": "`, `},`) read as letters land
    /// within ±25% (260; 277 before). The whole islands: IG #1 +32% → +22%, IG #2 +13% → +4%, TikTok +16% → +9%.
    #[test]
    fn json_marks_in_a_run_are_cheap() {
        let island = "l206      \"core_ux_fix\": {\nl207       \"vid\": \"v1\"\nl208      },\nl209      \"creator_center_connect\": {\nl210       \"vid\": \"v1\"\nl211      },\nl212      \"creator_center_connect_global\": {\nl213       \"vid\": \"v2\"\nl214      },\nl215      \"creator_center_global_comment_management\": {\nl216       \"vid\": \"v1\"\nl217      },\nl218      \"creator_center_global_post_management\": {\nl219       \"vid\": \"v1\"\nl220      },\nl221      \"creator_center_test\": {\nl222       \"vid\": \"v2\"\nl223      },\nl224      \"csp_merge_support\": {\nl225       \"vid\": \"v1\"";
        let t = tok(island);
        assert!((165..=275).contains(&t), "{t} vs 220 real");
        assert_eq!(tok_cut(island, t), island.len());
        assert!(tok(&island[..tok_cut(island, 40)]) <= 40);
    }

    /// A thread's stated size = the count nearest the thread before it, shown, outside a list item; two there
    /// claim none: a related post's, a pruned sidebar's, a link's label, a count after the thread never name it.
    #[test]
    fn a_thread_states_its_size() {
        let text = "a reply that says one thing in a sentence long enough to be a paragraph here";
        let c = |i: usize, kids: &str| {
            format!("<li><article><b>user{i}</b> <time>Jan {i}</time><p>{text}</p></article>{kids}</li>")
        };
        let thread: String = (0..4).map(|i| c(i, &format!("<ol>{}</ol>", c(i + 10, "")))).collect();
        let at = |own: &str, between: &str, after: &str| {
            let html = format!(
                "<html><body><article><h1>A post</h1>{own}<p>{text}</p></article>{between}<div><h2>Discussion</h2>\
                 <ol>{thread}</ol>{after}</div></body></html>"
            );
            build_doc(&parse(&html), "https://a.example/p", "https://a.example/p", false).info.get("comments").cloned()
        };
        let related = "<section><h3>Related</h3><ul><li><a href=/o>Another post</a> <span>450 comments</span></li></ul></section>";
        assert_eq!(at("", related, ""), None, "a related post's count");
        assert_eq!(at("", "<aside><p><a href=/hot>99 comments</a></p></aside>", ""), None, "a pruned sidebar's");
        assert_eq!(
            at("", r#"<nav><a href=/prev aria-label="88 comments">Previous</a></nav>"#, ""),
            None,
            "a nav label"
        );
        assert_eq!(at("", "", " 77 comments"), None, "after the thread");
        assert_eq!(at("<a href=#c>12 comments</a>", related, "").as_deref(), Some("12"), "the post's own");
        assert_eq!(at("<p>12 comments</p><p>14 comments</p>", "", ""), None, "a tie claims none");
        assert_eq!(at("<p>1,204&nbsp;Comments</p>", "", "").as_deref(), Some("1204"));
        assert_eq!(at("", r#"<div data-comment-count="301"></div>"#, "").as_deref(), Some("301"), "a count attribute");
        assert_eq!(at(r#"<x data-comment-id="12345"></x><p>7 comment threads</p>"#, "", ""), None, "an id, a phrase");
    }

    /// A card group before a thread in one parent keeps the thread's comments: their headings are untitled, and
    /// only the card members' emptied lines drop.
    #[test]
    fn cards_beside_a_thread_keep_its_comments() {
        let text = "a reply that says one thing in a sentence long enough to be a paragraph here";
        let c = |i: usize, kids: &str| {
            format!("<li><article><b>user{i}</b> <time>Jan {i}</time><p>{text}</p></article>{kids}</li>")
        };
        let thread: String = (0..4).map(|i| c(i, &format!("<ol>{}</ol>", c(i + 10, "")))).collect();
        let card =
            |i: usize| format!("<div class=c><h3><a href=/p{i}>A related post {i}</a></h3><p>{i}2 votes</p></div>");
        let html = format!("<main><h1>Post</h1>{}<ol>{thread}</ol></main>", (1..=4).map(card).collect::<String>());
        let d = build_doc(&parse(&html), "https://a.example/p", "https://a.example/p", false);
        let cards = d.sections.iter().flat_map(|s| &s.lines).filter(|l| l.starts_with(CARD)).count();
        assert_eq!((cards, d.sections.iter().filter(|s| s.level >= COMMENT).count()), (4, 8));
    }

    /// A layout table (HN) is its cells' blocks; a thread table, ≥ 8 rows indented by a spacer, is a comment
    /// a row (HN's 373 comments were one 41k-token line); a data table with long cells stays rows.
    #[test]
    fn spacer_rows_are_comments() {
        let para = "this comment says one thing in a sentence long enough to be prose";
        // HN's shape: a row a comment, its indent a text-less 1 px spacer (`width` 0, 40, 80), a vote spacer after
        let post = |(i, w): (usize, usize)| {
            let spacer = format!("<img height=1 width={w}><img height=1 width=14>");
            format!(
                "<tr><td><table><tr><td>{spacer}</td><td>user{i} 2 hours ago</td><td>{para} {i}</td></tr></table></td></tr>"
            )
        };
        let page = |rows: String| {
            let html =
                format!("<html><body><table><tr><td><b>Story</b><table>{rows}</table></td></tr></table></body></html>");
            build_doc(&parse(&html), "https://a.example/item", "https://a.example/item", false).sections
        };
        let tree = [0, 40, 80, 40, 0, 40, 0, 0].into_iter().enumerate().map(post).collect::<String>();
        let levels: Vec<usize> = page(tree).iter().map(|s| s.level).collect();
        assert_eq!(levels, [0, 7, 8, 9, 8, 7, 8, 7, 7], "the story, then a comment a row at 7 + its depth");
        let flat = [0; 8].into_iter().enumerate().map(post).collect::<String>();
        assert_eq!(page(flat).len(), 1, "flat posts are a feed, no thread");
        let short = [0, 40, 80, 40, 0, 40, 0].into_iter().enumerate().map(post).collect::<String>();
        assert_eq!(page(short).len(), 1, "7 comments: one block");
        // a data table's rows hold no spacer: no comment
        let cell = |i: usize| format!("<tr><td>10.{i}</td><td>{para} {i}</td></tr>");
        let data = page((1..=8).map(cell).collect());
        assert_eq!((data.len(), data[0].lines.len()), (1, 9), "the story line, then a line a row: {:?}", data[0].lines);
        // a table-layout news page: a photo's `width`, a sized cell's, are no indent (a spacer is text-less, 1 high)
        let story = |(i, w): (usize, usize)| {
            let photo = format!("<img src=p{i}.jpg width={w} height=200>");
            format!("<tr><td><table><tr><td width={w}>{photo}</td><td>{para} {i}</td></tr></table></td></tr>")
        };
        let news = [120, 300, 600, 300, 120, 300, 120, 120].into_iter().enumerate().map(story).collect::<String>();
        assert!(page(news).iter().all(|s| s.level < COMMENT), "photos and cell widths are no thread");
        // other spacer tables: no step starts at 0, too few rows hold a paragraph, or each row heads its own story
        let rows = |cell: &dyn Fn(usize, usize) -> String, ws: &[usize]| {
            let row = |(i, &w): (usize, &usize)| format!("<tr><td><img height=1 width={w}></td>{}</tr>", cell(i, w));
            page(ws.iter().enumerate().map(row).collect()).into_iter().filter(|s| s.level >= COMMENT).count()
        };
        let stair = [0, 20, 20, 0, 20, 40, 40, 0, 20, 20];
        let toc = |i: usize, _| format!("<td><a href=#s{i}>{i} Part {i}</a></td><td>{i}</td>");
        let files = |i: usize, _| format!("<td><a href=/f{i}.c>f{i}.c</a></td><td>r{i}</td><td>2004-03-0{i}</td>");
        let gov = |i: usize, _| format!("<td><b>News {i}</b><br>{para} {i}</td>");
        let letter = |i: usize, _| format!("<td><h3>Story {i}</h3><p>{para} {i}</p></td>");
        let found = [
            rows(&toc, &stair),
            rows(&files, &stair),
            rows(&gov, &[10, 1, 1, 10, 1, 1, 10, 1, 1, 10]),
            rows(&letter, &stair),
        ];
        assert_eq!(found, [0; 4], "a contents list, a file tree, a layout, a newsletter");
        // a hostile chain (every row one spacer step deeper) is clamped at `thread_depth`, in linear time
        let t0 = std::time::Instant::now();
        let chain = (0..4_000usize).map(|i| (i, if i.is_multiple_of(2) { 0 } else { i })).map(post).collect::<String>();
        let deepest = page(chain).iter().map(|s| s.level).max();
        assert_eq!(deepest, Some(COMMENT + limit::<usize>("thread_depth")));
        assert!(t0.elapsed().as_secs() < 20, "{:?}", t0.elapsed());
    }

    /// A thread in a block the clean render prunes (an `<aside>` widget) is never counted: comments are what a
    /// render serves, so a dropped block opens none, nor makes a live blog's dated updates (one post) a thread.
    #[test]
    fn a_pruned_thread_counts_nothing() {
        let para = "this comment says one thing in a sentence long enough to be prose";
        let li = |i: usize, kids: &str| format!("<li><time>{i}h ago</time><p>{para} {i}</p>{kids}</li>");
        let tree: String = (0..4).map(|i| li(i, &format!("<ol>{}</ol>", li(10 + i, "")))).collect();
        let body: String = (0..40).map(|i| format!("<p>{para} body {i}</p>")).collect();
        let html = format!("<html><body><main>{body}</main><aside><ol>{tree}</ol></aside></body></html>");
        let doc = build_doc(&parse(&html), "https://a.example/p", "https://a.example/p", false);
        assert_eq!(doc.mode, "clean");
        assert!(doc.sections.iter().all(|s| s.level < COMMENT), "{:?}", doc.sections.len());
        let post = |i: usize, w: &str| format!("<article><time>{w} {i}</time><p>{para} {w} {i}</p></article>");
        let (upd, side): (String, String) =
            ((0..9).map(|i| post(i, "update")).collect(), (0..2).map(|i| post(i, "side")).collect());
        let blog = format!(
            "<html><body><article><h1>Live</h1><time>now</time><p>{para}</p>{upd}</article><aside>{side}</aside></body></html>"
        );
        let doc = build_doc(&parse(&blog), "https://a.example/p", "https://a.example/p", false);
        assert!(doc.sections.iter().all(|s| s.level < COMMENT), "the sidebar's posts were its second top");
    }

    /// BOM, then the header, then the page's `<meta>`, then UTF-8; a page cannot claim UTF-16 from ASCII.
    /// A server's Markdown is a page: headings are sections (a `#` inside a fence is code), `![alt](src)` a media
    /// row, `[text](url)` a link; no frontmatter title → the first `# ` heading.
    #[test]
    fn markdown_is_a_page() {
        let md = "# Camera modules?\n\nIntro with [a link](/a) and enough words to keep.\n\n## Wiring\n\n```\n# not a \
                  heading\n```\n\n![The board](/b.jpg)\n\n## Code\n\nMore words.\n\n```json\n\
                  {\"@context\":\"https://schema.org\",\"@type\":\"WebPage\",\"dateModified\":\"2026-10-08\"}\n```\n";
        let d = markdown(md.as_bytes(), "https://ex.com/t/1", "https://ex.com/t/1");
        let titles: Vec<&str> = d.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            (d.mode.as_str(), d.title.as_str(), titles),
            ("md", "Camera modules?", vec!["Camera modules?", "Wiring", "Code"])
        );
        assert_eq!(
            (d.media.len(), d.media[0].src.as_str(), d.media[0].alt.as_str()),
            (1, "https://ex.com/b.jpg", "The board")
        );
        assert_eq!(d.links.first().map(String::as_str), Some("https://ex.com/a"));
        let modified = d.info.get("modified").map(String::as_str);
        assert_eq!(modified, Some("2026-10-08"), "JSON-LD read as such: {:?}", d.info);
        assert!(!d.sections.iter().flat_map(|s| &s.lines).any(|l| l.contains("@context")), "never text");
    }

    /// THE detection rule: magic, then a PDF/zip type, then an HTML page by type or start, then text documents.
    #[test]
    fn kind_of_bodies() {
        let cases: &[(&str, &[u8], &str)] = &[
            ("text/plain", b"%PDF-1.7", "pdf"),
            ("application/pdf", b"<html>", "pdf"),
            ("text/plain; charset=utf-8", b"local x = 1\n", "text"),
            ("text/csv", b"a,b\n1,2", "text"),
            ("text/markdown", b"# Title", "md"),
            ("text/plain", b"<!DOCTYPE html><title>x</title>", "html"),
            ("text/html", b"{\"a\":1}", "html"),
            ("application/json", b"not json", "json"),
            ("text/plain", b" [1, 2]", "json"),
            ("text/plain", b"[section]\nkey=1", "text"),
            ("", b"{\"a\": [1]}", "json"),
            ("", b"{ broken", "html"),
            ("application/rss+xml", b"<rss>", "xml"),
            ("", b"<?xml version=\"1.0\"?><urlset/>", "xml"),
            ("", b"<?xml version=\"1.0\"?><html xmlns=\"x\">", "html"),
            ("application/xhtml+xml", b"<?xml?><html>", "html"),
            ("", b"<p>local notes</p>", "html"),
            ("application/octet-stream", b"plain words", "html"),
        ];
        for (ct, body, want) in cases {
            assert_eq!(kind_of(ct, body), *want, "{ct} {}", String::from_utf8_lossy(body));
        }
    }

    #[test]
    fn charset_order() {
        let cz = b"<meta charset=\"windows-1250\"><p>\x8Elu\x9D cena 120 \x80</p>".to_vec();
        let d = |b: &[u8], h| {
            let (t, n) = decode(b.to_vec(), h);
            (String::from_utf8(t).unwrap_or_default(), n)
        };
        assert!(d(&cz, None).0.contains("Žluť cena 120 €") && d(&cz, None).1 == "windows-1250");
        let eq = b"<META HTTP-EQUIV='Content-Type' CONTENT='text/html; charset=ISO-8859-2'><p>\xAE</p>";
        assert_eq!(
            d(eq, None),
            ("<META HTTP-EQUIV='Content-Type' CONTENT='text/html; charset=ISO-8859-2'><p>Ž</p>".into(), "ISO-8859-2")
        );
        assert_eq!(d(&cz, Some("utf-8")).1, "", "the header wins over the page");
        assert_eq!(
            d(b"\xEF\xBB\xBF<meta charset=windows-1250>\xC5\xBD", Some("iso-8859-1")).0,
            "<meta charset=windows-1250>Ž"
        );
        assert_eq!(d(b"<meta charset=utf-16><p>x</p>", None).1, "", "utf-16 from ASCII bytes = UTF-8");
        assert_eq!(d(b"<p>plain \xC5\xBD</p>", None), ("<p>plain Ž</p>".into(), ""));
        assert_eq!(d(b"<meta name=x><meta charset=bogus>", None).1, "", "an unknown label is ignored");
    }

    #[test]
    fn facts_and_fold() {
        let line = [Line::T("Preis 1 299,99 € · 2026-10-11 · 7 · m² 45 · EUR 12 · $30 · 40 USD · 50% off".into())];
        let f = facts(&line, false);
        assert!(f.contains("299,99") && f.contains("2026-10-11") && f.contains("45") && !f.contains("7"));
        let priced: BTreeSet<String> = facts(&line, true).into_iter().collect();
        assert_eq!(priced.into_iter().collect::<Vec<_>>(), ["12", "299,99", "30", "40"], "next to € EUR $ USD");
        assert_eq!(fold("Žluťoučký ĚŠ Café"), "zlutoucky es cafe");
        assert_eq!(fold("Brand™ ½ Ｆｕｌｌ ﬁ"), "brandtm 1⁄2 full fi");
        assert_eq!(fold("한").chars().count(), 3);
        assert_eq!(collapse("  a \u{a0}\n b  "), "a b");
        let ours = [Line::T("Home [l10] · Shop [l11] · 12 items [l12]".into())];
        assert_eq!(facts(&ours, false).into_iter().collect::<Vec<_>>(), ["12"], "[lN] ids are ours");
    }

    /// A page with no digits and 40 nav links: light counted 32 "numbers" (our ids l10..l40), so the guard's
    /// mode was set by link ids, not by the page.
    #[test]
    fn link_ids_are_not_page_numbers() {
        let nav: String = (0..40).map(|i| format!("<a href='/p{i}'>page</a> ")).collect();
        let h = format!("<nav>{nav}</nav><h1>Words</h1><p>Only words here, no digits at all.</p>");
        let d = build_doc(&parse(&h), "https://ex.com/", "https://ex.com/", false);
        assert_eq!((d.mode.as_str(), d.loss.light_facts, d.loss.vs_light_lost), ("clean", 0, 0));
    }

    /// `<pre>` lines break where Python's `splitlines` breaks (`\r\n` once, form feed, U+2028); a combining mark
    /// at either end of a table range is dropped, its neighbours outside the range are kept.
    #[test]
    fn pre_lines_and_combining_edges() {
        let h = "<pre>a\r\nb\x0cc\u{2028}d\r\r\ne  \n\n</pre>";
        let d = build_doc(&parse(h), "https://x.io/", "https://x.io/", false);
        assert_eq!(d.sections[0].lines, ["a", "b", "c", "d", "e"]);
        let (lo, hi) = COMBINING[0];
        let (last_lo, last_hi) = COMBINING[COMBINING.len() - 1];
        let f = |cp: u32| fold(&format!("x{}", char::from_u32(cp).expect("a char")));
        assert_eq!((f(lo), f(hi), f(last_lo), f(last_hi)), ("x".into(), "x".into(), "x".into(), "x".into()));
        assert_eq!(f(lo - 1).chars().count(), 2);
        assert_eq!(f(0x2FF).chars().count(), 2, "just below U+0300");
    }

    #[test]
    fn clean_prunes_chrome_and_keeps_links() {
        let h = "<html><head><title>T</title></head><body><nav><a href='/x'>menu</a></nav>\
             <h1>One</h1><p>Price <a href='/p'>12 EUR</a></p><h2>Two</h2><p>b 24 36 48</p>\
             <footer>foot 2026</footer></body></html>";
        let d = build_doc(&parse(h), "https://ex.com/", "https://ex.com/", false);
        assert_eq!(d.links, vec!["https://ex.com/p".to_string()]);
        assert_eq!(d.sections[0].lines, vec!["Price 12 EUR [l1]".to_string()]);
        assert_eq!((d.mode.as_str(), d.loss.vs_light_lost), ("clean", 1)); // "2026" sat in the footer: 1 of 5
    }

    /// A `<button>`'s text is on screen (trivago's hotel cards are buttons: 44 facts were dropped blind); a
    /// button in chrome (a search form) goes with its region.
    #[test]
    fn button_text_is_page_text() {
        let h = "<h1>Hotels in Seville</h1><p>Two hotels found for your dates in the old town.</p>\
                 <div><button><span>9.1</span> Excellent (17k ratings) Seville, 0.5 miles to City center</button></div>\
                 <form><input name=q><button>Search 2.0</button></form>";
        let d = build_doc(&parse(h), "https://ex.com/", "https://ex.com/", false);
        assert!(has(&d, "9.1 Excellent (17k ratings) Seville, 0.5 miles") && !has(&d, "Search 2.0"));
    }

    /// aria-hidden text is on screen (Amazon's star line, Google's `4.7 (1.4K)`): kept in clean; an aria-hidden
    /// icon glyph is not.
    #[test]
    fn aria_hidden_text_is_on_screen() {
        let h = "<h1>Earbuds</h1><p>Wireless earbuds with a case and three tip sizes in the box.</p>\
                 <p><span aria-hidden=true>4.6 out of 5 stars</span> <span aria-hidden=true>16,544</span>\
                 <i aria-hidden=true>★</i> ratings</p>";
        let d = build_doc(&parse(h), "https://ex.com/", "https://ex.com/", false);
        assert_eq!(d.mode, "clean");
        assert!(has(&d, "4.6 out of 5 stars 16,544 ratings"));
    }

    /// Two numbers in two elements never glue into a third (coinmarketcap `$109.720.24%`, SO `4,33633`); a
    /// superscript stays glued on purpose (`419⁰⁰ €`), digits inside one text node are untouched.
    #[test]
    fn digits_at_an_element_boundary_get_a_space() {
        let h = "<h1>Coins</h1><p>Prices of the day for the top coins on the market.</p>\
                 <p><span>$109.72</span><span>0.24%</span> · <span>4,336</span><span>3</span> gold · 419<sup>00</sup> €</p>";
        let d = build_doc(&parse(h), "https://ex.com/", "https://ex.com/", false);
        assert!(has(&d, "$109.72 0.24% · 4,336 3 gold · 419⁰⁰ €"));
    }

    fn has(d: &Doc, needle: &str) -> bool {
        d.sections.iter().flat_map(|s| s.lines.iter()).any(|l| l.contains(needle))
    }

    #[test]
    fn loss_guard_measures_against_light() {
        // the price sits in a <form>: clean and hard both prune it, so comparing them never fired
        let h = include_str!("../tests/fixtures/price-rows.html");
        let d = build_doc(&parse(h), "https://store.example/p/x", "https://store.example/p/x", false);
        assert_eq!((d.mode.as_str(), d.loss.vs_light_lost), ("light", 0));
        assert!(has(&d, "Regular price") && has(&d, "13,50 €"));
    }

    /// JSON-LD values as Python reads them: keys in document order (the first object wins), a repeated key in its
    /// first place with its last value, an integer printed as written and a float with its point.
    #[test]
    fn ld_keeps_document_order_and_number_spelling() {
        let ld = |j: &str| {
            let v: Value = serde_json::from_str(j).expect("json");
            let mut objs = Vec::new();
            walk_ld(&v, 0, &mut objs);
            let mut info = IndexMap::new();
            objs.iter().for_each(|o| ld_info(o, &mut info));
            info.into_iter().collect::<Vec<_>>()
        };
        let i = ld(
            r#"{"offers":{"price":330,"priceCurrency":"EUR"},"aggregateRating":{"ratingValue":4.5,"ratingCount":12}}"#,
        );
        assert_eq!(i, [("price".into(), "330 EUR".into()), ("rating".into(), "4.5 (12 reviews)".into())]);
        assert_eq!(ld(r#"{"offers":{"price":12.0}}"#)[0].1, "12.0");
        let dup = ld(r#"{"offers":{"price":"1","priceCurrency":"EUR","price":"2"}}"#);
        assert_eq!(dup[0].1, "2 EUR", "last value, first place");
        let first = ld(r#"{"a":{"datePublished":"2026-01-01"},"b":{"datePublished":"2026-02-02"}}"#);
        assert_eq!(first[0], ("published".into(), "2026-01-01".into()), "document order, not sorted keys");
        let v: Value = serde_json::from_str(r#"{"b":1,"a":2}"#).expect("json");
        assert!(matches!(&v, Value::Object(kv) if kv.keys().eq(["b", "a"])));
        // an island prints the page's numbers as written (serde_json `float_roundtrip`; best-effort parsing printed
        // a Booking.com aspect ratio 1.4984391259105099 as 1.49843912591051)
        assert_eq!(one_space(r#"{"r":1.4984391259105099}"#), b"{\n \"r\": 1.4984391259105099\n}");
    }

    /// MEDIA: the og card first with its alt (JSON escapes decoded: `\u2019`, a surrogate pair, `\n`), size and
    /// type; the `<picture>`'s largest candidate; data:, svg, a 1 px pixel and aria-hidden dropped; the
    /// avatar-sized image and the bare `@handle` alt last, demoted.
    #[test]
    fn og_card_and_body_media() {
        let h = include_str!("../tests/fixtures/og-card.html");
        let d = build_doc(&parse(h), "https://social.example/a/status/1", "https://social.example/a/status/1", false);
        let src: Vec<&str> = d.media.iter().map(|m| m.src.as_str()).collect();
        assert_eq!(
            src,
            [
                "https://img.example/media/galaxy?format=webp&name=large",
                "https://social.example/media/arm-1600.jpg",
                // demoted, in the order the rest holds them (alt length); `share.png` is the share card
                "https://social.example/profile/other.jpg",
                "https://social.example/profile/agency_normal.jpg",
                "https://social.example/static/share.png",
            ]
        );
        let og = &d.media[0];
        assert_eq!((og.w, og.h, og.ty.as_str(), og.video), (5955, 3969, "webp", false));
        assert!(og.alt.starts_with("A starfield filled with thousands of stars"), "{og:?}");
        assert!(og.alt.ends_with(" dust. The Milky Way’s bar, seen edge-on 🌌."), "{og:?}");
        assert_eq!(d.media[1].alt, "The bar of stars");
        assert_eq!(d.media.iter().map(|m| m.demoted).collect::<Vec<_>>(), [false, false, true, true, true]);
        assert_eq!(
            unjson(r"a\u00e9 \ud83c b\x \u12"),
            "aé \u{fffd} b\\x \\u12",
            "a lone half is U+FFFD; the rest is kept"
        );
    }

    /// Honest media: one row per picture at its largest copy (the og card's 1287 px
    /// over the body's 393 px, alt from the copy that has one), the tag's hi-res or lazy source over its `src`,
    /// fillers dropped (a `pxl.gif`, a spacer, a placeholder), and listed but not counted: a JSON-LD logo, a logo or
    /// a badge by name or class, an avatar-sized square, cards linked to another page (around or laid over). A small
    /// non-square thumbnail and a picture linked to its own file page stay content; tracking parameters are cut.
    #[test]
    fn media_is_counted_honestly() {
        let h = include_str!("../tests/fixtures/media-honest.html");
        let d = build_doc(&parse(h), "https://news.example/a/river", "https://news.example/a/river", false);
        let base = |s: &str| s.trim_start_matches("https://news.example").to_string();
        let pick = |demoted: bool| d.media.iter().filter(|m| m.demoted == demoted).map(|m| base(&m.src)).collect();
        let counted: Vec<String> = pick(false);
        assert_eq!(
            counted,
            [
                "/img/2026/2511401-685193-1287x836.jpg?v=2",
                "/img/reply-box-3301.jpg",
                "/img/2026/crowd-5512-150x84.jpg",
                "/img/lantern-4410.jpg",
                "/img/hero-77001-1500.jpg",
                "/thumb/250px-Old_bridge_at_dusk.jpg",
            ]
        );
        assert_eq!(d.media[0].alt, "Boats on the river");
        let mut demoted: Vec<String> = pick(true);
        demoted.sort();
        let want = ["/b/build.png", "/brand/site_logo.jpg", "/img/2026/feed-8801-150x84.jpg"];
        let more = ["/img/2026/other-9901-393x221.jpg", "/people/editor.jpg", "/static/poster-1024x576.png"];
        assert_eq!(demoted, [want, more].concat());
        let size = |u: &str| key_of(&url::Url::parse(u).expect("url"));
        assert_eq!(size("https://x.io/a/1280px-Big_bridge.jpg").0, size("https://x.io/b/Big_bridge@2x.jpg").0);
        assert_eq!(size("https://x.io/p/AbC123=s1600").1, 1600);
        assert_eq!(size("https://x.io/p/photo_640x480.jpg?w=1200").1, 1200);
    }

    /// One picture's key is its own file name without size words,
    /// never a folder or a query a CDN sizes or crops in (a proxied original in the query is kept), so different
    /// pictures never merge; a picture linked to its own page (a gallery) counts, a feed card does not; a strip
    /// (≥ 10:1 by its name's size) is dropped; a generated share card and a short alt naming the site or a map are
    /// listed, not counted.
    #[test]
    fn media_keys_galleries_and_cards() {
        let key = |u: &str| key_of(&url::Url::parse(u).expect("url")).0;
        let wiki = "https://up.wiki/c/thumb/a/ab/Gate_of_Dawn.jpg/250px-Gate_of_Dawn.jpg";
        assert_eq!(key(wiki), key("https://up.wiki/c/a/ab/Gate_of_Dawn.jpg"));
        let cdn = "https://cdn.io/fetch/w_1456,c_limit/https%3A%2F%2Fs3.io%2Fimages%2F";
        assert_ne!(key(&format!("{cdn}acafbb9a-17a6_389x557.png")), key(&format!("{cdn}46d227b9-ef48_943x796.png")));
        assert_eq!(
            key(&format!("{cdn}acafbb9a-17a6_389x557.png")),
            key("https://cdn.io/w_40/acafbb9a-17a6_389x557.png")
        );
        assert_ne!(
            key("https://x.io/_next/image?url=%2Fa%2Fp1.jpg&w=750"),
            key("https://x.io/_next/image?url=%2Fa%2Fp2.jpg")
        );
        assert_ne!(key("https://x.io/img/IMG_2026.10.11.jpg"), key("https://x.io/img/IMG_2026.10.12.jpg"));
        // a `._The_` inside a name is no CDN suffix: two pictures of one series stay two
        assert_ne!(key("https://up.wiki/c/History._The_warrior.jpg"), key("https://up.wiki/c/History._A_helmet.jpg"));
        assert_eq!(key("https://ae.io/kf/S5501ck.jpg_960x960q75.jpg_.avif"), key("https://ae.io/kf/S5501ck.jpg"));
        let thumbs: String = (1..=3)
            .map(|i| {
                format!("<div><a href='/photos/{i}'><img src='/img/p{i}-thumb.jpg' width=120 height=120></a></div>")
            })
            .collect();
        let h = format!(
            "<meta property='og:site_name' content='Jane Photo'><main><p>My portfolio</p>{thumbs}\
             <div><a href='/blog/next-post'><img src='/img/next.jpg' alt='Next post'></a></div>\
             <p><img src='/img/1500x40whitespace.jpg'><img src='/img/jane.png' alt='Jane Photo logo'>\
             <img src='https://opengraph.example/abc/jane/portfolio'><img src='/tile.png' alt='Map'></p></main>"
        );
        let d = build_doc(&parse(&h), "https://jane.example/portfolio", "https://jane.example/portfolio", false);
        let rows: Vec<(&str, bool)> =
            d.media.iter().map(|m| (m.src.rsplit('/').next().unwrap_or_default(), m.demoted)).collect();
        let (gallery, rest) = rows.split_at(3);
        assert!(gallery.iter().all(|r| !r.1) && rest.iter().all(|r| r.1), "{rows:?}");
        assert_eq!(rest.iter().map(|r| r.0).collect::<Vec<_>>(), ["jane.png", "next.jpg", "tile.png", "portfolio"]);
    }

    /// A share card is named by whole generic words of its own file name or its host's first label: never a folder
    /// (`/product-card-image/1.jpg` is a product photo), never a brand word; a host only by an Open Graph label
    /// (`share_host`), never `social.` or `cards.` (a site's own pictures).
    #[test]
    fn share_cards_by_whole_words() {
        let card = |src: &str| {
            let h = format!(
                "<main><p>{}</p><img src='{src}' alt='a photo of the ridge at dawn'></main>",
                "Text. ".repeat(9)
            );
            build_doc(&parse(&h), "https://ex.io/p", "https://ex.io/p", false)
                .media
                .iter()
                .map(|m| m.demoted)
                .collect::<Vec<_>>()
        };
        let host = ["https://mastodon.social/a.png", "https://social.ex.io/m/a.png", "https://cards.ex.io/x/1.jpg"];
        for src in [&["/product-card-image/1.jpg", "/img/discard.jpg", "/i/blog.png"][..], &host].concat() {
            assert_eq!(card(src), [false], "{src}");
        }
        let proxied = "https://cdn.io/fetch/w_1200/https%3A%2F%2Fs3.io%2Fp%2Fog-image.png";
        let og = ["https://opengraph.gh.io/x/repo", "https://og-image.ex.app/a.png", proxied];
        for src in [&["/static/og-image.png", "/a/share_card.jpg", "/s/social-preview.jpg"][..], &og].concat() {
            assert_eq!(card(src), [true], "{src}");
        }
    }

    /// A size in the file name filters (a strip, an icon) but is never stored as declared.
    #[test]
    fn size_in_name_is_not_declared() {
        let imgs = "<img src='/a/boats-1287x836.jpg' alt='Boats'><img src='/a/rule-1500x40.jpg' alt='a rule'>\
                    <img src='/a/dot-12x12.png' alt='a dot'><img src='/a/x.jpg' width=640 height=480 alt='X'>";
        let h = format!("<main><p>{}</p>{imgs}</main>", "Text. ".repeat(9));
        let d = build_doc(&parse(&h), "https://ex.io/p", "https://ex.io/p", false);
        let rows: Vec<(&str, u32, u32)> = d.media.iter().map(|m| (m.alt.as_str(), m.w, m.h)).collect();
        assert_eq!(rows, [("Boats", 0, 0), ("X", 640, 480)]);
    }

    /// The site's name demotes a picture only as whole words: site "X" substring-matched every short alt with an x in
    /// it, so a fox photo on x.com printed `0 photos`.
    #[test]
    fn site_name_by_whole_words() {
        let demoted = |site: &str, alt: &str| {
            let h = format!(
                "<meta property='og:site_name' content='{site}'><main><p>Post</p>\
                 <img src='/a/p.jpg' alt='{alt}' width=800 height=600></main>"
            );
            let d = build_doc(&parse(&h), "https://s.example/p", "https://s.example/p", false);
            d.media.iter().map(|m| m.demoted).collect::<Vec<_>>()
        };
        assert_eq!(demoted("X", "Red fox"), [false]);
        assert_eq!(demoted("X", "X"), [true]);
        assert_eq!(demoted("Jane Photo", "Jane Photo logo"), [true]);
        assert_eq!(demoted("Jane Photo", "Jane Photo"), [true]);
        assert_eq!(demoted("Jane Photo", "Jane at the photo fair"), [false]);
    }

    #[test]
    fn digit_sup_is_superscript() {
        // `419<sup>00</sup> €` glued into "41900 €" reads as a price a hundred times too high
        let h = include_str!("../tests/fixtures/price-sup-digits.html");
        let d = build_doc(&parse(h), "https://shop.example/p", "https://shop.example/p", false);
        assert!(
            has(&d, "Regular price 419⁰⁰ €") && has(&d, "17⁹⁹ €") && has(&d, "12 m², "),
            "{:?}",
            d.sections.iter().map(|s| &s.lines).collect::<Vec<_>>()
        );
        assert!(!has(&d, "41900"));
    }

    /// Cards: a thread's comments hold their replies (their own shape inside) and stay lines, a flat list's open
    /// with a byline (a name, a `<time>`) and stay lines too; a menu's
    /// `name [lN] 33` is two texts, no card; a block holding code keeps its lines.
    #[test]
    fn cards_and_what_stays_lines() {
        let lines = |body: &str| {
            let d = build_doc(&parse(body), "https://ex.com/", "https://ex.com/", false);
            d.sections.into_iter().flat_map(|s| s.lines).collect::<Vec<_>>()
        };
        let reply = |n: usize, inner: &str| {
            format!("<div class=c><a href=/u{n}>user{n}</a><div>{n}h ago</div><div>Reply text {n}</div>{inner}</div>")
        };
        let thread: String = (1..=4).map(|n| reply(n, &reply(n + 10, ""))).collect();
        assert!(!lines(&thread).iter().any(|l| l.starts_with(CARD)), "{:?}", lines(&thread));
        // a flat comment list: a two-word name, then a `<time>` before the text; never cards
        let flat = (1..=5).map(|n| {
            format!("<div class=c><a href=/u{n}>John Smith{n}</a> <time>{n}h ago</time><p>I think option {n} wins.</p><p>Also see version 0.{n} notes.</p></div>")
        });
        let flat = lines(&flat.collect::<String>());
        assert!(!flat.iter().any(|l| l.starts_with(CARD)) && flat[0] == "John Smith1 [l1] 1h ago", "{flat:?}");
        // a date before a linked heading, a link around the whole card: cards, never a byline
        let dated = (1..=5)
            .map(|n| format!("<div class=d><time>May {n}</time><h3><a href=/p{n}>Post title {n} here</a></h3></div>"));
        let wrapped = (1..=5).map(|n| {
            format!("<div class=w><a href=/r{n}><div>Release notes {n}</div><div><time>May {n}</time></div></a></div>")
        });
        for page in [dated.collect::<String>(), wrapped.collect()] {
            assert_eq!(lines(&page).iter().filter(|l| l.starts_with(CARD)).count(), 5, "{:?}", lines(&page));
        }
        let menu: String = (1..=5).map(|n| format!("<li class=m><a href=/r{n}>Region {n}</a> 3{n}</li>")).collect();
        assert!(!lines(&format!("<ul>{menu}</ul>")).iter().any(|l| l.starts_with(CARD)));
        let code: String = (1..=4)
            .map(|n| format!("<div class=k><a href=/f{n}>fn f{n}</a><p>Since {n}.0</p><pre>f{n}()</pre></div>"))
            .collect();
        assert!(!lines(&code).iter().any(|l| l.starts_with(CARD)), "{:?}", lines(&code));
    }

    /// A card group is decided whole: every member one row when four in five are one item linked by its title,
    /// else none. The main link: the one around the heading (inside it or around it), else the one around the
    /// most text (a job beside its company's profile link); a document's sections (a heading no link holds, its
    /// hatnote link aside; a type named inside a heading) and forum posts (a name links) stay lines.
    #[test]
    fn cards_are_whole_groups_of_titled_items() {
        let doc = |body: &str| build_doc(&parse(body), "https://ex.com/", "https://ex.com/", false);
        let rows = |d: &Doc| {
            d.sections.iter().flat_map(|s| s.lines.clone()).filter(|l| l.starts_with(CARD)).collect::<Vec<_>>()
        };
        let story = |n: usize| {
            format!(
                "<div class=t><a href=/a{n}><h3>Story number {n} today</h3><p>Teaser {n}</p><span>{n} hrs ago</span></a></div>"
            )
        };
        let d = doc(&format!("<h1>News</h1>{}", (1..=4).map(story).collect::<String>()));
        assert_eq!(
            rows(&d).get(1).map(String::as_str),
            Some("▸ Story number 2 today [l2] · Teaser 2 · 2 hrs ago"),
            "{:?}",
            d.sections.iter().map(|s| (&s.title, &s.lines)).collect::<Vec<_>>()
        );
        assert_eq!(d.links[1], "https://ex.com/a2");
        let job = |n: usize| {
            format!(
                "<li class=j><a href=/co{n}>View Company Profile</a><a href=/job{n}><div>New</div><div>Senior engineer {n}</div><div>Acme Ltd</div><div>$100,000 USD</div></a></li>"
            )
        };
        let d = doc(&format!("<ul>{}</ul>", (1..=4).map(job).collect::<String>()));
        assert_eq!(rows(&d)[0], "▸ Senior engineer 1 [l2] · View Company Profile [l1] · New · Acme Ltd · $100,000 USD");
        let wiki = |n: usize| {
            format!(
                "<section class=s><h2>Topic {n}</h2><div class=hat>Main article: <a href=/t{n}>Topic {n}</a></div><p>In 19{n}0 the <a href=/w{n}>thing {n}</a> grew.</p></section>"
            )
        };
        let d = doc(&format!("<h1>History</h1>{}", (1..=5).map(wiki).collect::<String>()));
        assert!(rows(&d).is_empty() && d.sections.len() == 6, "{:?}", rows(&d));
        let method = |n: usize| {
            format!(
                "<div class=m><h3 id=f{n}><a href=#f{n}>fn set_{n}(x: u{n}) -> Self</a></h3><a href=/src{n}>source</a><p>Sets {n}.</p></div>"
            )
        };
        assert!(rows(&doc(&(1..=5).map(method).collect::<String>())).is_empty());
        let auto = |n: usize| {
            format!("<section class=impl><a href=#i{n}>§</a><h3>impl !Send{n} for <a href=/T.html>T</a></h3></section>")
        };
        assert!(rows(&doc(&(1..=5).map(auto).collect::<String>())).is_empty());
        let post = |n: usize| {
            format!(
                "<div class=p><a href=/u/user{n}>user{n}</a><span>Oct {n}, 2025</span><p>My reply number {n}.</p></div>"
            )
        };
        assert!(rows(&doc(&(1..=5).map(post).collect::<String>())).is_empty());
        // four in five → all five rows, the odd one (an unlinked heading) too: no section between the cards
        let odd = "<div class=t><h3>Our sponsor of 2026</h3><p>Ad copy</p><a href=/ad>more</a></div>";
        let d = doc(&format!("<h1>News</h1>{}{odd}", (1..=4).map(story).collect::<String>()));
        assert!(rows(&d).len() == 5 && d.sections.len() == 1, "{:?}", rows(&d));
        let d = doc(&format!("<h1>News</h1>{}{odd}{odd}{odd}", (1..=2).map(story).collect::<String>()));
        assert!(rows(&d).is_empty(), "{:?}", rows(&d));
    }

    /// A card row keeps the page's order, a repeated piece too (only the title's repeat, an alt, drops); a
    /// member holding a second heading is an article (a blog post), never one item: its sections stay; a link the
    /// members share is no title.
    #[test]
    fn card_pieces_keep_order_and_articles_stay_sections() {
        let doc = |body: &str| build_doc(&parse(body), "https://ex.com/", "https://ex.com/", false);
        let rows = |d: &Doc| {
            d.sections.iter().flat_map(|s| s.lines.clone()).filter(|l| l.starts_with(CARD)).collect::<Vec<_>>()
        };
        let note = |n: usize| {
            format!(
                "<div class=r><a href=/v{n}><h3>Release {n} notes</h3></a><img alt='Release {n} notes'><p>This release brings many small fixes to the parser and the cache layer.</p><li>Bump dependencies</li><li>Fix the docs</li><li>Bump dependencies</li></div>"
            )
        };
        let d = doc(&(1..=4).map(note).collect::<String>());
        let want = "▸ Release 1 notes [l1] · This release brings many small fixes to the parser and the cache layer. · Bump dependencies · Fix the docs · Bump dependencies";
        assert_eq!(rows(&d).first().map(String::as_str), Some(want), "{:?}", rows(&d));
        let post = |n: usize| {
            format!(
                "<article class=post><h2><a href=/p{n}>Post title number {n}</a></h2><time>Oct {n}</time><p>Intro {n}.</p><h3>Background {n}</h3><p>More {n}.</p></article>"
            )
        };
        let d = doc(&format!("<h1>Blog</h1>{}", (1..=4).map(post).collect::<String>()));
        assert!(rows(&d).is_empty() && d.sections.len() == 9, "{:?}", rows(&d));
        // a link every member shares (a store's "learn more") never titles one: the item's own link does
        let app = |n: usize| {
            format!(
                "<div class=a><div><a href=/help>Learn more about results and reviews.</a></div><div><a href=/app{n}>Markdown tool {n}</a></div><div>4.{n}</div></div>"
            )
        };
        let d = doc(&(1..=4).map(app).collect::<String>());
        let want = "▸ Markdown tool 2 [l3] · Learn more about results and reviews. [l1] · 4.2";
        assert_eq!(rows(&d).get(1).map(String::as_str), Some(want), "{:?}", rows(&d));
    }

    /// Siblings are sorted into roles in one pass: ten times the list items take about ten times as long (a
    /// pass per kid over its siblings took 22 s on a 5 MB page).
    #[test]
    fn many_siblings_are_linear() {
        let run = |n: usize| {
            let body = format!("<ul>{}</ul>", (0..n).map(|k| format!("<li>{k}</li>")).collect::<String>());
            let t = std::time::Instant::now();
            let d = build_doc(&parse(&body), "https://ex.com/", "https://ex.com/", false);
            assert_eq!(d.sections.iter().map(|s| s.lines.len()).sum::<usize>(), n);
            t.elapsed().as_secs_f64()
        };
        let (small, big) = (run(4_000), run(40_000));
        assert!(big < 40.0 * small + 0.5, "{small:.3} s → {big:.3} s");
    }
}
