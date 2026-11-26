//! Every position reported from a page body points at the page source, however many lines its blocks span.

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_a11y::Rule;
use reuben_core::check::Finding;
use reuben_core::doc::Span;
use reuben_core::links::LinkProblem;
use reuben_core::site::{RenderOptions, Site};
use reuben_core::{Error, ErrorKind};

const FRONT_MATTER: &str = "---\ntitle: T\n---\n";

/// Renders a site whose `content/p.md` has `body` after three lines of front matter, with a `c.md` to link to.
fn render(body: &str, extra: &[(&str, &str)]) -> (Vec<Error>, Vec<Finding>) {
    render_page(&format!("{FRONT_MATTER}{body}"), extra)
}

/// Renders a site whose `content/p.md` is `page`, front matter included.
fn render_page(page: &str, extra: &[(&str, &str)]) -> (Vec<Error>, Vec<Finding>) {
    let tmp = TempDir::new().unwrap();
    tmp.child("content/p.md").write_str(page).unwrap();
    tmp.child("content/c.md")
        .write_str(&format!("{FRONT_MATTER}# C\n"))
        .unwrap();
    tmp.child("static/a.png").write_str("png").unwrap();
    for (path, text) in extra {
        tmp.child(path).write_str(text).unwrap();
    }
    let site = Site::load(tmp.path()).unwrap();
    match site.render(&RenderOptions {
        drafts: false,
        jobs: Some(1),
    }) {
        Ok((_, report)) => (Vec::new(), report.findings),
        Err(failure) => (failure.errors, failure.findings),
    }
}

/// A page with errors isn't checked, so its findings come from the same page with its broken link fixed.
fn checked(body: &str) -> Vec<Finding> {
    let (errors, findings) = render(&body.replace("@/nope.md", "@/c.md"), &[]);
    assert_eq!(errors.len(), 0, "{errors:?}");
    findings
}

fn at(line: usize, column: usize) -> Span {
    Span { line, column }
}

/// Where each finding of `rule` on `p.md` is.
fn found(findings: &[Finding], rule: Rule) -> Vec<Span> {
    findings
        .iter()
        .filter(|f| f.rule == rule && f.path.ends_with("p.md"))
        .map(|f| f.location.expect("a tree-pass finding has a location"))
        .collect()
}

/// Where each missing-page link error on `p.md` is.
fn missing_pages(errors: &[Error]) -> Vec<Span> {
    errors
        .iter()
        .filter(|e| e.path.ends_with("p.md"))
        .filter_map(|e| match &e.kind {
            ErrorKind::Link {
                problem: LinkProblem::NoAnchor { .. },
                ..
            } => None,
            ErrorKind::Link { location, .. } => Some(*location),
            _ => None,
        })
        .collect()
}

fn missing_anchors(errors: &[Error]) -> Vec<Span> {
    errors
        .iter()
        .filter(|e| e.path.ends_with("p.md"))
        .filter_map(|e| match &e.kind {
            ErrorKind::Link {
                problem: LinkProblem::NoAnchor { .. },
                location,
            } => Some(*location),
            _ => None,
        })
        .collect()
}

#[test]
fn findings_after_a_multi_line_setup_block() {
    let body = "{%\n  let a = 1;\n  let b = 2;\n%}\n\n## Start\n\n#### Skipped\n\n![](/a.png)\n";
    let (errors, _) = render(body, &[]);
    let findings = checked(body);
    assert_eq!(errors.len(), 0, "{errors:?}");
    assert_eq!(found(&findings, Rule::HeadingOrder), [at(11, 1)]);
    assert_eq!(found(&findings, Rule::ImgAlt), [at(13, 1)]);
}

#[test]
fn link_errors_after_a_multi_line_output_block() {
    let body = "{{\n  \"x\"\n}}\n\n[a](@/nope.md) and [b](@/c.md#nope)\n";
    let (errors, _) = render(body, &[]);
    assert_eq!(missing_pages(&errors), [at(8, 1)], "{errors:?}");
    assert_eq!(missing_anchors(&errors), [at(8, 20)], "{errors:?}");
}

#[test]
fn inline_block_in_a_paragraph() {
    // The paragraph holds the block's output as inline HTML, so its own findings are all at its start; the image
    // gets a paragraph of its own.
    let body = "Text {{ 1 +\n 2 }} and [bad](@/nope.md) here\n\n![](/a.png)\n";
    let (errors, _) = render(body, &[]);
    let findings = checked(body);
    assert_eq!(missing_pages(&errors), [at(5, 11)], "{errors:?}");
    assert_eq!(found(&findings, Rule::ImgAlt), [at(7, 1)]);
}

#[test]
fn inline_block_in_a_list_item() {
    let body = "- one {{ \"a\" +\n  \"b\" }} [x](@/nope.md)\n- two\n\n  ![](/a.png)\n";
    let (errors, _) = render(body, &[]);
    let findings = checked(body);
    assert_eq!(missing_pages(&errors), [at(5, 10)], "{errors:?}");
    assert_eq!(found(&findings, Rule::ImgAlt), [at(8, 3)]);
}

#[test]
fn inline_block_in_a_table_cell() {
    let body = "| h | i |\n| - | - |\n| {{ \"a\" +\n\"b\" }} | [x](@/nope.md) |\n\n#### Skipped\n";
    let (errors, _) = render(body, &[]);
    let findings = checked(body);
    assert_eq!(missing_pages(&errors), [at(7, 10)], "{errors:?}");
    assert_eq!(found(&findings, Rule::HeadingOrder), [at(9, 1)]);
}

#[test]
fn inline_block_in_a_heading() {
    let body = "## A {{ 1 +\n2 }} [x](@/nope.md)\n\n#### Skip\n";
    let (errors, _) = render(body, &[]);
    let findings = checked(body);
    assert_eq!(missing_pages(&errors), [at(5, 6)], "{errors:?}");
    assert_eq!(found(&findings, Rule::HeadingOrder), [at(7, 1)]);
}

#[test]
fn several_blocks_and_blocks_in_code() {
    let body = "{% let a = 1;\nlet b = 2; %}\n\n```text\n{{ not\na block }}\n```\n\n`{{ inline\ncode }}` {{ a +\nb }} [x](@/nope.md)\n\n{{\n  a\n}}\n\n![](/a.png)\n";
    let (errors, _) = render(body, &[]);
    let findings = checked(body);
    assert_eq!(missing_pages(&errors), [at(14, 6)], "{errors:?}");
    assert_eq!(found(&findings, Rule::ImgAlt), [at(20, 1)]);
}

#[test]
fn longer_front_matter() {
    let tmp = TempDir::new().unwrap();
    tmp.child("content/p.md")
        .write_str(
            "---\ntitle: T\ndescription: D\nweight: 1\n---\n{%\nlet a = 1;\n%}\n\n[x](@/nope.md)\n",
        )
        .unwrap();
    let errors = Site::load(tmp.path())
        .unwrap()
        .render(&RenderOptions {
            drafts: false,
            jobs: Some(1),
        })
        .unwrap_err()
        .errors;
    assert_eq!(missing_pages(&errors), [at(10, 1)], "{errors:?}");
}

#[test]
fn errors_inside_a_block_keep_their_location() {
    let body = "{%\n  let a = 1;\n%}\n\n{{\n  nope_fn()\n}}\n\n[x](@/nope.md)\n";
    let (errors, _) = render(body, &[]);
    let script: Vec<_> = errors
        .iter()
        .filter_map(|e| match &e.kind {
            ErrorKind::Script(s) => s.location,
            _ => None,
        })
        .collect();
    assert_eq!(script, [at(9, 3)], "{errors:?}");
    assert_eq!(missing_pages(&errors), [at(12, 1)], "{errors:?}");
}

#[test]
fn findings_in_an_html_block_after_multi_line_blocks() {
    let body = "<div>\n{{ \"a\" +\n\"b\" }}\n{%\nlet c = 1;\n%}\n{{ raw(\"<img src=/b.png>\") }}\n<img src=\"/a.png\">\n</div>\n\n#### Skipped\n";
    let (errors, _) = render(body, &[]);
    let findings = checked(body);
    assert_eq!(errors.len(), 0, "{errors:?}");
    assert_eq!(found(&findings, Rule::ImgAlt), [at(10, 1), at(11, 1)]);
    assert_eq!(found(&findings, Rule::HeadingOrder), [at(14, 1)]);
}

#[test]
fn math_error_after_a_multi_line_block() {
    let body = "{%\nlet a = 1;\n%}\n\nSome $\\frac{1}{$ here\n";
    let (errors, _) = render(body, &[("reuben.toml", "[markdown]\nmath = true\n")]);
    let math: Vec<_> = errors
        .iter()
        .filter_map(|e| match &e.kind {
            ErrorKind::Math { location, .. } => Some(*location),
            _ => None,
        })
        .collect();
    assert_eq!(math, [at(8, 6)], "{errors:?}");
}

#[test]
fn crlf_line_endings() {
    let page = "---\r\ntitle: T\r\n---\r\n{%\r\n  let a = 1;\r\n%}\r\n\r\nText {{ a +\r\n 1 }} and [bad](@/nope.md)\r\n\r\n![](/a.png)\r\n";
    let (errors, _) = render_page(page, &[]);
    assert_eq!(missing_pages(&errors), [at(9, 11)], "{errors:?}");
    let (errors, findings) = render_page(&page.replace("@/nope.md", "@/c.md"), &[]);
    assert_eq!(errors.len(), 0, "{errors:?}");
    assert_eq!(found(&findings, Rule::ImgAlt), [at(11, 1)]);
}

#[test]
fn two_multi_line_blocks_on_one_line() {
    // Line 5 ends one block and starts the next.
    let body = "x {% let a = 1 +\n2; %} y {% let b = 3 +\n4; %} ![](/a.png) [a](@/nope.md)\n![](/a.png) [b](@/nope.md)\n";
    let (errors, _) = render(body, &[]);
    let findings = checked(body);
    assert_eq!(missing_pages(&errors), [at(6, 19), at(7, 13)], "{errors:?}");
    assert_eq!(found(&findings, Rule::ImgAlt), [at(6, 7), at(7, 1)]);
}
