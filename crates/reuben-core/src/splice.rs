//! Replaces block placeholders in a parsed document with each block's output.

use crate::doc::{Document, Node, NodeKind};

/// What a block produced. `Html` is already escaped for its context; `Nothing` is a setup block or `()`.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Html(String),
    Nothing,
}

const OPEN: char = '\u{E000}';
const CLOSE: char = '\u{E001}';

pub fn splice(doc: &mut Document, outputs: &[Output]) {
    splice_children(doc, outputs);
}

/// One piece of a string split on placeholders.
enum Piece<'a> {
    Text(&'a str),
    Slot(usize),
}

fn pieces(s: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start + OPEN.len_utf8()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let closed = digits > 0 && after[digits..].starts_with(CLOSE);
        match (closed, after[..digits].parse::<usize>()) {
            (true, Ok(n)) => {
                if start > 0 {
                    out.push(Piece::Text(&rest[..start]));
                }
                out.push(Piece::Slot(n));
                rest = &after[digits + CLOSE.len_utf8()..];
            }
            _ => {
                let end = start + OPEN.len_utf8();
                out.push(Piece::Text(&rest[..end]));
                rest = &rest[end..];
            }
        }
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest));
    }
    out
}

fn has_placeholder(s: &str) -> bool {
    pieces(s).iter().any(|p| matches!(p, Piece::Slot(_)))
}

fn output(outputs: &[Output], n: usize) -> &Output {
    debug_assert!(n < outputs.len(), "placeholder {n} has no output");
    outputs.get(n).unwrap_or(&Output::Nothing)
}

/// Replaces placeholders with output HTML, mapping each output through `f`.
fn replace_with(s: &str, outputs: &[Output], f: impl Fn(&str) -> String) -> String {
    let mut res = String::new();
    for piece in pieces(s) {
        match piece {
            Piece::Text(t) => res.push_str(t),
            Piece::Slot(n) => {
                if let Output::Html(h) = output(outputs, n) {
                    res.push_str(&f(h));
                }
            }
        }
    }
    res
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// The slot index when the paragraph holds one placeholder and nothing else but whitespace.
fn sole_placeholder(node: &Node) -> Option<usize> {
    if node.kind != NodeKind::Paragraph {
        return None;
    }
    let mut slot = None;
    for child in &node.children {
        match &child.kind {
            NodeKind::SoftBreak => {}
            NodeKind::Text(t) => {
                for piece in pieces(t) {
                    match piece {
                        Piece::Text(s) if s.trim().is_empty() => {}
                        Piece::Slot(n) if slot.is_none() => slot = Some(n),
                        _ => return None,
                    }
                }
            }
            _ => return None,
        }
    }
    slot
}

fn splice_children(node: &mut Node, outputs: &[Output]) {
    let children = std::mem::take(&mut node.children);
    for mut child in children {
        if let Some(n) = sole_placeholder(&child) {
            if let Output::Html(h) = output(outputs, n) {
                child.kind = NodeKind::HtmlBlock(h.clone());
                child.children.clear();
                node.children.push(child);
            }
            continue;
        }
        if let NodeKind::Text(t) = &child.kind {
            if has_placeholder(t) {
                for piece in pieces(t) {
                    let kind = match piece {
                        Piece::Text(s) => NodeKind::Text(s.to_string()),
                        Piece::Slot(n) => match output(outputs, n) {
                            Output::Html(h) => NodeKind::HtmlInline(h.clone()),
                            Output::Nothing => continue,
                        },
                    };
                    node.children.push(Node {
                        kind,
                        span: child.span,
                        children: Vec::new(),
                    });
                }
                continue;
            }
        }
        match &mut child.kind {
            NodeKind::HtmlBlock(s) | NodeKind::HtmlInline(s) | NodeKind::Raw(s) => {
                *s = replace_with(s, outputs, str::to_string);
            }
            NodeKind::Link { url, title } | NodeKind::Image { url, title } => {
                *url = replace_with(url, outputs, unescape);
                *title = replace_with(title, outputs, unescape);
            }
            _ => {}
        }
        splice_children(&mut child, outputs);
        node.children.push(child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(md: &str) -> Document {
        crate::markdown::parse(md, 1)
    }
    fn p(n: usize) -> String {
        crate::blocks::placeholder(n)
    }
    fn html(md: &str, outputs: &[Output]) -> String {
        let mut d = doc(md);
        splice(&mut d, outputs);
        crate::html::render(&d)
    }

    #[test]
    fn block_level_output_replaces_paragraph() {
        assert_eq!(
            html(
                &format!("{}\n", p(0)),
                &[Output::Html("<div>x</div>".into())]
            ),
            "<div>x</div>"
        );
    }
    #[test]
    fn setup_block_paragraph_disappears() {
        assert_eq!(
            html(&format!("{}\n\nHi\n", p(0)), &[Output::Nothing]),
            "<p>Hi</p>\n"
        );
    }
    #[test]
    fn inline_output_in_heading() {
        assert_eq!(
            html(
                &format!("# A {} B\n", p(0)),
                &[Output::Html("&lt;T&gt;".into())]
            ),
            "<h1>A &lt;T&gt; B</h1>\n"
        );
    }
    #[test]
    fn output_in_link_url_is_not_double_escaped() {
        assert_eq!(
            html(
                &format!("[me]({})\n", p(0)),
                &[Output::Html("/a?x=1&amp;y=2".into())]
            ),
            "<p><a href=\"/a?x=1&amp;y=2\">me</a></p>\n"
        );
    }
    #[test]
    fn output_in_html_block_attribute() {
        assert_eq!(
            html(
                &format!("<div class=\"{}\">\n</div>\n", p(0)),
                &[Output::Html("note".into())]
            ),
            "<div class=\"note\">\n</div>\n"
        );
    }
    #[test]
    fn output_in_table_cell() {
        assert!(
            html(
                &format!("| a |\n|---|\n| {} |\n", p(0)),
                &[Output::Html("<b>z</b>".into())]
            )
            .contains("<td><b>z</b></td>")
        );
    }
}
