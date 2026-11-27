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
        .success()
        .stderr(predicate::str::contains("warning").not());
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

#[test]
fn new_quotes_a_path_with_a_space_in_the_next_steps() {
    let tmp = TempDir::new().unwrap();
    reuben(&tmp)
        .args(["new", "My Site"])
        .assert()
        .success()
        .stdout(predicate::str::contains("cd 'My Site'"));
    assert!(tmp.path().join("My Site/reuben.toml").is_file());

    let tmp = TempDir::new().unwrap();
    reuben(&tmp)
        .args(["new", "it's"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r"cd 'it'\''s'"));
}

#[test]
fn new_in_the_current_folder_prints_no_cd() {
    let tmp = TempDir::new().unwrap();
    let here = tmp.child("here");
    here.create_dir_all().unwrap();
    Command::cargo_bin("reuben")
        .unwrap()
        .current_dir(here.path())
        .args(["new", "."])
        .assert()
        .success()
        .stdout(predicate::str::contains("reuben serve"))
        .stdout(predicate::str::contains("cd ").not());
    assert!(here.path().join("reuben.toml").is_file());
}

#[test]
fn new_git_init_ignores_git_dir_from_the_environment() {
    if !git_available() {
        eprintln!("skipping: git isn't installed");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let elsewhere = TempDir::new().unwrap();
    reuben(&tmp)
        .env("GIT_DIR", elsewhere.path())
        .args(["new", "site"])
        .assert()
        .success();
    assert!(tmp.path().join("site/.git").exists());
    assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn new_never_runs_a_git_from_a_relative_or_empty_path_entry() {
    use std::os::unix::fs::PermissionsExt;

    for (i, path) in [".", "bin", "", "bin:."].into_iter().enumerate() {
        let tmp = TempDir::new().unwrap();
        let script = "#!/bin/sh\ntouch \"$PWD/ran-git\"\n";
        for dir in [".", "bin"] {
            tmp.child(dir).create_dir_all().unwrap();
            let git = tmp.child(dir).child("git");
            git.write_str(script).unwrap();
            std::fs::set_permissions(git.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Command::cargo_bin("reuben")
            .unwrap()
            .current_dir(tmp.path())
            .env("PATH", path)
            .args(["new", "site"])
            .assert()
            .success()
            .stdout(predicate::str::contains("git isn't installed"));
        assert!(!tmp.path().join("ran-git").exists(), "case {i}: {path:?}");
        assert!(!tmp.path().join("site/ran-git").exists(), "case {i}");
        assert!(!tmp.path().join("bin/ran-git").exists(), "case {i}");
    }
}

const IGNORE: &str = "dist/\n.dist.reuben-staging/\n.dist.reuben-old/\n.dist.reuben-output\n";

fn file(tmp: &TempDir, rel: &str) -> String {
    std::fs::read_to_string(tmp.path().join(rel)).unwrap()
}

#[test]
fn new_writes_exactly_these_files() {
    let tmp = TempDir::new().unwrap();
    reuben(&tmp).args(["new", "my site"]).assert().success();

    assert_eq!(file(&tmp, "my site/.gitignore"), IGNORE);
    assert_eq!(
        file(&tmp, "my site/content/index.md"),
        "---\n\
         title: Welcome\n\
         ---\n\
         \n\
         This is your new Reuben site.\n\
         Edit `content/index.md` and save to see the page change.\n\
         \n\
         The site is called {{ site.title }}.\n"
    );
    assert_eq!(
        file(&tmp, "my site/reuben.toml"),
        "title = \"my site\"\n\
         \n\
         # Change this to your site's real address before deploying.\n\
         base_url = \"https://example.com/\"\n\
         \n\
         # Write a sitemap.xml listing every page.\n\
         # sitemap = true\n\
         \n\
         # Highlight fenced code blocks at build time; the colours come from a stylesheet made with `reuben css highlight`.\n\
         # [markdown]\n\
         # highlight = true\n\
         \n\
         # Add content hashes to static file names for long-lived caching.\n\
         # [assets]\n\
         # hash = true\n"
    );
    let mut names: Vec<_> = std::fs::read_dir(tmp.path().join("my site"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != ".git")
        .collect();
    names.sort();
    assert_eq!(names, [".gitignore", "content", "reuben.toml"]);
}

#[test]
fn new_npm_writes_exactly_these_files() {
    let tmp = TempDir::new().unwrap();
    reuben(&tmp)
        .args(["new", "my site", "--npm"])
        .assert()
        .success();

    assert_eq!(
        file(&tmp, "my site/.gitignore"),
        format!("{IGNORE}node_modules/\n")
    );
    assert_eq!(
        file(&tmp, "my site/package.json"),
        format!(
            "{{\n  \"name\": \"my-site\",\n  \"private\": true,\n  \"scripts\": {{\n    \"dev\": \"reuben serve\",\n    \"build\": \"reuben build\",\n    \"check\": \"reuben check\"\n  }},\n  \"devDependencies\": {{\n    \"@reuben-ssg/reuben\": \"{}\"\n  }}\n}}\n",
            env!("CARGO_PKG_VERSION")
        )
    );
}
