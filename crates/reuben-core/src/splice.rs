//! Replaces block placeholders in a parsed document with each block's output.

use std::path::Path;

use crate::doc::{Document, Fill, Node, NodeKind, Origin};
use crate::error::{Error, ErrorKind};

/// What a block produced. `Text` is escaped text from strings and scalars, `Html` is raw markup from `Html` values,
/// and `Nothing` is a setup block or `()`.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Text(String),
    Html(Markup),
    Nothing,
}

/// Raw markup plus the include lines that wrote parts of it. Each range is the byte range of `html` contributed by
/// one `Html` value that came from an include.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Markup {
    pub html: String,
    pub origins: Vec<(std::ops::Range<usize>, Origin)>,
}

impl From<&str> for Markup {
    fn from(html: &str) -> Self {
        html.to_string().into()
    }
}

impl From<String> for Markup {
    fn from(html: String) -> Self {
        Self {
            html,
            origins: Vec::new(),
        }
    }
}

impl Output {
    fn markup(&self) -> Option<&str> {
        match self {
            Output::Text(s) => Some(s),
            Output::Html(m) => Some(&m.html),
            Output::Nothing => None,
        }
    }
}

const OPEN: char = '\u{E000}';
const CLOSE: char = '\u{E001}';
/// What comrak makes of `OPEN`/`CLOSE` when it percent-encodes a link URL.
const OPEN_ENCODED: &str = "%EE%80%80";
const CLOSE_ENCODED: &str = "%EE%80%81";

pub fn splice(doc: &mut Document, outputs: &[Output]) {
    splice_children(doc, outputs, false);
}

/// One piece of a string split on placeholders.
enum Piece<'a> {
    Text(&'a str),
    Slot(usize),
}

fn pieces(s: &str) -> Vec<Piece<'_>> {
    pieces_between(s, &OPEN.to_string(), &CLOSE.to_string())
}

fn pieces_between<'a>(s: &'a str, open: &str, close: &str) -> Vec<Piece<'a>> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find(open) {
        let after = &rest[start + open.len()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let closed = digits > 0 && after[digits..].starts_with(close);
        match (closed, after[..digits].parse::<usize>()) {
            (true, Ok(n)) => {
                if start > 0 {
                    out.push(Piece::Text(&rest[..start]));
                }
                out.push(Piece::Slot(n));
                rest = &after[digits + close.len()..];
            }
            _ => {
                let end = start + open.len();
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

fn replace_pieces(
    pieces: Vec<Piece<'_>>,
    outputs: &[Output],
    f: impl Fn(&str) -> String,
) -> String {
    let mut res = String::new();
    for piece in pieces {
        match piece {
            Piece::Text(t) => res.push_str(t),
            Piece::Slot(n) => {
                if let Some(h) = output(outputs, n).markup() {
                    res.push_str(&f(h));
                }
            }
        }
    }
    res
}

/// Replaces placeholders with output HTML, mapping each output through `f`.
fn replace_with(s: &str, outputs: &[Output], f: impl Fn(&str) -> String) -> String {
    replace_pieces(pieces(s), outputs, f)
}

/// Fills an HTML block's placeholders with their output as is, recording where each output went.
fn fill_html_block(s: &str, outputs: &[Output]) -> NodeKind {
    let mut html = String::new();
    let mut fills = Vec::new();
    let mut line = 1;
    for piece in pieces(s) {
        match piece {
            Piece::Text(t) => {
                html.push_str(t);
                line += t.matches('\n').count();
            }
            Piece::Slot(block) => {
                if let Some(markup) = output(outputs, block).markup() {
                    let start = html.len();
                    html.push_str(markup);
                    fills.push(Fill {
                        range: start..html.len(),
                        block,
                        line,
                    });
                }
            }
        }
    }
    NodeKind::FilledHtmlBlock { html, fills }
}

/// Like `replace_with` for raw HTML, where a placeholder in a URL may be percent-encoded.
fn replace_in_raw(s: &str, outputs: &[Output]) -> String {
    let decoded = replace_pieces(
        pieces_between(s, OPEN_ENCODED, CLOSE_ENCODED),
        outputs,
        |h| crate::html::url_attr(&unescape(h)),
    );
    replace_with(&decoded, outputs, str::to_string)
}

/// The safety net: a placeholder that survives into finished HTML means a splice rule missed it.
pub fn ensure_no_placeholders(path: &Path, html: &str) -> Result<(), Error> {
    let left = |open: &str, close: &str| {
        pieces_between(html, open, close)
            .iter()
            .any(|p| matches!(p, Piece::Slot(_)))
    };
    if left(&OPEN.to_string(), &CLOSE.to_string()) || left(OPEN_ENCODED, CLOSE_ENCODED) {
        return Err(Error::new(path, ErrorKind::UnsplicedPlaceholder));
    }
    Ok(())
}

pub(crate) fn unescape(s: &str) -> String {
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

/// `in_image` is true inside an image's alt text, where an output must stay text so `plain_text` can read it.
fn splice_children(node: &mut Node, outputs: &[Output], in_image: bool) {
    let children = std::mem::take(&mut node.children);
    for mut child in children {
        if let Some(n) = sole_placeholder(&child) {
            match output(outputs, n) {
                Output::Html(m) => {
                    child.kind = NodeKind::Generated {
                        html: m.html.clone(),
                        block: n,
                        inline: false,
                    };
                    child.children.clear();
                    node.children.push(child);
                    continue;
                }
                Output::Nothing => continue,
                Output::Text(_) => {}
            }
        }
        if let NodeKind::Text(t) = &child.kind {
            if has_placeholder(t) {
                for piece in pieces(t) {
                    let kind = match piece {
                        Piece::Text(s) => NodeKind::Text(s.to_string()),
                        Piece::Slot(n) => match output(outputs, n) {
                            Output::Nothing => continue,
                            out if in_image => NodeKind::Text(unescape(out.markup().unwrap_or(""))),
                            Output::Html(m) => NodeKind::Generated {
                                html: m.html.clone(),
                                block: n,
                                inline: true,
                            },
                            Output::Text(s) => NodeKind::HtmlInline(s.clone()),
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
            NodeKind::HtmlBlock(s) if has_placeholder(s) => {
                child.kind = fill_html_block(s, outputs);
            }
            NodeKind::HtmlBlock(s) | NodeKind::HtmlInline(s) => {
                *s = replace_with(s, outputs, str::to_string);
            }
            NodeKind::Raw(s) => *s = replace_in_raw(s, outputs),
            NodeKind::Link { url, title } | NodeKind::Image { url, title } => {
                *url = replace_with(url, outputs, unescape);
                *title = replace_with(title, outputs, unescape);
            }
            _ => {}
        }
        let image = in_image || matches!(child.kind, NodeKind::Image { .. });
        splice_children(&mut child, outputs, image);
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
    fn generated_nodes_carry_their_block() {
        let mut d = doc(&format!("{}\n\nmid {} sentence\n", p(0), p(1)));
        splice(
            &mut d,
            &[
                Output::Html("<b>a</b>".into()),
                Output::Html("<i>b</i>".into()),
            ],
        );
        assert_eq!(
            d.children[0].kind,
            NodeKind::Generated {
                html: "<b>a</b>".into(),
                block: 0,
                inline: false
            }
        );
        let inline: Vec<_> = d.children[1]
            .children
            .iter()
            .filter(|c| matches!(c.kind, NodeKind::Generated { .. }))
            .map(|c| &c.kind)
            .collect();
        assert_eq!(
            inline,
            [&NodeKind::Generated {
                html: "<i>b</i>".into(),
                block: 1,
                inline: true
            }]
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
                &[Output::Text("&lt;T&gt;".into())]
            ),
            "<h1>A &lt;T&gt; B</h1>\n"
        );
    }
    #[test]
    fn output_in_link_url_is_not_double_escaped() {
        assert_eq!(
            html(
                &format!("[me]({})\n", p(0)),
                &[Output::Text("/a?x=1&amp;y=2".into())]
            ),
            "<p><a href=\"/a?x=1&amp;y=2\">me</a></p>\n"
        );
    }
    #[test]
    fn output_in_html_block_attribute() {
        assert_eq!(
            html(
                &format!("<div class=\"{}\">\n</div>\n", p(0)),
                &[Output::Text("note".into())]
            ),
            "<div class=\"note\">\n</div>\n"
        );
    }
    #[test]
    fn filled_html_block_records_each_fill() {
        let mut d = doc(&format!("<div>\n{}\n{}\n</div>\n", p(0), p(1)));
        splice(
            &mut d,
            &[Output::Html("<b>\n</b>".into()), Output::Text("t".into())],
        );
        let NodeKind::FilledHtmlBlock { html, fills } = &d.children[0].kind else {
            panic!("{:?}", d.children[0].kind);
        };
        assert_eq!(html, "<div>\n<b>\n</b>\nt\n</div>\n");
        assert_eq!(
            fills,
            &[
                Fill {
                    range: 6..14,
                    block: 0,
                    line: 2
                },
                Fill {
                    range: 15..16,
                    block: 1,
                    line: 3
                }
            ]
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

#[cfg(test)]
mod guard_tests {
    use super::*;

    #[test]
    fn leftover_placeholder_is_an_error() {
        let raw = crate::blocks::placeholder(3);
        assert!(ensure_no_placeholders(std::path::Path::new("a.md"), &raw).is_err());
        let encoded = "<a href=\"%EE%80%803%EE%80%81\">";
        let err = ensure_no_placeholders(std::path::Path::new("a.md"), encoded).unwrap_err();
        assert!(err.to_string().contains("a.md"), "{err}");
        assert!(ensure_no_placeholders(std::path::Path::new("a.md"), "clean").is_ok());
    }
}
