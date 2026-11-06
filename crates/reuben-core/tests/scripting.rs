use std::fs;
use std::path::Path;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::site::{BuildOptions, Site};
use reuben_core::{Error, ErrorKind};

fn fixture(name: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sites")
        .join(name);
    tmp.copy_from(src, &["**"]).unwrap();
    tmp
}

fn build(root: &Path) -> Result<(), Vec<Error>> {
    let site = Site::load(root)?;
    site.build(&BuildOptions {
        drafts: false,
        out: root.join("dist"),
    })
    .map(|_| ())
}

#[test]
fn page_blocks_render() {
    let tmp = fixture("scripted");
    fs::remove_file(tmp.path().join("content/broken.md")).unwrap();
    build(tmp.path()).unwrap();
    let html = fs::read_to_string(tmp.path().join("dist/index.html")).unwrap();
    assert!(html.contains("<h1>Home by Nick</h1>"), "{html}");
    assert!(html.contains("Tags: 2"), "{html}");
    assert!(
        html.contains("<a class=\"card\" href=\"/about/\">About</a>"),
        "{html}"
    );
    assert!(html.contains("<code>{{ not code }}</code>"), "{html}");
}

#[test]
fn sidecar_variables_reach_blocks() {
    let tmp = fixture("scripted");
    fs::remove_file(tmp.path().join("content/broken.md")).unwrap();
    build(tmp.path()).unwrap();
    let html = fs::read_to_string(tmp.path().join("dist/resume/index.html")).unwrap();
    assert!(html.contains("AB"), "{html}");
}

#[test]
fn script_error_points_at_file_line() {
    let tmp = fixture("scripted");
    let errs = build(tmp.path()).unwrap_err();
    let err = errs
        .iter()
        .find(|e| e.path.ends_with("broken.md"))
        .unwrap_or_else(|| panic!("{errs:?}"));
    match &err.kind {
        ErrorKind::Script(s) => assert_eq!(s.location.unwrap().line, 6),
        other => panic!("{other:?}"),
    }
}

#[test]
fn unknown_config_key_is_an_error() {
    let tmp = fixture("scripted");
    tmp.child("reuben.toml")
        .write_str("titel = \"x\"\n")
        .unwrap();
    let errs = Site::load(tmp.path()).unwrap_err();
    assert!(
        errs.iter().any(|e| e.to_string().contains("titel")),
        "{errs:?}"
    );
}
