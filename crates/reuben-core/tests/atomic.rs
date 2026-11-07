use std::fs;
use std::path::Path;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::site::{BuildOptions, RenderOptions, Site};

fn fixture(name: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sites")
        .join(name);
    tmp.copy_from(src, &["**"]).unwrap();
    tmp
}

fn build(root: &Path) -> Result<(), Vec<reuben_core::Error>> {
    Site::load(root)?
        .build(&BuildOptions {
            drafts: false,
            out: root.join("dist"),
        })
        .map(|_| ())
}

#[test]
fn failed_build_leaves_previous_output() {
    let tmp = fixture("basic");
    build(tmp.path()).unwrap();
    let before = fs::read(tmp.path().join("dist/index.html")).unwrap();
    tmp.child("content/broken.md")
        .write_str("---\ntitle: Broken\n---\n{{ nope( }}\n")
        .unwrap();
    assert!(build(tmp.path()).is_err());
    assert_eq!(
        fs::read(tmp.path().join("dist/index.html")).unwrap(),
        before
    );
    assert!(!tmp.path().join(".dist.reuben-staging").exists());
    assert!(!tmp.path().join(".dist.reuben-old").exists());
}

#[test]
fn render_writes_nothing() {
    let tmp = fixture("basic");
    let site = Site::load(tmp.path()).unwrap();
    let (output, _) = site
        .render(&RenderOptions {
            drafts: false,
            jobs: None,
        })
        .unwrap();
    assert!(output.files.contains_key(Path::new("index.html")));
    assert!(!tmp.path().join("dist").exists());
}
