use std::fs;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::ErrorKind;
use reuben_core::site::{BuildOptions, Failure, Site};

fn site_of(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (path, text) in files {
        tmp.child(path).write_str(text).unwrap();
    }
    tmp
}

fn build_with(tmp: &TempDir, drafts: bool) -> Result<(), Failure> {
    Site::load(tmp.path())?
        .build(&BuildOptions {
            drafts,
            out: tmp.path().join("dist"),
            jobs: None,
        })
        .map(|_| ())
}

fn build(tmp: &TempDir) -> Result<(), Failure> {
    build_with(tmp, false)
}

fn page(body: &str) -> String {
    format!("---\ntitle: T\n---\n{body}\n")
}

fn built(tmp: &TempDir, file: &str) -> String {
    fs::read_to_string(tmp.path().join("dist").join(file)).unwrap()
}

#[test]
fn links_build_to_page_urls() {
    let tmp = site_of(&[
        ("content/a/b.md", &page("[x](@/c.md#top) and [y](../c.md)")),
        ("content/c.md", &page("# Top")),
    ]);
    build(&tmp).unwrap();
    let html = built(&tmp, "a/b/index.html");
    assert!(html.contains("href=\"/c/#top\""), "{html}");
    assert!(html.contains("href=\"/c/\""), "{html}");
}

#[test]
fn base_path_is_prefixed() {
    let tmp = site_of(&[
        (
            "reuben.toml",
            "title = \"S\"\nbase_url = \"https://x.dev/sub/\"\n",
        ),
        ("content/b.md", &page("[y](c.md)")),
        ("content/c.md", &page("c")),
    ]);
    build(&tmp).unwrap();
    assert!(
        built(&tmp, "b/index.html").contains("href=\"/sub/c/\""),
        "{}",
        built(&tmp, "b/index.html")
    );
}

#[test]
fn every_bad_link_is_reported() {
    let tmp = site_of(&[
        (
            "content/p.md",
            &page(
                "[a](@/nope.md)\n\n[b](@/_index.md)\n\n[c](../../x.md)\n\n[d](@/d)\n\n$\\frac{1}{$",
            ),
        ),
        ("content/index.md", &page("home")),
    ]);
    let errors = build(&tmp).unwrap_err();
    let on_page: Vec<_> = errors.iter().filter(|e| e.path.ends_with("p.md")).collect();
    assert_eq!(on_page.len(), 5, "{errors:?}");
    let message = |line: usize| {
        on_page
            .iter()
            .find(|e| e.location().is_some_and(|l| l.line == line))
            .unwrap_or_else(|| panic!("no error on line {line}: {on_page:?}"))
            .kind
            .to_string()
    };
    assert_eq!(message(4), "no page `nope.md`");
    assert_eq!(message(6), "no page `_index.md`; did you mean `index.md`?");
    assert_eq!(message(8), "link `../../x.md` points outside content/");
    assert_eq!(message(10), "link `@/d` must name a .md file");
    assert!(message(12).starts_with("math:"));
    assert!(
        on_page
            .iter()
            .filter(|e| matches!(e.kind, ErrorKind::Link { .. }))
            .count()
            == 4
    );
}

#[test]
fn drafts_need_the_flag() {
    let tmp = site_of(&[
        ("content/a.md", &page("[d](@/d.md)")),
        ("content/d.md", "---\ntitle: D\ndraft: true\n---\nd\n"),
    ]);
    let errors = build(&tmp).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(
        errors[0].kind.to_string(),
        "links to `d.md`, a draft that isn't built without --drafts"
    );
    build_with(&tmp, true).unwrap();
    assert!(built(&tmp, "a/index.html").contains("href=\"/d/\""));
}

#[test]
fn failed_target_is_not_doubled() {
    let tmp = site_of(&[
        ("content/b.md", &page("[c](@/c.md)")),
        ("content/c.md", "---\ntitle: [\n---\nc\n"),
    ]);
    let errors = build(&tmp).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].path.ends_with("c.md"));
}
