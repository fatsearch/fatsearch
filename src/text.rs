//! Text documents (`clean::Lists::text`: plain text, CSV, Markdown, JSON, XML incl. RSS/Atom/sitemaps) → Doc, served as
//! they are: each line one row, no id (`lN` names a link, so a numbered line read as one: `? lN unknown`), 300 lines
//! per section (`u1#2` = lines 301-600). Never the page path: a text body judged as a page reads as a JS shell and goes
//! to every provider.

use crate::clean::{Doc, Section, collapse, pretty};
use crate::outline::{Page, Shown, paged};
use serde_json::Value;

/// Lines per section.
pub const LINES: usize = 300;
/// Names an outline lists before `+N`.
const LIST: usize = 20;

/// Bytes (UTF-8, decoded by the fetch) → Doc with `mode` = the kind; `info.outline` = JSON top-level keys, XML
/// root children with counts, or a feed's item titles.
pub fn build(raw: &[u8], kind: &str, url: &str, fin: &str) -> Doc {
    let text = String::from_utf8_lossy(raw);
    let all: Vec<&str> = text.lines().collect();
    let chunk = |(i, c): (usize, &[&str])| Section {
        level: 1,
        title: format!("lines {}-{}", i * LINES + 1, i * LINES + c.len()),
        lines: c.iter().map(|l| l.trim_end().into()).collect(),
    };
    let sections: Vec<Section> = all.chunks(LINES).enumerate().map(chunk).collect();
    let outline = match kind {
        "json" => serde_json::from_slice(raw).map(|v| json_outline(&v)).unwrap_or_default(),
        "xml" => xml_outline(&text),
        _ => String::new(),
    };
    let info = [("outline".to_string(), outline)].into_iter().filter(|(_, o)| !o.is_empty()).collect();
    Doc { info, ..Doc::new(url, fin, crate::pdf::file_name(fin), sections, kind) }
}

/// Lines in a text document.
pub fn lines(d: &Doc) -> usize {
    d.sections.iter().map(|s| s.lines.len()).sum()
}

fn list(names: impl Iterator<Item = String>) -> String {
    let all: Vec<String> = names.collect();
    let more = if all.len() > LIST { format!(" +{}", all.len() - LIST) } else { String::new() };
    format!("{}{more}", all[..all.len().min(LIST)].join(", "))
}

/// `keys: ok, data[20], meta{3}` for an object; `120 items · item keys: id, name` for an array; keys in document
/// order (as the server wrote them); else the value, cut at 60 chars.
fn json_outline(v: &Value) -> String {
    let shape = |v: &Value| match v {
        Value::Array(a) => format!("[{}]", a.len()),
        Value::Object(o) => format!("{{{}}}", o.len()),
        _ => String::new(),
    };
    match &v {
        Value::Object(o) => format!("keys: {}", list(o.iter().map(|(k, x)| format!("{k}{}", shape(x))))),
        Value::Array(a) => {
            let keys =
                a.first().and_then(Value::as_object).map(|o| format!(" · item keys: {}", list(o.keys().cloned())));
            format!("{} items{}", a.len(), keys.unwrap_or_default())
        }
        _ => crate::lang::short(&v.to_string(), 60),
    }
}

/// A path into a JSON document (`fats u5 .data.chains`, `.data.chains[0].name`): the value as JSON, pretty; one `[]`
/// reads the rest of the path in every item, a value a row (a second `[]` is refused, never read as `.`). A path that is not there names the keys where it stops:
/// `? u5 .data.foo · keys at .data: currency, chains[12]`.
pub fn path(p: &Page, path: &str, (off, limit): (usize, Option<usize>)) -> Result<Shown, String> {
    let body = p.doc.sections.iter().map(|s| s.lines.join("\n")).collect::<Vec<_>>().join("\n");
    let root: Value = serde_json::from_str(&body).map_err(|e| format!("? {} {path} · {e}", p.id))?;
    let (head, each) = path.split_once("[]").map_or((path, None), |(h, t)| (h, Some(t)));
    each.filter(|r| r.contains("[]")).map_or(Ok(()), |_| Err(format!("? {} {path} · one [] per path", p.id)))?;
    let value = root.pointer(&pointer(head)).ok_or_else(|| stop(&p.id, path, &root, head))?;
    let (kind, rows) = match each {
        None => (json_outline(value), pretty(value).lines().map(String::from).collect()),
        Some(rest) => {
            let items = value.as_array().map_or(&[][..], Vec::as_slice);
            let found: Vec<String> =
                items.iter().filter_map(|v| Some(v.pointer(&pointer(rest))?.to_string())).collect();
            // none found: where the first item's path stops (a value that is no array: the path stops at it)
            if found.is_empty() && value.as_array().is_none_or(|a| !a.is_empty()) {
                return Err(stop(&p.id, path, &root, &format!("{head}[0]{rest}")));
            }
            (format!("{} values", found.len()), found)
        }
    };
    let head = format!("= {} {path} · {kind} · {}tok", p.eq, crate::lang::ksize(crate::clean::tok(&rows.join("\n"))));
    Ok(paged(&p.id, head, &rows, (off, limit, crate::settings::limit("read_cap"), path)))
}

/// `.data.chains[0]` → the JSON pointer `/data/chains/0` (RFC 6901; `.` alone = the whole document): a key holding
/// `.`, `[` or `/` is out of reach.
fn pointer(path: &str) -> String {
    path.replace(['[', ']'], ".").split('.').filter(|s| !s.is_empty()).map(|s| format!("/{s}")).collect()
}

/// Where `path` stops in `root`: the keys there, else what is there (`.[0]` = a root array's first item).
fn stop(id: &str, typed: &str, root: &Value, path: &str) -> String {
    let ptr = pointer(path);
    let found = ptr.match_indices('/').rev().find_map(|(i, _)| Some((&ptr[..i], root.pointer(&ptr[..i])?)));
    let (prefix, here) = found.unwrap_or(("", root));
    let step = |s: &str| if s.parse::<usize>().is_ok() { format!("[{s}]") } else { format!(".{s}") };
    let at = format!(".{}", prefix.split('/').skip(1).map(step).collect::<String>().trim_start_matches('.'));
    match json_outline(here) {
        shape if shape.starts_with("keys: ") => format!("? {id} {typed} · keys at {at}: {}", &shape[6..]),
        shape => format!("? {id} {typed} · {at} is {shape}"),
    }
}

/// A feed (`item`, `entry`): `12 items: title · title`; else `<urlset> url ×500`: the root's child elements
/// with counts. A tag scanner: comments, CDATA, `<?…?>` and `<!…>` are skipped; names keep their prefix
/// (`media:title` is not an item's title).
fn xml_outline(t: &str) -> String {
    let (mut depth, mut i, mut root) = (0usize, 0, String::new());
    let (mut kids, mut titles, mut items) = (indexmap::IndexMap::<String, usize>::new(), Vec::<String>::new(), 0);
    // the open item's depth, and where its own `<title>` text starts
    let (mut item, mut title): (Option<usize>, Option<usize>) = (None, None);
    while let Some(o) = t[i..].find('<') {
        let (s, tail) = (i + o, &t[i + o..]);
        let skip = [("<!--", "-->"), ("<![CDATA[", "]]>")].iter().find(|(a, _)| tail.starts_with(a));
        let close = skip.map_or(">", |(_, b)| b);
        let end = tail.find(close).map_or(tail.len(), |e| e + close.len());
        i = s + end;
        let tag = &tail[..end];
        if skip.is_some() || tag.starts_with("<?") || tag.starts_with("<!") {
            continue;
        }
        let stop = |c: char| c.is_whitespace() || c == '>' || c == '/';
        let name = tag.trim_start_matches(['<', '/']).split(stop).next().unwrap_or_default().to_string();
        if tag.starts_with("</") {
            depth = depth.saturating_sub(1);
            if let Some(a) = title.take().filter(|_| name == "title") {
                let x = t[a..s].trim().trim_start_matches("<![CDATA[").trim_end_matches("]]>");
                titles.push(collapse(&crate::html::unescape(x)));
            }
            item = item.filter(|&d| d != depth);
            continue;
        }
        match depth {
            0 if root.is_empty() => root = name.clone(),
            1 => *kids.entry(name.clone()).or_default() += 1, // in document order
            _ => {}
        }
        let open = !tag.ends_with("/>");
        if matches!(name.as_str(), "item" | "entry") {
            items += 1;
            item = item.or(open.then_some(depth));
        } else if name == "title" && open && item.is_some_and(|d| d + 1 == depth) {
            title = Some(i);
        }
        depth += usize::from(open);
    }
    if items > 0 {
        return format!("{items} items: {}", titles.into_iter().take(LIST).collect::<Vec<_>>().join(" · "));
    }
    let kids = kids.into_iter().map(|(n, c)| if c > 1 { format!("{n} ×{c}") } else { n });
    format!("<{root}> {}", list(kids)).trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_get_no_ids_and_sections_of_300() {
        let body: String = (1..=301).map(|i| format!("row {i}\n")).collect();
        let d = build(body.as_bytes(), "text", "https://a.example/x.csv", "https://a.example/x.csv");
        assert_eq!((d.sections.len(), d.title.as_str(), d.mode.as_str()), (2, "x.csv", "text"));
        assert_eq!((d.sections[0].lines[0].as_str(), d.sections[1].title.as_str()), ("row 1", "lines 301-301"));
        assert_eq!(d.sections[1].lines, vec!["row 301".to_string()]);
        assert_eq!(build(b"a \n\nb", "text", "u", "u").sections[0].lines, vec!["a", "", "b"]);
    }

    #[test]
    fn outlines() {
        let j = |s: &str| json_outline(&serde_json::from_str(s).expect("json"));
        assert_eq!(j(r#"{"ok":true,"data":[1,2],"meta":{"a":1}}"#), "keys: ok, data[2], meta{1}");
        assert_eq!(j(r#"[{"id":1,"name":"a"},{"id":2}]"#), "2 items · item keys: id, name");
        let rss = r#"<?xml version="1.0"?><rss><channel><title>Feed</title><item><title><![CDATA[First &amp; one]]></title>
            <media:title>x</media:title></item><item><title>Second</title></item></channel></rss>"#;
        assert_eq!(xml_outline(rss), "2 items: First & one · Second");
        let atom = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>F</title><entry><title type="html">A</title></entry></feed>"#;
        assert_eq!(xml_outline(atom), "1 items: A");
        let map = "<!-- s --><urlset><url><loc>a</loc></url><url><loc>b</loc></url><xs:note/></urlset>";
        assert_eq!(xml_outline(map), "<urlset> url ×2, xs:note");
    }
}
