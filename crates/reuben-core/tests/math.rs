use std::fs;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::ErrorKind;
use reuben_core::site::{BuildOptions, Failure, Site};

fn try_build(body: &str) -> (TempDir, Result<(), Failure>) {
    let tmp = TempDir::new().unwrap();
    tmp.child("content/index.md").write_str(body).unwrap();
    let site = Site::load(tmp.path()).unwrap();
    let result = site
        .build(&BuildOptions {
            drafts: false,
            out: tmp.path().join("dist"),
            jobs: None,
        })
        .map(|_| ());
    (tmp, result)
}

fn build(body: &str) -> String {
    let (tmp, result) = try_build(body);
    result.unwrap();
    fs::read_to_string(tmp.path().join("dist/index.html")).unwrap()
}

#[test]
fn math_renders_to_mathml() {
    let html = build("---\ntitle: T\n---\n$e^{i\\pi}$\n");
    assert!(html.contains("<math"), "{html}");
    assert!(!html.contains('$'), "{html}");
}

#[test]
fn bad_latex_names_page_and_line() {
    let (_tmp, result) = try_build("---\ntitle: T\n---\n\nText.\n$\\frac{1}{$\n");
    let errors = result.unwrap_err();
    let e = errors
        .iter()
        .find(|e| matches!(e.kind, ErrorKind::Math { .. }))
        .unwrap_or_else(|| panic!("{errors:?}"));
    assert!(e.path.ends_with("index.md"));
    let ErrorKind::Math { location, .. } = &e.kind else {
        unreachable!()
    };
    assert_eq!(location.line, 6);
}

#[test]
fn math_in_code_is_untouched() {
    let html = build("---\ntitle: T\n---\n`$x$`\n");
    assert!(html.contains("<code>$x$</code>"), "{html}");
}

#[test]
fn math_in_a_markdown_helper_converts() {
    let html = build("---\ntitle: T\n---\n{{ markdown(\"$x$\") }}\n");
    assert!(html.contains("<math"), "{html}");
}

#[test]
fn double_brace_in_inline_math_is_literal() {
    let html = build("---\ntitle: T\n---\n$x^{{2}}$\n");
    assert!(html.contains("<msup>"), "{html}");
}

#[test]
fn spaced_double_brace_in_math_is_literal() {
    let html = build("---\ntitle: T\n---\n$x = {{ 3 }}$\n");
    assert!(html.contains("<math"), "{html}");
    assert!(html.contains('3'), "{html}");
}

#[test]
fn double_brace_fraction_renders() {
    let html = build("---\ntitle: T\n---\n$\\frac{{a+b}}{c}$\n");
    assert!(html.contains("<mfrac>"), "{html}");
}

#[test]
fn double_brace_in_display_math_block() {
    let html = build("---\ntitle: T\n---\n$$\na = {{n}}\nb = 2\n$$\n");
    assert!(html.contains("<math"), "{html}");
}

#[test]
fn blocks_outside_math_still_evaluate() {
    let html = build("---\ntitle: T\n---\n$x^{{2}}$\n\n{{ 1 + 1 }}\n");
    assert!(html.contains("<msup>"), "{html}");
    assert!(html.contains("<p>2</p>"), "{html}");
}
