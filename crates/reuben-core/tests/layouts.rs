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

fn site_with(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (path, text) in files {
        tmp.child(path).write_str(text).unwrap();
    }
    tmp
}

fn page_html(files: &[(&str, &str)]) -> String {
    let tmp = site_with(files);
    build(tmp.path()).unwrap();
    fs::read_to_string(tmp.path().join("dist/index.html")).unwrap()
}

#[test]
fn table_link_with_output_url_is_spliced() {
    let html = page_html(&[(
        "content/index.md",
        "---\ntitle: T\n---\n| a |\n|---|\n| [link]({{ \"/x?a=1&b=2\" }}) |\n",
    )]);
    assert!(html.contains("href=\"/x?a=1&amp;b=2\""), "{html}");
}

#[test]
fn footnote_link_with_output_url_is_spliced() {
    let html = page_html(&[(
        "content/index.md",
        "---\ntitle: T\n---\nA[^1]\n\n[^1]: See [here]({{ \"/foot\" }}).\n",
    )]);
    assert!(html.contains("href=\"/foot\""), "{html}");
}

#[test]
fn image_alt_keeps_output() {
    let html = page_html(&[(
        "content/index.md",
        "---\ntitle: Rye & \"Swiss\"\n---\n![{{ page.title }}](/img.png)\n",
    )]);
    assert!(
        html.contains("alt=\"Rye &amp; &quot;Swiss&quot;\""),
        "{html}"
    );
}

#[test]
fn layout_output_escapes_single_quotes() {
    let html = page_html(&[
        ("content/index.md", "---\ntitle: Nick's \"home\"\n---\nHi\n"),
        (
            "layouts/default.html",
            "<div title='{{ page.title }}'></div>",
        ),
    ]);
    assert!(
        html.contains("title='Nick&#39;s &quot;home&quot;'"),
        "{html}"
    );
}

#[test]
fn sole_text_output_keeps_its_paragraph() {
    let html = page_html(&[("content/index.md", "---\ntitle: T\n---\n{{ \"plain\" }}\n")]);
    assert!(html.contains("<p>plain</p>"), "{html}");
}

#[test]
fn sole_html_output_has_no_paragraph() {
    let html = page_html(&[(
        "content/index.md",
        "---\ntitle: T\n---\n{{ raw(\"<div>x</div>\") }}\n",
    )]);
    assert!(html.contains("<div>x</div>"), "{html}");
    assert!(!html.contains("<p><div>"), "{html}");
    assert!(!html.contains("<p>"), "{html}");
}

#[test]
fn layout_name_cannot_escape_layouts_dir() {
    let tmp = site_with(&[
        (
            "site/content/index.md",
            "---\ntitle: T\nlayout: ../../x\n---\nHi\n",
        ),
        ("site/layouts/keep.html", ""),
        ("x.html", "SECRET"),
    ]);
    let errors = build(&tmp.path().join("site")).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    let msg = errors[0].to_string();
    assert!(msg.contains("../../x"), "{msg}");
    assert!(msg.contains("index.md"), "{msg}");
}

#[test]
fn mistyped_theme_is_an_error() {
    let tmp = site_with(&[
        ("reuben.toml", "theme = \"themes/plian\"\n"),
        ("content/index.md", "---\ntitle: T\n---\nHi\n"),
    ]);
    let errors = Site::load(tmp.path()).unwrap_err();
    let msg = errors[0].to_string();
    assert!(msg.contains("themes/plian"), "{msg}");
    assert!(msg.contains("reuben.toml"), "{msg}");
}
