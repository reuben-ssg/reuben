//! Markdown to Reuben's typed document tree.

use comrak::nodes::{AstNode, ListType, NodeValue};
use comrak::{Arena, Options};

use crate::doc::{Document, Node, NodeKind, Span};

fn options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    options.render.r#unsafe = true;
    options
}

/// Parses a page body. `body_line` is the file line the body starts on, so spans point into the whole file.
pub fn parse(body: &str, body_line: usize) -> Document {
    let arena = Arena::new();
    let options = options();
    let root = comrak::parse_document(&arena, body, &options);
    let mut doc = convert(root, body_line - 1, &options);
    if let Some(footnotes) = footnotes_section(root, body_line - 1, &options) {
        doc.children.push(footnotes);
    }
    doc
}

fn is_footnote_definition(node: &AstNode<'_>) -> bool {
    matches!(node.data.borrow().value, NodeValue::FootnoteDefinition(_))
}

/// All footnote definitions as one `<section>`. comrak only numbers and wraps them correctly when it renders the
/// whole document, so render it once and keep the footnotes section. `convert` skips the individual definitions.
fn footnotes_section<'a>(
    root: &'a AstNode<'a>,
    line_offset: usize,
    options: &Options,
) -> Option<Node> {
    let first = root.children().find(|c| is_footnote_definition(c))?;
    let span = Span {
        line: first.data.borrow().sourcepos.start.line + line_offset,
        column: 1,
    };
    let mut html = String::new();
    comrak::format_html(root, options, &mut html).expect("writing to a String cannot fail");
    let start = html.find("<section class=\"footnotes\"")?;
    Some(Node {
        kind: NodeKind::Raw(html[start..].to_string()),
        span,
        children: Vec::new(),
    })
}

fn convert<'a>(node: &'a AstNode<'a>, line_offset: usize, options: &Options) -> Node {
    let ast = node.data.borrow();
    let span = Span {
        line: ast.sourcepos.start.line + line_offset,
        column: ast.sourcepos.start.column,
    };
    let kind = match &ast.value {
        NodeValue::Document => NodeKind::Document,
        NodeValue::Paragraph => NodeKind::Paragraph,
        NodeValue::Heading(h) => NodeKind::Heading { level: h.level },
        NodeValue::Text(t) => NodeKind::Text(t.to_string()),
        NodeValue::Emph => NodeKind::Emphasis,
        NodeValue::Strong => NodeKind::Strong,
        NodeValue::Code(c) => NodeKind::Code(c.literal.clone()),
        NodeValue::CodeBlock(c) => NodeKind::CodeBlock {
            info: c.info.clone(),
            literal: c.literal.clone(),
        },
        NodeValue::Link(l) => NodeKind::Link {
            url: l.url.clone(),
            title: l.title.clone(),
        },
        NodeValue::Image(l) => NodeKind::Image {
            url: l.url.clone(),
            title: l.title.clone(),
        },
        NodeValue::List(l) => NodeKind::List {
            ordered: l.list_type == ListType::Ordered,
            start: l.start,
            tight: l.tight,
        },
        NodeValue::Item(_) => NodeKind::Item,
        NodeValue::TaskItem(t) => NodeKind::TaskItem {
            checked: t.symbol.is_some_and(|c| c != ' '),
        },
        NodeValue::BlockQuote => NodeKind::BlockQuote,
        NodeValue::ThematicBreak => NodeKind::ThematicBreak,
        NodeValue::SoftBreak => NodeKind::SoftBreak,
        NodeValue::LineBreak => NodeKind::LineBreak,
        NodeValue::HtmlBlock(h) => NodeKind::HtmlBlock(h.literal.clone()),
        NodeValue::HtmlInline(h) => NodeKind::HtmlInline(h.clone()),
        _ => {
            drop(ast);
            let mut html = String::new();
            comrak::format_html(node, options, &mut html).expect("writing to a String cannot fail");
            return Node {
                kind: NodeKind::Raw(html),
                span,
                children: Vec::new(),
            };
        }
    };
    drop(ast);
    Node {
        kind,
        span,
        children: node
            .children()
            .filter(|child| !is_footnote_definition(child))
            .map(|child| convert(child, line_offset, options))
            .collect(),
    }
}

/// Byte ranges of `body` covered by code blocks and inline code, where `{{` must stay literal.
pub fn code_ranges(body: &str) -> Vec<std::ops::Range<usize>> {
    let mut line_starts = vec![0];
    line_starts.extend(body.match_indices('\n').map(|(i, _)| i + 1));
    let offset = |line: usize, column: usize| {
        let start = line_starts.get(line - 1).copied().unwrap_or(body.len());
        (start + column - 1).min(body.len())
    };
    let arena = Arena::new();
    let root = comrak::parse_document(&arena, body, &options());
    root.descendants()
        .filter_map(|node| {
            let ast = node.data.borrow();
            if !matches!(ast.value, NodeValue::CodeBlock(_) | NodeValue::Code(_)) {
                return None;
            }
            let (start, end) = (ast.sourcepos.start, ast.sourcepos.end);
            Some(offset(start.line, start.column)..offset(end.line, end.column + 1))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{NodeKind, Span};

    #[test]
    fn heading_and_paragraph_with_spans() {
        let d = parse("# Title\n\nHello *you*.\n", 4);
        assert_eq!(d.children[0].kind, NodeKind::Heading { level: 1 });
        assert_eq!(d.children[0].span, Span { line: 4, column: 1 });
        assert_eq!(d.children[1].kind, NodeKind::Paragraph);
        assert_eq!(d.children[1].span.line, 6);
        assert_eq!(d.children[1].children[1].kind, NodeKind::Emphasis);
    }

    #[test]
    fn code_block_keeps_info_and_literal() {
        let d = parse("```rust\nfn main() {}\n```\n", 1);
        assert_eq!(
            d.children[0].kind,
            NodeKind::CodeBlock {
                info: "rust".into(),
                literal: "fn main() {}\n".into()
            }
        );
    }

    #[test]
    fn link_and_image() {
        let d = parse("[a](/x \"t\") ![alt](/i.png)\n", 1);
        let p = &d.children[0];
        assert_eq!(
            p.children[0].kind,
            NodeKind::Link {
                url: "/x".into(),
                title: "t".into()
            }
        );
        assert!(matches!(p.children[2].kind, NodeKind::Image { .. }));
    }

    #[test]
    fn raw_html_and_comments_pass_through() {
        let d = parse("<!-- vale off -->\n\n<div>hi</div>\n", 1);
        assert!(matches!(d.children[0].kind, NodeKind::HtmlBlock(_)));
    }

    #[test]
    fn unmodeled_nodes_become_raw() {
        let d = parse("| a |\n|---|\n| b |\n", 1);
        assert!(matches!(&d.children[0].kind, NodeKind::Raw(h) if h.contains("<table>")));
    }

    #[test]
    fn code_ranges_are_exact_for_spans_and_indented_blocks() {
        let body = "a `{{ x }}` b\n\n    {{ z }}\n";
        let slices: Vec<&str> = code_ranges(body).iter().map(|r| &body[r.clone()]).collect();
        assert_eq!(slices[0], "`{{ x }}`");
        assert!(slices[1].contains("{{ z }}"));
    }

    #[test]
    fn code_ranges_cover_fences_and_spans() {
        let body = "a `{{ x }}` b\n\n```\n{{ y }}\n```\n";
        let r = code_ranges(body);
        assert!(r.iter().any(|r| body[r.clone()].contains("{{ x }}")));
        assert!(r.iter().any(|r| body[r.clone()].contains("{{ y }}")));
    }
}
