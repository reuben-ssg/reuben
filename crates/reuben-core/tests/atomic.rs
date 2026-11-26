use std::fs;
use std::path::Path;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::site::{BuildOptions, Failure, RenderOptions, Site};

fn fixture(name: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sites")
        .join(name);
    tmp.copy_from(src, &["**"]).unwrap();
    tmp
}

fn build(root: &Path) -> Result<(), Failure> {
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
    fs::write(other.path().join(".www.reuben-output"), "").unwrap();
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

#[test]
fn rebuild_removes_old_output() {
    let tmp = fixture("basic");
    tmp.child("content/gone.md")
        .write_str("---\ntitle: Gone\n---\nbye\n")
        .unwrap();
    build(tmp.path()).unwrap();
    assert!(tmp.path().join("dist/gone/index.html").exists());
    fs::remove_file(tmp.path().join("content/gone.md")).unwrap();
    build(tmp.path()).unwrap();
    assert!(tmp.path().join("dist/index.html").exists());
    assert!(!tmp.path().join("dist/gone").exists());
    assert!(!tmp.path().join(".dist.reuben-old").exists());
    assert!(!tmp.path().join(".dist.reuben-staging").exists());
}

/// Every file under `dir`, keyed by its path relative to `dir`.
fn snapshot(dir: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn walk(
        root: &Path,
        dir: &Path,
        files: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
    ) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                files.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    walk(dir, dir, &mut files);
    files
}

fn entry_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn streamed_build_matches_in_memory() {
    use reuben_core::output::OutputFile;
    let tmp = fixture("templates");
    tmp.child("data/site.json")
        .write_str(r#"{"name": "Rye"}"#)
        .unwrap();
    tmp.child("content/deli.md")
        .write_str("---\ntitle: Deli\n---\nBaked at {{ data.site.name }}.\n")
        .unwrap();
    let site = Site::load(tmp.path()).unwrap();
    let (output, _) = site
        .render(&RenderOptions {
            drafts: false,
            jobs: None,
        })
        .unwrap();
    let expected: std::collections::BTreeMap<_, _> = output
        .files
        .iter()
        .map(|(rel, file)| {
            let bytes = match file {
                OutputFile::Bytes(b) => b.clone(),
                OutputFile::Copy(src) => fs::read(src).unwrap(),
                other => panic!("{rel:?} is {other:?}"),
            };
            (rel.clone(), bytes)
        })
        .collect();
    assert!(expected.contains_key(Path::new("theme.css")));
    assert!(expected.contains_key(Path::new("logo.png")));
    assert!(expected.contains_key(Path::new("deli/index.html")));
    build(tmp.path()).unwrap();
    let built = snapshot(&tmp.path().join("dist"));
    assert!(!built.contains_key(Path::new(".reuben-output")));
    assert_eq!(built, expected);
}

#[test]
fn failed_streamed_build_keeps_dist() {
    let tmp = fixture("basic");
    for i in 0..50 {
        tmp.child(format!("content/many/p{i:02}.md"))
            .write_str(&format!("---\ntitle: P{i}\n---\nPage {i}\n"))
            .unwrap();
    }
    build(tmp.path()).unwrap();
    let before = snapshot(&tmp.path().join("dist"));
    tmp.child("content/many/p25.md")
        .write_str("---\ntitle: P25\n---\nok\n\n{{ nope() }}\n")
        .unwrap();
    tmp.child("content/many/p00.md")
        .write_str("---\ntitle: P0\n---\nchanged\n")
        .unwrap();
    let errs = build(tmp.path()).unwrap_err();
    assert!(errs.iter().any(|e| e.path.ends_with("p25.md")), "{errs:?}");
    assert_eq!(snapshot(&tmp.path().join("dist")), before);
    assert!(!tmp.path().join(".dist.reuben-staging").exists());
    assert!(!tmp.path().join(".dist.reuben-old").exists());
}

#[cfg(unix)]
#[test]
fn streamed_build_into_symlink() {
    let tmp = fixture("basic");
    let real = tmp.path().join("real");
    fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, tmp.path().join("dist")).unwrap();
    build(tmp.path()).unwrap();
    assert!(
        fs::symlink_metadata(tmp.path().join("dist"))
            .unwrap()
            .is_symlink()
    );
    assert_eq!(fs::read_link(tmp.path().join("dist")).unwrap(), real);
    assert!(real.join("index.html").is_file());
    assert!(real.join("notes/index.html").is_file());
    assert!(!real.join(".reuben-output").exists());
    assert!(tmp.path().join(".real.reuben-output").is_file());
    assert!(!tmp.path().join(".real.reuben-staging").exists());
    assert!(!tmp.path().join(".dist.reuben-staging").exists());
}

#[test]
fn out_dir_refused_before_rendering() {
    let tmp = fixture("basic");
    tmp.child("dist/keep.txt").write_str("mine").unwrap();
    let before = entry_names(tmp.path());
    let errs = build(tmp.path()).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e.kind, reuben_core::error::ErrorKind::UnsafeOutputDir)),
        "{errs:?}"
    );
    assert_eq!(entry_names(tmp.path()), before);
    assert_eq!(
        entry_names(&tmp.path().join("dist")),
        vec!["keep.txt".to_string()]
    );
}

#[test]
fn build_keeps_the_marker_out_of_dist_and_rebuilds() {
    let tmp = fixture("basic");
    build(tmp.path()).unwrap();
    assert!(tmp.path().join(".dist.reuben-output").is_file());
    assert!(!tmp.path().join("dist/.reuben-output").exists());
    build(tmp.path()).unwrap();
    assert!(!tmp.path().join("dist/.reuben-output").exists());
    assert!(tmp.path().join("dist/index.html").is_file());
}

#[test]
fn build_migrates_a_0_3_0_output_folder() {
    let tmp = fixture("basic");
    tmp.child("dist/.reuben-output").write_str("").unwrap();
    tmp.child("dist/stale.html").write_str("old").unwrap();
    build(tmp.path()).unwrap();
    assert!(!tmp.path().join("dist/.reuben-output").exists());
    assert!(!tmp.path().join("dist/stale.html").exists());
    assert!(tmp.path().join("dist/index.html").is_file());
    assert!(tmp.path().join(".dist.reuben-output").is_file());
}
