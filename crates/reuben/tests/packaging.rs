use assert_cmd::Command;
use std::fs;
use std::path::Path;

#[test]
fn licence_copies_match_root() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for krate in ["reuben", "reuben-core", "reuben-a11y"] {
        for name in ["LICENSE-MIT", "LICENSE-APACHE"] {
            let copy = root.join("crates").join(krate).join(name);
            let copy_meta = fs::symlink_metadata(&copy)
                .unwrap_or_else(|e| panic!("{} is missing: {e}", copy.display()));
            assert!(
                !copy_meta.file_type().is_symlink(),
                "{} must be a real file, not a symlink",
                copy.display()
            );
            let expected = fs::read(root.join(name)).unwrap();
            let actual = fs::read(&copy).unwrap();
            assert!(
                actual == expected,
                "{} differs from the repo-root {name}",
                copy.display()
            );
        }
    }
}

#[test]
fn version_is_0_1_0() {
    Command::cargo_bin("reuben")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout("reuben 0.1.0\n");
}
