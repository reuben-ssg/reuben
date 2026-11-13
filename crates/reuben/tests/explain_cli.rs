use assert_cmd::Command;
use assert_fs::TempDir;
use assert_fs::prelude::*;
use predicates::Predicate;

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

fn stdout_of(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

#[test]
fn explain_prints_aligned_rows() {
    let tmp = site(&[
        ("reuben.toml", "title = \"T\"\n"),
        ("content/blog/_dir.toml", "# folder\n\nsection = \"blog\"\n"),
        ("content/blog/post.md", "---\ntitle: Hello\n---\nBody.\n"),
    ]);
    let out = stdout_of(
        reuben()
            .args(["explain", "blog/post.md", "--root"])
            .arg(tmp.path())
            .assert()
            .success(),
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "content/blog/post.md (/blog/post/)");
    assert_eq!(lines[1], "");
    let title =
        predicates::str::is_match(r#"^page\.title\s+"Hello"\s+content/blog/post\.md:2$"#).unwrap();
    assert!(lines.iter().any(|l| title.eval(l)), "{out}");
    let section =
        predicates::str::is_match(r#"^page\.section\s+"blog"\s+content/blog/_dir\.toml:3$"#)
            .unwrap();
    assert!(lines.iter().any(|l| section.eval(l)), "{out}");
    assert!(lines.iter().any(|l| l.ends_with("  computed")), "{out}");
    assert!(
        lines.iter().any(|l| l.ends_with("  collection blog")),
        "{out}"
    );
    assert!(lines.iter().any(|l| l.ends_with("  built-in")), "{out}");
    // Rows line up: the value column starts two spaces after the longest name in every row.
    let rows = &lines[2..];
    let width = rows
        .iter()
        .map(|r| r.split_whitespace().next().unwrap().chars().count())
        .max()
        .unwrap();
    for row in rows {
        let chars: Vec<char> = row.chars().collect();
        assert!(
            chars[width] == ' ' && chars[width + 1] == ' ' && chars[width + 2] != ' ',
            "{row:?}"
        );
    }
}

#[test]
fn explain_reports_errors_and_exits_1() {
    let tmp = site(&[
        ("content/a.md", "---\ntitle: A\n---\nA\n"),
        ("content/a.rhai", "throw \"boom\";\n"),
    ]);
    reuben()
        .args(["explain", "a.md", "--root"])
        .arg(tmp.path())
        .assert()
        .code(1)
        .stdout(predicates::str::contains("page.title"))
        .stderr(predicates::str::contains("boom"));
}

#[test]
fn explain_unknown_page_fails() {
    let tmp = site(&[("content/a.md", "---\ntitle: A\n---\nA\n")]);
    reuben()
        .args(["explain", "/nope/", "--root"])
        .arg(tmp.path())
        .assert()
        .code(1)
        .stderr(predicates::str::contains("/nope/"));
}
