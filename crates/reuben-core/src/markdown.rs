//! Markdown to Reuben's typed document tree.

use comrak::nodes::{AstNode, ListType, NodeValue, TableAlignment};
use comrak::{Arena, Options};

use crate::blocks::{Extracted, SourceMap};
use crate::doc::{Alignment, Document, Node, NodeKind, Span};

pub(crate) fn options(math: bool) -> Options<'static> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    options.extension.math_dollars = math;
    options.parse.smart = true;
    options.render.r#unsafe = true;
    options
}

/// Parses a page body. `body_line` is the file line the body starts on, so spans point into the whole file.
pub fn parse(body: &str, body_line: usize) -> Document {
    parse_with(body, body_line, false)
}

/// Like `parse`, with `$...$` and `$$...$$` read as math when `math` is set.
pub fn parse_with(body: &str, body_line: usize, math: bool) -> Document {
    parse_mapped(body, &SourceMap::default(), body_line, math)
}

/// Parses a body whose blocks were extracted, with spans pointing where each node is in the body before extraction.
pub fn parse_extracted(extracted: &Extracted, body_line: usize, math: bool) -> Document {
    #[cfg(test)]
    PARSES
        .lock()
        .unwrap()
        .push((Pass::Body, extracted.text.clone()));
    parse_mapped(&extracted.text, &extracted.map, body_line, math)
}

/// A full comrak parse of a page, for tests that count them.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pass {
    Body,
    CodeRanges,
}

/// Every page parse and the text it parsed, for tests that check each page is parsed once.
#[cfg(test)]
pub(crate) static PARSES: std::sync::Mutex<Vec<(Pass, String)>> = std::sync::Mutex::new(Vec::new());

fn parse_mapped(text: &str, map: &SourceMap, body_line: usize, math: bool) -> Document {
    let arena = Arena::new();
    let options = options(math);
    let root = comrak::parse_document(&arena, text, &options);
    let place = Place {
        map,
        line_offset: body_line - 1,
        options: &options,
    };
    let mut doc = convert(root, &place);
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

/// What `convert` needs to turn the parser's positions into file positions.
struct Place<'a> {
    map: &'a SourceMap,
    line_offset: usize,
    options: &'a Options<'a>,
}

fn convert<'a>(node: &'a AstNode<'a>, place: &Place) -> Node {
    let options = place.options;
    let ast = node.data.borrow();
    let start = ast.sourcepos.start;
    let in_body = place.map.source(start.line, start.column);
    let span = Span {
        line: in_body.line + place.line_offset,
        column: in_body.column,
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
        NodeValue::Math(m) => NodeKind::Math {
            display: m.display_math,
            // A `$$` block's literal keeps the newlines around the formula.
            latex: m.literal.trim().to_string(),
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
        children: node.children().map(|child| convert(child, place)).collect(),
    }
}

/// Whether `body` holds a block opener, `{{` or `{%`. Without one there is nothing to extract, so the code-range
/// parse can be skipped.
pub(crate) fn has_blocks(body: &str) -> bool {
    body.contains("{{") || body.contains("{%")
}

/// Byte ranges of `body` covered by code blocks, inline code and math, where `{{` must stay literal.
pub fn code_ranges(body: &str) -> Vec<std::ops::Range<usize>> {
    code_ranges_with(body, false)
}

/// Like `code_ranges`; math spans count only when `math` is set.
pub fn code_ranges_with(body: &str, math: bool) -> Vec<std::ops::Range<usize>> {
    #[cfg(test)]
    PARSES
        .lock()
        .unwrap()
        .push((Pass::CodeRanges, body.to_string()));
    let mut line_starts = vec![0];
    line_starts.extend(body.match_indices('\n').map(|(i, _)| i + 1));
    let offset = |line: usize, column: usize| {
        let start = line_starts.get(line - 1).copied().unwrap_or(body.len());
        (start + column - 1).min(body.len())
    };
    let arena = Arena::new();
    let root = comrak::parse_document(&arena, body, &options(math));
    root.descendants()
        .filter_map(|node| {
            let ast = node.data.borrow();
            if !matches!(
                ast.value,
                NodeValue::CodeBlock(_) | NodeValue::Code(_) | NodeValue::Math(_)
            ) {
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
    fn has_blocks_finds_openers() {
        assert!(has_blocks("a {{ b }}"));
        assert!(has_blocks("{% x %}"));
        assert!(!has_blocks("a { b } %"));
        assert!(!has_blocks("}}"));
    }

    #[test]
    fn smart_punctuation_in_text() {
        let html = crate::html::render(&parse("\"Rye\" -- it's good...\n", 1));
        assert_eq!(html, "<p>“Rye” – it’s good…</p>\n");
    }

    #[test]
    fn smart_punctuation_leaves_code_html_and_urls() {
        let render = |s: &str| crate::html::render(&parse(s, 1));
        assert!(render("`\"a\" -- b`").contains("<code>&quot;a&quot; -- b</code>"));
        assert!(render("```\n\"a\" -- b\n```").contains("&quot;a&quot; -- b\n</code>"));
        assert!(
            render("<span title=\"a -- b\">x</span>").contains("<span title=\"a -- b\">x</span>")
        );
        assert!(render("[x](/a--b \"t\")").contains("href=\"/a--b\""));
    }

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
    fn spans_after_a_multi_line_block_are_body_lines() {
        let body =
            "{%\n  let a = 1;\n  let b = 2;\n%}\n\n# Heading\n\nSee {{ a +\n  b }} [x](/x/).\n";
        let e = crate::blocks::extract(body, 1, &[]).unwrap();
        let d = parse_extracted(&e, 3, false);
        let heading = &d.children[1];
        assert!(matches!(heading.kind, NodeKind::Heading { .. }));
        assert_eq!(heading.span, Span { line: 8, column: 1 });
        let paragraph = &d.children[2];
        assert_eq!(
            paragraph.span,
            Span {
                line: 10,
                column: 1
            }
        );
        let link = paragraph
            .children
            .iter()
            .find(|n| matches!(n.kind, NodeKind::Link { .. }))
            .unwrap();
        assert_eq!(
            link.span,
            Span {
                line: 11,
                column: 8
            }
        );
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

    fn math_nodes(n: &Node, out: &mut Vec<NodeKind>) {
        if matches!(n.kind, NodeKind::Math { .. }) {
            out.push(n.kind.clone());
        }
        n.children.iter().for_each(|c| math_nodes(c, out));
    }

    #[test]
    fn dollars_in_prose_stay_text() {
        let mut found = Vec::new();
        let d = parse("costs $5 and $10\n", 1);
        math_nodes(&d, &mut found);
        assert!(found.is_empty());
        assert_eq!(crate::html::render(&d), "<p>costs $5 and $10</p>\n");
        let d = parse("\\$100 and $ x $\n", 1);
        math_nodes(&d, &mut found);
        assert!(found.is_empty());
    }

    #[test]
    fn dollars_are_text_when_math_off() {
        assert_eq!(
            crate::html::render(&parse("costs $5 and $10\n", 1)),
            "<p>costs $5 and $10</p>\n"
        );
        assert_eq!(crate::html::render(&parse("\\$x\\$\n", 1)), "<p>$x$</p>\n");
    }

    #[test]
    fn code_ranges_ignore_dollar_spans_when_math_off() {
        let body = "a $x^{{2}}$ b\n";
        assert!(code_ranges(body).is_empty());
    }

    #[test]
    fn math_nodes_parse() {
        let mut found = Vec::new();
        math_nodes(&parse_with("$a$ and\n\n$$\nb\n$$\n", 1, true), &mut found);
        assert_eq!(
            found,
            vec![
                NodeKind::Math {
                    display: false,
                    latex: "a".into()
                },
                NodeKind::Math {
                    display: true,
                    latex: "b".into()
                },
            ]
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

    #[test]
    fn code_ranges_cover_whole_math_spans() {
        let body = "a $x^{{2}}$ b\n\n$$\nq {{n}}\n$$\n";
        let slices: Vec<&str> = code_ranges_with(body, true)
            .iter()
            .map(|r| &body[r.clone()])
            .collect();
        assert_eq!(slices, ["$x^{{2}}$", "$$\nq {{n}}\n$$"]);
    }
}
