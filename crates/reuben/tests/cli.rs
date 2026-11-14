use assert_cmd::Command;

fn reuben() -> Command {
    Command::cargo_bin("reuben").unwrap()
}

#[test]
fn prints_version() {
    reuben()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::starts_with("reuben "));
}

fn fixture(name: &str) -> assert_fs::TempDir {
    use assert_fs::prelude::*;
    let tmp = assert_fs::TempDir::new().unwrap();
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../reuben-core/tests/fixtures/sites")
        .join(name);
    tmp.copy_from(src, &["**"]).unwrap();
    tmp
}

#[test]
fn build_succeeds_on_fixture() {
    let tmp = fixture("basic");
    reuben()
        .args(["build", "--root"])
        .arg(tmp.path())
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Built 3 pages, copied 2 static files (1 draft skipped)",
        ));
    assert!(tmp.path().join("dist/about/index.html").is_file());
}

#[test]
fn build_failure_exits_1_and_names_file() {
    let tmp = fixture("bad");
    reuben()
        .args(["build", "--root"])
        .arg(tmp.path())
        .assert()
        .code(1)
        .stderr(predicates::str::contains("error:"))
        .stderr(predicates::str::contains("no-title.md"))
        .stderr(predicates::str::contains("title"));
}

#[test]
fn script_error_shows_snippet() {
    let tmp = fixture("scripted");
    reuben()
        .args(["build", "--root"])
        .arg(tmp.path())
        .assert()
        .code(1)
        .stderr(predicates::str::contains("broken.md:6:"))
        .stderr(predicates::str::contains("{{ nope( }}"))
        .stderr(predicates::str::contains("^"));
}

#[test]
fn build_accepts_env_flag() {
    let tmp = fixture("basic");
    reuben()
        .args(["build", "--env", "production", "--root"])
        .arg(tmp.path())
        .assert()
        .success();
}

#[test]
fn warnings_print_to_stderr() {
    let tmp = fixture("basic");
    reuben()
        .args(["build", "--root"])
        .arg(tmp.path())
        .assert()
        .success()
        .stderr(predicates::str::contains("warning: sitemap.xml skipped"));
}

#[test]
fn css_highlight_prints_css() {
    reuben()
        .args(["css", "highlight"])
        .assert()
        .success()
        .stdout(predicates::str::contains(".hl-"));
}

#[test]
fn css_highlight_bad_theme_fails() {
    reuben()
        .args(["css", "highlight", "--theme", "nope"])
        .assert()
        .code(1)
        .stderr(predicates::str::contains("nope"));
}

#[test]
fn build_accepts_jobs_flag() {
    let tmp = fixture("basic");
    reuben()
        .args(["build", "--jobs", "2", "--root"])
        .arg(tmp.path())
        .assert()
        .success();
    assert!(tmp.path().join("dist/about/index.html").is_file());
}

fn broken_site(files: &[(&str, &str)]) -> assert_fs::TempDir {
    use assert_fs::prelude::*;
    let tmp = assert_fs::TempDir::new().unwrap();
    for (path, text) in files {
        tmp.child(path).write_str(text).unwrap();
    }
    tmp
}

#[test]
fn summary_line_counts_errors_and_files() {
    let tmp = broken_site(&[
        ("content/a.md", "---\n---\nx\n"),
        (
            "content/b.md",
            "---\ntitle: B\n---\n{{ nope( }}\n\n{{ nope2( }}\n",
        ),
    ]);
    reuben()
        .args(["build", "--root"])
        .arg(tmp.path())
        .assert()
        .code(1)
        .stderr(predicates::str::ends_with("Found 3 errors in 2 files\n"));

    let tmp = broken_site(&[("content/a.md", "---\n---\nx\n")]);
    reuben()
        .args(["build", "--root"])
        .arg(tmp.path())
        .assert()
        .code(1)
        .stderr(predicates::str::ends_with("Found 1 error in 1 file\n"));
}

#[test]
fn a_css_error_found_directly_and_through_an_import_is_one_error() {
    let tmp = broken_site(&[
        ("content/index.md", "---\ntitle: H\n---\nhi\n"),
        ("static/a.css", "a {\n}}} x {{\n"),
        ("static/b.css", "@import \"a.css\";\nb{color:red}\n"),
    ]);
    // The default root `.` is where a walked path and a resolved import could be spelled differently.
    reuben()
        .current_dir(tmp.path())
        .arg("build")
        .assert()
        .code(1)
        .stderr(predicates::str::contains("--> static/a.css:2:2"))
        .stderr(predicates::str::ends_with("Found 1 error in 1 file\n"));
}

#[test]
fn build_warns_but_succeeds() {
    use assert_fs::prelude::*;
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("content/a.md")
        .write_str("---\ntitle: A\n---\n# A\n\n[click here](/a/)\n")
        .unwrap();
    reuben()
        .args(["build", "--root"])
        .arg(tmp.path())
        .assert()
        .success()
        .stderr(predicates::str::contains("warning[link-text]"))
        .stderr(predicates::str::contains(
            "1 accessibility warning in 1 file",
        ));
    assert!(tmp.path().join("dist/a/index.html").is_file());
}

#[test]
fn failed_build_still_ends_with_error_summary() {
    use assert_fs::prelude::*;
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("content/a.md")
        .write_str("---\ntitle: A\n---\n# A\n\n[click here](/a/)\n")
        .unwrap();
    tmp.child("content/b.md")
        .write_str("---\ntitle: B\n---\n{{ nope( }}\n")
        .unwrap();
    let out = reuben()
        .args(["build", "--root"])
        .arg(tmp.path())
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines[lines.len() - 1], "Found 1 error in 1 file");
    assert_eq!(lines[lines.len() - 2], "1 accessibility warning in 1 file");
}
