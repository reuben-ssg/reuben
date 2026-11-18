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
    assert_eq!(
        on_page
            .iter()
            .filter(|e| matches!(e.kind, ErrorKind::Link { .. }))
            .count(),
        4
    );
}

#[test]
fn block_error_keeps_link_errors() {
    let tmp = site_of(&[("content/p.md", &page("{{ nope( }}\n\n[a](@/nope.md)"))]);
    let errors = build(&tmp).unwrap_err();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e.kind, ErrorKind::Script(_))),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.kind.to_string() == "no page `nope.md`"),
        "{errors:?}"
    );
}

#[test]
fn images_are_left_alone() {
    let tmp = site_of(&[("content/p.md", &page("![alt](x.md)"))]);
    build(&tmp).unwrap();
    assert!(
        built(&tmp, "p/index.html").contains("src=\"x.md\""),
        "{}",
        built(&tmp, "p/index.html")
    );
}

#[test]
fn percent_encoded_fragment_matches_a_non_ascii_id() {
    let tmp = site_of(&[
        ("content/b.md", &page("[c](@/c.md#caf%C3%A9)")),
        ("content/c.md", &page("# Café")),
    ]);
    build(&tmp).unwrap();
}

#[test]
fn markdown_in_data_needs_a_page() {
    let tmp = site_of(&[
        ("data/x.rhai", "#{ h: markdown(\"[c](@/c.md)\") }\n"),
        ("content/c.md", &page("c")),
    ]);
    let errors = build(&tmp).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.kind.to_string().contains("needs a page being built")),
        "{errors:?}"
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

#[test]
fn anchors_must_exist() {
    let tmp = site_of(&[
        ("content/b.md", &page("[x](@/c.md#top)\n\n[y](@/c.md#nope)")),
        ("content/c.md", &page("# Top")),
    ]);
    let errors = build(&tmp).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].path.ends_with("b.md"));
    assert_eq!(errors[0].location().unwrap().line, 6);
    assert_eq!(
        errors[0].kind.to_string(),
        "links to `#nope` on `c.md`, which has no such id"
    );
}

#[test]
fn layout_and_raw_ids_count() {
    let tmp = site_of(&[
        (
            "layouts/wide.html",
            "<!doctype html><html lang=\"en\"><head><title>T</title></head><body><nav id=\"menu\"></nav><main>{{ content }}</main></body></html>",
        ),
        ("content/b.md", &page("[m](@/c.md#menu) [r](@/c.md#raw)")),
        (
            "content/c.md",
            "---\ntitle: C\nlayout: wide\n---\n<div id=\"raw\">x</div>\n",
        ),
    ]);
    build(&tmp).unwrap();
}

#[test]
fn self_anchor_via_file() {
    let tmp = site_of(&[("content/b.md", &page("[l](b.md#later)\n\n## Later"))]);
    build(&tmp).unwrap();
}

#[test]
fn link_errors_are_identical_across_jobs() {
    let mut files: Vec<(String, String)> = Vec::new();
    for i in 0..40 {
        files.push((
            format!("content/p{i}.md"),
            page(&format!(
                "# H{i}\n\n[a](@/p{}.md#nope{i})\n\n[b](@/p{}.md#h{})",
                (i + 1) % 40,
                (i + 2) % 40,
                (i + 2) % 40
            )),
        ));
    }
    files.push(("content/bad.md".into(), page("[z](@/missing.md)")));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let tmp = site_of(&refs);
    let run = |jobs| {
        let failure = Site::load(tmp.path())
            .unwrap()
            .build(&BuildOptions {
                drafts: false,
                out: tmp.path().join("dist"),
                jobs,
            })
            .unwrap_err();
        failure
            .errors
            .iter()
            .map(|e| format!("{e} {:?}", e.location()))
            .collect::<Vec<_>>()
    };
    let one = run(Some(1));
    assert!(one.len() >= 41, "{one:?}");
    assert_eq!(one, run(None));
}

fn messages(errors: &[reuben_core::Error]) -> Vec<String> {
    errors
        .iter()
        .map(|e| {
            format!(
                "{}: {}",
                e.path.file_name().unwrap().to_string_lossy(),
                e.kind
            )
        })
        .collect()
}

#[test]
fn anchor_errors_survive_a_bad_link() {
    let tmp = site_of(&[
        (
            "content/b.md",
            &page("[a](@/missing.md)\n\n[b](@/c.md#nope)"),
        ),
        ("content/c.md", &page("# Top")),
    ]);
    let errors = build(&tmp).unwrap_err();
    assert_eq!(
        messages(&errors),
        [
            "b.md: no page `missing.md`",
            "b.md: links to `#nope` on `c.md`, which has no such id"
        ]
    );
}

#[test]
fn anchor_errors_survive_a_layout_error() {
    let tmp = site_of(&[
        (
            "content/b.md",
            "---\ntitle: B\nlayout: nolayout\n---\n[b](@/c.md#nope)\n",
        ),
        ("content/c.md", &page("# Top")),
    ]);
    let errors = build(&tmp).unwrap_err();
    assert_eq!(
        messages(&errors),
        [
            "b.md: layout `nolayout` not found in the site or its theme",
            "b.md: links to `#nope` on `c.md`, which has no such id"
        ]
    );
}

#[test]
fn failed_target_anchor_is_skipped() {
    let tmp = site_of(&[
        ("content/b.md", &page("[x](@/c.md#x)")),
        ("content/c.md", &page("{{ nope( }}")),
    ]);
    let errors = build(&tmp).unwrap_err();
    assert!(
        errors.iter().all(|e| e.path.ends_with("c.md")),
        "{errors:?}"
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
}

#[test]
fn template_pages_are_not_targets() {
    let tmp = site_of(&[
        ("content/b.md", &page("[t](@/t.md#x) [c](@/c.md#top)")),
        ("content/c.md", &page("# Top")),
        ("content/t.html", "<div id=\"x\">x</div>"),
    ]);
    let errors = build(&tmp).unwrap_err();
    let messages = messages(&errors);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].starts_with("b.md: no page `t.md`"),
        "{messages:?}"
    );
}

#[test]
fn url_for_in_a_layout() {
    let tmp = site_of(&[
        (
            "reuben.toml",
            "title = \"S\"\nbase_url = \"https://x.dev/sub/\"\n",
        ),
        (
            "layouts/l.html",
            "<!doctype html><html lang=\"en\"><head><title>T</title></head><body><main><a href=\"{{ url_for(\"@/c.md#top\") }}\">c</a>{{ content }}</main></body></html>",
        ),
        ("content/b.md", "---\ntitle: B\nlayout: l\n---\nb\n"),
        ("content/c.md", &page("# Top")),
    ]);
    build(&tmp).unwrap();
    let html = built(&tmp, "b/index.html");
    assert!(html.contains("href=\"https://x.dev/sub/c/#top\""), "{html}");
}

#[test]
fn url_for_relative_follows_the_page() {
    let tmp = site_of(&[
        ("includes/card.rhai", "fn card() { url_for(\"c.md\") }\n"),
        ("content/a/b.md", &page("[{{ card() }}]()")),
        ("content/a/c.md", &page("c")),
        ("content/x.md", &page("[{{ card() }}]()")),
        ("content/c.md", &page("c")),
    ]);
    build(&tmp).unwrap();
    let b = built(&tmp, "a/b/index.html");
    assert!(b.contains("<a href=\"\">/a/c/</a>"), "{b}");
    let x = built(&tmp, "x/index.html");
    assert!(x.contains("<a href=\"\">/c/</a>"), "{x}");
}

#[test]
fn url_for_errors_are_script_errors() {
    let tmp = site_of(&[(
        "content/p.md",
        &page("text\n\n{{ url_for(\"@/nope.md\") }}"),
    )]);
    let errors = build(&tmp).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(matches!(errors[0].kind, ErrorKind::Script(_)), "{errors:?}");
    assert_eq!(errors[0].location().unwrap().line, 6);
    assert!(
        errors[0].kind.to_string().contains("no page `nope.md`"),
        "{errors:?}"
    );
}

#[test]
fn url_for_in_data_needs_a_page() {
    let tmp = site_of(&[
        ("data/x.rhai", "url_for(\"@/c.md\")\n"),
        ("content/c.md", &page("c")),
    ]);
    let errors = build(&tmp).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.kind.to_string().contains("needs a page being built")),
        "{errors:?}"
    );
}

#[test]
fn markdown_helper_resolves_links() {
    let tmp = site_of(&[
        ("content/b.md", &page("{{ markdown(\"[c](@/c.md)\") }}")),
        ("content/c.md", &page("c")),
    ]);
    build(&tmp).unwrap();
    assert!(built(&tmp, "b/index.html").contains("href=\"/c/\""));

    let tmp = site_of(&[("content/b.md", &page("{{ markdown(\"[n](@/nope.md)\") }}"))]);
    let errors = build(&tmp).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(matches!(errors[0].kind, ErrorKind::Script(_)), "{errors:?}");
    assert!(errors[0].kind.to_string().contains("nope.md"), "{errors:?}");
}
