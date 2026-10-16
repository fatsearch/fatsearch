//! HTML → node arena. html5ever (WHATWG tree construction) feeds a small `TreeSink`; the arena is what the
//! renderer walks. Replaces the hand-ported `html.parser` tokenizer and its 2,231-row entity table.

use crate::settings::limit;
use html5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tokenizer::TokenizerOpts;
use html5ever::tree_builder::TreeBuilderOpts;
use html5ever::{Attribute, LocalName, ParseOpts, QualName, parse_document, parse_fragment};
use std::borrow::Cow;
use std::cell::RefCell;
use std::time::{Duration, Instant};

const FEED: usize = 64 * 1024;

pub enum Kid {
    Text(String),
    Node(usize),
}

impl Kid {
    /// The element's id; a text has none.
    pub fn node(&self) -> Option<usize> {
        if let Kid::Node(c) = self { Some(*c) } else { None }
    }
}

pub struct Node {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub kids: Vec<Kid>,
    pub parent: Option<usize>,
}

impl Node {
    pub fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    }

    /// `template#deferred-comments`, `script[application/json]`, else the bare tag.
    pub fn label(&self) -> String {
        let set = |k| self.attr(k).filter(|v| !v.is_empty());
        match (set("id"), set("type")) {
            (Some(id), _) => format!("{}#{id}", self.tag),
            (None, Some(ty)) => format!("{}[{ty}]", self.tag),
            (None, None) => self.tag.clone(),
        }
    }
}

/// `nodes[0]` = the document. `cut` = bytes left unparsed when the time budget ran out.
pub struct Tree {
    pub nodes: Vec<Node>,
    pub meta: Vec<(String, String)>,
    /// Data scripts (a JSON type, or an id ending in `__`: `__NEXT_DATA__`, `__NUXT__`): (`label`, text, JSON-LD).
    pub json: Vec<(String, String, bool)>,
    pub title: String,
    pub cut: usize,
}

/// One stop of `Tree::walk`: an element or a text node.
pub enum Visit<'a> {
    Node(&'a Node),
    Text(&'a str),
}

impl Tree {
    pub fn meta(&self, k: &str) -> Option<&str> {
        self.meta.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    }

    /// Node `id` and everything below it in document order, minus the elements named in `skip` (and their
    /// subtrees). One loop over a stack: no recursion however deep the page.
    pub fn walk<'a>(&'a self, id: usize, skip: &'a [&str]) -> impl Iterator<Item = Visit<'a>> {
        let mut stack = vec![Visit::Node(&self.nodes[id])];
        std::iter::from_fn(move || {
            let step = stack.pop()?;
            if let Visit::Node(n) = step {
                stack.extend(n.kids.iter().rev().filter_map(|k| match k {
                    Kid::Text(s) => Some(Visit::Text(s)),
                    Kid::Node(c) => Some(&self.nodes[*c]).filter(|c| !skip.contains(&c.tag.as_str())).map(Visit::Node),
                }));
            }
            Some(step)
        })
    }

    /// The text nodes of `walk`.
    pub fn texts<'a>(&'a self, id: usize, skip: &'a [&str]) -> impl Iterator<Item = &'a str> {
        self.walk(id, skip).filter_map(|s| if let Visit::Text(t) = s { Some(t) } else { None })
    }
}

/// html5ever sink: handles are arena indices; comments, doctypes and PIs become detached `#` nodes.
struct Sink {
    nodes: RefCell<Vec<Node>>,
    names: RefCell<Vec<QualName>>,
}

impl Sink {
    fn new_node(&self, tag: String, q: QualName, attrs: Vec<(String, String)>) -> usize {
        let mut n = self.nodes.borrow_mut();
        n.push(Node { tag, attrs, kids: Vec::new(), parent: None });
        self.names.borrow_mut().push(q);
        n.len() - 1
    }

    fn detach(&self, id: usize) {
        let mut n = self.nodes.borrow_mut();
        if let Some(p) = n[id].parent.take() {
            n[p].kids.retain(|k| !matches!(k, Kid::Node(c) if *c == id));
        }
    }

    /// Insert at `at` in `parent`'s kids, merging adjacent text like the DOM does.
    fn insert(&self, parent: usize, at: usize, child: NodeOrText<usize>) {
        match child {
            NodeOrText::AppendText(t) => {
                let mut n = self.nodes.borrow_mut();
                let kids = &mut n[parent].kids;
                if at > 0
                    && let Some(Kid::Text(s)) = kids.get_mut(at - 1)
                {
                    s.push_str(&t);
                } else if let Some(Kid::Text(s)) = kids.get_mut(at) {
                    s.insert_str(0, &t);
                } else {
                    kids.insert(at, Kid::Text(t.to_string()));
                }
            }
            NodeOrText::AppendNode(c) => {
                self.detach(c);
                let mut n = self.nodes.borrow_mut();
                if n[c].tag.starts_with('#') {
                    return;
                }
                let at = at.min(n[parent].kids.len());
                n[parent].kids.insert(at, Kid::Node(c));
                n[c].parent = Some(parent);
            }
        }
    }
}

fn qname(local: &str) -> QualName {
    QualName::new(None, html5ever::ns!(), LocalName::from(local))
}

impl TreeSink for Sink {
    type Handle = usize;
    type Output = Self;
    type ElemName<'a> = std::cell::Ref<'a, QualName>;

    fn finish(self) -> Self {
        self
    }
    fn parse_error(&self, _msg: Cow<'static, str>) {}
    fn get_document(&self) -> usize {
        0
    }
    fn elem_name<'a>(&'a self, target: &'a usize) -> Self::ElemName<'a> {
        std::cell::Ref::map(self.names.borrow(), |v| &v[*target])
    }
    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> usize {
        let a = attrs.into_iter().map(|a| (a.name.local.to_string(), a.value.to_string())).collect();
        let id = self.new_node(name.local.to_string(), name, a);
        if flags.template {
            // template contents live in a detached fragment right after the element
            self.new_node("#fragment".into(), qname("#fragment"), Vec::new());
        }
        id
    }
    fn create_comment(&self, _text: StrTendril) -> usize {
        self.new_node("#comment".into(), qname("#comment"), Vec::new())
    }
    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> usize {
        self.new_node("#pi".into(), qname("#pi"), Vec::new())
    }
    fn append(&self, parent: &usize, child: NodeOrText<usize>) {
        let at = self.nodes.borrow()[*parent].kids.len();
        self.insert(*parent, at, child);
    }
    fn append_based_on_parent_node(&self, element: &usize, prev: &usize, child: NodeOrText<usize>) {
        if self.nodes.borrow()[*element].parent.is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev, child);
        }
    }
    fn append_doctype_to_document(&self, _: StrTendril, _: StrTendril, _: StrTendril) {}
    fn get_template_contents(&self, target: &usize) -> usize {
        target + 1
    }
    fn same_node(&self, x: &usize, y: &usize) -> bool {
        x == y
    }
    fn set_quirks_mode(&self, _mode: QuirksMode) {}
    fn append_before_sibling(&self, sibling: &usize, new_node: NodeOrText<usize>) {
        let Some(p) = self.nodes.borrow()[*sibling].parent else { return };
        let at = self.nodes.borrow()[p].kids.iter().position(|k| matches!(k, Kid::Node(c) if c == sibling));
        if let Some(i) = at {
            self.insert(p, i, new_node);
        }
    }
    fn add_attrs_if_missing(&self, target: &usize, attrs: Vec<Attribute>) {
        let mut n = self.nodes.borrow_mut();
        for a in attrs {
            let k = a.name.local.to_string();
            if n[*target].attr(&k).is_none() {
                n[*target].attrs.push((k, a.value.to_string()));
            }
        }
    }
    fn remove_from_parent(&self, target: &usize) {
        self.detach(*target);
    }
    fn reparent_children(&self, node: &usize, new_parent: &usize) {
        let mut n = self.nodes.borrow_mut();
        let kids = std::mem::take(&mut n[*node].kids);
        for c in kids.iter().filter_map(Kid::node) {
            n[c].parent = Some(*new_parent);
        }
        n[*new_parent].kids.extend(kids);
    }
}

#[cfg(test)]
thread_local!(pub static PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) });

/// Parse a document (scripting off: `<noscript>` content is markup, as a no-JS reader sees it).
pub fn parse(h: &str) -> Tree {
    #[cfg(test)]
    PARSES.with(|n| n.set(n.get() + 1));
    let sink = Sink { nodes: RefCell::new(Vec::new()), names: RefCell::new(Vec::new()) };
    sink.new_node("#document".into(), qname("#document"), Vec::new());
    let opts = ParseOpts {
        tree_builder: TreeBuilderOpts { scripting_enabled: false, ..Default::default() },
        ..Default::default()
    };
    let mut p = parse_document(sink, opts);
    // tree construction stops at `parse_budget_s` and keeps what it has (hostile nesting can make it quadratic)
    let (t0, mut at, budget) = (Instant::now(), 0, Duration::from_secs(limit("parse_budget_s")));
    while at < h.len() && t0.elapsed() < budget {
        let mut e = (at + FEED).min(h.len());
        while !h.is_char_boundary(e) {
            e += 1;
        }
        p.process(StrTendril::from_slice(&h[at..e]));
        at = e;
    }
    let sink = p.finish();
    flatten(sink.nodes.into_inner(), h.len() - at)
}

/// Document-order pass: cap depth, collect meta / JSON-LD / title.
fn flatten(mut nodes: Vec<Node>, cut: usize) -> Tree {
    let mut t = Tree { nodes: Vec::new(), meta: Vec::new(), json: Vec::new(), title: String::new(), cut };
    let mut stack = vec![(0usize, 0usize)];
    // deeper elements are attached to their ancestor at `max_depth` (Blink's parser does the same at 512), so every
    // later walk fits the default stack
    let max_depth: usize = limit("max_depth");
    while let Some((id, depth)) = stack.pop() {
        let kids = std::mem::take(&mut nodes[id].kids);
        let n = &nodes[id];
        match n.tag.as_str() {
            "meta" => {
                let get = |k: &str| n.attr(k).filter(|v| !v.is_empty());
                let key = get("property").or_else(|| get("name")).or_else(|| get("itemprop")).map(str::to_lowercase);
                if let (Some(key), Some(content)) = (key, get("content"))
                    && t.meta(&key).is_none()
                {
                    t.meta.push((key, content.to_string()));
                }
            }
            "script" | "style" => {
                let text: String =
                    kids.iter().filter_map(|k| if let Kid::Text(s) = k { Some(s.as_str()) } else { None }).collect();
                let ty = n.attr("type").unwrap_or("").to_lowercase();
                let data = ty.contains("json") || n.attr("id").is_some_and(|id| id.ends_with("__"));
                if n.tag == "script" && !text.is_empty() && data {
                    t.json.push((n.label(), text, ty.contains("ld+json")));
                }
                continue; // their text is never page text
            }
            "title" if t.title.is_empty() => {
                if let Some(Kid::Text(s)) = kids.first() {
                    t.title = py_strip(s).to_string();
                }
            }
            _ => {}
        }
        let mut kids = kids;
        if n.tag == "template" {
            stack.push((id + 1, depth.min(max_depth - 1) + 1)); // its contents (an island maybe), capped too
        }
        if depth == max_depth {
            kids = spill(&mut nodes, kids, id, &mut t.title);
        }
        stack.extend(kids.iter().rev().filter_map(Kid::node).map(|c| (c, depth + 1)));
        nodes[id].kids = kids;
    }
    t.nodes = nodes;
    t
}

/// Depth cap: everything below a node at MAX_DEPTH becomes its direct child, in document order, so a hostile
/// nest of unclosed tags costs one pass (moving nodes one at a time was quadratic: 200k `<b>` took 8.8 s).
/// script/style keep their text for the caller; the first title is read on the way.
fn spill(nodes: &mut [Node], kids: Vec<Kid>, owner: usize, title: &mut String) -> Vec<Kid> {
    let mut out = Vec::with_capacity(kids.len());
    let mut todo: Vec<Kid> = kids.into_iter().rev().collect();
    while let Some(k) = todo.pop() {
        let Kid::Node(c) = k else {
            out.push(k);
            continue;
        };
        nodes[c].parent = Some(owner);
        out.push(Kid::Node(c));
        if matches!(nodes[c].tag.as_str(), "script" | "style") {
            continue;
        }
        let below = std::mem::take(&mut nodes[c].kids);
        if nodes[c].tag == "title"
            && title.is_empty()
            && let Some(Kid::Text(s)) = below.first()
        {
            *title = py_strip(s).to_string();
        }
        todo.extend(below.into_iter().rev());
    }
    out
}

/// Python `str.isspace()` / regex `\s` on str: Unicode White_Space plus the ASCII separators 0x1C–0x1F.
pub fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

pub fn py_strip(s: &str) -> &str {
    s.trim_matches(py_space)
}

/// Character references decoded the way the parser decodes `<textarea>` text (WHATWG RCDATA, so `\r\n` → `\n` and
/// `&#1;` stays a control, as in page text): for text outside the parser (JSON-LD values, feed titles, the raw HTML a
/// wall rule reads). Replaces a hand port of CPython's `html.unescape`.
pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let sink = Sink { nodes: RefCell::new(Vec::new()), names: RefCell::new(Vec::new()) };
    sink.new_node("#document".into(), qname("#document"), Vec::new());
    let opts =
        ParseOpts { tokenizer: TokenizerOpts { discard_bom: false, ..Default::default() }, ..Default::default() };
    let textarea = QualName::new(None, html5ever::ns!(html), LocalName::from("textarea"));
    let nodes = parse_fragment(sink, opts, textarea, Vec::new(), false).one(s).nodes.into_inner();
    nodes.into_iter().flat_map(|n| n.kids).filter_map(|k| if let Kid::Text(t) = k { Some(t) } else { None }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_matches_cpython() {
        assert_eq!(unescape("a &amp; b &lt;c&gt;"), "a & b <c>");
        assert_eq!(unescape("&copy2026 x"), "©2026 x");
        assert_eq!(unescape("&#x41;&#66;&#0;&#128;"), "AB\u{fffd}€");
        assert_eq!(unescape("&nosuch; & &#;"), "&nosuch; & &#;");
        assert_eq!(unescape("&notit; <b>&amp;</b></textarea>"), "¬it; <b>&</b></textarea>");
        // the parser's rule, not CPython's: newlines normalised, a control reference kept
        assert_eq!(unescape("a\r\nb&#1;"), "a\nb\u{1}");
    }

    fn tags(t: &Tree, id: usize) -> Vec<&str> {
        t.nodes[id].kids.iter().filter_map(Kid::node).map(|c| t.nodes[c].tag.as_str()).collect()
    }

    #[test]
    fn tree_rules() {
        let t = parse(
            "<title> T </title><meta property=og:title content=X><p>a<div>b</div><ul><li>1<li>2</ul>\
             <script type='application/ld+json'>{\"a\":1}</script><!-- c --><a href=\"x\" title='y>z' hidden>t</a>",
        );
        assert_eq!((t.title.as_str(), t.meta("og:title"), t.json.iter().filter(|j| j.2).count()), ("T", Some("X"), 1));
        let body = t.nodes.iter().position(|n| n.tag == "body").unwrap();
        assert_eq!(tags(&t, body), ["p", "div", "ul", "script", "a"]); // div closes p; comment dropped
        let a = t.nodes.iter().find(|n| n.tag == "a").unwrap();
        assert_eq!((a.attr("title"), a.attr("hidden")), (Some("y>z"), Some("")));
    }

    #[test]
    fn depth_is_capped_and_hostile_input_ends() {
        let max_depth: usize = limit("max_depth");
        let deep = format!("{}x", "<div>".repeat(3 * max_depth));
        let t = parse(&deep);
        // walk from the root: reparenting can give a child a lower index than its parent
        let (mut stack, mut max) = (vec![(0usize, 0usize)], 0);
        while let Some((i, d)) = stack.pop() {
            max = max.max(d);
            stack.extend(t.nodes[i].kids.iter().filter_map(Kid::node).map(|c| (c, d + 1)));
        }
        assert!(max > max_depth / 2 && max <= max_depth + 1, "depth {max}");
        let nest = format!("{}x", "<b>".repeat(200_000));
        let t0 = Instant::now();
        assert_eq!(parse(&nest).cut, 0);
        // the quadratic spill took 8.8 s (7bf579b), one pass 0.43 s at load 2: 4 s still catches it on a loaded machine
        assert!(t0.elapsed() < Duration::from_secs(4), "spill is one pass: {:?}", t0.elapsed());
        let hostile = format!("{}{}", "<div>".repeat(3999), "</q>".repeat(200_000));
        let t0 = Instant::now();
        let _ = parse(&hostile);
        // the budget + one feed chunk + finish: 0.67 s at load 2 (no cut); + 3 s leaves a loaded machine room
        assert!(t0.elapsed() < Duration::from_secs(limit::<u64>("parse_budget_s") + 3), "{:?}", t0.elapsed());
    }
}
