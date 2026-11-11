use std::fs;
use std::path::Path;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::site::{BuildOptions, Site};

#[test]
fn output_is_unchanged() {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sites/provenance");
    tmp.copy_from(src, &["**"]).unwrap();
    let root = tmp.path();
    let site = Site::load(root).unwrap();
    site.build(&BuildOptions {
        drafts: false,
        out: root.join("dist"),
        jobs: None,
    })
    .unwrap();
    for file in ["index.html", "about/index.html", "feed.xml"] {
        let html = fs::read_to_string(root.join("dist").join(file)).unwrap();
        insta::assert_snapshot!(file.replace('/', "__"), html);
    }
}
