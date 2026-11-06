//! Reuben's document tree to HTML.

use std::borrow::Cow;
use std::fmt::Write;

use crate::doc::{Document, Node, NodeKind};

/// Renders a page body. Output follows the CommonMark reference renderer's layout.
pub fn render(doc: &Document) -> String {
    let mut out = String::new();
    block(&mut out, doc, false);
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

/// Percent-encodes characters that are unsafe in a URL, then escapes the result for an attribute.
fn url_attr(url: &str) -> String {
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

fn children(out: &mut String, node: &Node, tight: bool) {
    for child in &node.children {
        block(out, child, tight);
    }
}

/// Renders a node. `tight` is true for the direct contents of a tight list item, whose paragraphs drop `<p>`.
fn block(out: &mut String, node: &Node, tight: bool) {
    match &node.kind {
        NodeKind::Document => children(out, node, false),
        NodeKind::Paragraph if tight => inlines(out, node),
        NodeKind::Paragraph => {
            cr(out);
            out.push_str("<p>");
            inlines(out, node);
            out.push_str("</p>\n");
        }
        NodeKind::Heading { level } => {
            cr(out);
            let _ = write!(out, "<h{level}>");
            inlines(out, node);
            let _ = writeln!(out, "</h{level}>");
        }
        NodeKind::CodeBlock { info, literal } => {
            cr(out);
            out.push_str("<pre><code");
            if let Some(lang) = info.split_whitespace().next() {
                let _ = write!(out, " class=\"language-{}\"", escape_attr(lang));
            }
            out.push('>');
            out.push_str(&escape_text(literal));
            out.push_str("</code></pre>\n");
        }
        NodeKind::BlockQuote => {
            cr(out);
            out.push_str("<blockquote>\n");
            children(out, node, false);
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
                children(out, item, *list_tight);
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
        _ => inline(out, node),
    }
}

fn inlines(out: &mut String, node: &Node) {
    for child in &node.children {
        inline(out, child);
    }
}

fn inline(out: &mut String, node: &Node) {
    match &node.kind {
        NodeKind::Text(t) => out.push_str(&escape_text(t)),
        NodeKind::Emphasis => wrap(out, node, "em"),
        NodeKind::Strong => wrap(out, node, "strong"),
        NodeKind::Code(c) => {
            let _ = write!(out, "<code>{}</code>", escape_text(c));
        }
        NodeKind::Link { url, title } => {
            let _ = write!(out, "<a href=\"{}\"", url_attr(url));
            if !title.is_empty() {
                let _ = write!(out, " title=\"{}\"", escape_attr(title));
            }
            out.push('>');
            inlines(out, node);
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
        NodeKind::SoftBreak => out.push('\n'),
        NodeKind::LineBreak => out.push_str("<br />\n"),
        NodeKind::HtmlInline(html) | NodeKind::Raw(html) => out.push_str(html),
        _ => block(out, node, false),
    }
}

fn wrap(out: &mut String, node: &Node, tag: &str) {
    let _ = write!(out, "<{tag}>");
    inlines(out, node);
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
    fn code_block_language_class() {
        let d = crate::markdown::parse("```rust\nlet x = 1 < 2;\n```\n", 1);
        assert_eq!(
            render(&d),
            "<pre><code class=\"language-rust\">let x = 1 &lt; 2;\n</code></pre>\n"
        );
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
