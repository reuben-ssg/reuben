use std::fs;
use std::path::Path;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::site::{BuildOptions, Site};

const FIXTURE: &str = "tests/fixtures/sites/templates";

fn build() -> TempDir {
    let tmp = TempDir::new().unwrap();
    tmp.copy_from(Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE), &["**"])
        .unwrap();
    let site = Site::load(tmp.path()).unwrap();
    site.build(&BuildOptions {
        drafts: false,
        out: tmp.path().join("dist"),
    })
    .unwrap();
    tmp
}

fn dist(tmp: &TempDir, rel: &str) -> String {
    fs::read_to_string(tmp.path().join("dist").join(rel)).unwrap()
}

#[test]
fn xml_template_escapes_output() {
    let tmp = build();
    assert_eq!(
        dist(&tmp, "feed.xml"),
        "<rss>&lt;item&gt;Post A&lt;/item&gt;</rss>\n"
    );
}

#[test]
fn json_template_writes_raw_strings() {
    let tmp = build();
    let json = dist(&tmp, "search.json");
    assert!(json.contains("Post A & \"x\""), "{json}");
}

#[test]
fn html_template_gets_trailing_slash_route() {
    let tmp = build();
    assert!(dist(&tmp, "about/index.html").contains("<h1>About</h1>"));
    assert!(!tmp.path().join("dist/about.html").exists());
}

#[test]
fn static_template_is_processed() {
    let tmp = build();
    assert_eq!(dist(&tmp, "theme.css"), ":root { --x: 2; }\n");
}

#[test]
fn plain_and_binary_static_are_copied_verbatim() {
    let tmp = build();
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(FIXTURE)
        .join("static");
    for name in ["site.css", "logo.png", "dashes.bin"] {
        assert_eq!(
            fs::read(tmp.path().join("dist").join(name)).unwrap(),
            fs::read(src.join(name)).unwrap(),
            "{name}"
        );
    }
}

#[test]
fn only_html_templates_join_collections() {
    let tmp = TempDir::new().unwrap();
    tmp.copy_from(Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE), &["**"])
        .unwrap();
    tmp.child("content/posts/b.html")
        .write_str("---\ntitle: Post B\n---\n<p>b</p>\n")
        .unwrap();
    tmp.child("content/posts/data.json")
        .write_str("---\ntitle: Data\n---\n{}\n")
        .unwrap();
    tmp.child("content/index.md")
        .write_str("---\ntitle: Home\n---\n{{ collections.posts.map(|p| p.title + \",\") }}\n")
        .unwrap();
    let site = Site::load(tmp.path()).unwrap();
    site.build(&BuildOptions {
        drafts: false,
        out: tmp.path().join("dist"),
    })
    .unwrap();
    let home = dist(&tmp, "index.html");
    assert!(home.contains("Post A,Post B,"), "{home}");
    assert!(!home.contains("Data"), "{home}");
}

fn fixture_copy() -> TempDir {
    let tmp = TempDir::new().unwrap();
    tmp.copy_from(Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE), &["**"])
        .unwrap();
    tmp
}

fn build_in(tmp: &TempDir, drafts: bool) -> Result<(), Vec<reuben_core::Error>> {
    Site::load(tmp.path())?
        .build(&BuildOptions {
            drafts,
            out: tmp.path().join("dist"),
        })
        .map(|_| ())
}

#[test]
fn static_template_sidecar_is_not_published() {
    let tmp = fixture_copy();
    tmp.child("static/side.css")
        .write_str("---\ntitle: S\n---\na { color: {{ accent }}; }\n")
        .unwrap();
    tmp.child("static/side.rhai")
        .write_str("let accent = \"red\";\n")
        .unwrap();
    build_in(&tmp, false).unwrap();
    assert!(dist(&tmp, "side.css").contains("red"));
    assert!(!tmp.path().join("dist/side.rhai").exists());
}

#[test]
fn content_assets_are_copied_verbatim() {
    let tmp = fixture_copy();
    let bytes = b"\x89PNG\xff\xfe\x00";
    tmp.child("content/posts/rye.png")
        .write_binary(bytes)
        .unwrap();
    tmp.child("content/posts/a.rhai")
        .write_str("let x = 1;")
        .unwrap();
    tmp.child("content/_dir.toml").write_str("").unwrap();
    build_in(&tmp, false).unwrap();
    assert_eq!(
        fs::read(tmp.path().join("dist/posts/rye.png")).unwrap(),
        bytes
    );
    assert!(!tmp.path().join("dist/posts/a.rhai").exists());
    assert!(!tmp.path().join("dist/_dir.toml").exists());
}

#[test]
fn content_and_static_asset_collide() {
    let tmp = fixture_copy();
    tmp.child("content/posts/rye.png")
        .write_binary(b"a")
        .unwrap();
    tmp.child("static/posts/Rye.png")
        .write_binary(b"b")
        .unwrap();
    let errs = build_in(&tmp, false).unwrap_err();
    let text = format!("{errs:?}");
    assert!(text.contains("OutputCollision"), "{text}");
    assert!(
        text.contains("rye.png") && text.contains("Rye.png"),
        "{text}"
    );
}

#[test]
fn assets_in_drafted_folders_need_drafts_flag() {
    let tmp = fixture_copy();
    tmp.child("content/wip/_dir.toml")
        .write_str("draft = true\n")
        .unwrap();
    tmp.child("content/wip/pic.png").write_binary(b"x").unwrap();
    build_in(&tmp, false).unwrap();
    assert!(!tmp.path().join("dist/wip/pic.png").exists());
    build_in(&tmp, true).unwrap();
    assert!(tmp.path().join("dist/wip/pic.png").exists());
}

#[test]
fn uppercase_rhai_in_content_is_not_copied() {
    let tmp = fixture_copy();
    tmp.child("content/notes.RHAI")
        .write_str("let x = 1;")
        .unwrap();
    build_in(&tmp, false).unwrap();
    assert!(!tmp.path().join("dist/notes.RHAI").exists());
}

// Whether `side.RHAI` resolves as the sidecar of `side.css` depends on the filesystem's case sensitivity, so the
// check only runs where it does (macOS default). Elsewhere the file is an ordinary asset.
#[test]
fn uppercase_static_sidecar_is_not_published() {
    let tmp = fixture_copy();
    tmp.child("static/side.css")
        .write_str("---\ntitle: S\n---\na { color: {{ accent }}; }\n")
        .unwrap();
    tmp.child("static/side.RHAI")
        .write_str("let accent = \"red\";\n")
        .unwrap();
    let site = Site::load(tmp.path()).unwrap();
    if !site.pages.iter().any(|p| p.sidecar.is_some()) {
        return;
    }
    build_in(&tmp, false).unwrap();
    assert!(dist(&tmp, "side.css").contains("red"));
    assert!(!tmp.path().join("dist/side.RHAI").exists());
}

#[test]
fn junk_and_hidden_files_are_not_copied() {
    let tmp = fixture_copy();
    for junk in [
        "content/.DS_Store",
        "content/.git/config",
        "content/posts/draft.md.swp",
        "content/posts/a.png~",
        "content/Thumbs.db",
        "static/.DS_Store",
        "static/x.swp",
    ] {
        tmp.child(junk).write_str("junk").unwrap();
    }
    tmp.child("static/.well-known/security.txt")
        .write_str("Contact: x")
        .unwrap();
    build_in(&tmp, false).unwrap();
    let dist = tmp.path().join("dist");
    for gone in [
        ".DS_Store",
        ".git/config",
        "posts/draft.md.swp",
        "posts/a.png~",
        "Thumbs.db",
        "x.swp",
    ] {
        assert!(!dist.join(gone).exists(), "{gone}");
    }
    assert!(dist.join(".well-known/security.txt").exists());
}
