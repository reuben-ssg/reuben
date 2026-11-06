//! Heading ids and the table of contents, computed on the final tree.

use std::collections::{HashMap, HashSet};

use crate::blocks::strip_placeholders;
use crate::doc::{Document, Node, NodeKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    pub level: u8,
    pub id: String,
    pub text: String,
}

/// GitHub's slug rule, without de-duplication.
pub fn slug(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

/// The `page.toc` value: an array of `#{ level, id, text }`.
pub fn toc_array(toc: &[TocEntry]) -> rhai::Array {
    toc.iter()
        .map(|e| {
            let mut m = rhai::Map::new();
            m.insert("level".into(), i64::from(e.level).into());
            m.insert("id".into(), e.id.clone().into());
            m.insert("text".into(), e.text.clone().into());
            m.into()
        })
        .collect()
}

#[derive(Default)]
struct Ids {
    used: HashSet<String>,
    next: HashMap<String, usize>,
}

impl Ids {
    /// A `-n` suffix can collide with a heading literally named that way, so candidates are probed until one is free.
    fn claim(&mut self, base: &str) -> String {
        let mut candidate = base.to_string();
        while self.used.contains(&candidate) {
            let n = self.next.entry(base.to_string()).or_insert(1);
            candidate = format!("{base}-{n}");
            *n += 1;
        }
        self.used.insert(candidate.clone());
        candidate
    }
}

/// Gives every heading a unique id, in document order, and returns the table of contents.
pub fn assign_ids(doc: &mut Document) -> Vec<TocEntry> {
    let mut ids = Ids::default();
    let mut toc = Vec::new();
    walk(doc, &mut ids, &mut toc, false);
    toc
}

/// Like [`assign_ids`] for a tree whose blocks haven't run. A heading holding a block placeholder
/// has unknown text, so it is left out of the toc and doesn't take a place in the id sequence.
pub fn assign_ids_before_blocks(doc: &mut Document) -> Vec<TocEntry> {
    let mut ids = Ids::default();
    let mut toc = Vec::new();
    walk(doc, &mut ids, &mut toc, true);
    toc
}

fn walk(node: &mut Node, ids: &mut Ids, toc: &mut Vec<TocEntry>, skip_placeholders: bool) {
    if let NodeKind::Heading { level, id } = &mut node.kind {
        let mut raw = String::new();
        node.children.iter().for_each(|c| collect(c, &mut raw));
        if skip_placeholders && raw.contains('\u{E000}') {
            return;
        }
        let text = strip_placeholders(&raw).trim().to_string();
        let base = match slug(&text) {
            s if s.is_empty() => "section".to_string(),
            s => s,
        };
        *id = ids.claim(&base);
        toc.push(TocEntry {
            level: *level,
            id: id.clone(),
            text,
        });
        return;
    }
    for child in &mut node.children {
        walk(child, ids, toc, skip_placeholders);
    }
}

/// Unlike `html::plain_text`, this reads spliced `{{ }}` output: tags are dropped and basic entities decoded.
fn collect(node: &Node, s: &mut String) {
    match &node.kind {
        NodeKind::Text(t) | NodeKind::Code(t) => s.push_str(t),
        NodeKind::Math { latex, .. } => s.push_str(latex),
        NodeKind::SoftBreak | NodeKind::LineBreak => s.push(' '),
        NodeKind::HtmlInline(h) | NodeKind::Raw(h) => s.push_str(&decode(&strip_tags(h))),
        _ => node.children.iter().for_each(|c| collect(c, s)),
    }
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn decode(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_slugs() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(slug("Café & Crème"), "café--crème");
        assert_eq!(slug("snake_case and-dash"), "snake_case-and-dash");
        assert_eq!(slug("Version 2.0"), "version-20");
    }

    #[test]
    fn duplicates_and_empty_headings() {
        let mut d = crate::markdown::parse("# Intro\n\n## Intro\n\n##\n\n## Intro\n", 1);
        let toc = assign_ids(&mut d);
        let ids: Vec<_> = toc.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["intro", "intro-1", "section", "intro-2"]);
    }

    #[test]
    fn explicit_suffix_does_not_collide() {
        let mut d = crate::markdown::parse("# A\n\n# A\n\n# A 1\n\n# A-1\n", 1);
        let ids: Vec<_> = assign_ids(&mut d).into_iter().map(|e| e.id).collect();
        assert_eq!(ids, ["a", "a-1", "a-1-1", "a-1-2"]);
    }

    #[test]
    fn spliced_output_counts_as_text() {
        let mut d = crate::markdown::parse("# Rye\n", 1);
        d.children[0].children.push(Node {
            kind: NodeKind::HtmlInline(" &amp; <em>Co</em>".into()),
            span: Default::default(),
            children: Vec::new(),
        });
        let toc = assign_ids(&mut d);
        assert_eq!(toc[0].text, "Rye & Co");
        assert_eq!(toc[0].id, "rye--co");
    }
}
