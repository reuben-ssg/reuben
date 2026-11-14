use assert_cmd::Command;
use assert_fs::TempDir;
use assert_fs::prelude::*;

fn reuben() -> Command {
    Command::cargo_bin("reuben").unwrap()
}

fn site(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (path, text) in files {
        tmp.child(path).write_str(text).unwrap();
    }
    tmp
}

fn check(tmp: &TempDir) -> assert_cmd::assert::Assert {
    reuben().args(["check", "--root"]).arg(tmp.path()).assert()
}

fn last_line(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .last()
        .unwrap_or("")
        .to_string()
}

#[test]
fn check_passes_a_clean_site() {
    let tmp = site(&[
        (
            "content/index.md",
            "---\ntitle: Home\n---\n# Home\n\nSee [the about page](/about/).\n",
        ),
        ("content/about.md", "---\ntitle: About\n---\nAbout.\n"),
    ]);
    let out = check(&tmp).success().get_output().clone();
    assert_eq!(last_line(&out.stderr), "No problems found in 2 pages");
    assert!(!tmp.path().join("dist").exists());
}

#[test]
fn check_fails_on_a_finding() {
    let tmp = site(&[(
        "content/a.md",
        "---\ntitle: A\n---\n# A\n\n[click here](/a/)\n",
    )]);
    let out = check(&tmp)
        .code(1)
        .stderr(predicates::str::contains("warning[link-text]"))
        .stderr(predicates::str::contains("--> "))
        .stderr(predicates::str::contains("content/a.md:6:"))
        .get_output()
        .clone();
    assert_eq!(
        last_line(&out.stderr),
        "Found 1 accessibility finding in 1 file"
    );
    assert!(!tmp.path().join("dist").exists());
}

#[test]
fn check_counts_errors_and_findings() {
    let tmp = site(&[
        (
            "content/a.md",
            "---\ntitle: A\n---\n{{ nope( }}\n\n{{ also( }}\n",
        ),
        (
            "content/b.md",
            "---\ntitle: B\n---\n# B\n\n[click here](/b/)\n",
        ),
    ]);
    let out = check(&tmp).code(1).get_output().clone();
    assert_eq!(
        last_line(&out.stderr),
        "Found 2 errors and 1 accessibility finding in 2 files"
    );
    assert!(!tmp.path().join("dist").exists());
}

#[test]
fn include_note_is_printed() {
    let tmp = site(&[
        (
            "includes/card.rhai",
            "// a card\nfn card() { raw(\"<a href='/a/'>read more</a>\") }\n",
        ),
        ("content/a.md", "---\ntitle: A\n---\n# A\n\n{{ card() }}\n"),
    ]);
    check(&tmp).code(1).stderr(predicates::str::contains(
        "= note: from includes/card.rhai:2",
    ));
}

#[test]
fn diagnostics_name_files_relative_to_the_root() {
    let tmp = site(&[
        (
            "includes/card.rhai",
            "// a card\nfn card() { raw(\"<a href='/a/'>read more</a>\") }\n",
        ),
        (
            "includes/fail.rhai",
            "fn fail() {\n    throw \"boom\";\n}\n",
        ),
        ("content/a.md", "---\ntitle: A\n---\n# A\n\n{{ card() }}\n"),
        (
            "content/b.md",
            "---\ntitle: B\n---\n# One\n\n<h1>Two</h1>\n",
        ),
        ("content/c.md", "---\ntitle: C\n---\n# C\n\n{{ fail() }}\n"),
        ("content/d.md", "---\ndraft: false\n---\n# D\n"),
    ]);
    assert!(tmp.path().is_absolute());
    let out = check(&tmp).code(1).get_output().clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    for expected in [
        "--> content/a.md:6:1",
        "= note: from includes/card.rhai:2",
        "--> content/b.md (built as b/index.html, line ",
        "--> content/c.md:6:",
        "= in include `fail` (includes/fail.rhai:2:",
        "error: content/d.md: front matter: `title` is required",
    ] {
        assert!(stderr.contains(expected), "{expected:?} in:\n{stderr}");
    }
    let root = tmp.path().display().to_string();
    assert!(!stderr.contains(&root), "{root} in:\n{stderr}");
}

#[test]
fn built_location_for_html_findings() {
    let tmp = site(&[(
        "content/a.md",
        "---\ntitle: A\n---\n# One\n\n<h1>Two</h1>\n",
    )]);
    check(&tmp)
        .code(1)
        .stderr(predicates::str::contains("(built as a/index.html, line "));
}
