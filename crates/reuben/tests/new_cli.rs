use assert_cmd::Command;
use assert_fs::TempDir;
use assert_fs::prelude::*;
use predicates::prelude::*;

fn reuben(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("reuben").unwrap();
    cmd.current_dir(dir.path());
    cmd
}

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok()
}

fn builds_and_checks_clean(args: &[&str]) {
    let tmp = TempDir::new().unwrap();
    reuben(&tmp).args(args).assert().success();
    reuben(&tmp)
        .args(["build", "--root", "site"])
        .assert()
        .success();
    reuben(&tmp)
        .args(["check", "--root", "site"])
        .assert()
        .success()
        .stderr(predicate::str::contains("No problems found"));
}

#[test]
fn new_creates_a_site_that_builds_and_checks() {
    builds_and_checks_clean(&["new", "site"]);
}

#[test]
fn new_with_npm_creates_a_site_that_builds_and_checks() {
    builds_and_checks_clean(&["new", "site", "--npm"]);
}

#[test]
fn new_initialises_git() {
    if !git_available() {
        eprintln!("skipping: git isn't installed");
        return;
    }
    let tmp = TempDir::new().unwrap();
    reuben(&tmp).args(["new", "site"]).assert().success();
    assert!(tmp.path().join("site/.git").exists());
}

#[test]
fn new_without_git_still_succeeds() {
    let tmp = TempDir::new().unwrap();
    let empty_path = TempDir::new().unwrap();
    let bin = assert_cmd::cargo::cargo_bin("reuben");
    Command::new(bin)
        .current_dir(tmp.path())
        .env("PATH", empty_path.path())
        .args(["new", "site"])
        .assert()
        .success()
        .stdout(predicate::str::contains("git isn't installed"));
    assert!(tmp.path().join("site/reuben.toml").is_file());
    assert!(tmp.path().join("site/content/index.md").is_file());
    assert!(!tmp.path().join("site/.git").exists());
}

#[test]
fn new_refuses_a_non_empty_folder() {
    let tmp = TempDir::new().unwrap();
    tmp.child("site/keep.txt").write_str("keep").unwrap();
    reuben(&tmp)
        .args(["new", "site"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("already exists and isn't empty"));
    let names: Vec<_> = std::fs::read_dir(tmp.path().join("site"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["keep.txt"]);
}

#[test]
fn new_prints_next_steps() {
    let tmp = TempDir::new().unwrap();
    reuben(&tmp)
        .args(["new", "site"])
        .assert()
        .success()
        .stdout(predicate::str::contains("cd site"))
        .stdout(predicate::str::contains("reuben serve"))
        .stdout(predicate::str::contains("npm install").not());

    let tmp = TempDir::new().unwrap();
    reuben(&tmp)
        .args(["new", "site", "--npm"])
        .assert()
        .success()
        .stdout(predicate::str::contains("cd site"))
        .stdout(predicate::str::contains("npm install"))
        .stdout(predicate::str::contains("npm run dev"));
}

#[test]
fn help_lists_new() {
    let tmp = TempDir::new().unwrap();
    reuben(&tmp)
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::is_match(r"(?m)^\s+new\s").unwrap());
}
