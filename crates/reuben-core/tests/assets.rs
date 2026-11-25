//! The `asset()` helper in a built site.

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::error::ErrorKind;
use reuben_core::site::{BuildOptions, Failure, Site};

fn site_of(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (path, text) in files {
        tmp.child(path).write_str(text).unwrap();
    }
    tmp
}

fn build(tmp: &TempDir) -> Result<(), Failure> {
    Site::load(tmp.path())?
        .build(&BuildOptions {
            drafts: false,
            out: tmp.path().join("dist"),
            jobs: None,
        })
        .map(|_| ())
}

#[test]
fn asset_without_hashing_returns_plain_paths() {
    let tmp = site_of(&[
        ("reuben.toml", "[assets]\nhash = false\n"),
        ("content/index.md", "---\ntitle: H\n---\nhi"),
        ("static/css/site.css", "body { color: red; }\n"),
        (
            "layouts/default.html",
            "<link rel=\"stylesheet\" href=\"{{ asset(\"css/site.css\") }}\">",
        ),
    ]);
    build(&tmp).unwrap();
    let html = std::fs::read_to_string(tmp.path().join("dist/index.html")).unwrap();
    assert!(html.contains("href=\"/css/site.css\""), "{html}");
}

#[test]
fn asset_misses_on_two_pages_are_all_reported() {
    let tmp = site_of(&[
        (
            "content/a.md",
            "---\ntitle: A\n---\n{{ asset(\"nope.css\") }}",
        ),
        (
            "content/b.md",
            "---\ntitle: B\n---\n{{ asset(\"nope.css\") }}",
        ),
    ]);
    let Err(failure) = build(&tmp) else {
        panic!("build should fail");
    };
    let errors = &failure.errors;
    let misses: Vec<_> = errors
        .iter()
        .filter(|e| matches!(&e.kind, ErrorKind::Asset { spec } if spec == "nope.css"))
        .collect();
    assert_eq!(misses.len(), 2, "{errors:?}");
}
