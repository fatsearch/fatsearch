//! PDF bytes → Doc: one section per page, so `u3#4` is page 4. Text comes from pdf_oxide's per-page extractor;
//! no OCR, so a page without a text layer stays an empty section and the outline counts it.

use crate::clean::{Doc, Section, collapse, fold, is_word};
use pdf_oxide::PdfDocument;
use std::time::{Duration, Instant};

/// Bytes → Doc with `mode = "pdf"`. `info` keeps the PDF's own title (when it is one) and `read_ms`, the time the
/// text extraction took, so the outline can show the per-page cost. A page whose extraction fails reads as empty.
/// Pages that would start after `budget_s` seconds (`fats config` `pdf_budget_s`) are not read (page 1 always is);
/// `cut` = how many.
pub fn build_pdf(raw: &[u8], url: &str, fin: &str, budget_s: u64) -> Result<Doc, String> {
    let t0 = Instant::now();
    let pdf = PdfDocument::from_bytes(raw.to_vec()).map_err(|e| format!("pdf: {e}"))?;
    let n = pdf.page_count().map_err(|e| format!("pdf: {e}"))?;
    let page = |i| Section {
        level: 1,
        title: format!("page {}", i + 1),
        lines: pdf.extract_text(i).map(|t| t.lines().filter_map(line).collect()).unwrap_or_default(),
    };
    let read = (0..n).take_while(|&i| i == 0 || t0.elapsed() < Duration::from_secs(budget_s));
    let sections: Vec<Section> = read.map(page).collect();
    let cut = n - sections.len();
    let first = sections.first().map(|s| fold(&s.lines.join(" "))).unwrap_or_default();
    let title = info_title(&pdf).filter(|t| on_page(t, &first));
    let mut info: indexmap::IndexMap<_, _> = title.iter().map(|t| ("title".to_string(), t.clone())).collect();
    info.insert("read_ms".into(), t0.elapsed().as_millis().to_string());
    Ok(Doc { info, cut, ..Doc::new(url, fin, title.unwrap_or_else(|| file_name(fin)), sections, "pdf") })
}

/// Pages with no text at all (scans, image-only pages), 1-based.
pub fn empty_pages(d: &Doc) -> Vec<usize> {
    d.sections.iter().enumerate().filter(|(_, s)| s.lines.is_empty()).map(|(i, _)| i + 1).collect()
}

/// A file whose end is missing: no `%%EOF` in its last 1024 bytes (the marker ends every PDF; readers look for
/// it in the last 1024 bytes, PDF Reference 1.7 Appendix H note 18) and a page without text, the page the cut
/// went through. pdf_oxide rebuilds such a file from what is there, so a cut that missed every page (an
/// end-of-file line short) reads whole and is not flagged. A missing end with an image-only page is flagged too:
/// from the bytes alone the two look the same.
pub fn cut_short(raw: &[u8], d: &Doc) -> bool {
    let tail = &raw[raw.len().saturating_sub(1024)..];
    !tail.windows(5).any(|w| w == b"%%EOF") && !empty_pages(d).is_empty()
}

/// The `startxref` offset lands on the cross-reference it names: `xref`, or an `N G obj` stream (PDF 1.7 §7.5.5,
/// §7.5.8). A copy whose bytes shifted does not (Oxylabs' `\xNN` text decoded where the file held that text
/// itself, `provider::file_bytes`); pdf_oxide rebuilds such a file and silently loses text, so it is checked
/// here. No `startxref` in the last 1024 bytes, or no number after it: nothing to check (`cut_short` judges a
/// missing end).
pub fn xref_ok(raw: &[u8]) -> bool {
    let from = raw.len().saturating_sub(1024);
    let Some(i) = raw[from..].windows(9).rposition(|w| w == b"startxref") else { return true };
    fn num(b: &[u8]) -> Option<(usize, &[u8])> {
        let b = b.trim_ascii_start();
        let n = b.iter().take_while(|c| c.is_ascii_digit()).count();
        std::str::from_utf8(&b[..n]).ok()?.parse::<usize>().ok().map(|v| (v, &b[n..]))
    }
    let Some((at, _)) = num(&raw[from + i + 9..]) else { return true };
    let Some(t) = raw.get(at..).map(<[u8]>::trim_ascii_start) else { return false };
    let obj = num(t).and_then(|(_, r)| num(r)).is_some_and(|(_, r)| r.trim_ascii_start().starts_with(b"obj"));
    t.starts_with(b"xref") || obj
}

/// One extracted line: whitespace collapsed, dot leaders (`. . . . 23`, `.......`) folded to `…`; None if blank.
fn line(s: &str) -> Option<String> {
    let s = collapse(s);
    let words: Vec<&str> = s.split(' ').collect();
    let dots = |w: &str| !w.is_empty() && w.chars().all(|c| c == '.');
    // a run of dot words is one leader: up to 3 dots kept, more = `…`
    let leader = |run: &[&str]| match run.iter().map(|w| w.len()).sum::<usize>() {
        _ if !dots(run[0]) => run[0].to_string(),
        n @ 1..=3 => ".".repeat(n),
        _ => "…".into(),
    };
    let l = words.chunk_by(|a, b| dots(a) && dots(b)).map(leader).collect::<Vec<_>>().join(" ");
    (!l.is_empty()).then_some(l)
}

/// The Info dictionary's `/Title`, decoded (UTF-16 with BOM, else Latin-1); None when absent or blank.
fn info_title(pdf: &PdfDocument) -> Option<String> {
    let info = pdf.resolve_object(pdf.trailer().as_dict()?.get("Info")?).ok()?;
    let t = pdf.resolve_object(info.as_dict()?.get("Title")?).ok()?;
    let b = t.as_string()?;
    let utf16 = |unit: fn([u8; 2]) -> u16| {
        let u: Vec<u16> = b[2..].as_chunks::<2>().0.iter().copied().map(unit).collect();
        String::from_utf16_lossy(&u)
    };
    let s = match b {
        [0xFE, 0xFF, ..] => utf16(u16::from_be_bytes),
        [0xFF, 0xFE, ..] => utf16(u16::from_le_bytes),
        _ => b.iter().map(|&c| c as char).collect(),
    };
    let s = collapse(&s);
    (!s.is_empty() && !s.chars().any(char::is_control)).then_some(s)
}

/// A real title shares at least half of its words (3+ letters) with page 1. Rejects tool names and file names
/// left in the metadata (`printmgr file`, `2025.09.02_Kyiv_meniu_A3_darbinis`).
fn on_page(title: &str, page1: &str) -> bool {
    let f = fold(title);
    let words: Vec<&str> = f.split(|c: char| !is_word(c) || c == '_').filter(|w| w.chars().count() >= 3).collect();
    let hits = words.iter().filter(|w| page1.contains(**w)).count();
    title.contains(' ') && !words.is_empty() && hits * 2 >= words.len()
}

/// Last path segment of the URL (`file:///tmp/menu.pdf` → `menu.pdf`; a Windows file's `\` too), without a query.
pub fn file_name(u: &str) -> String {
    let p = u.split(['?', '#']).next().unwrap_or(u);
    let name = p.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(p);
    if name.is_empty() { u.to_string() } else { name.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_fold_dot_leaders_and_spaces() {
        assert_eq!(line("TUNA TARTARE  wontons . . . . . . . 23").as_deref(), Some("TUNA TARTARE wontons … 23"));
        assert_eq!(line("Contents ........ 4").as_deref(), Some("Contents … 4"));
        assert_eq!(line("Inc. 2025.").as_deref(), Some("Inc. 2025."));
        assert_eq!(line("a ... b").as_deref(), Some("a ... b"));
        assert_eq!(line("   "), None);
    }

    #[test]
    fn metadata_title_must_be_on_page_one() {
        let p1 = fold("Token Efficient Task Execution via Application Behavior Modeling Alexandru Ianta");
        assert!(on_page("Token Efficient Task Execution via Application Behavior Modeling for Web Agents", &p1));
        assert!(!on_page("printmgr file", &fold("BERKSHIRE HATHAWAY INC. 2025 ANNUAL REPORT")));
        assert!(!on_page("2025.09.02_Kyiv_meniu_A3_darbinis", &fold("Kyiv meniu")));
    }

    /// `startxref` lands on its table in a whole file; one Oxylabs `\xNN` decoded where the file had that text
    /// (4 bytes → 1, before the table) moves it off.
    #[test]
    fn xref_offset_checks_the_copy() {
        let whole: &[u8] = include_bytes!("../tests/fixtures/literal-escape.pdf");
        assert!(xref_ok(whole) && xref_ok(include_bytes!("../tests/fixtures/three-pages.pdf")));
        let at = whole.windows(4).position(|w| w == b"\\x9c").expect("the literal text");
        let shifted = [&whole[..at], &[0x9c], &whole[at + 4..]].concat();
        assert!(!xref_ok(&shifted));
        assert!(xref_ok(b"%PDF-1.4 no end"), "nothing to check");
        let stream = b"%PDF-1.5\n7 0 obj\n<< /Type /XRef >>\nstartxref\n9\n%%EOF";
        assert!(xref_ok(stream), "an xref stream");
        assert!(!xref_ok(b"%PDF-1.5\nstartxref\n99999\n%%EOF"), "past the end");
    }

    #[test]
    fn file_name_of_url() {
        assert_eq!(file_name("file:///tmp/a/menu.pdf"), "menu.pdf");
        assert_eq!(file_name(r"file://C:\tmp\menu.pdf"), "menu.pdf");
        assert_eq!(file_name("https://x.example/doc.pdf?x=1"), "doc.pdf");
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(build_pdf(b"%PDF-1.7 garbage", "u", "u", 10).is_err());
    }

    /// A missing end-of-file line is tolerated when every page reads; a cut through a page is not.
    #[test]
    fn cut_short_needs_a_lost_page() {
        let pdf: &[u8] = include_bytes!("../tests/fixtures/three-pages.pdf");
        let read =
            |b: &[u8]| build_pdf(b, "u", "u", 10).map(|d| (d.sections.len() - empty_pages(&d).len(), cut_short(b, &d)));
        assert_eq!(read(pdf), Ok((3, false)));
        assert_eq!(read(&pdf[..pdf.len() - 6]), Ok((3, false)), "`%%EOF` line lost, all 3 pages read");
        let mid = pdf.windows(7).position(|w| w == b"5 0 obj").expect("page 2") + 40;
        assert_eq!(read(&pdf[..mid]), Ok((1, true)), "cut inside page 2");
    }

    #[test]
    fn budget_cuts_at_a_page() {
        let pdf = include_bytes!("../tests/fixtures/two-pages.pdf");
        let d = build_pdf(pdf, "u", "u", 10).expect("pdf");
        assert_eq!((d.sections.len(), d.cut), (2, 0));
        let d = build_pdf(pdf, "u", "u", 0).expect("pdf");
        assert_eq!((d.sections.len(), d.cut), (1, 1), "page 1 is always read");
    }
}
