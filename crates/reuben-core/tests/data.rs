use std::fs;
use std::path::Path;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::ErrorKind;
use reuben_core::site::{BuildOptions, Site};

fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sites/data");
    tmp.copy_from(src, &["**"]).unwrap();
    tmp
}

#[test]
fn data_files_nest_and_scripts_compute() {
    let tmp = fixture();
    let site = Site::load(tmp.path()).unwrap();
    site.build(&BuildOptions {
        drafts: false,
        out: tmp.path().join("dist"),
        jobs: None,
    })
    .unwrap();
    let html = fs::read_to_string(tmp.path().join("dist/index.html")).unwrap();
    assert!(html.contains("Rye Nick Writer 1 Deli"), "{html}");
}

#[test]
fn data_sources_are_recorded() {
    let tmp = fixture();
    let site = Site::load(tmp.path()).unwrap();
    assert!(site.data.sources["team.members.people"].ends_with("data/team/members.toml"));
    assert!(site.data.sources["stats"].ends_with("data/stats.rhai"));
}

#[test]
fn file_and_folder_collision_is_an_error() {
    let tmp = fixture();
    tmp.child("data/team.toml").write_str("x = 1\n").unwrap();
    let errs = Site::load(tmp.path()).unwrap().load_errors;
    let text = errs.iter().map(ToString::to_string).collect::<String>();
    assert!(text.contains("team.toml"), "{text}");
    assert!(text.contains("data/team"), "{text}");
}

#[test]
fn script_colliding_with_plain_data_is_an_error() {
    let tmp = fixture();
    tmp.child("data/jobs.rhai").write_str("1").unwrap();
    let errs = Site::load(tmp.path()).unwrap().load_errors;
    let text = errs.iter().map(ToString::to_string).collect::<String>();
    assert!(
        text.contains("jobs.rhai") && text.contains("jobs.yaml"),
        "{text}"
    );
}

#[test]
fn data_script_cannot_see_collections() {
    let tmp = fixture();
    tmp.child("data/bad.rhai").write_str("collections").unwrap();
    let errs = Site::load(tmp.path()).unwrap().load_errors;
    assert!(
        errs.iter().any(|e| e.path.ends_with("bad.rhai")),
        "{errs:?}"
    );
}

#[test]
fn data_script_error_has_location() {
    let tmp = fixture();
    tmp.child("data/err.rhai").write_str("\n  nope()").unwrap();
    let errs = Site::load(tmp.path()).unwrap().load_errors;
    let e = errs.iter().find(|e| e.path.ends_with("err.rhai")).unwrap();
    let ErrorKind::Script(s) = &e.kind else {
        panic!("{e:?}")
    };
    assert_eq!(s.location.unwrap().line, 2);
}

#[test]
fn hidden_and_junk_data_files_are_skipped() {
    let tmp = TempDir::new().unwrap();
    tmp.child("data/team.toml").write_str("n = 1\n").unwrap();
    tmp.child("data/._team.toml")
        .write_str("\u{0}\u{1}[[")
        .unwrap();
    tmp.child("data/.DS_Store").write_str("junk").unwrap();
    tmp.child("data/.hidden/x.json").write_str("{").unwrap();
    tmp.child("content/index.md")
        .write_str("---\ntitle: H\n---\n{{ data.team.n }}")
        .unwrap();
    Site::load(tmp.path())
        .unwrap()
        .build(&BuildOptions {
            drafts: false,
            out: tmp.path().join("dist"),
            jobs: None,
        })
        .unwrap();
    let html = fs::read_to_string(tmp.path().join("dist/index.html")).unwrap();
    assert!(html.contains('1'), "{html}");
}
