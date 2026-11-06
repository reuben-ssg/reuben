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
