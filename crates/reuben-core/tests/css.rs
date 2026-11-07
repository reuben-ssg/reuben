//! CSS in the built site: processed, bundled and partial-free.

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::Error;
use reuben_core::site::{BuildOptions, Site};

fn site_of(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (path, text) in files {
        tmp.child(path).write_str(text).unwrap();
    }
    tmp
}

fn build(tmp: &TempDir) -> Result<(), Vec<Error>> {
    Site::load(tmp.path())?
        .build(&BuildOptions {
            drafts: false,
            out: tmp.path().join("dist"),
            jobs: None,
        })
        .map(|_| ())
}

fn dist(tmp: &TempDir, rel: &str) -> String {
    std::fs::read_to_string(tmp.path().join("dist").join(rel)).unwrap()
}

const INDEX: (&str, &str) = ("content/index.md", "---\ntitle: H\n---\nhi");

#[test]
fn static_css_is_processed_and_partials_dropped() {
    let tmp = site_of(&[
        INDEX,
        (
            "static/site.css",
            "@import \"_vars.css\";\nbody {\n  color: var(--c);\n}\n",
        ),
        ("static/_vars.css", ":root {\n  --c: red;\n}\n"),
    ]);
    build(&tmp).unwrap();
    let css = dist(&tmp, "site.css");
    assert!(css.contains("--c:red") && css.contains("var(--c)"), "{css}");
    assert!(!css.contains('\n') && !css.contains("@import"), "{css}");
    assert!(!tmp.path().join("dist/_vars.css").exists());
}

#[test]
fn css_template_output_is_processed() {
    let tmp = site_of(&[
        INDEX,
        ("static/_vars.css", ":root { --c: blue; }\n"),
        (
            "static/theme.css",
            "---\n---\n@import \"_vars.css\";\na {\n  width: {{ 1 + 1 }}px;\n}\n",
        ),
    ]);
    build(&tmp).unwrap();
    let css = dist(&tmp, "theme.css");
    assert!(
        css.contains("--c:#00f") || css.contains("--c:blue"),
        "{css}"
    );
    assert!(css.contains("2px") && !css.contains('\n'), "{css}");
}

#[test]
fn content_asset_and_theme_css_are_processed() {
    let tmp = site_of(&[
        ("reuben.toml", "theme = \"t\"\n"),
        INDEX,
        ("content/doc.css", "a {\n  color: red;\n}\n"),
        ("t/static/th.css", "b {\n  color: red;\n}\n"),
    ]);
    build(&tmp).unwrap();
    assert!(!dist(&tmp, "doc.css").contains('\n'));
    assert!(!dist(&tmp, "th.css").contains('\n'));
}

#[test]
fn minify_off_keeps_readable_output() {
    let tmp = site_of(&[
        ("reuben.toml", "[css]\nminify = false\n"),
        INDEX,
        ("static/site.css", "a{color:red}b{color:blue}"),
    ]);
    build(&tmp).unwrap();
    assert!(dist(&tmp, "site.css").contains('\n'));
}

#[test]
fn every_css_error_is_reported_and_nothing_is_written() {
    let tmp = site_of(&[
        INDEX,
        ("static/a.css", "a {\n}}} x {{\n"),
        ("static/b.css", "@import \"nope.css\";\n"),
        ("content/c.css", "@import \"../static/site.css\";\n"),
    ]);
    let errs = build(&tmp).unwrap_err();
    let paths: Vec<String> = errs.iter().map(|e| e.path.display().to_string()).collect();
    assert_eq!(errs.len(), 3, "{paths:?}");
    assert!(paths[0].ends_with("content/c.css"), "{paths:?}");
    assert!(paths[1].ends_with("static/a.css"), "{paths:?}");
    assert!(paths[2].ends_with("static/b.css"), "{paths:?}");
    assert!(!tmp.path().join("dist").exists());
}
