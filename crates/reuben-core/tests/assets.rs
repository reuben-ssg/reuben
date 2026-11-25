//! The `asset()` helper and content-hashed static file names in a built site.

use std::path::Path;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_core::error::ErrorKind;
use reuben_core::output::OutputFile;
use reuben_core::site::{BuildOptions, Failure, OUTPUT_MARKER, RenderOptions, Site};

fn site_of(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (path, text) in files {
        tmp.child(path).write_str(text).unwrap();
    }
    tmp
}

fn build(tmp: &TempDir) -> Result<(), Failure> {
    Site::load(tmp.path())?
        .build(&BuildOptions {
            drafts: false,
            out: tmp.path().join("dist"),
            jobs: None,
        })
        .map(|_| ())
}

/// Every built file, relative to `dist/` with `/` separators, sorted.
fn dist_files(tmp: &TempDir) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                let rel = path.strip_prefix(base).unwrap();
                if rel != Path::new(OUTPUT_MARKER) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    let base = tmp.path().join("dist");
    let mut out = Vec::new();
    walk(&base, &base, &mut out);
    out.sort();
    out
}

fn dist(tmp: &TempDir, rel: &str) -> String {
    std::fs::read_to_string(tmp.path().join("dist").join(rel)).unwrap()
}

/// The one built file named `<stem>.<8 hex>.<ext>` for the source path `rel`.
fn hashed(tmp: &TempDir, rel: &str) -> String {
    let (stem, ext) = rel.rsplit_once('.').unwrap();
    let found: Vec<String> = dist_files(tmp)
        .into_iter()
        .filter(|f| {
            f.strip_prefix(&format!("{stem}."))
                .and_then(|r| r.strip_suffix(&format!(".{ext}")))
                .is_some_and(|h| h.len() == 8 && h.chars().all(|c| c.is_ascii_hexdigit()))
        })
        .collect();
    assert_eq!(found.len(), 1, "{rel} in {:?}", dist_files(tmp));
    found.into_iter().next().unwrap()
}

const HASH_ON: (&str, &str) = ("reuben.toml", "[assets]\nhash = true\n");
const INDEX: (&str, &str) = ("content/index.md", "---\ntitle: H\n---\nhi");
const LAYOUT: (&str, &str) = (
    "layouts/default.html",
    "<link rel=\"stylesheet\" href=\"{{ asset(\"css/site.css\") }}\">{{ content }}",
);
const SITE_CSS: (&str, &str) = (
    "static/css/site.css",
    "@font-face { font-family: F; src: url(\"../fonts/f.woff2\"); }\nbody { font-family: F; }\n",
);

#[test]
fn asset_without_hashing_returns_plain_paths() {
    let tmp = site_of(&[
        ("reuben.toml", "[assets]\nhash = false\n"),
        ("content/index.md", "---\ntitle: H\n---\nhi"),
        ("static/css/site.css", "body { color: red; }\n"),
        (
            "layouts/default.html",
            "<link rel=\"stylesheet\" href=\"{{ asset(\"css/site.css\") }}\">",
        ),
    ]);
    build(&tmp).unwrap();
    let html = std::fs::read_to_string(tmp.path().join("dist/index.html")).unwrap();
    assert!(html.contains("href=\"/css/site.css\""), "{html}");
}

#[test]
fn asset_misses_on_two_pages_are_all_reported() {
    let tmp = site_of(&[
        (
            "content/a.md",
            "---\ntitle: A\n---\n{{ asset(\"nope.css\") }}",
        ),
        (
            "content/b.md",
            "---\ntitle: B\n---\n{{ asset(\"nope.css\") }}",
        ),
    ]);
    let Err(failure) = build(&tmp) else {
        panic!("build should fail");
    };
    let errors = &failure.errors;
    let misses: Vec<_> = errors
        .iter()
        .filter(|e| matches!(&e.kind, ErrorKind::Asset { spec } if spec == "nope.css"))
        .collect();
    assert_eq!(misses.len(), 2, "{errors:?}");
}

#[test]
fn hashed_names_are_stable() {
    let tmp = site_of(&[
        HASH_ON,
        INDEX,
        LAYOUT,
        SITE_CSS,
        ("static/fonts/f.woff2", "font bytes"),
        ("static/img/a.png", "png bytes"),
    ]);
    build(&tmp).unwrap();
    let first = dist_files(&tmp);
    build(&tmp).unwrap();
    assert_eq!(first, dist_files(&tmp));
    assert!(!first.contains(&"css/site.css".to_string()), "{first:?}");
    assert!(!first.contains(&"img/a.png".to_string()), "{first:?}");
    hashed(&tmp, "css/site.css");
    hashed(&tmp, "fonts/f.woff2");
    hashed(&tmp, "img/a.png");
}

#[test]
fn font_change_renames_the_stylesheet() {
    let tmp = site_of(&[
        HASH_ON,
        INDEX,
        LAYOUT,
        SITE_CSS,
        ("static/fonts/f.woff2", "font one"),
    ]);
    build(&tmp).unwrap();
    let (css1, font1) = (hashed(&tmp, "css/site.css"), hashed(&tmp, "fonts/f.woff2"));
    assert!(
        dist(&tmp, &css1).contains(&format!("/{font1}")),
        "{}",
        dist(&tmp, &css1)
    );

    tmp.child("static/fonts/f.woff2")
        .write_str("font two")
        .unwrap();
    build(&tmp).unwrap();
    let (css2, font2) = (hashed(&tmp, "css/site.css"), hashed(&tmp, "fonts/f.woff2"));
    assert_ne!(css1, css2);
    assert_ne!(font1, font2);
    assert!(
        dist(&tmp, &css2).contains(&format!("/{font2}")),
        "{}",
        dist(&tmp, &css2)
    );
    let html = dist(&tmp, "index.html");
    assert!(html.contains(&format!("href=\"/{css2}\"")), "{html}");
}

#[test]
fn page_links_point_at_hashed_files() {
    let tmp = site_of(&[
        HASH_ON,
        INDEX,
        LAYOUT,
        SITE_CSS,
        ("static/fonts/f.woff2", "font"),
    ]);
    build(&tmp).unwrap();
    let css = hashed(&tmp, "css/site.css");
    let html = dist(&tmp, "index.html");
    assert!(html.contains(&format!("href=\"/{css}\"")), "{html}");
}

#[test]
fn root_well_known_keep_and_partials_untouched() {
    let tmp = site_of(&[
        (
            "reuben.toml",
            "[assets]\nhash = true\nkeep = [\"img/keep-*\"]\n",
        ),
        INDEX,
        ("static/robots.txt", "User-agent: *\n"),
        ("static/.well-known/x.txt", "x"),
        ("static/img/keep-1.png", "kept"),
        ("static/img/b.png", "hashed"),
        ("static/css/_part.css", "a { color: red; }\n"),
        ("static/css/site.css", "@import \"_part.css\";\n"),
    ]);
    build(&tmp).unwrap();
    let files = dist_files(&tmp);
    for name in ["robots.txt", ".well-known/x.txt", "img/keep-1.png"] {
        assert!(files.contains(&name.to_string()), "{name} in {files:?}");
    }
    assert!(!files.iter().any(|f| f.contains("_part")), "{files:?}");
    hashed(&tmp, "img/b.png");
    assert!(dist(&tmp, &hashed(&tmp, "css/site.css")).contains("red"));
}

#[test]
fn root_stylesheet_keeps_its_name_but_urls_are_rewritten() {
    let tmp = site_of(&[
        HASH_ON,
        INDEX,
        ("static/site.css", "a { background: url(\"img/a.png\"); }\n"),
        ("static/img/a.png", "png"),
    ]);
    build(&tmp).unwrap();
    let png = hashed(&tmp, "img/a.png");
    let css = dist(&tmp, "site.css");
    assert!(css.contains(&format!("/{png}")), "{css}");
}

#[test]
fn template_stylesheet_is_hashed() {
    let tmp = site_of(&[
        HASH_ON,
        INDEX,
        (
            "layouts/default.html",
            "<link rel=\"stylesheet\" href=\"{{ asset(\"css/theme.css\") }}\">{{ content }}",
        ),
        ("static/img/logo.png", "logo"),
        (
            "content/css/theme.css",
            "---\n---\nb {\n  width: {{ 1 + 1 }}px;\n  --logo: \"{{ asset(\"img/logo.png\") }}\";\n}\n",
        ),
    ]);
    build(&tmp).unwrap();
    let files = dist_files(&tmp);
    assert!(!files.contains(&"css/theme.css".to_string()), "{files:?}");
    let css = hashed(&tmp, "css/theme.css");
    let logo = hashed(&tmp, "img/logo.png");
    let text = dist(&tmp, &css);
    assert!(
        text.contains("2px") && text.contains(&format!("/{logo}")),
        "{text}"
    );
    let html = dist(&tmp, "index.html");
    assert!(html.contains(&format!("href=\"/{css}\"")), "{html}");
}

#[test]
fn asset_on_a_stylesheet_from_a_stylesheet_is_a_miss() {
    let tmp = site_of(&[
        HASH_ON,
        INDEX,
        ("static/css/site.css", "a { color: red; }\n"),
        (
            "content/css/theme.css",
            "---\n---\nb { --x: \"{{ asset(\"css/site.css\") }}\"; }\n",
        ),
    ]);
    let Err(failure) = build(&tmp) else {
        panic!("build should fail");
    };
    let errors = &failure.errors;
    assert!(
        errors
            .iter()
            .any(|e| matches!(&e.kind, ErrorKind::Asset { spec } if spec == "css/site.css")),
        "{errors:?}"
    );
}

#[test]
fn site_file_overrides_theme_file_before_hashing() {
    let tmp = site_of(&[
        ("reuben.toml", "theme = \"t\"\n[assets]\nhash = true\n"),
        INDEX,
        ("t/static/css/site.css", "a { width: 1px; }\n"),
        ("static/css/site.css", "a { width: 2px; }\n"),
        ("t/static/img/a.png", "theme png"),
        ("static/img/a.png", "site png"),
    ]);
    build(&tmp).unwrap();
    let files = dist_files(&tmp);
    assert_eq!(files.iter().filter(|f| f.starts_with("css/")).count(), 1);
    assert_eq!(files.iter().filter(|f| f.starts_with("img/")).count(), 1);
    assert!(dist(&tmp, &hashed(&tmp, "css/site.css")).contains("2px"));
    assert_eq!(dist(&tmp, &hashed(&tmp, "img/a.png")), "site png");
}

#[test]
fn base_path_prefixes_urls() {
    let tmp = site_of(&[
        (
            "reuben.toml",
            "base_url = \"https://x.test/docs/\"\n[assets]\nhash = true\n",
        ),
        INDEX,
        LAYOUT,
        SITE_CSS,
        ("static/fonts/f.woff2", "font"),
    ]);
    build(&tmp).unwrap();
    let css = hashed(&tmp, "css/site.css");
    let font = hashed(&tmp, "fonts/f.woff2");
    let html = dist(&tmp, "index.html");
    assert!(html.contains(&format!("href=\"/docs/{css}\"")), "{html}");
    let text = dist(&tmp, &css);
    assert!(text.contains(&format!("/docs/{font}")), "{text}");
}

#[test]
fn asset_and_url_misses_reported_in_one_build() {
    let tmp = site_of(&[
        HASH_ON,
        (
            "content/index.md",
            "---\ntitle: H\n---\n{{ asset(\"img/nope.png\") }}",
        ),
        (
            "static/css/site.css",
            "a { background: url(\"../img/missing.png\"); }\n",
        ),
    ]);
    let Err(failure) = build(&tmp) else {
        panic!("build should fail");
    };
    let errors = &failure.errors;
    assert!(
        errors
            .iter()
            .any(|e| matches!(&e.kind, ErrorKind::Asset { spec } if spec == "img/nope.png")),
        "{errors:?}"
    );
    assert!(
        errors.iter().any(|e| matches!(&e.kind,
            ErrorKind::Css { message, .. } if message.contains("../img/missing.png"))),
        "{errors:?}"
    );
}

#[test]
fn in_memory_render_hashes_too() {
    let tmp = site_of(&[
        HASH_ON,
        INDEX,
        LAYOUT,
        SITE_CSS,
        ("static/fonts/f.woff2", "font"),
    ]);
    let (output, _) = Site::load(tmp.path())
        .unwrap()
        .render(&RenderOptions {
            drafts: false,
            jobs: None,
        })
        .unwrap();
    let css = output
        .files
        .keys()
        .map(|k| k.to_string_lossy().replace('\\', "/"))
        .find(|k| k.starts_with("css/site.") && k != "css/site.css")
        .unwrap();
    let Some(OutputFile::Bytes(html)) = output.files.get(Path::new("index.html")) else {
        panic!("index.html should be in memory");
    };
    let html = String::from_utf8_lossy(html);
    assert!(html.contains(&format!("href=\"/{css}\"")), "{html}");
}

#[test]
fn content_assets_keep_their_names() {
    let tmp = site_of(&[HASH_ON, INDEX, ("content/img/pic.png", "pic")]);
    build(&tmp).unwrap();
    assert!(dist_files(&tmp).contains(&"img/pic.png".to_string()));
}

#[test]
fn hash_off_changes_no_names() {
    let tmp = site_of(&[
        INDEX,
        (
            "layouts/default.html",
            "<link href=\"{{ asset(\"css/theme.css\") }}\">{{ content }}",
        ),
        SITE_CSS,
        ("static/fonts/f.woff2", "font"),
        ("static/robots.txt", "User-agent: *\n"),
        ("static/img/a.png", "png"),
        ("content/css/theme.css", "---\n---\nb { width: 1px; }\n"),
    ]);
    build(&tmp).unwrap();
    assert_eq!(
        dist_files(&tmp),
        [
            "css/site.css",
            "css/theme.css",
            "fonts/f.woff2",
            "img/a.png",
            "index.html",
            "robots.txt",
        ]
    );
    assert!(dist(&tmp, "index.html").contains("href=\"/css/theme.css\""));
}
