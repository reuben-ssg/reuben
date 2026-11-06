use std::fs;
use std::path::{Path, PathBuf};

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::site::{BuildOptions, BuildReport, OUTPUT_MARKER, Site};
use reuben_core::{Error, ErrorKind};

fn fixture(name: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sites")
        .join(name);
    tmp.copy_from(src, &["**"]).unwrap();
    tmp
}

fn build(root: &Path, drafts: bool) -> Result<BuildReport, Vec<Error>> {
    let site = Site::load(root)?;
    site.build(&BuildOptions {
        drafts,
        out: root.join("dist"),
    })
}

fn names(paths: &[&Path]) -> Vec<String> {
    let mut v: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    v.sort();
    v
}

#[test]
fn builds_basic_site() {
    let tmp = fixture("basic");
    let report = build(tmp.path(), false).unwrap();
    assert_eq!(
        report,
        BuildReport {
            pages_written: 3,
            static_copied: 2,
            drafts_skipped: 1
        }
    );
    let out = tmp.path().join("dist");
    assert!(out.join("index.html").is_file());
    assert!(out.join("about/index.html").is_file());
    assert!(out.join("notes/index.html").is_file());
    assert!(out.join("CNAME").is_file());
    assert!(!out.join("notes/one/index.html").exists());
    insta::assert_snapshot!(fs::read_to_string(out.join("about/index.html")).unwrap());
}

#[test]
fn drafts_flag_includes_drafts() {
    let tmp = fixture("basic");
    build(tmp.path(), true).unwrap();
    assert!(tmp.path().join("dist/notes/one/index.html").is_file());
}

fn collision(errors: &[Error]) -> Vec<String> {
    match errors {
        [
            Error {
                path,
                kind: ErrorKind::OutputCollision { other, .. },
            },
        ] => names(&[path, other]),
        other => panic!("expected one OutputCollision, got {other:?}"),
    }
}

#[test]
fn page_collision_names_both() {
    let tmp = fixture("collide");
    let errors = build(tmp.path(), false).unwrap_err();
    let got = collision(&errors);
    assert!(got[0].ends_with("content/a.md"), "{got:?}");
    assert!(got[1].ends_with("content/a/index.md"), "{got:?}");
}

#[test]
fn static_collision_names_both() {
    let tmp = fixture("static-collide");
    let errors = build(tmp.path(), false).unwrap_err();
    let got = collision(&errors);
    assert!(got[0].ends_with("content/about.md"), "{got:?}");
    assert!(got[1].ends_with("static/about/index.html"), "{got:?}");
}

#[test]
fn refuses_foreign_out_dir() {
    let tmp = fixture("basic");
    tmp.child("dist/keep.txt").write_str("mine").unwrap();
    let errors = build(tmp.path(), false).unwrap_err();
    assert!(matches!(
        errors.as_slice(),
        [Error {
            kind: ErrorKind::UnsafeOutputDir,
            ..
        }]
    ));
    assert!(tmp.path().join("dist/keep.txt").is_file());
}

#[test]
fn clears_own_out_dir() {
    let tmp = fixture("basic");
    build(tmp.path(), false).unwrap();
    assert!(tmp.path().join("dist").join(OUTPUT_MARKER).is_file());
    fs::remove_file(tmp.path().join("content/about.md")).unwrap();
    tmp.child("dist/stale.html").write_str("old").unwrap();
    build(tmp.path(), false).unwrap();
    assert!(!tmp.path().join("dist/about").exists());
    assert!(!tmp.path().join("dist/stale.html").exists());
    assert!(tmp.path().join("dist/index.html").is_file());
}

#[test]
fn reports_all_bad_files() {
    let tmp = fixture("bad");
    let errors = match Site::load(tmp.path()) {
        Err(e) => e,
        Ok(_) => panic!("expected load errors"),
    };
    let paths: Vec<PathBuf> = errors.iter().map(|e| e.path.clone()).collect();
    let got = names(&paths.iter().map(PathBuf::as_path).collect::<Vec<_>>());
    assert_eq!(got.len(), 2, "{errors:?}");
    assert!(got[0].ends_with("content/binary.md"));
    assert!(got[1].ends_with("content/no-title.md"));
    assert!(errors.iter().any(|e| matches!(e.kind, ErrorKind::NotUtf8)));
}

#[test]
fn case_only_collision_names_both() {
    let tmp = TempDir::new().unwrap();
    tmp.child("content/Notes.md")
        .write_str("---\ntitle: Upper\n---\n")
        .unwrap();
    tmp.child("content/notes/index.md")
        .write_str("---\ntitle: Lower\n---\n")
        .unwrap();
    let errors = build(tmp.path(), false).unwrap_err();
    let got = collision(&errors);
    assert!(got[0].ends_with("content/Notes.md"), "{got:?}");
    assert!(got[1].ends_with("content/notes/index.md"), "{got:?}");
}
