use std::fs;
use std::path::Path;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::Error;
use reuben_core::site::{BuildOptions, BuildReport, Site};

fn cascade_site() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sites/cascade");
    tmp.copy_from(src, &["**"]).unwrap();
    tmp
}

fn build(root: &Path, drafts: bool) -> Result<BuildReport, Vec<Error>> {
    Site::load(root)?.build(&BuildOptions {
        drafts,
        out: root.join("dist"),
    })
}

#[test]
fn dir_defaults_cascade_and_front_matter_wins() {
    let tmp = cascade_site();
    let site = Site::load(tmp.path()).unwrap();
    let a = site
        .pages
        .iter()
        .find(|p| p.rel == Path::new("guide/a.md"))
        .unwrap();
    assert_eq!(a.front_matter.layout.as_deref(), Some("doc"));
    assert_eq!(a.front_matter.extra["tags"], serde_json::json!(["own"]));
    assert_eq!(
        a.front_matter.extra["params"]["section"],
        serde_json::json!("root")
    );
    assert!(a.front_matter_sources["layout"].ends_with("guide/_dir.toml"));
    assert!(a.front_matter_sources["tags"].ends_with("guide/a.md"));
    assert!(a.front_matter_sources["params.section"].ends_with("content/_dir.toml"));
}

#[test]
fn dir_toml_draft_excludes_subtree_despite_page() {
    let tmp = cascade_site();
    let report = build(tmp.path(), false).unwrap();
    assert!(!tmp.path().join("dist/guide/old/x/index.html").exists());
    assert!(tmp.path().join("dist/guide/a/index.html").exists());
    // x, notes/index and notes/n
    assert_eq!(report.drafts_skipped, 3);
}

#[test]
fn draft_index_excludes_folder() {
    let tmp = cascade_site();
    build(tmp.path(), false).unwrap();
    assert!(!tmp.path().join("dist/notes").exists());
}

#[test]
fn drafts_flag_includes_everything() {
    let tmp = cascade_site();
    let report = build(tmp.path(), true).unwrap();
    assert!(tmp.path().join("dist/guide/old/x/index.html").exists());
    assert!(tmp.path().join("dist/notes/n/index.html").exists());
    assert_eq!(report.drafts_skipped, 0);
}

#[test]
fn bad_dir_toml_names_its_file() {
    let tmp = cascade_site();
    fs::write(tmp.path().join("content/guide/_dir.toml"), "layout = [\n").unwrap();
    let errors = Site::load(tmp.path()).unwrap_err();
    assert!(
        errors.iter().any(|e| e.path.ends_with("guide/_dir.toml")),
        "{errors:?}"
    );
}

#[test]
fn dir_toml_is_not_output() {
    let tmp = cascade_site();
    build(tmp.path(), true).unwrap();
    assert!(!tmp.path().join("dist/_dir.toml").exists());
    assert!(!tmp.path().join("dist/guide/_dir.toml").exists());
}

fn site_of(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (path, text) in files {
        tmp.child(path).write_str(text).unwrap();
    }
    tmp
}

#[test]
fn draft_home_page_only_excludes_itself() {
    let tmp = site_of(&[
        ("content/index.md", "---\ntitle: H\ndraft: true\n---\nhome"),
        ("content/about.md", "---\ntitle: A\n---\na"),
        ("content/posts/a.md", "---\ntitle: P\n---\np"),
        ("content/img.png", "png"),
    ]);
    let report = build(tmp.path(), false).unwrap();
    assert!(!tmp.path().join("dist/index.html").exists());
    assert!(tmp.path().join("dist/about/index.html").exists());
    assert!(tmp.path().join("dist/posts/a/index.html").exists());
    assert!(tmp.path().join("dist/img.png").exists());
    assert_eq!(report.drafts_skipped, 1);
}

#[test]
fn root_dir_toml_draft_still_drafts_everything() {
    let tmp = site_of(&[
        ("content/_dir.toml", "draft = true\n"),
        ("content/about.md", "---\ntitle: A\n---\na"),
    ]);
    let report = build(tmp.path(), false).unwrap();
    assert_eq!(report.drafts_skipped, 1);
}

#[test]
fn page_draft_reflects_inherited_drafting() {
    let tmp = site_of(&[
        ("content/old/_dir.toml", "draft = true\n"),
        (
            "content/old/p.md",
            "---\ntitle: P\ndraft: false\n---\n{{ page.draft }}",
        ),
    ]);
    build(tmp.path(), true).unwrap();
    let html = fs::read_to_string(tmp.path().join("dist/old/p/index.html")).unwrap();
    assert!(html.contains("true"), "{html}");
}
