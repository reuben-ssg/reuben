//! CSS in the built site: processed, bundled and partial-free.

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::site::{BuildOptions, Failure, Site};

fn site_of(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (path, text) in files {
        tmp.child(path).write_str(text).unwrap();
    }
    tmp
}

fn build(tmp: &TempDir) -> Result<(), Failure> {
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

#[test]
fn templated_partial_is_imported_rendered_and_not_counted() {
    let tmp = site_of(&[
        INDEX,
        ("static/_vars.css", "---\n---\n:root{--w:{{ 1 + 1 }}px}"),
        (
            "static/site.css",
            "@import \"_vars.css\";\na{width:var(--w)}",
        ),
    ]);
    let report = Site::load(tmp.path())
        .unwrap()
        .build(&BuildOptions {
            drafts: false,
            out: tmp.path().join("dist"),
            jobs: None,
        })
        .unwrap();
    let css = dist(&tmp, "site.css");
    assert!(css.contains("--w:2px") && !css.contains("---"), "{css}");
    assert_eq!(report.pages_written, 1);
    assert!(!tmp.path().join("dist/_vars.css").exists());
}

#[test]
fn error_in_template_output_quotes_the_rendered_line() {
    let tmp = site_of(&[
        INDEX,
        (
            "static/t.css",
            "---\n---\n}}} b {{ \"{\" }} width: {{ 1 + 1 }}px }\n",
        ),
    ]);
    let errs = build(&tmp).unwrap_err();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].path.ends_with("static/t.css"));
    assert_eq!(errs[0].location(), None);
    assert!(
        errs[0].to_string().contains("`}}} b { width: 2px }`"),
        "{}",
        errs[0]
    );
}

#[test]
fn error_in_a_shared_partial_is_reported_once() {
    let tmp = site_of(&[
        INDEX,
        ("static/_bad.css", "a{color:red}\n}}} b {{\n"),
        ("static/x.css", "@import \"_bad.css\";\n"),
        ("static/y.css", "@import \"_bad.css\";\n"),
    ]);
    let errs = build(&tmp).unwrap_err();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].path.ends_with("_bad.css"));
}

#[test]
fn site_partial_overrides_the_one_a_theme_stylesheet_imports() {
    let tmp = site_of(&[
        ("reuben.toml", "theme = \"t\"\n"),
        INDEX,
        (
            "t/static/main.css",
            "@import \"_vars.css\";\na{color:var(--c)}\n",
        ),
        ("t/static/_vars.css", ":root{--c:red}\n"),
        ("static/_vars.css", ":root{--c:blue}\n"),
    ]);
    build(&tmp).unwrap();
    let css = dist(&tmp, "main.css");
    assert!(
        css.contains("--c:#00f") || css.contains("--c:blue"),
        "{css}"
    );
    assert!(!css.contains("red"), "{css}");
}

#[test]
fn theme_partial_is_used_when_the_site_has_none() {
    let tmp = site_of(&[
        ("reuben.toml", "theme = \"t\"\n"),
        INDEX,
        (
            "t/static/main.css",
            "@import \"sub/_vars.css\";\na{color:var(--c)}\n",
        ),
        ("t/static/sub/_vars.css", ":root{--c:red}\n"),
    ]);
    build(&tmp).unwrap();
    let css = dist(&tmp, "main.css");
    assert!(css.contains("--c:red"), "{css}");
}

#[test]
fn a_non_partial_css_template_is_imported_rendered() {
    let tmp = site_of(&[
        INDEX,
        ("static/t.css", "---\n---\n:root{--w:{{ 1 + 1 }}px}\n"),
        ("static/m.css", "@import \"t.css\";\na{width:var(--w)}\n"),
    ]);
    build(&tmp).unwrap();
    let css = dist(&tmp, "m.css");
    assert!(css.contains("--w:2px") && !css.contains("---"), "{css}");
}

#[test]
fn importing_a_drafted_css_template_is_a_located_error() {
    let tmp = site_of(&[
        INDEX,
        ("static/_d.css", "---\ndraft: true\n---\n:root{--c:red}\n"),
        (
            "static/m.css",
            "/* m */\n@import \"_d.css\";\na{color:var(--c)}\n",
        ),
    ]);
    let errs = build(&tmp).unwrap_err();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].path.ends_with("static/m.css"), "{errs:?}");
    assert_eq!(errs[0].location().map(|l| l.line), Some(2), "{errs:?}");
    assert!(
        errs[0].to_string().contains("isn't rendered (draft)"),
        "{}",
        errs[0]
    );
    let with_drafts = Site::load(tmp.path()).unwrap().build(&BuildOptions {
        drafts: true,
        out: tmp.path().join("dist"),
        jobs: None,
    });
    assert!(with_drafts.is_ok(), "{with_drafts:?}");
    assert!(dist(&tmp, "m.css").contains("--c:red"));
}

#[test]
fn processed_css_is_not_counted_as_copied() {
    let tmp = site_of(&[
        INDEX,
        ("static/site.css", "a{color:red}"),
        ("static/robots.txt", "x"),
    ]);
    let report = Site::load(tmp.path())
        .unwrap()
        .build(&BuildOptions {
            drafts: false,
            out: tmp.path().join("dist"),
            jobs: None,
        })
        .unwrap();
    assert_eq!(report.static_copied, 1);
}
