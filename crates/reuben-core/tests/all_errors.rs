//! One build reports every independent error, and stops early only for a broken config.

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

#[test]
fn one_build_reports_every_independent_error() {
    let tmp = site_of(&[
        ("content/a.md", "---\n---\nno title\n"),
        ("content/b.md", "---\ntitle: B\n---\n{{ nope( }}\n"),
        ("content/c.md", "---\ntitle: C\n---\n\n$\\frac{1}{$\n"),
        ("content/d.md", "---\ntitle: D\n---\nd\n"),
        ("content/d/index.md", "---\ntitle: D2\n---\nd\n"),
        ("data/bad.toml", "x = [\n"),
        ("static/bad.css", "a {\n}}} x {{\n"),
        ("content/ok.md", "---\ntitle: Ok\n---\nfine\n"),
    ]);
    let errors = build(&tmp).unwrap_err();
    for file in [
        "content/a.md",
        "content/b.md",
        "content/c.md",
        "data/bad.toml",
        "static/bad.css",
    ] {
        assert!(
            errors.iter().any(|e| e.path.ends_with(file)),
            "no error for {file}: {errors:?}"
        );
    }
    assert!(
        errors
            .iter()
            .any(|e| e.path.ends_with("content/d.md") || e.path.ends_with("content/d/index.md")),
        "no collision error: {errors:?}"
    );
    assert!(
        !errors.iter().any(|e| e.path.ends_with("content/ok.md")),
        "{errors:?}"
    );
    assert!(!tmp.path().join("dist").exists());
}

#[test]
fn body_failure_suppresses_its_layout_error() {
    let tmp = site_of(&[
        (
            "content/a.md",
            "---\ntitle: A\nlayout: broken\n---\n{{ nope( }}\n",
        ),
        ("layouts/broken.html", "<main>{{ also_nope( }}</main>"),
    ]);
    let errors = build(&tmp).unwrap_err();
    assert!(
        errors.iter().all(|e| e.path.ends_with("content/a.md")),
        "{errors:?}"
    );
    assert!(!errors.is_empty());
}

#[test]
fn config_error_still_stops_early() {
    let tmp = site_of(&[
        ("reuben.toml", "titel = \"x\"\n"),
        ("content/a.md", "---\n---\nno title\n"),
    ]);
    let errors = Site::load(tmp.path()).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].path.ends_with("reuben.toml"));
}

#[test]
fn load_errors_fail_the_build_even_if_the_rest_renders() {
    let tmp = site_of(&[
        ("content/a.md", "---\n---\nno title\n"),
        ("content/ok.md", "---\ntitle: Ok\n---\nfine\n"),
    ]);
    let site = Site::load(tmp.path()).unwrap();
    assert_eq!(site.load_errors.len(), 1);
    assert!(site.pages.iter().all(|p| !p.source.ends_with("a.md")));
    assert_eq!(build(&tmp).unwrap_err().len(), 1);
}

#[test]
fn broken_dir_toml_is_reported_once_not_per_page() {
    let tmp = site_of(&[
        ("content/g/_dir.toml", "layout = [\n"),
        ("content/g/one.md", "---\n---\nx\n"),
        ("content/g/two.md", "---\n---\nx\n"),
    ]);
    let errors = build(&tmp).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].path.ends_with("g/_dir.toml"));
}

#[test]
fn errors_are_sorted_and_not_interleaved_at_one_location() {
    let tmp = site_of(&[
        ("content/z.md", "---\n---\nx\n"),
        ("content/b.md", "---\n---\nx\n"),
    ]);
    let errors = build(&tmp).unwrap_err();
    let paths: Vec<_> = errors.iter().map(|e| e.path.clone()).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
}
