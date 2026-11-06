//! Markdown to Reuben's typed document tree.

use comrak::nodes::{AstNode, ListType, NodeValue, TableAlignment};
use comrak::{Arena, Options};

use crate::doc::{Alignment, Document, Node, NodeKind, Span};

pub(crate) fn options() -> Options<'static> {
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
    // comrak numbers footnotes by first reference and moves the definitions to the end of the document in that order.
    let definitions = doc
        .children
        .iter()
        .position(|c| matches!(c.kind, NodeKind::FootnoteDefinition { .. }));
    if let Some(first) = definitions {
        let mut section = Node {
            kind: NodeKind::FootnoteSection,
            span: doc.children[first].span,
            children: doc.children.split_off(first),
        };
        for (i, def) in section.children.iter_mut().enumerate() {
            if let NodeKind::FootnoteDefinition { index, .. } = &mut def.kind {
                *index = i + 1;
            }
        }
        doc.children.push(section);
    }
    doc
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
        NodeValue::Heading(h) => NodeKind::Heading {
            level: h.level,
            id: String::new(),
        },
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
        NodeValue::Table(t) => NodeKind::Table {
            alignments: t
                .alignments
                .iter()
                .map(|a| match a {
                    TableAlignment::None => Alignment::None,
                    TableAlignment::Left => Alignment::Left,
                    TableAlignment::Center => Alignment::Center,
                    TableAlignment::Right => Alignment::Right,
                })
                .collect(),
        },
        NodeValue::TableRow(header) => NodeKind::TableRow { header: *header },
        NodeValue::TableCell => NodeKind::TableCell,
        NodeValue::Strikethrough => NodeKind::Strikethrough,
        NodeValue::FootnoteReference(r) => NodeKind::FootnoteReference {
            label: r.name.clone(),
            index: r.ix as usize,
        },
        // `parse` numbers the definitions once they're gathered into a section.
        NodeValue::FootnoteDefinition(d) => NodeKind::FootnoteDefinition {
            label: d.name.clone(),
            index: 0,
        },
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
    use crate::doc::{Alignment, NodeKind, Span};

    #[test]
    fn heading_and_paragraph_with_spans() {
        let d = parse("# Title\n\nHello *you*.\n", 4);
        assert_eq!(
            d.children[0].kind,
            NodeKind::Heading {
                level: 1,
                id: String::new()
            }
        );
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
    fn tables_are_tree_nodes() {
        let d = parse("| a | b |\n|:-|-:|\n| 1 | 2 |\n", 1);
        assert_eq!(
            d.children[0].kind,
            NodeKind::Table {
                alignments: vec![Alignment::Left, Alignment::Right]
            }
        );
        assert_eq!(
            d.children[0].children[0].kind,
            NodeKind::TableRow { header: true }
        );
        assert_eq!(
            d.children[0].children[1].kind,
            NodeKind::TableRow { header: false }
        );
        assert_eq!(
            d.children[0].children[1].children[0].kind,
            NodeKind::TableCell
        );
    }

    #[test]
    fn footnotes_are_tree_nodes() {
        let d = parse("A[^x] B[^y]\n\n[^x]: one\n[^y]: two\n", 1);
        let section = d.children.last().unwrap();
        assert!(matches!(section.kind, NodeKind::FootnoteSection));
        assert_eq!(section.children.len(), 2);
        assert_eq!(
            section.children[1].kind,
            NodeKind::FootnoteDefinition {
                label: "y".into(),
                index: 2
            }
        );
        assert_eq!(
            d.children[0].children[1].kind,
            NodeKind::FootnoteReference {
                label: "x".into(),
                index: 1
            }
        );
    }

    #[test]
    fn strikethrough_is_a_node() {
        let d = parse("~~gone~~\n", 1);
        assert_eq!(d.children[0].children[0].kind, NodeKind::Strikethrough);
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
