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

    // (rule, page, source line for a tree-pass finding or built line for an HTML-pass one)
    let source = |rule, line| (rule, "tree.md", Some(line), None);
    let built = |rule, page, file, line| (rule, page, None, Some((file, line)));
    let expected = [
        source(Rule::HeadingOrder, 6),
        source(Rule::LinkText, 10),
        source(Rule::ImgAlt, 12),
        source(Rule::HeadingIdUnique, 8),
        source(Rule::TableHeader, 16),
        built(Rule::HtmlLang, "bare.md", "bare/index.html", 1),
        built(Rule::PageTitle, "bare.md", "bare/index.html", 1),
        built(Rule::SingleH1, "bare.md", "bare/index.html", 1),
        built(Rule::MainLandmark, "bare.md", "bare/index.html", 1),
        built(Rule::IdUnique, "bare.md", "bare/index.html", 2),
        built(Rule::FormLabel, "bare.md", "bare/index.html", 2),
        built(Rule::LinkTarget, "tree.md", "tree/index.html", 17),
    ];
    for (rule, page, line, built) in expected {
        let f = with_rule(&all, rule)[0];
        assert_eq!(names(&f.path), page, "{f:?}");
        assert_eq!(f.location.map(|s| s.line), line, "{f:?}");
        assert_eq!(
            f.built.as_ref().map(|(p, l)| (p.to_str().unwrap(), *l)),
            built,
            "{f:?}"
        );
        assert_eq!(f.include, None);
    }
}

#[test]
fn task_lists_need_no_labels() {
    let tmp = site(&[(
        "content/index.md",
        "---\ntitle: Home\n---\n# Home\n\n- [ ] one\n- [x] two\n",
    )]);
    assert_eq!(findings(&tmp), vec![]);
}

#[test]
fn markdown_links_named_by_title_or_image() {
    let tmp = site(&[(
        "content/index.md",
        "---\ntitle: Home\n---\n# Home\n\n[](/ \"Home\") [![Logo](/logo.png)](/)\n",
    )]);
    let all = findings(&tmp);
    assert_eq!(with_rule(&all, Rule::LinkText), Vec::<&Finding>::new());
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

/// Each `link-text` finding as (page line, include file name and line).
fn link_text_lines(all: &[Finding]) -> Vec<(usize, Option<(String, usize)>)> {
    with_rule(all, Rule::LinkText)
        .iter()
        .map(|f| {
            (
                f.location.unwrap().line,
                f.include.as_ref().map(|o| (names(&o.file), o.line)),
            )
        })
        .collect()
}

#[test]
fn include_output_inside_an_html_block_is_located_at_its_call() {
    let tmp = site(&[
        (
            "includes/card.rhai",
            "// a card\nfn card(name) {\n    raw(\"<div>\\n<a href='/\" + name + \"/'>read more</a>\\n</div>\")\n}\n",
        ),
        (
            "content/index.md",
            "---\ntitle: Home\n---\n# Home\n\n\
             <div class=\"grid\">\n\
             {{ card(\"a\") }}\n\
             {{ card(\"b\") }}\n\
             <a href=\"/x/\">here</a>\n\
             </div>\n",
        ),
    ]);
    let card = Some(("card.rhai".to_string(), 3));
    assert_eq!(
        link_text_lines(&findings(&tmp)),
        [(7, card.clone()), (8, card), (9, None)]
    );
}

#[test]
fn include_calling_include_names_the_inner_one() {
    let tmp = site(&[
        ("includes/outer.rhai", "fn outer() {\n    inner()\n}\n"),
        (
            "includes/inner.rhai",
            "// inner\n\nfn inner() {\n    raw(\"<a href='/'>here</a>\")\n}\n",
        ),
        (
            "includes/md.rhai",
            "fn md() {\n    markdown(\"[more](/)\")\n}\n",
        ),
        (
            "content/index.md",
            "---\ntitle: Home\n---\n# Home\n\n{{ outer() }}\n\n<div>\n{{ md() }}\n</div>\n",
        ),
    ]);
    assert_eq!(
        link_text_lines(&findings(&tmp)),
        [
            (6, Some(("inner.rhai".to_string(), 4))),
            (9, Some(("md.rhai".to_string(), 2)))
        ]
    );
}

#[test]
fn page_made_generated_blocks_are_located_at_the_call() {
    let tmp = site(&[(
        "content/index.md",
        "---\ntitle: Home\n---\n# Home\n{{ markdown(\"Intro\\n\\nMore:\\n\\n[here](/)\") }}\n\nEnd.\n",
    )]);
    assert_eq!(link_text_lines(&findings(&tmp)), [(5, None)]);
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

#[test]
fn rule_switches_accumulate_per_layer() {
    let page = "---\ntitle: T\na11y:\n  disable: [img-alt]\n---\n# T\n\n[click here](/quiet/a/) ![](a.png)\n";
    let tmp = site(&[
        (
            "content/quiet/_dir.toml",
            "[a11y]\ndisable = [\"link-text\"]\n",
        ),
        ("content/quiet/a.md", page),
    ]);
    assert_eq!(findings(&tmp), vec![]);
}

#[test]
fn unknown_id_in_a_dir_toml_survives_a_page_override() {
    let tmp = site(&[
        ("content/docs/_dir.toml", "[a11y]\ndisable = [\"nope\"]\n"),
        (
            "content/docs/a.md",
            "---\ntitle: A\na11y:\n  disable: [img-alt, also-nope]\n---\n# A\n",
        ),
    ]);
    let failure = render_with(&tmp, false, None).unwrap_err();
    let mut found: Vec<(String, String)> = failure
        .iter()
        .map(|e| {
            (
                names(&e.path),
                e.to_string().split_once(": ").unwrap().1.to_string(),
            )
        })
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            (
                "_dir.toml".to_string(),
                "unknown accessibility rule `nope`".to_string()
            ),
            (
                "a.md".to_string(),
                "unknown accessibility rule `also-nope`".to_string()
            ),
        ]
    );
}

#[test]
fn front_matter_and_rule_errors_are_both_reported() {
    let tmp = site(&[(
        "content/a.md",
        "---\ntitle: A\nweight: heavy\na11y:\n  disable: [nope]\n---\n# A\n",
    )]);
    let failure = render_with(&tmp, false, None).unwrap_err();
    assert_eq!(failure.len(), 2, "{:#?}", *failure);
    assert!(
        failure
            .iter()
            .any(|e| matches!(&e.kind, ErrorKind::UnknownRule(id) if id == "nope")),
        "{:#?}",
        *failure
    );
    assert!(
        failure
            .iter()
            .any(|e| matches!(e.kind, ErrorKind::FrontMatter(_))),
        "{:#?}",
        *failure
    );
}

#[test]
fn every_unknown_config_id_is_reported() {
    let tmp = site(&[("reuben.toml", "[a11y]\ndisable = [\"a\", \"b\"]\n")]);
    let errors = Site::load(tmp.path()).unwrap_err();
    let text = errors[0].to_string();
    assert!(text.contains("unknown accessibility rule `a`"), "{text}");
    assert!(text.contains("unknown accessibility rule `b`"), "{text}");
}
