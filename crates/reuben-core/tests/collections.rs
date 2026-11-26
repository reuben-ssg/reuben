use std::fs;
use std::path::Path;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::site::{BuildOptions, Failure, Site};

fn build(drafts: bool) -> (TempDir, String) {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sites/collections");
    tmp.copy_from(src, &["**"]).unwrap();
    let site = Site::load(tmp.path()).unwrap();
    site.build(&BuildOptions {
        drafts,
        out: tmp.path().join("dist"),
        jobs: None,
    })
    .unwrap();
    let html = fs::read_to_string(tmp.path().join("dist/index.html")).unwrap();
    (tmp, html)
}

#[test]
fn collections_sort_rename_and_exclude_drafts() {
    let (_tmp, html) = build(false);
    assert!(html.contains("BAC|3|"), "{html}");
}

#[test]
fn tree_has_dirs_and_leaves() {
    let (_tmp, html) = build(false);
    assert!(html.contains("|StyleZ|"), "{html}");
}

#[test]
fn array_sort_by_ascending() {
    let (_tmp, html) = build(false);
    assert!(html.contains("|ZStyleGuideVoice"), "{html}");
}

#[test]
fn collection_default_layout_applies() {
    let (tmp, _) = build(false);
    let html = fs::read_to_string(tmp.path().join("dist/posts/a/index.html")).unwrap();
    assert!(html.contains("<article>"), "{html}");
}

#[test]
fn drafts_flag_adds_drafts_to_collections() {
    let (_tmp, html) = build(true);
    assert!(html.contains("DBAC|4|"), "{html}");
}

fn build_with_page(body: &str) -> Failure {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sites/collections");
    tmp.copy_from(src, &["**"]).unwrap();
    tmp.child("content/bad.md").write_str(body).unwrap();
    let site = Site::load(tmp.path()).unwrap();
    site.build(&BuildOptions {
        drafts: false,
        out: tmp.path().join("dist"),
        jobs: None,
    })
    .unwrap_err()
}

#[test]
fn error_in_map_closure_names_page_and_line() {
    let errs = build_with_page(
        "---\ntitle: Bad\n---\n\nintro\n\n{{ collections.blog.map(|p| p.nope.x) }}\n",
    );
    let e = errs.iter().find(|e| e.path.ends_with("bad.md")).unwrap();
    let reuben_core::ErrorKind::Script(s) = &e.kind else {
        panic!("{e:?}")
    };
    assert_eq!(s.location.unwrap().line, 7, "{s:?}");
}

#[test]
fn array_sort_by_helper_orders_filtered_collection() {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sites/collections");
    tmp.copy_from(src, &["**"]).unwrap();
    tmp.child("content/sorted.md")
        .write_str("---\ntitle: S\n---\n{{ collections.blog.filter(|p| true).sort_by(\"date\").map(|p| p.title) }}|{{ [#{d: 2, path: \"b\"}, #{d: 1, path: \"a\"}].sort_by(\"d\").map(|p| p.path) }}\n")
        .unwrap();
    let site = Site::load(tmp.path()).unwrap();
    site.build(&BuildOptions {
        drafts: false,
        out: tmp.path().join("dist"),
        jobs: None,
    })
    .unwrap();
    let html = fs::read_to_string(tmp.path().join("dist/sorted/index.html")).unwrap();
    assert!(html.contains("ABC|ab"), "{html}");
}

/// The errors of a build of a site with two collections named alike and a member page whose block fails, with or
/// without a script that names `collections`.
fn duplicate_name_errors(names_collections: bool) -> Vec<String> {
    let tmp = TempDir::new().unwrap();
    tmp.child("reuben.toml")
        .write_str("[collections.a]\nname = \"same\"\n[collections.b]\nname = \"same\"\n")
        .unwrap();
    tmp.child("content/a/x.md")
        .write_str("---\ntitle: X\n---\n{{ nope( }}\n")
        .unwrap();
    tmp.child("content/b/y.md")
        .write_str("---\ntitle: Y\n---\ny\n")
        .unwrap();
    if names_collections {
        tmp.child("layouts/unused.html")
            .write_str("{{ collections.len() }}")
            .unwrap();
    }
    let failure = Site::load(tmp.path())
        .unwrap()
        .build(&BuildOptions {
            drafts: false,
            out: tmp.path().join("dist"),
            jobs: None,
        })
        .unwrap_err();
    let root = tmp.path().to_string_lossy().into_owned();
    failure
        .errors
        .iter()
        .map(|e| e.to_string().replace(&root, ""))
        .collect()
}

#[test]
fn duplicate_collection_names_are_reported_once_whether_or_not_a_script_reads_collections() {
    for names_collections in [false, true] {
        let errors = duplicate_name_errors(names_collections);
        let duplicates = errors
            .iter()
            .filter(|e| e.contains("two collections are named \"same\""))
            .count();
        assert_eq!(duplicates, 1, "{names_collections}: {errors:?}");
        let page = errors.iter().filter(|e| e.contains("a/x.md")).count();
        assert_eq!(page, 1, "{names_collections}: {errors:?}");
    }
    assert_eq!(duplicate_name_errors(false), duplicate_name_errors(true));
}
