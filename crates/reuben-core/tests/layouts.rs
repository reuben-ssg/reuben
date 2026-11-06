use std::fs;
use std::path::Path;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::site::{BuildOptions, Site};
use reuben_core::{Error, ErrorKind};

/// Copies the whole fixtures tree so the site's `../../themes/plain` theme path still resolves.
fn layouts_site() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
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

fn root(tmp: &TempDir) -> std::path::PathBuf {
    tmp.path().join("sites/layouts")
}

#[test]
fn theme_default_layout_wraps_pages() {
    let tmp = layouts_site();
    let root = root(&tmp);
    fs::remove_file(root.join("content/missing.md")).unwrap();
    build(&root).unwrap();
    let html = fs::read_to_string(root.join("dist/index.html")).unwrap();
    assert!(html.contains("<title>Home · Rye</title>"), "{html}");
    assert!(html.contains("<main><p>Hello</p>\n</main>"), "{html}");
    assert!(html.contains("{{ theme"), "{html}");
}

#[test]
fn site_layout_sees_page_scope_and_theme_includes() {
    let tmp = layouts_site();
    let root = root(&tmp);
    fs::remove_file(root.join("content/missing.md")).unwrap();
    build(&root).unwrap();
    let html = fs::read_to_string(root.join("dist/post/index.html")).unwrap();
    assert_eq!(
        html.trim_end(),
        "<article><p>Body</p>\n<i>plain</i>S</article>"
    );
}

#[test]
fn site_static_overrides_theme_static() {
    let tmp = layouts_site();
    let root = root(&tmp);
    fs::remove_file(root.join("content/missing.md")).unwrap();
    build(&root).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("dist/style.css")).unwrap(),
        "/* site */"
    );
}

#[test]
fn theme_static_is_copied_when_site_has_none() {
    let tmp = layouts_site();
    let root = root(&tmp);
    fs::remove_file(root.join("content/missing.md")).unwrap();
    fs::remove_file(root.join("static/style.css")).unwrap();
    build(&root).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("dist/style.css")).unwrap(),
        "/* theme */"
    );
}

#[test]
fn missing_named_layout_is_an_error() {
    let tmp = layouts_site();
    let errors = build(&root(&tmp)).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].path.ends_with("content/missing.md"), "{errors:?}");
    assert!(errors[0].to_string().contains("nope"), "{errors:?}");
    assert!(matches!(errors[0].kind, ErrorKind::LayoutNotFound(_)));
}
