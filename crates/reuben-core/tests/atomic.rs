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
            jobs: None,
        })
        .map(|_| ())
}

#[test]
fn failed_build_leaves_previous_output() {
    let tmp = fixture("basic");
    build(tmp.path()).unwrap();
    let before = fs::read(tmp.path().join("dist/index.html")).unwrap();
    tmp.child("content/broken.md")
        .write_str("---\ntitle: Broken\n---\nok\n\n{{ nope( }}\n")
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

#[cfg(unix)]
#[test]
fn every_unreadable_static_and_page_error_is_reported() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = fixture("basic");
    build(tmp.path()).unwrap();
    let before = fs::read(tmp.path().join("dist/index.html")).unwrap();
    for name in ["a.png", "b.png", "c.png"] {
        tmp.child(format!("static/{name}")).write_str("x").unwrap();
    }
    tmp.child("content/broken.md")
        .write_str("---\ntitle: Broken\n---\nok\n\n{{ nope() }}\n")
        .unwrap();
    // Load opens every static file, so they only become unreadable after it, as with a theme's files or a race.
    let site = Site::load(tmp.path()).unwrap();
    let mut locked = Vec::new();
    for name in ["a.png", "b.png", "c.png"] {
        let f = tmp.path().join("static").join(name);
        fs::set_permissions(&f, fs::Permissions::from_mode(0o000)).unwrap();
        locked.push(f);
    }
    let errs = site
        .build(&BuildOptions {
            drafts: false,
            out: tmp.path().join("dist"),
            jobs: None,
        })
        .unwrap_err();
    for p in &locked {
        assert!(errs.iter().any(|e| &e.path == p), "{p:?} in {errs:?}");
    }
    assert!(
        errs.iter().any(|e| e.path.ends_with("broken.md")),
        "{errs:?}"
    );
    assert_eq!(
        fs::read(tmp.path().join("dist/index.html")).unwrap(),
        before
    );
    assert!(!tmp.path().join(".dist.reuben-staging").exists());
}

#[cfg(unix)]
#[test]
fn symlinked_out_dir_keeps_its_link() {
    let site = fixture("basic");
    let other = TempDir::new().unwrap();
    let target = other.path().join("www");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join(".reuben-output"), "").unwrap();
    fs::write(target.join("index.html"), "old").unwrap();
    std::os::unix::fs::symlink(&target, site.path().join("dist")).unwrap();
    build(site.path()).unwrap();
    let dist = site.path().join("dist");
    assert!(fs::symlink_metadata(&dist).unwrap().is_symlink());
    assert_ne!(
        fs::read_to_string(target.join("index.html")).unwrap(),
        "old"
    );
    for dir in [site.path(), other.path()] {
        for e in fs::read_dir(dir).unwrap() {
            let n = e.unwrap().file_name().to_string_lossy().into_owned();
            assert!(
                !n.contains("reuben-staging") && !n.contains("reuben-old"),
                "{n}"
            );
        }
    }
}
