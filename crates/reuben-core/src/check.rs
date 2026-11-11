//! Accessibility checks on a page, in two passes: the tree pass over the final document, and the HTML pass over the
//! finished page. This module converts between Reuben's tree and the rule crate's items and findings.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use reuben_a11y::{Item, LocalLink, Rule, RuleSet, check_page, check_tree, fragment_items};

use crate::doc::{Document, Node, NodeKind, Origin, Span};
use crate::html;
use crate::splice::{Markup, Output};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The page's source file.
    pub path: PathBuf,
    /// The line in `path` the problem is traced to. `None` for HTML-pass findings, which have `built` instead.
    pub location: Option<Span>,
    pub rule: Rule,
    pub message: String,
    /// The include line that wrote the offending HTML, when it came from an include.
    pub include: Option<Origin>,
    /// The output file and the line in it, for findings from the HTML pass.
    pub built: Option<(PathBuf, usize)>,
}

type SortKey = (
    PathBuf,
    usize,
    usize,
    Option<(PathBuf, usize)>,
    Rule,
    String,
    Option<(PathBuf, usize)>,
);

/// Orders findings by path, then line and column (a finding with no location counts as line 0 and is ordered by its
/// built line), then rule and message, and drops repeats, so the result doesn't depend on render order.
pub fn sort(findings: &mut Vec<Finding>) {
    let key = |f: &Finding| -> SortKey {
        let (line, column) = f.location.map_or((0, 0), |s| (s.line, s.column));
        (
            f.path.clone(),
            line,
            column,
            f.built.clone(),
            f.rule,
            f.message.clone(),
            f.include.as_ref().map(|o| (o.file.clone(), o.line)),
        )
    };
    findings.sort_by_cached_key(key);
    findings.dedup();
}

/// Switches off the rules named by `ids`, returning the ids that aren't rules.
pub fn switch_off(mut rules: RuleSet, ids: &[String]) -> (RuleSet, Vec<String>) {
    let mut unknown = Vec::new();
    for id in ids {
        match Rule::from_id(id) {
            Some(rule) => rules.disable(rule),
            None => unknown.push(id.clone()),
        }
    }
    (rules, unknown)
}

/// Where a tree item came from: its position in the page source, and the include that wrote it, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct At {
    pub span: Span,
    pub include: Option<Origin>,
}

/// What the tree pass found, plus what the HTML pass and the page shell need to know about it.
pub struct TreeReport {
    pub findings: Vec<Finding>,
    /// Heading ids already reported as duplicates, so the HTML pass doesn't report them again.
    pub reported_ids: HashSet<String>,
    pub has_h1: bool,
}

/// Runs the tree-pass rules over a page's final body tree.
pub fn tree_pass(source: &Path, rules: &RuleSet, doc: &Document, outputs: &[Output]) -> TreeReport {
    let items = tree_items(doc, outputs);
    let has_h1 = items
        .iter()
        .any(|item| matches!(item, Item::Heading { level: 1, .. }));
    let mut reported_ids = HashSet::new();
    if rules.is_on(Rule::HeadingIdUnique) {
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for item in &items {
            if let Item::Heading { id, .. } = item
                && !id.is_empty()
            {
                let count = seen.entry(id).or_default();
                *count += 1;
                if *count == 2 {
                    reported_ids.insert(id.clone());
                }
            }
        }
    }
    let findings = check_tree(&items, rules)
        .into_iter()
        .map(|f| Finding {
            path: source.to_path_buf(),
            location: Some(f.at.span),
            rule: f.rule,
            message: f.message,
            include: f.at.include,
            built: None,
        })
        .collect();
    TreeReport {
        findings,
        reported_ids,
        has_h1,
    }
}

pub struct HtmlReport {
    pub findings: Vec<Finding>,
    /// Local links still to be checked against what the build produced.
    pub links: Vec<LocalLink>,
}

/// Runs the HTML-pass rules over a page's finished HTML. `built` is the output file the HTML is written to.
pub fn html_pass(
    source: &Path,
    built: &Path,
    html: &str,
    rules: &RuleSet,
    reported_ids: &HashSet<String>,
) -> HtmlReport {
    let report = check_page(html, rules, reported_ids);
    let findings = report
        .findings
        .into_iter()
        .map(|f| Finding {
            path: source.to_path_buf(),
            location: None,
            rule: f.rule,
            message: f.message,
            include: None,
            built: Some((built.to_path_buf(), f.line)),
        })
        .collect();
    HtmlReport {
        findings,
        links: report.links,
    }
}

/// The path part of `base_url`, with a leading and a trailing slash.
pub fn base_path(base_url: &str) -> String {
    let path = match base_url.split_once("://") {
        Some((_, rest)) => rest.find('/').map_or("", |i| &rest[i..]),
        None => base_url,
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    let mut out = path.to_string();
    if !out.starts_with('/') {
        out.insert(0, '/');
    }
    if !out.ends_with('/') {
        out.push('/');
    }
    out
}

/// A `link-target` finding for each of a page's links that no built file answers.
/// `page_url` is the page's URL without the base path; `built_exists` says whether an output file exists.
pub fn unbuilt_links(
    source: &Path,
    built: &Path,
    page_url: &str,
    base_path: &str,
    links: &[LocalLink],
    built_exists: impl Fn(&str) -> bool,
) -> Vec<Finding> {
    let full_url = format!("{}{page_url}", base_path.trim_end_matches('/'));
    links
        .iter()
        .filter(
            |link| match reuben_a11y::link_keys(&full_url, base_path, &link.href) {
                reuben_a11y::LinkKeys::External => false,
                reuben_a11y::LinkKeys::Candidates(keys) => !keys.iter().any(|k| built_exists(k)),
            },
        )
        .map(|link| Finding {
            path: source.to_path_buf(),
            location: None,
            rule: Rule::LinkTarget,
            message: format!("links to `{}`, which isn't built", link.href),
            include: None,
            built: Some((built.to_path_buf(), link.line)),
        })
        .collect()
}

/// Walks the final body tree and turns what the tree pass looks at into items. HTML that Reuben didn't parse
/// (raw blocks, include output) is parsed as HTML so its headings, links and images are seen too.
pub fn tree_items(doc: &Document, outputs: &[Output]) -> Vec<Item<At>> {
    let mut items = Vec::new();
    walk(doc, outputs, &mut items);
    items
}

fn walk(node: &Node, outputs: &[Output], items: &mut Vec<Item<At>>) {
    let at = At {
        span: node.span,
        include: None,
    };
    match &node.kind {
        NodeKind::HtmlBlock(text) | NodeKind::Raw(text) => {
            fragment(&[(text.clone(), None)], &[], node.span, true, items);
        }
        NodeKind::Generated {
            html,
            block,
            inline: false,
        } => {
            let mut origins = Vec::new();
            let pieces = generated_pieces(html, outputs.get(*block), &mut origins);
            fragment(&pieces, &origins, node.span, true, items);
        }
        _ if has_inline_html(node) => mixed(node, outputs, items),
        NodeKind::Heading { level, id } => {
            items.push(Item::Heading {
                level: *level,
                id: id.clone(),
                text: html::plain_text(node),
                at,
            });
            walk_children(node, outputs, items);
        }
        NodeKind::Link { url, .. } => {
            items.push(Item::Link {
                name: html::plain_text(node),
                href: url.clone(),
                at,
            });
            walk_children(node, outputs, items);
        }
        NodeKind::Image { .. } => {
            // Markdown can't mark an image as decorative, so an empty alt is a missing one.
            let alt = Some(html::plain_text(node)).filter(|alt| !alt.is_empty());
            items.push(Item::Image { alt, at });
        }
        NodeKind::Table { .. } => {
            let has_header = node.children.first().is_some_and(|row| {
                matches!(row.kind, NodeKind::TableRow { header: true })
                    && row.children.iter().any(|cell| !cell_is_empty(cell))
            });
            items.push(Item::Table { has_header, at });
            walk_children(node, outputs, items);
        }
        _ => walk_children(node, outputs, items),
    }
}

fn walk_children(node: &Node, outputs: &[Output], items: &mut Vec<Item<At>>) {
    for child in &node.children {
        walk(child, outputs, items);
    }
}

fn cell_is_empty(cell: &Node) -> bool {
    cell.children.iter().all(|child| match &child.kind {
        NodeKind::Text(text) => text.trim().is_empty(),
        NodeKind::SoftBreak | NodeKind::LineBreak => true,
        _ => false,
    })
}

fn has_inline_html(node: &Node) -> bool {
    node.children.iter().any(|child| {
        matches!(
            child.kind,
            NodeKind::HtmlInline(_) | NodeKind::Generated { inline: true, .. }
        )
    })
}

/// A node with inline HTML among its children is rendered whole and parsed as HTML in one go, so a tag opened in
/// one child and closed in another (`<a href>` text `</a>`) is one element. Every item is at the node's own span.
fn mixed(node: &Node, outputs: &[Output], items: &mut Vec<Item<At>>) {
    let mut generated: Vec<(String, usize)> = Vec::new();
    let marked = with_markers(node, &mut generated);
    let rendered = html::render_node(&marked);

    let mut origins = Vec::new();
    let mut pieces: Vec<(String, Option<usize>)> = Vec::new();
    let mut rest = rendered.as_str();
    while let Some(start) = rest.find(MARK_OPEN) {
        pieces.push((rest[..start].to_string(), None));
        let after = &rest[start + MARK_OPEN.len_utf8()..];
        let Some((number, tail)) = after.split_once(MARK_CLOSE) else {
            rest = after;
            continue;
        };
        if let Some((text, block)) = number.parse::<usize>().ok().and_then(|n| generated.get(n)) {
            pieces.extend(generated_pieces(text, outputs.get(*block), &mut origins));
        }
        rest = tail;
    }
    pieces.push((rest.to_string(), None));
    fragment(&pieces, &origins, node.span, false, items);
}

const MARK_OPEN: char = '\u{E100}';
const MARK_CLOSE: char = '\u{E101}';

/// A copy of `node` where each inline `Generated` child is a marker naming its place in `generated`, and each
/// Markdown image with an empty alt has none, so the HTML parser sees what the tree pass counts as missing.
fn with_markers(node: &Node, generated: &mut Vec<(String, usize)>) -> Node {
    let replaced = match &node.kind {
        NodeKind::Generated {
            html,
            block,
            inline: true,
        } => {
            generated.push((html.clone(), *block));
            Some(format!("{MARK_OPEN}{}{MARK_CLOSE}", generated.len() - 1))
        }
        NodeKind::Image { .. } if html::plain_text(node).is_empty() => {
            Some(html::render_node(node).replace(" alt=\"\"", ""))
        }
        _ => None,
    };
    match replaced {
        Some(text) => Node {
            kind: NodeKind::Raw(text),
            span: node.span,
            children: Vec::new(),
        },
        None => Node {
            kind: node.kind.clone(),
            span: node.span,
            children: node
                .children
                .iter()
                .map(|child| with_markers(child, generated))
                .collect(),
        },
    }
}

/// Cuts a block's `html` at the ranges its `Markup` says came from includes. A range's piece is labelled with an
/// index into `origins`; gaps are labelled `None`. A `Markup` that doesn't match `html` contributes no origins.
fn generated_pieces(
    text: &str,
    output: Option<&Output>,
    origins: &mut Vec<Origin>,
) -> Vec<(String, Option<usize>)> {
    let mut ranges: Vec<&(Range<usize>, Origin)> = match output {
        Some(Output::Html(Markup { html, origins })) if html == text => origins.iter().collect(),
        _ => Vec::new(),
    };
    // Where ranges nest, the outer one is cut first and the inner is skipped.
    ranges.sort_by_key(|(range, _)| (range.start, Reverse(range.end)));
    let mut pieces = Vec::new();
    let mut pos = 0;
    for (range, origin) in ranges {
        let (Some(gap), Some(piece)) = (text.get(pos..range.start), text.get(range.clone())) else {
            continue;
        };
        if !gap.is_empty() {
            pieces.push((gap.to_string(), None));
        }
        origins.push(origin.clone());
        pieces.push((piece.to_string(), Some(origins.len() - 1)));
        pos = range.end;
    }
    if let Some(tail) = text.get(pos..).filter(|tail| !tail.is_empty()) {
        pieces.push((tail.to_string(), None));
    }
    pieces
}

/// Parses `pieces` as HTML and adds the items found. With `offset`, an item's line is the node's line plus its line
/// within the HTML; otherwise every item is at the node's line.
fn fragment(
    pieces: &[(String, Option<usize>)],
    origins: &[Origin],
    span: Span,
    offset: bool,
    items: &mut Vec<Item<At>>,
) {
    let refs: Vec<(&str, Option<usize>)> = pieces.iter().map(|(t, l)| (t.as_str(), *l)).collect();
    for item in fragment_items(&refs) {
        items.push(map_at(item, |(label, line)| At {
            span: Span {
                line: if offset {
                    span.line + line - 1
                } else {
                    span.line
                },
                column: span.column,
            },
            include: label.and_then(|i| origins.get(i).cloned()),
        }));
    }
}

fn map_at<O, P>(item: Item<O>, f: impl FnOnce(O) -> P) -> Item<P> {
    match item {
        Item::Heading {
            level,
            id,
            text,
            at,
        } => Item::Heading {
            level,
            id,
            text,
            at: f(at),
        },
        Item::Link { name, href, at } => Item::Link {
            name,
            href,
            at: f(at),
        },
        Item::Image { alt, at } => Item::Image { alt, at: f(at) },
        Item::Table { has_header, at } => Item::Table {
            has_header,
            at: f(at),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::placeholder;
    use crate::{markdown, splice};

    fn items(md: &str, outputs: &[Output]) -> Vec<Item<At>> {
        let mut doc = markdown::parse(md, 1);
        splice::splice(&mut doc, outputs);
        tree_items(&doc, outputs)
    }

    fn at(line: usize, include: Option<Origin>) -> At {
        At {
            span: Span { line, column: 1 },
            include,
        }
    }

    #[test]
    fn inline_html_link_is_one_item() {
        assert_eq!(
            items("see <a href=\"/x/\">here</a> now\n", &[]),
            [Item::Link {
                name: "here".into(),
                href: "/x/".into(),
                at: at(1, None),
            }]
        );
    }

    #[test]
    fn generated_items_carry_the_include() {
        let html = "<h4>Card</h4>";
        let origin = Origin {
            file: "includes/card.rhai".into(),
            line: 3,
        };
        let outputs = [Output::Html(Markup {
            html: html.into(),
            origins: vec![(0..html.len(), origin.clone())],
        })];
        assert_eq!(
            items(&format!("{}\n", placeholder(0)), &outputs),
            [Item::Heading {
                level: 4,
                id: String::new(),
                text: "Card".into(),
                at: at(1, Some(origin)),
            }]
        );
    }

    #[test]
    fn generated_inline_links_keep_their_own_include() {
        let html = "<a href=\"/c/\">more</a>";
        let origin = Origin {
            file: "includes/more.rhai".into(),
            line: 2,
        };
        let outputs = [Output::Html(Markup {
            html: html.into(),
            origins: vec![(0..html.len(), origin.clone())],
        })];
        let found = items(
            &format!("a {} b <a href=\"/d/\">d</a>\n", placeholder(0)),
            &outputs,
        );
        let includes: Vec<_> = found
            .iter()
            .map(|item| match item {
                Item::Link { href, at, .. } => (href.as_str(), at.include.clone()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(includes, [("/c/", Some(origin)), ("/d/", None)]);
    }

    #[test]
    fn empty_markdown_alt_is_missing() {
        let alt = |md: &str| match items(md, &[]).remove(0) {
            Item::Image { alt, .. } => alt,
            other => panic!("{other:?}"),
        };
        assert_eq!(alt("![](a.png)\n"), None);
        assert_eq!(alt("![Logo](a.png)\n"), Some("Logo".into()));
    }

    #[test]
    fn empty_markdown_alt_stays_missing_beside_inline_html() {
        let found = items("<b>x</b> ![](a.png)\n", &[]);
        assert_eq!(
            found,
            [Item::Image {
                alt: None,
                at: at(1, None)
            }]
        );
    }

    #[test]
    fn table_header_needs_a_non_empty_cell() {
        let has_header = |md: &str| match items(md, &[]).remove(0) {
            Item::Table { has_header, .. } => has_header,
            other => panic!("{other:?}"),
        };
        assert!(has_header("| A | B |\n| - | - |\n| 1 | 2 |\n"));
        assert!(!has_header("|  |  |\n| - | - |\n| 1 | 2 |\n"));
    }

    #[test]
    fn html_blocks_are_offset_by_their_line() {
        let found = items("text\n\n<div>\n<h2>Inside</h2>\n</div>\n", &[]);
        assert_eq!(
            found,
            [Item::Heading {
                level: 2,
                id: String::new(),
                text: "Inside".into(),
                at: at(4, None),
            }]
        );
    }

    #[test]
    fn base_path_is_the_path_of_the_base_url() {
        assert_eq!(base_path("https://e.x/blog/"), "/blog/");
        assert_eq!(base_path("https://e.x"), "/");
        assert_eq!(base_path("/"), "/");
        assert_eq!(base_path("/blog"), "/blog/");
    }

    #[test]
    fn sort_orders_and_drops_repeats() {
        let finding = |path: &str, line: usize, message: &str| Finding {
            path: path.into(),
            location: Some(Span { line, column: 1 }),
            rule: Rule::LinkText,
            message: message.into(),
            include: None,
            built: None,
        };
        let built = Finding {
            location: None,
            built: Some(("index.html".into(), 3)),
            ..finding("a.md", 0, "built")
        };
        let mut all = vec![
            finding("b.md", 1, "x"),
            finding("a.md", 2, "x"),
            built.clone(),
            finding("a.md", 2, "x"),
            finding("a.md", 1, "x"),
        ];
        sort(&mut all);
        let order: Vec<_> = all
            .iter()
            .map(|f| (f.path.to_str().unwrap(), f.location.map(|s| s.line)))
            .collect();
        assert_eq!(
            order,
            [
                ("a.md", None),
                ("a.md", Some(1)),
                ("a.md", Some(2)),
                ("b.md", Some(1))
            ]
        );
    }
}
