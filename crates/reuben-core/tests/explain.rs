use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::ErrorKind;
use reuben_core::explain::{Entry, Explanation, Source};
use reuben_core::site::{LoadOptions, Site};

fn site(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (path, text) in files {
        tmp.child(path).write_str(text).unwrap();
    }
    tmp
}

fn explain(tmp: &TempDir, page: &str) -> Explanation {
    Site::load(tmp.path()).unwrap().explain(page).unwrap()
}

fn entry<'a>(ex: &'a Explanation, name: &str) -> &'a Entry {
    ex.entries
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no entry {name}: {:?}", ex.entries))
}

const POST: &str = "---\ntitle: Hello\n---\nBody.\n";

#[test]
fn four_layers_show_the_winner() {
    let tmp = site(&[
        (
            "reuben.toml",
            "title = \"T\"\ninclude = [\"config/*.toml\"]\n",
        ),
        ("config/a.toml", "[params]\na = 1\n"),
        ("reuben.production.toml", "# env\n[params]\nb = 2\n"),
        ("content/_dir.toml", "section = \"x\"\n"),
        ("content/blog/_dir.toml", "# folder\n\nsection = \"blog\"\n"),
        ("content/blog/post.md", POST),
    ]);
    let site = Site::load_with(
        tmp.path(),
        &LoadOptions {
            env: Some("production".into()),
        },
    )
    .unwrap();
    let ex = site.explain("content/blog/post.md").unwrap();
    let at = |file: &str, line| (tmp.path().join(file), Some(line));
    let file_line = |s: &Source| match s {
        Source::FrontMatter { file, line }
        | Source::DirDefaults { file, line }
        | Source::Config { file, line } => (file.clone(), *line),
        other => panic!("{other:?}"),
    };

    let a = entry(&ex, "site.params.a");
    assert!(matches!(a.source, Source::Config { .. }));
    assert_eq!(file_line(&a.source), at("config/a.toml", 2));
    let b = entry(&ex, "site.params.b");
    assert!(matches!(b.source, Source::Config { .. }));
    assert_eq!(file_line(&b.source), at("reuben.production.toml", 3));
    let section = entry(&ex, "page.section");
    assert!(matches!(section.source, Source::DirDefaults { .. }));
    assert_eq!(section.value, "\"blog\"");
    assert_eq!(file_line(&section.source), at("content/blog/_dir.toml", 3));
    let title = entry(&ex, "page.title");
    assert!(matches!(title.source, Source::FrontMatter { .. }));
    assert_eq!(file_line(&title.source), at("content/blog/post.md", 2));
    assert_eq!(entry(&ex, "site.title").value, "\"T\"");
}

#[test]
fn names_come_in_the_documented_order() {
    let tmp = site(&[
        ("reuben.toml", "title = \"T\"\n"),
        ("data/info.toml", "name = \"Rye\"\n"),
        ("includes/card.rhai", "fn card(p) {\n    p\n}\n"),
        ("content/blog/post.md", POST),
        ("content/blog/other.md", "---\ntitle: Other\n---\n"),
    ]);
    let ex = explain(&tmp, "blog/post.md");
    let names: Vec<&str> = ex.entries.iter().map(|e| e.name.as_str()).collect();
    let position = |n: &str| names.iter().position(|x| *x == n).unwrap();
    assert!(position("page.title") < position("page.url"));
    assert!(position("page.summary") < position("site.title"));
    assert!(position("site.base_url") < position("data.info"));
    assert!(position("data.info") < position("collections.blog"));
    assert!(position("collections.blog") < position("card/1"));
    assert_eq!(names.last(), Some(&"helpers"));
    assert_eq!(entry(&ex, "collections.blog").value, "2 pages");
    assert_eq!(
        entry(&ex, "collections.blog").source,
        Source::Collection {
            name: "blog".into()
        }
    );
    assert_eq!(entry(&ex, "page.url").source, Source::Computed);
    assert_eq!(entry(&ex, "helpers").source, Source::Helper);
    assert_eq!(
        entry(&ex, "data.info").source,
        Source::Data {
            file: tmp.path().join("data/info.toml"),
            line: None
        }
    );
}

#[test]
fn sidecar_and_include_origins() {
    let tmp = site(&[
        ("content/blog/post.md", POST),
        (
            "content/blog/post.rhai",
            "// sidecar\nlet a = 1;\nlet posts = [1,2,3];\n",
        ),
        ("includes/card.rhai", "fn card(p) {\n    p\n}\n"),
    ]);
    let ex = explain(&tmp, "content/blog/post.md");
    let posts = entry(&ex, "posts");
    assert_eq!(posts.value, "[1, 2, 3]");
    assert_eq!(
        posts.source,
        Source::Sidecar {
            file: tmp.path().join("content/blog/post.rhai"),
            line: Some(3)
        }
    );
    assert_eq!(
        entry(&ex, "card/1").source,
        Source::Include {
            file: tmp.path().join("includes/card.rhai"),
            line: Some(1)
        }
    );
}

#[test]
fn lookup_by_path_or_url() {
    let tmp = site(&[("content/blog/post.md", POST)]);
    let site = Site::load(tmp.path()).unwrap();
    let pages: Vec<_> = ["content/blog/post.md", "blog/post.md", "/blog/post/"]
        .into_iter()
        .map(|q| site.explain(q).unwrap().page)
        .collect();
    assert!(pages.iter().all(|p| *p == pages[0]), "{pages:?}");
    let errors = site.explain("/nope/").unwrap_err();
    assert!(matches!(&errors[0].kind, ErrorKind::PageNotFound(q) if q == "/nope/"));
}

#[test]
fn drafts_can_be_explained() {
    let tmp = site(&[(
        "content/wip.md",
        "---\ntitle: WIP\ndraft: true\n---\nSoon.\n",
    )]);
    assert_eq!(explain(&tmp, "/wip/").url, "/wip/");
}

#[test]
fn long_values_are_cut() {
    let big: Vec<String> = (0..500).map(|i| i.to_string()).collect();
    let tmp = site(&[(
        "content/p.md",
        &format!(
            "---\ntitle: P\nlong: {}\nbig: [{}]\n---\n",
            "x".repeat(200),
            big.join(", ")
        ),
    )]);
    let ex = explain(&tmp, "p.md");
    let long = &entry(&ex, "page.long").value;
    assert_eq!(long.chars().count(), 60);
    assert!(long.ends_with('…'));
    assert_eq!(entry(&ex, "page.big").value, "[500 items]");
}

#[test]
fn sidecar_error_is_reported() {
    let tmp = site(&[
        ("content/a.md", "---\ntitle: A\n---\nA\n"),
        ("content/a.rhai", "let x = 1;\nthrow \"boom\";\n"),
        ("content/b.md", "---\ntitle: B\n---\n{{ nope( }}\n"),
    ]);
    let ex = explain(&tmp, "a.md");
    assert_eq!(ex.errors.len(), 1, "{:?}", ex.errors);
    assert!(ex.errors[0].path.ends_with("a.rhai"));
    assert!(
        !ex.entries
            .iter()
            .any(|e| matches!(e.source, Source::Sidecar { .. })),
        "{:?}",
        ex.entries
    );
    assert!(ex.entries.iter().any(|e| e.name == "page.title"));
}

#[test]
fn broken_dir_defaults_and_data_are_the_pages_errors() {
    let tmp = site(&[
        ("content/blog/_dir.toml", "section = \n"),
        ("content/blog/post.md", POST),
        ("data/bad.json", "{"),
        ("content/other/_dir.toml", "also = \n"),
    ]);
    let ex = explain(&tmp, "blog/post.md");
    let paths: Vec<_> = ex.errors.iter().map(|e| e.path.clone()).collect();
    assert_eq!(
        paths,
        [
            tmp.path().join("content/blog/_dir.toml"),
            tmp.path().join("data/bad.json")
        ]
    );
}
