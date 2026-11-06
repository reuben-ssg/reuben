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
