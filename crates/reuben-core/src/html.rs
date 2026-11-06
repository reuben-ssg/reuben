//! Reuben's document tree to HTML.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write;

use crate::doc::{Alignment, Document, Node, NodeKind};
use crate::highlight::{Highlighter, default_highlighter};

/// State the writer needs across the tree: the code highlighter, and footnote bookkeeping (how many times each footnote is referenced (for the
/// times each footnote is referenced, for the backrefs, and how many references to each it has written so far,
/// for repeated reference ids).
struct Footnotes<'a> {
    highlighter: &'a dyn Highlighter,
    totals: HashMap<usize, u32>,
    written: RefCell<HashMap<usize, u32>>,
}

impl<'a> Footnotes<'a> {
    fn of(doc: &Document, highlighter: &'a dyn Highlighter) -> Self {
        fn walk(node: &Node, totals: &mut HashMap<usize, u32>) {
            if let NodeKind::FootnoteReference { index, .. } = node.kind {
                *totals.entry(index).or_default() += 1;
            }
            node.children.iter().for_each(|c| walk(c, totals));
        }
        let mut totals = HashMap::new();
        walk(doc, &mut totals);
        Footnotes {
            highlighter,
            totals,
            written: RefCell::default(),
        }
    }
}

/// Renders a page body. Output follows the CommonMark reference renderer's layout.
pub fn render(doc: &Document) -> String {
    render_with(doc, default_highlighter())
}

/// Like [`render`], with a caller-supplied code highlighter.
pub fn render_with(doc: &Document, highlighter: &dyn Highlighter) -> String {
    let mut out = String::new();
    block(&mut out, doc, false, &Footnotes::of(doc, highlighter));
    out
}

pub fn escape_text(s: &str) -> Cow<'_, str> {
    escape(s)
}

pub fn escape_attr(s: &str) -> Cow<'_, str> {
    escape(s)
}

fn escape(s: &str) -> Cow<'_, str> {
    if !s.contains(['&', '<', '>', '"']) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Escapes script output. Unlike Markdown text it also maps `'`, since a layout may put a value in a
/// single-quoted attribute.
pub fn escape_output(s: &str) -> String {
    escape(s).replace('\'', "&#39;")
}

/// Percent-encodes characters that are unsafe in a URL, then escapes the result for an attribute.
pub(crate) fn url_attr(url: &str) -> String {
    const SAFE: &str = "-_.!~*'();/?:@&=+$,%#[]";
    let mut encoded = String::with_capacity(url.len());
    for c in url.chars() {
        if c.is_ascii_alphanumeric() || SAFE.contains(c) {
            encoded.push(c);
        } else {
            let mut buf = [0; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                let _ = write!(encoded, "%{b:02X}");
            }
        }
    }
    escape_attr(&encoded).into_owned()
}

/// Starts a new line unless output already ends with one.
fn cr(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

fn children(out: &mut String, node: &Node, tight: bool, fns: &Footnotes) {
    for child in &node.children {
        block(out, child, tight, fns);
    }
}

/// Renders a node. `tight` is true for the direct contents of a tight list item, whose paragraphs drop `<p>`.
fn block(out: &mut String, node: &Node, tight: bool, fns: &Footnotes) {
    match &node.kind {
        NodeKind::Document => children(out, node, false, fns),
        NodeKind::Paragraph if tight => inlines(out, node, fns),
        NodeKind::Paragraph => {
            cr(out);
            out.push_str("<p>");
            inlines(out, node, fns);
            out.push_str("</p>\n");
        }
        NodeKind::Heading { level, id } => {
            cr(out);
            if id.is_empty() {
                let _ = write!(out, "<h{level}>");
            } else {
                let _ = write!(out, "<h{level} id=\"{}\">", escape_attr(id));
            }
            inlines(out, node, fns);
            let _ = writeln!(out, "</h{level}>");
        }
        NodeKind::CodeBlock { info, literal } => {
            cr(out);
            out.push_str("<pre><code");
            if let Some(lang) = info.split_whitespace().next() {
                let _ = write!(out, " class=\"language-{}\"", escape_attr(lang));
            }
            out.push('>');
            let highlighted = info
                .split_whitespace()
                .next()
                .and_then(|lang| fns.highlighter.highlight(lang, literal));
            match highlighted {
                Some(html) => out.push_str(&html),
                None => out.push_str(&escape_text(literal)),
            }
            out.push_str("</code></pre>\n");
        }
        NodeKind::BlockQuote => {
            cr(out);
            out.push_str("<blockquote>\n");
            children(out, node, false, fns);
            cr(out);
            out.push_str("</blockquote>\n");
        }
        NodeKind::List {
            ordered,
            start,
            tight: list_tight,
        } => {
            cr(out);
            match (ordered, start) {
                (false, _) => out.push_str("<ul>\n"),
                (true, 1) => out.push_str("<ol>\n"),
                (true, n) => {
                    let _ = writeln!(out, "<ol start=\"{n}\">");
                }
            }
            for item in &node.children {
                cr(out);
                match &item.kind {
                    NodeKind::Raw(html) => {
                        out.push_str(html);
                        continue;
                    }
                    NodeKind::TaskItem { checked } => {
                        out.push_str("<li><input type=\"checkbox\" disabled=\"\"");
                        if *checked {
                            out.push_str(" checked=\"\"");
                        }
                        out.push_str(" /> ");
                    }
                    _ => out.push_str("<li>"),
                }
                children(out, item, *list_tight, fns);
                if !*list_tight {
                    cr(out);
                }
                out.push_str("</li>\n");
            }
            out.push_str(if *ordered { "</ol>\n" } else { "</ul>\n" });
        }
        NodeKind::ThematicBreak => {
            cr(out);
            out.push_str("<hr />\n");
        }
        NodeKind::HtmlBlock(html) | NodeKind::Raw(html) => {
            cr(out);
            out.push_str(html);
        }
        NodeKind::Table { alignments } => table(out, node, alignments, fns),
        NodeKind::FootnoteSection => footnote_section(out, node, fns),
        NodeKind::TableRow { .. } | NodeKind::TableCell | NodeKind::FootnoteDefinition { .. } => {
            children(out, node, false, fns)
        }
        _ => inline(out, node, fns),
    }
}

fn table(out: &mut String, node: &Node, alignments: &[Alignment], fns: &Footnotes) {
    cr(out);
    out.push_str("<table>\n");
    for (i, row) in node.children.iter().enumerate() {
        let header = matches!(row.kind, NodeKind::TableRow { header: true });
        cr(out);
        if header {
            out.push_str("<thead>\n");
        } else if i > 0
            && matches!(
                node.children[i - 1].kind,
                NodeKind::TableRow { header: true }
            )
        {
            out.push_str("<tbody>\n");
        }
        out.push_str("<tr>");
        let tag = if header { "th" } else { "td" };
        for (col, cell) in row.children.iter().enumerate() {
            cr(out);
            let _ = write!(out, "<{tag}");
            match alignments.get(col) {
                Some(Alignment::Left) => out.push_str(" align=\"left\""),
                Some(Alignment::Right) => out.push_str(" align=\"right\""),
                Some(Alignment::Center) => out.push_str(" align=\"center\""),
                _ => {}
            }
            out.push('>');
            inlines(out, cell, fns);
            let _ = write!(out, "</{tag}>");
        }
        cr(out);
        out.push_str("</tr>");
        if header {
            cr(out);
            out.push_str("</thead>");
        }
    }
    if node.children.len() > 1 {
        cr(out);
        out.push_str("</tbody>\n");
    }
    cr(out);
    out.push_str("</table>\n");
}

fn footnote_section(out: &mut String, node: &Node, fns: &Footnotes) {
    cr(out);
    out.push_str("<section class=\"footnotes\" data-footnotes>\n<ol>\n");
    for def in &node.children {
        let NodeKind::FootnoteDefinition { label, index } = &def.kind else {
            continue;
        };
        let _ = write!(out, "<li id=\"fn-{}\">", url_attr(label));
        let backref = footnote_backref(label, *index, fns.totals.get(index).copied().unwrap_or(0));
        let (last, rest) = match def.children.split_last() {
            Some((last, rest)) if last.kind == NodeKind::Paragraph => (Some(last), rest),
            _ => (None, def.children.as_slice()),
        };
        for child in rest {
            block(out, child, false, fns);
        }
        match last {
            Some(p) => {
                cr(out);
                out.push_str("<p>");
                inlines(out, p, fns);
                out.push(' ');
                out.push_str(&backref);
                out.push_str("</p>\n");
            }
            None => {
                out.push_str(&backref);
                out.push('\n');
            }
        }
        out.push_str("</li>\n");
    }
    out.push_str("</ol>\n</section>\n");
}

/// One back-link per reference to the footnote, the second onwards numbered.
fn footnote_backref(label: &str, index: usize, total: u32) -> String {
    let label = url_attr(label);
    let mut html = String::new();
    for n in 1..=total {
        let (suffix, sup) = if n > 1 {
            html.push(' ');
            (
                format!("-{n}"),
                format!("<sup class=\"footnote-ref\">{n}</sup>"),
            )
        } else {
            (String::new(), String::new())
        };
        let _ = write!(
            html,
            "<a href=\"#fnref-{label}{suffix}\" class=\"footnote-backref\" data-footnote-backref data-footnote-backref-idx=\"{index}{suffix}\" aria-label=\"Back to reference {index}{suffix}\">↩{sup}</a>"
        );
    }
    html
}

fn inlines(out: &mut String, node: &Node, fns: &Footnotes) {
    for child in &node.children {
        inline(out, child, fns);
    }
}

fn inline(out: &mut String, node: &Node, fns: &Footnotes) {
    match &node.kind {
        NodeKind::Text(t) => out.push_str(&escape_text(t)),
        NodeKind::Emphasis => wrap(out, node, "em", fns),
        NodeKind::Strong => wrap(out, node, "strong", fns),
        NodeKind::Strikethrough => wrap(out, node, "del", fns),
        NodeKind::Code(c) => {
            let _ = write!(out, "<code>{}</code>", escape_text(c));
        }
        NodeKind::Link { url, title } => {
            let _ = write!(out, "<a href=\"{}\"", url_attr(url));
            if !title.is_empty() {
                let _ = write!(out, " title=\"{}\"", escape_attr(title));
            }
            out.push('>');
            inlines(out, node, fns);
            out.push_str("</a>");
        }
        NodeKind::Image { url, title } => {
            let _ = write!(
                out,
                "<img src=\"{}\" alt=\"{}\"",
                url_attr(url),
                escape_attr(&plain_text(node))
            );
            if !title.is_empty() {
                let _ = write!(out, " title=\"{}\"", escape_attr(title));
            }
            out.push_str(" />");
        }
        NodeKind::FootnoteReference { label, index } => {
            let n = {
                let mut written = fns.written.borrow_mut();
                let n = written.entry(*index).or_default();
                *n += 1;
                *n
            };
            let suffix = if n > 1 {
                format!("-{n}")
            } else {
                String::new()
            };
            let _ = write!(
                out,
                "<sup class=\"footnote-ref\"><a href=\"#fn-{label}\" id=\"fnref-{label}{suffix}\" data-footnote-ref>{index}</a></sup>",
                label = url_attr(label),
                suffix = url_attr(&suffix),
            );
        }
        NodeKind::SoftBreak => out.push('\n'),
        NodeKind::LineBreak => out.push_str("<br />\n"),
        // `math::render_math` normally replaces these first; this keeps the source visible if it didn't run.
        NodeKind::Math { display, latex } => {
            let d = if *display { "$$" } else { "$" };
            let _ = write!(out, "{d}{}{d}", escape_text(latex));
        }
        NodeKind::HtmlInline(html) | NodeKind::Raw(html) => out.push_str(html),
        _ => block(out, node, false, fns),
    }
}

fn wrap(out: &mut String, node: &Node, tag: &str, fns: &Footnotes) {
    let _ = write!(out, "<{tag}>");
    inlines(out, node, fns);
    let _ = write!(out, "</{tag}>");
}

/// The text content of a node, used for image alt text.
pub fn plain_text(node: &Node) -> String {
    let mut s = String::new();
    collect_text(node, &mut s);
    s
}

fn collect_text(node: &Node, s: &mut String) {
    match &node.kind {
        NodeKind::Text(t) | NodeKind::Code(t) => s.push_str(t),
        NodeKind::SoftBreak | NodeKind::LineBreak => s.push(' '),
        _ => node.children.iter().for_each(|c| collect_text(c, s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_text_and_attributes() {
        let d = crate::markdown::parse("[a < b](/a?x=1&y=\"2\")\n", 1);
        assert_eq!(
            render(&d),
            "<p><a href=\"/a?x=1&amp;y=%222%22\">a &lt; b</a></p>\n"
        );
    }

    #[test]
    fn code_block_uses_highlighter() {
        let d = crate::markdown::parse("```rust\nfn a() {}\n```\n", 1);
        assert!(render(&d).contains("<pre><code class=\"language-rust\"><span class=\"hl-"));
    }

    #[test]
    fn unknown_language_stays_plain() {
        let d = crate::markdown::parse("```rhai\nlet x = 1 < 2;\n```\n", 1);
        assert_eq!(
            render(&d),
            "<pre><code class=\"language-rhai\">let x = 1 &lt; 2;\n</code></pre>\n"
        );
    }

    #[test]
    fn code_block_language_class() {
        let html = render(&crate::markdown::parse("```rust\nlet x = 1 < 2;\n```\n", 1));
        assert!(html.starts_with("<pre><code class=\"language-rust\"><span class=\"hl-"));
        assert!(html.contains("&lt;"));
    }

    #[test]
    fn table_footnote_and_strike_html_match_comrak() {
        let inputs = [
            "| a | b |\n|:-|:-:|\n| 1 | 2 |\n",
            "| a | b | c |\n|--|-:|---|\n| 1 | **2** | [x](/y) |\n| | | |\n",
            "| a |\n|---|\n",
            "x[^1]\n\n[^1]: note\n",
            "A[^a] B[^b] A again[^a]\n\n[^a]: one\n[^b]: two\n",
            "A[^a]\n\n[^a]: one\n\n    ```\n    code\n    ```\n",
            "A[^a]\n\n[^a]: one\n\n    second\n",
            "~~gone~~ and *em*\n",
        ];
        for input in inputs {
            let expected = comrak::markdown_to_html(input, &crate::markdown::options());
            assert_eq!(
                render(&crate::markdown::parse(input, 1)),
                expected,
                "{input:?}"
            );
        }
    }

    #[test]
    fn snapshot_mixed_document() {
        let d = crate::markdown::parse(include_str!("../tests/fixtures/mixed.md"), 1);
        insta::assert_snapshot!(render(&d));
    }
}

#[cfg(test)]
mod review_fixes {
    use super::*;

    #[test]
    fn task_list_items_keep_content() {
        let html = render(&crate::markdown::parse("- [ ] todo\n- [x] done\n", 1));
        assert!(html.contains("todo"), "{html}");
        assert!(html.contains("done"), "{html}");
        assert_eq!(html.matches("type=\"checkbox\"").count(), 2, "{html}");
        assert_eq!(html.matches("checked").count(), 1, "{html}");
    }

    #[test]
    fn footnotes_share_one_section() {
        let html = render(&crate::markdown::parse(
            "A[^1] B[^2]\n\n[^1]: one\n[^2]: two\n",
            1,
        ));
        assert_eq!(html.matches("<section").count(), 1, "{html}");
        assert_eq!(html.matches("<ol").count(), 1, "{html}");
        assert!(html.contains("one") && html.contains("two"), "{html}");
    }
}
