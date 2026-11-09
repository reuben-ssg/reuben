//! One build reports every independent error, and stops early only for a broken config.

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::Error;
use reuben_core::site::{BuildOptions, RenderOptions, Site};

fn site_of(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (path, text) in files {
        tmp.child(path).write_str(text).unwrap();
    }
    tmp
}

fn build(tmp: &TempDir) -> Result<(), Vec<Error>> {
    Site::load(tmp.path())?
        .build(&BuildOptions {
            drafts: false,
            out: tmp.path().join("dist"),
            jobs: None,
        })
        .map(|_| ())
}

#[test]
fn one_build_reports_every_independent_error() {
    let tmp = site_of(&[
        ("content/a.md", "---\n---\nno title\n"),
        ("content/b.md", "---\ntitle: B\n---\n{{ nope( }}\n"),
        ("content/c.md", "---\ntitle: C\n---\n\n$\\frac{1}{$\n"),
        ("content/d.md", "---\ntitle: D\n---\nd\n"),
        ("content/d/index.md", "---\ntitle: D2\n---\nd\n"),
        ("data/bad.toml", "x = [\n"),
        ("static/bad.css", "a {\n}}} x {{\n"),
        ("content/ok.md", "---\ntitle: Ok\n---\nfine\n"),
    ]);
    let errors = build(&tmp).unwrap_err();
    for file in [
        "content/a.md",
        "content/b.md",
        "content/c.md",
        "data/bad.toml",
        "static/bad.css",
    ] {
        assert!(
            errors.iter().any(|e| e.path.ends_with(file)),
            "no error for {file}: {errors:?}"
        );
    }
    assert!(
        errors
            .iter()
            .any(|e| e.path.ends_with("content/d.md") || e.path.ends_with("content/d/index.md")),
        "no collision error: {errors:?}"
    );
    assert!(
        !errors.iter().any(|e| e.path.ends_with("content/ok.md")),
        "{errors:?}"
    );
    assert!(!tmp.path().join("dist").exists());
}

#[test]
fn body_failure_suppresses_its_layout_error() {
    let tmp = site_of(&[
        (
            "content/a.md",
            "---\ntitle: A\nlayout: broken\n---\n{{ nope( }}\n",
        ),
        ("layouts/broken.html", "<main>{{ also_nope( }}</main>"),
    ]);
    let errors = build(&tmp).unwrap_err();
    assert!(
        errors.iter().all(|e| e.path.ends_with("content/a.md")),
        "{errors:?}"
    );
    assert!(!errors.is_empty());
}

#[test]
fn config_error_still_stops_early() {
    let tmp = site_of(&[
        ("reuben.toml", "titel = \"x\"\n"),
        ("content/a.md", "---\n---\nno title\n"),
    ]);
    let errors = Site::load(tmp.path()).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].path.ends_with("reuben.toml"));
}

#[test]
fn load_errors_fail_the_build_even_if_the_rest_renders() {
    let tmp = site_of(&[
        ("content/a.md", "---\n---\nno title\n"),
        ("content/ok.md", "---\ntitle: Ok\n---\nfine\n"),
    ]);
    let site = Site::load(tmp.path()).unwrap();
    assert_eq!(site.load_errors.len(), 1);
    assert!(site.pages.iter().all(|p| !p.source.ends_with("a.md")));
    assert_eq!(build(&tmp).unwrap_err().len(), 1);
}

#[test]
fn broken_dir_toml_is_reported_once_not_per_page() {
    let tmp = site_of(&[
        ("content/g/_dir.toml", "layout = [\n"),
        ("content/g/one.md", "---\n---\nx\n"),
        ("content/g/two.md", "---\n---\nx\n"),
    ]);
    let errors = build(&tmp).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].path.ends_with("g/_dir.toml"));
}

#[test]
fn errors_are_ordered_by_path_then_line_with_several_per_file() {
    let tmp = site_of(&[
        ("content/z.md", "---\n---\nx\n"),
        (
            "content/m.md",
            "---\ntitle: M\n---\n{{ nope2( }}\n\n{{ nope1( }}\n\n$\\frac{1}{$\n",
        ),
        ("content/b.md", "---\n---\nx\n"),
    ]);
    let errors = build(&tmp).unwrap_err();
    let keys: Vec<_> = errors
        .iter()
        .map(|e| (e.path.clone(), e.location().map_or(0, |l| l.line)))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    let in_m: Vec<_> = keys.iter().filter(|(p, _)| p.ends_with("m.md")).collect();
    assert!(in_m.len() >= 2, "{errors:?}");
}

#[test]
fn broken_dir_toml_still_reports_unreadable_and_unparsable_pages() {
    let tmp = site_of(&[
        ("content/g/_dir.toml", "layout = [\n"),
        ("content/g/yaml.md", "---\ntitle: [\n---\nx\n"),
        ("content/g/sub/open.md", "---\ntitle: T\nno closing\n"),
        ("content/g/notitle.md", "---\n---\nx\n"),
    ]);
    std::fs::write(tmp.path().join("content/g/bin.md"), [0xff, 0xfe, 0x00]).unwrap();
    let errors = build(&tmp).unwrap_err();
    for file in ["g/_dir.toml", "g/yaml.md", "g/sub/open.md", "g/bin.md"] {
        assert!(
            errors.iter().any(|e| e.path.ends_with(file)),
            "no error for {file}: {errors:?}"
        );
    }
    assert!(
        !errors.iter().any(|e| e.path.ends_with("g/notitle.md")),
        "{errors:?}"
    );
}

#[test]
fn broken_dir_toml_hides_render_errors_that_only_follow_from_it() {
    let tmp = site_of(&[
        ("content/g/_dir.toml", "[params\nx = 1\n"),
        (
            "content/g/one.md",
            "---\ntitle: One\n---\n{{ page.params.sub.y }}\n",
        ),
        (
            "content/g/two.md",
            "---\ntitle: Two\n---\n{{ page.params.sub.y }}\n",
        ),
    ]);
    let errors = build(&tmp).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].path.ends_with("g/_dir.toml"));
}

#[test]
fn templated_css_partial_that_fails_to_load_adds_no_css_error() {
    let tmp = site_of(&[
        ("content/index.md", "---\ntitle: H\n---\nhi\n"),
        ("static/_vars.css", "---\ntitle: [\n---\n:root{--c:red}\n"),
        (
            "static/main.css",
            "@import \"_vars.css\";\na{color:var(--c)}\n",
        ),
    ]);
    let errors = build(&tmp).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].path.ends_with("static/_vars.css"));
}

#[test]
fn error_list_is_identical_whatever_the_thread_count() {
    let tmp = site_of(&[
        ("content/a.md", "---\n---\nno title\n"),
        ("content/b.md", "---\ntitle: B\n---\n{{ nope( }}\n"),
        ("content/c.md", "---\ntitle: C\n---\n\n$\\frac{1}{$\n"),
        ("content/d.md", "---\ntitle: D\n---\nd\n"),
        ("content/d/index.md", "---\ntitle: D2\n---\nd\n"),
        ("data/bad.toml", "x = [\n"),
        ("static/bad.css", "a {\n}}} x {{\n"),
    ]);
    let site = Site::load(tmp.path()).unwrap();
    let run = |jobs| {
        site.render(&RenderOptions {
            drafts: false,
            jobs: Some(jobs),
        })
        .map(|_| ())
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
    };
    let one = run(1);
    assert!(one.len() >= 5, "{one:?}");
    assert_eq!(one, run(8));
}

#[test]
fn broken_include_is_reported_with_every_other_error() {
    let tmp = site_of(&[
        ("includes/good.rhai", "fn helper() { \"h\" }\n"),
        ("includes/h.rhai", "fn x( {\n"),
        ("content/a.md", "---\n---\nno title\n"),
        ("content/b.md", "---\ntitle: B\n---\n{{ helper() }}\n"),
        ("content/c.md", "---\ntitle: C\nlayout: l\n---\nc\n"),
        ("layouts/l.html", "<main>{{ helper() }}</main>"),
        ("data/bad.toml", "x = [\n"),
        ("data/s.rhai", "helper()\n"),
        ("static/bad.css", "a {\n}}} x {{\n"),
        ("static/t.css", "---\n---\na{content:\"{{ helper() }}\"}\n"),
        (
            "static/main.css",
            "@import \"_p.css\";\na{color:var(--c)}\n",
        ),
        ("static/_p.css", "---\n---\n:root{--c:{{ helper() }}}\n"),
        ("content/d.md", "---\ntitle: D\n---\nd\n"),
        ("content/d/index.md", "---\ntitle: D2\n---\nd\n"),
    ]);
    let errors = build(&tmp).unwrap_err();
    for file in [
        "includes/h.rhai",
        "content/a.md",
        "data/bad.toml",
        "static/bad.css",
    ] {
        assert!(
            errors.iter().any(|e| e.path.ends_with(file)),
            "no error for {file}: {errors:?}"
        );
    }
    assert!(
        errors
            .iter()
            .any(|e| e.path.ends_with("content/d.md") || e.path.ends_with("content/d/index.md")),
        "no collision error: {errors:?}"
    );
    for file in [
        "content/b.md",
        "content/c.md",
        "layouts/l.html",
        "data/s.rhai",
        "static/t.css",
        "static/main.css",
        "static/_p.css",
    ] {
        assert!(
            !errors.iter().any(|e| e.path.ends_with(file)),
            "cascade error for {file}: {errors:?}"
        );
    }
    assert!(!tmp.path().join("dist").exists());
}
