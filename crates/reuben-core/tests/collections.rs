use std::fs;
use std::path::Path;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::site::{BuildOptions, Site};

fn build(drafts: bool) -> (TempDir, String) {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sites/collections");
    tmp.copy_from(src, &["**"]).unwrap();
    let site = Site::load(tmp.path()).unwrap();
    site.build(&BuildOptions {
        drafts,
        out: tmp.path().join("dist"),
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
