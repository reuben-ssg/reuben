use std::fs;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::site::{BuildOptions, Site};

fn build(files: &[(&str, &str)], out: &str) -> String {
    let tmp = TempDir::new().unwrap();
    for (path, text) in files {
        tmp.child(path).write_str(text).unwrap();
    }
    let site = Site::load(tmp.path()).unwrap();
    site.build(&BuildOptions {
        drafts: false,
        out: tmp.path().join("dist"),
    })
    .unwrap();
    fs::read_to_string(tmp.path().join("dist").join(out)).unwrap()
}

#[test]
fn heading_from_block_output_gets_its_id() {
    let html = build(
        &[(
            "content/index.md",
            "---\ntitle: Rye Bread\n---\n# {{ page.title }}\n",
        )],
        "index.html",
    );
    assert!(
        html.contains("<h1 id=\"rye-bread\">Rye Bread</h1>"),
        "{html}"
    );
}

#[test]
fn layout_toc_matches_rendered_ids() {
    let html = build(
        &[
            (
                "content/index.md",
                "---\ntitle: Rye Bread\n---\n# {{ page.title }}\n\n## Intro\n",
            ),
            (
                "layouts/default.html",
                "[{{ page.toc.map(|e| e.id + \":\" + e.level) }}]{{ content }}",
            ),
        ],
        "index.html",
    );
    assert!(html.contains("[rye-bread:1intro:2]"), "{html}");
    assert!(html.contains("<h2 id=\"intro\">Intro</h2>"), "{html}");
}

#[test]
fn block_toc_is_the_pre_block_one() {
    let html = build(
        &[(
            "content/index.md",
            "---\ntitle: T\n---\n# A\n\n## B\n\n{{ page.toc.len() }}\n",
        )],
        "index.html",
    );
    assert!(html.contains("<p>2</p>"), "{html}");
}

#[test]
fn collection_items_carry_toc() {
    let html = build(
        &[
            (
                "content/index.md",
                "---\ntitle: Home\n---\n{{ collections.docs.map(|p| p.toc.len()) }}\n",
            ),
            ("content/docs/a.md", "---\ntitle: A\n---\n# One\n\n## Two\n"),
            ("content/docs/b.md", "---\ntitle: B\n---\nNo headings.\n"),
        ],
        "index.html",
    );
    assert!(html.contains("20"), "{html}");
}
