use std::fs;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::ErrorKind;
use reuben_core::site::{BuildOptions, Site};

fn try_build(body: &str) -> (TempDir, Result<(), Vec<reuben_core::Error>>) {
    let tmp = TempDir::new().unwrap();
    tmp.child("content/index.md").write_str(body).unwrap();
    let site = Site::load(tmp.path()).unwrap();
    let result = site
        .build(&BuildOptions {
            drafts: false,
            out: tmp.path().join("dist"),
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
