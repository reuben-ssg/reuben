use std::fmt::Write as _;
use std::path::PathBuf;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::site::{RenderOptions, Site};
use reuben_core::{Error, ErrorKind};

const COLLECTIONS: [&str; 4] = ["alpha", "beta", "gamma", "delta"];

fn big_site() -> TempDir {
    let tmp = TempDir::new().unwrap();
    tmp.child("layouts/default.html")
        .write_str("<main>{{ content }}</main>")
        .unwrap();
    tmp.child("reuben.toml")
        .write_str("[collections.alpha]\nlayout = \"default.html\"\n")
        .unwrap();
    tmp.child("data/info.toml")
        .write_str("name = \"Rye\"\n")
        .unwrap();
    tmp.child("content/index.md")
        .write_str(
            "---\ntitle: Home\n---\n{{ collections.alpha.map(|p| p.title) }} {{ data.info.name }}\n",
        )
        .unwrap();
    for (c, name) in COLLECTIONS.iter().enumerate() {
        for i in 0..50 {
            let mut body = String::new();
            write!(
                body,
                "---\ntitle: Page {c}-{i}\nweight: {i}\n---\n# Heading {i}\n\n## Sub\n\n## Sub\n\n\
                 Math $x^{i}$ here.\n\n{{{{ page.slug + \" \" + data.info.name }}}}\n\n\
                 ```rust\nfn main() {{ let n = {i}; }}\n```\n"
            )
            .unwrap();
            tmp.child(format!("content/{name}/p{i:02}.md"))
                .write_str(&body)
                .unwrap();
        }
    }
    tmp
}

fn render(site: &Site, jobs: usize) -> Result<reuben_core::output::Output, Vec<Error>> {
    site.render(&RenderOptions {
        drafts: false,
        jobs: Some(jobs),
    })
    .map(|(o, _)| o)
}

#[test]
fn output_is_identical_across_thread_counts() {
    let tmp = big_site();
    let site = Site::load(tmp.path()).unwrap();
    let one = render(&site, 1).unwrap();
    let eight = render(&site, 8).unwrap();
    assert!(one.files.len() >= 201);
    assert_eq!(one.files, eight.files);
}

#[test]
fn error_order_is_stable() {
    let tmp = big_site();
    for i in 0..20 {
        let name = COLLECTIONS[i % 4];
        tmp.child(format!("content/{name}/bad{i:02}.md"))
            .write_str(&format!(
                "---\ntitle: Bad\n---\nok\n\n{{{{ nope{i}( }}}}\n\n{{{{ missing{i}() }}}}\n"
            ))
            .unwrap();
    }
    let site = Site::load(tmp.path()).unwrap();
    let key =
        |errors: &[Error]| -> Vec<String> { errors.iter().map(ToString::to_string).collect() };
    let one = render(&site, 1).unwrap_err();
    let eight = render(&site, 8).unwrap_err();
    assert!(one.len() >= 20);
    assert_eq!(key(&one), key(&eight));

    let sort_key = |e: &Error| {
        let loc = match &e.kind {
            ErrorKind::Script(s) => s.location,
            ErrorKind::Block(b) => Some(b.span),
            _ => None,
        };
        (e.path.clone(), loc.map(|s| (s.line, s.column)))
    };
    let keys: Vec<(PathBuf, Option<(usize, usize)>)> = one.iter().map(sort_key).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}

#[test]
fn pre_block_toc_computed_once() {
    let tmp = TempDir::new().unwrap();
    tmp.child("content/index.md")
        .write_str("---\ntitle: Home\n---\n{{ collections.docs.map(|p| p.toc.map(|t| t.id)) }}\n")
        .unwrap();
    tmp.child("content/docs/a.md")
        .write_str("---\ntitle: A\n---\n# One\n\n## One\n\n{{ \"x\" }}\n")
        .unwrap();
    let site = Site::load(tmp.path()).unwrap();
    let page = site.pages.iter().find(|p| p.rel.ends_with("a.md")).unwrap();
    let ids: Vec<&str> = page.pre_block_toc.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["one", "one-1"]);
    let output = render(&site, 2).unwrap();
    let home = output
        .files
        .get(std::path::Path::new("index.html"))
        .unwrap();
    let reuben_core::output::OutputFile::Bytes(bytes) = home else {
        panic!("expected bytes");
    };
    let html = String::from_utf8(bytes.clone()).unwrap();
    assert!(html.contains("one") && html.contains("one-1"), "{html}");
}

#[test]
fn site_is_sync() {
    fn assert_sync<T: Sync>() {}
    assert_sync::<Site>();
}

#[test]
fn summary_is_precomputed() {
    let tmp = TempDir::new().unwrap();
    tmp.child("content/index.md")
        .write_str("---\ntitle: Home\n---\nFirst paragraph here.\n\nSecond.\n")
        .unwrap();
    let site = Site::load(tmp.path()).unwrap();
    assert_eq!(site.pages[0].summary, "First paragraph here.");
}
