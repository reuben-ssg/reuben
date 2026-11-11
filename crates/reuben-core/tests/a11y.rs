use std::path::Path;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use reuben_a11y::Rule;
use reuben_core::check::Finding;
use reuben_core::site::{BuildReport, Failure, RenderOptions, Site};
use reuben_core::{ErrorKind, testsite};

fn site(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (path, text) in files {
        tmp.child(path).write_str(text).unwrap();
    }
    tmp
}

fn render_with(tmp: &TempDir, drafts: bool, jobs: Option<usize>) -> Result<BuildReport, Failure> {
    let site = Site::load(tmp.path()).map_err(Failure::from)?;
    site.render(&RenderOptions { drafts, jobs })
        .map(|(_, report)| report)
}

fn findings(tmp: &TempDir) -> Vec<Finding> {
    render_with(tmp, false, None).unwrap().findings
}

fn names(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

fn with_rule(all: &[Finding], rule: Rule) -> Vec<&Finding> {
    all.iter().filter(|f| f.rule == rule).collect()
}

#[test]
fn clean_site_has_no_findings() {
    let tmp = site(&[
        (
            "content/index.md",
            "---\ntitle: Home\n---\n# Home\n\nSee [the about page](/about/).\n",
        ),
        (
            "content/about.md",
            "---\ntitle: About\n---\nNo heading of its own, so the shell adds one.\n",
        ),
    ]);
    let report = render_with(&tmp, false, None).unwrap();
    assert_eq!(report.findings, vec![]);
}

#[test]
fn every_rule_fires_once() {
    let tmp = site(&[
        (
            "layouts/bare.html",
            "<html><body><div id=\"dup\">{{ content }}</div><div id=\"dup\"></div><input type=\"text\"></body></html>\n",
        ),
        (
            "content/bare.md",
            "---\ntitle: Bare\nlayout: bare.html\n---\nPlain text.\n",
        ),
        (
            "content/tree.md",
            "---\ntitle: Tree\n---\n\
             ## Start\n\n\
             #### Skipped\n\n\
             <h2 id=\"start\">Again</h2>\n\n\
             [click here](/tree/)\n\n\
             ![](a.png)\n\n\
             [Gone](/nope/)\n\n\
             |  |  |\n| - | - |\n| a | b |\n",
        ),
    ]);
    let all = findings(&tmp);
    for rule in Rule::ALL {
        let hits = with_rule(&all, rule);
        assert_eq!(hits.len(), 1, "{}: {all:#?}", rule.id());
    }
    assert_eq!(all.len(), Rule::ALL.len(), "{all:#?}");

    let page_wide = [
        Rule::HtmlLang,
        Rule::PageTitle,
        Rule::SingleH1,
        Rule::MainLandmark,
        Rule::IdUnique,
        Rule::FormLabel,
        Rule::LinkTarget,
    ];
    for f in &all {
        let expected = if page_wide.contains(&f.rule) {
            "bare.md"
        } else {
            "tree.md"
        };
        if f.rule == Rule::LinkTarget {
            assert_eq!(names(&f.path), "tree.md");
        } else {
            assert_eq!(names(&f.path), expected, "{f:?}");
        }
        if page_wide.contains(&f.rule) {
            let (file, line) = f.built.as_ref().expect("HTML-pass finding has `built`");
            assert!(file.ends_with("index.html"), "{f:?}");
            assert!(*line >= 1);
            assert_eq!(f.location, None);
        } else {
            assert_eq!(f.built, None);
            assert!(f.location.unwrap().line >= 5, "{f:?}");
        }
    }
    let order = with_rule(&all, Rule::HeadingOrder)[0];
    assert_eq!(order.location.unwrap().line, 6);
    assert_eq!(order.include, None);
}

#[test]
fn include_finding_names_both() {
    let tmp = site(&[
        (
            "includes/card.rhai",
            "// a card\nfn card() { raw(\"<a href='/a/'>read more</a>\") }\n",
        ),
        ("content/a.md", "---\ntitle: A\n---\n{{ card() }}\n"),
        ("content/b.md", "---\ntitle: B\n---\n\n{{ card() }}\n"),
    ]);
    let all = findings(&tmp);
    let hits = with_rule(&all, Rule::LinkText);
    assert_eq!(hits.len(), 2, "{all:#?}");
    for (hit, (page, line)) in hits.iter().zip([("a.md", 4), ("b.md", 5)]) {
        assert_eq!(names(&hit.path), page);
        assert_eq!(hit.location.unwrap().line, line);
        let origin = hit.include.as_ref().expect("include named");
        assert!(origin.file.ends_with("includes/card.rhai"), "{origin:?}");
        assert_eq!(origin.line, 2);
    }
}

#[test]
fn broken_link_and_draft_target() {
    let tmp = site(&[
        (
            "content/index.md",
            "---\ntitle: Home\n---\n# Home\n\n[Draft](/draft/) [Resume](/resume.pdf) [Missing](/nope/)\n",
        ),
        (
            "content/draft.md",
            "---\ntitle: Draft\ndraft: true\n---\n# Draft\n",
        ),
        ("static/resume.pdf", "%PDF"),
    ]);
    let messages = |drafts| {
        let report = render_with(&tmp, drafts, None).unwrap();
        report
            .findings
            .iter()
            .filter(|f| f.rule == Rule::LinkTarget)
            .map(|f| f.message.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        messages(false),
        [
            "links to `/draft/`, which isn't built",
            "links to `/nope/`, which isn't built"
        ]
    );
    assert_eq!(messages(true), ["links to `/nope/`, which isn't built"]);
}

#[test]
fn base_url_path_links() {
    let tmp = site(&[
        ("reuben.toml", "base_url = \"https://e.x/blog/\"\n"),
        (
            "content/index.md",
            "---\ntitle: Home\n---\n# Home\n\n[About](/blog/about/) and [Wrong](/about/)\n",
        ),
        ("content/about.md", "---\ntitle: About\n---\nAbout.\n"),
    ]);
    let all = findings(&tmp);
    let messages: Vec<_> = all.iter().map(|f| f.message.as_str()).collect();
    assert_eq!(messages, ["links to `/about/`, which isn't built"]);
    assert_eq!(all[0].rule, Rule::LinkTarget);
}

#[test]
fn rules_switch_off_at_each_level() {
    let vague = "---\ntitle: T\n---\n# T\n\n[click here](/) ![](a.png)\n";
    let fm = "---\ntitle: T\na11y:\n  disable: [link-text]\n---\n# T\n\n[click here](/)\n";
    let tmp = site(&[
        ("reuben.toml", "[a11y]\ndisable = [\"img-alt\"]\n"),
        ("content/index.md", vague),
        (
            "content/quiet/_dir.toml",
            "[a11y]\ndisable = [\"link-text\"]\n",
        ),
        ("content/quiet/a.md", vague),
        ("content/loud/b.md", vague),
        ("content/fm.md", fm),
        ("content/other.md", vague),
    ]);
    let all = findings(&tmp);
    assert_eq!(with_rule(&all, Rule::ImgAlt).len(), 0, "{all:#?}");
    let mut pages: Vec<String> = with_rule(&all, Rule::LinkText)
        .iter()
        .map(|f| names(&f.path))
        .collect();
    pages.sort();
    assert_eq!(pages, ["b.md", "index.md", "other.md"], "{all:#?}");
}

#[test]
fn unknown_rule_ids() {
    let tmp = site(&[("reuben.toml", "[a11y]\ndisable = [\"nope\"]\n")]);
    let errors = Site::load(tmp.path()).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].path.ends_with("reuben.toml"));
    assert_eq!(
        errors[0].to_string().split_once(": ").unwrap().1,
        "config: unknown accessibility rule `nope`"
    );

    let tmp = site(&[
        ("content/docs/_dir.toml", "[a11y]\ndisable = [\"nope\"]\n"),
        ("content/docs/a.md", "---\ntitle: A\n---\n# A\n"),
        ("content/docs/b.md", "---\ntitle: B\n---\n# B\n"),
        ("data/bad.json", "{"),
    ]);
    let failure = render_with(&tmp, false, None).unwrap_err();
    assert_eq!(failure.len(), 2, "{:#?}", *failure);
    let rule_error = failure
        .iter()
        .find(|e| matches!(&e.kind, ErrorKind::UnknownRule(id) if id == "nope"))
        .expect("unknown rule reported");
    assert!(rule_error.path.ends_with("docs/_dir.toml"), "{rule_error}");
    let data_error = failure
        .iter()
        .find(|e| matches!(e.kind, ErrorKind::Data(_)))
        .expect("data error reported");
    assert!(data_error.path.ends_with("data/bad.json"), "{data_error}");
}

#[test]
fn findings_survive_a_failed_build() {
    let tmp = site(&[
        (
            "content/broken.md",
            "---\ntitle: Broken\n---\n# Broken\n\n{{ nope() }}\n",
        ),
        (
            "content/vague.md",
            "---\ntitle: Vague\n---\n# Vague\n\n[click here](/vague/)\n",
        ),
    ]);
    let failure = render_with(&tmp, false, None).unwrap_err();
    assert_eq!(failure.errors.len(), 1, "{:#?}", failure.errors);
    assert!(matches!(failure.errors[0].kind, ErrorKind::Script(_)));
    assert_eq!(failure.findings.len(), 1, "{:#?}", failure.findings);
    assert_eq!(failure.findings[0].rule, Rule::LinkText);
    assert_eq!(names(&failure.findings[0].path), "vague.md");
}

#[test]
fn findings_are_identical_across_jobs() {
    let tmp = TempDir::new().unwrap();
    testsite::generate(tmp.path(), 40).unwrap();
    let one = render_with(&tmp, false, Some(1)).unwrap().findings;
    let many = render_with(&tmp, false, None).unwrap().findings;
    assert!(one.len() >= 40, "{} findings", one.len());
    assert_eq!(one, many);
}
