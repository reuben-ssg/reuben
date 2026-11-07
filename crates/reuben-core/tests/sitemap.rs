use std::fs;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::site::{BuildOptions, BuildReport, Site};

fn build(config: &str, files: &[(&str, &str)]) -> (TempDir, BuildReport) {
    let tmp = TempDir::new().unwrap();
    if !config.is_empty() {
        tmp.child("reuben.toml").write_str(config).unwrap();
    }
    for (path, text) in files {
        tmp.child(path).write_str(text).unwrap();
    }
    let site = Site::load(tmp.path()).unwrap();
    let report = site
        .build(&BuildOptions {
            drafts: false,
            out: tmp.path().join("dist"),
            jobs: None,
        })
        .unwrap();
    (tmp, report)
}

fn page(title: &str, extra: &str) -> String {
    format!("---\ntitle: {title}\n{extra}---\nbody\n")
}

fn pages() -> Vec<(&'static str, String)> {
    vec![
        ("content/index.md", page("Home", "")),
        ("content/about.md", page("About", "")),
        ("content/404.md", page("Missing", "")),
        ("content/notes/index.md", page("Notes", "draft: true\n")),
        ("content/notes/n.md", page("N", "")),
    ]
}

fn run(config: &str, extra: &[(&str, &str)]) -> (TempDir, BuildReport) {
    let owned = pages();
    let mut files: Vec<(&str, &str)> = owned.iter().map(|(p, t)| (*p, t.as_str())).collect();
    files.extend_from_slice(extra);
    build(config, &files)
}

#[test]
fn sitemap_lists_pages_without_drafts_or_404() {
    let (tmp, report) = run("base_url = \"https://x.dev/\"\n", &[]);
    let xml = fs::read_to_string(tmp.path().join("dist/sitemap.xml")).unwrap();
    let locs: Vec<&str> = xml
        .split("<loc>")
        .skip(1)
        .map(|s| s.split("</loc>").next().unwrap())
        .collect();
    assert_eq!(locs, ["https://x.dev/", "https://x.dev/about/"]);
    assert!(report.warnings.is_empty());
}

#[test]
fn relative_base_url_skips_with_warning() {
    let (tmp, report) = run("", &[]);
    assert!(!tmp.path().join("dist/sitemap.xml").exists());
    assert_eq!(
        report.warnings,
        ["sitemap.xml skipped: base_url must be absolute (http:// or https://)"]
    );
}

#[test]
fn user_sitemap_wins() {
    let (tmp, report) = run(
        "base_url = \"https://x.dev/\"\n",
        &[("static/sitemap.xml", "<mine/>")],
    );
    let out = fs::read_to_string(tmp.path().join("dist/sitemap.xml")).unwrap();
    assert_eq!(out, "<mine/>");
    assert!(report.warnings.is_empty());
}

#[test]
fn sitemap_can_be_disabled() {
    let (tmp, report) = run("sitemap = false\n", &[]);
    assert!(!tmp.path().join("dist/sitemap.xml").exists());
    assert!(report.warnings.is_empty());
}
