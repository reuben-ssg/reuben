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
