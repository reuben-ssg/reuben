use std::fs;
use std::path::{Path, PathBuf};

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::site::{BuildOptions, BuildReport, Failure, OUTPUT_MARKER, Site};
use reuben_core::{Error, ErrorKind};

fn fixture(name: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sites")
        .join(name);
    tmp.copy_from(src, &["**"]).unwrap();
    tmp
}

fn build(root: &Path, drafts: bool) -> Result<BuildReport, Failure> {
    let site = Site::load(root)?;
    site.build(&BuildOptions {
        drafts,
        out: root.join("dist"),
        jobs: None,
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
            drafts_skipped: 1,
            warnings: vec![],
            findings: vec![],
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
    assert!(tmp.path().join(".dist.reuben-output").is_file());
    assert!(!tmp.path().join("dist").join(OUTPUT_MARKER).exists());
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
    let errors = Site::load(tmp.path()).unwrap().load_errors;
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

#[test]
fn failed_collections_leave_previous_output() {
    let tmp = TempDir::new().unwrap();
    tmp.child("content/index.md")
        .write_str("---\ntitle: H\n---\nhi")
        .unwrap();
    tmp.child("content/a/x.md")
        .write_str("---\ntitle: X\n---\nx")
        .unwrap();
    tmp.child("content/b/y.md")
        .write_str("---\ntitle: Y\n---\ny")
        .unwrap();
    build(tmp.path(), false).unwrap();
    tmp.child("reuben.toml")
        .write_str("[collections.a]\nname = \"same\"\n[collections.b]\nname = \"same\"\n")
        .unwrap();
    assert!(build(tmp.path(), false).is_err());
    assert!(tmp.path().join("dist/index.html").exists());
}

#[test]
fn markdown_helper_follows_the_markdown_table() {
    let page = "---\ntitle: T\n---\n{{ markdown(\"a[^1]\\n\\n[^1]: b\\n\") }}\n";
    let render = |config: &str| {
        let tmp = TempDir::new().unwrap();
        tmp.child("reuben.toml").write_str(config).unwrap();
        tmp.child("content/index.md").write_str(page).unwrap();
        build(tmp.path(), false).unwrap();
        fs::read_to_string(tmp.path().join("dist/index.html")).unwrap()
    };
    assert!(render("").contains("aria-label=\"Back to reference 1\""));
    assert!(!render("[markdown]\nfootnote_backlinks = false\n").contains("Back to reference"));
}

#[test]
fn focusable_scroll_follows_the_markdown_table() {
    let page = "---\ntitle: T\n---\n# T\n\n```sh\nls\n```\n\n| a |\n|---|\n| 1 |\n\n{{ markdown(\"```\\nx\\n```\\n\") }}\n";
    let render = |config: &str| {
        let tmp = TempDir::new().unwrap();
        tmp.child("reuben.toml").write_str(config).unwrap();
        tmp.child("content/index.md").write_str(page).unwrap();
        build(tmp.path(), false).unwrap();
        fs::read_to_string(tmp.path().join("dist/index.html")).unwrap()
    };
    let on = render("");
    assert!(
        on.contains("<pre tabindex=\"0\" role=\"region\" aria-label=\"Code: sh\">"),
        "{on}"
    );
    assert!(
        on.contains("<pre tabindex=\"0\" role=\"region\" aria-label=\"Code\">"),
        "{on}"
    );
    assert!(
        on.contains(
            "<div class=\"table-wrap\" tabindex=\"0\" role=\"region\" aria-label=\"Table: T\">"
        ),
        "{on}"
    );
    let off = render("[markdown]\nfocusable_scroll = false\n");
    assert!(!off.contains("tabindex"), "{off}");
    assert!(!off.contains("table-wrap"), "{off}");
}

/// Every file under `dir`, relative to it, with its bytes.
fn tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files: Vec<_> = walkdir::WalkDir::new(dir)
        .sort_by_file_name()
        .into_iter()
        .map(Result::unwrap)
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            let rel = e.path().strip_prefix(dir).unwrap().to_path_buf();
            (rel, fs::read(e.path()).unwrap())
        })
        .collect();
    files.sort();
    files
}

#[test]
fn building_for_exit_writes_and_reports_the_same() {
    use reuben_core::site::LoadOptions;
    for name in ["basic", "collections", "bad"] {
        let tmp = fixture(name);
        let root = tmp.path();
        let opts = |out: &str| BuildOptions {
            drafts: false,
            out: root.join(out),
            jobs: None,
        };
        let kept = Site::load_and_build(root, &LoadOptions::default(), &opts("kept"));
        let left = Site::load_and_build_for_exit(root, &LoadOptions::default(), &opts("left"));
        match (kept, left) {
            (Ok(kept), Ok(left)) => {
                assert_eq!(kept, left, "{name}");
                assert_eq!(tree(&root.join("kept")), tree(&root.join("left")), "{name}");
            }
            (Err(kept), Err(left)) => {
                let messages =
                    |f: &Failure| f.errors.iter().map(ToString::to_string).collect::<Vec<_>>();
                assert_eq!(messages(&kept), messages(&left), "{name}");
                assert_eq!(kept.findings, left.findings, "{name}");
            }
            (kept, left) => panic!("{name}: {kept:?} against {left:?}"),
        }
    }
}
