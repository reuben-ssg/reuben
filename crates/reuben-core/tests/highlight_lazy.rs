//! The syntax set is process-global, so this check lives in its own test binary.

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::highlight::syntaxes_loaded;
use reuben_core::html::{HtmlOptions, render, render_opts};
use reuben_core::markdown::parse;
use reuben_core::site::{BuildOptions, Site};

fn build_site(config: &str) -> String {
    let tmp = TempDir::new().unwrap();
    tmp.child("reuben.toml").write_str(config).unwrap();
    tmp.child("content/index.md")
        .write_str("---\ntitle: T\n---\n```rust\nfn a() {}\n```\n")
        .unwrap();
    let site = Site::load(tmp.path()).unwrap();
    let out = tmp.path().join("dist");
    let opts = BuildOptions {
        drafts: false,
        out: out.clone(),
        jobs: None,
    };
    site.build(&opts).unwrap();
    std::fs::read_to_string(out.join("index.html")).unwrap()
}

fn highlighted(src: &str) -> String {
    let opts = HtmlOptions::from(&reuben_core::config::MarkdownConfig {
        highlight: true,
        ..Default::default()
    });
    render_opts(&parse(src, 1), &opts)
}

#[test]
fn syntax_set_loads_only_when_highlighting_is_on_and_a_fence_names_a_language() {
    assert!(!syntaxes_loaded());

    render(&parse("```rust\nfn a() {}\n```\n", 1));
    assert!(!syntaxes_loaded(), "highlighting off");

    let html = highlighted("# Title\n\nJust prose and `code`.\n");
    assert!(!syntaxes_loaded(), "no code blocks");
    assert!(!html.contains("hl-"));

    highlighted("```\nplain\n```\n");
    assert!(!syntaxes_loaded(), "language-less fence");

    let page = build_site("");
    assert!(!syntaxes_loaded(), "site without [markdown]");
    assert!(!page.contains("hl-"), "{page}");

    let html = highlighted("```rust\nfn a() {}\n```\n");
    assert!(syntaxes_loaded(), "rust fence");
    assert!(html.contains("class=\"hl-code\""));

    let page = build_site("[markdown]\nhighlight = true\n");
    assert!(page.contains("class=\"hl-code\""), "{page}");
}
