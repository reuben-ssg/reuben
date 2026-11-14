//! The checks that run on a finished page's HTML.

use std::collections::HashSet;

use crate::rules::{Rule, RuleSet};
use crate::scan::{Event, StartTag, scan};

#[derive(Debug, Clone, PartialEq)]
pub struct HtmlFinding {
    pub rule: Rule,
    pub message: String,
    /// 1-based line in the page's HTML where the offending start tag ends;
    /// page-wide findings use line 1.
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalLink {
    pub href: String,
    pub line: usize,
}

#[derive(Debug, Default)]
pub struct PageReport {
    pub findings: Vec<HtmlFinding>,
    /// Every `<a href>` that isn't plainly external, collected only when
    /// `link-target` is on.
    pub links: Vec<LocalLink>,
}

struct Control {
    tag: String,
    id: Option<String>,
    line: usize,
}

/// Checks one page. `reported_ids` are heading ids the tree pass already
/// flagged, so `id-unique` doesn't report them twice.
pub fn check_page(html: &str, rules: &RuleSet, reported_ids: &HashSet<String>) -> PageReport {
    let mut report = PageReport::default();
    let mut html_tag_seen = false;
    let mut html_lang_ok = false;
    let mut title_seen = false;
    let mut title_text = String::new();
    let mut in_title = false;
    let mut svg_depth = 0usize;
    let mut h1_lines: Vec<usize> = Vec::new();
    let mut main_seen = false;
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut label_depth = 0usize;
    let mut label_targets: HashSet<String> = HashSet::new();
    let mut unlabelled: Vec<Control> = Vec::new();

    for event in scan(&[(html, ())]) {
        match event {
            Event::Start(tag) => {
                match tag.name.as_str() {
                    "html" => {
                        html_tag_seen = true;
                        html_lang_ok |= non_blank(&tag, "lang");
                    }
                    "svg" => svg_depth += 1,
                    "title" if svg_depth == 0 => {
                        title_seen = true;
                        in_title = true;
                    }
                    "h1" => h1_lines.push(tag.line),
                    "main" => main_seen = true,
                    "label" => {
                        label_depth += 1;
                        if let Some(target) = tag.attr("for") {
                            label_targets.insert(target.to_string());
                        }
                    }
                    "a" => {
                        if let Some(href) = tag.attr("href").map(crate::links::trim_href)
                            && rules.is_on(Rule::LinkTarget)
                            && !is_external(href)
                        {
                            report.links.push(LocalLink {
                                href: href.to_string(),
                                line: tag.line,
                            });
                        }
                    }
                    "input" | "select" | "textarea" => {
                        // A disabled input can't be used, like the checkboxes of a Markdown task list.
                        let exempt = tag.name == "input"
                            && (tag.attr("disabled").is_some()
                                || tag.attr("type").is_some_and(|t| {
                                    ["hidden", "submit", "reset", "button", "image"]
                                        .iter()
                                        .any(|x| t.trim().eq_ignore_ascii_case(x))
                                }));
                        let named = ["aria-label", "aria-labelledby", "title"]
                            .iter()
                            .any(|a| non_blank(&tag, a));
                        if !exempt && !named && label_depth == 0 {
                            unlabelled.push(Control {
                                tag: tag.name.clone(),
                                id: tag.attr("id").map(str::to_string),
                                line: tag.line,
                            });
                        }
                    }
                    _ => {}
                }
                if tag.attr("role").is_some_and(|r| {
                    r.split_ascii_whitespace()
                        .any(|r| r.eq_ignore_ascii_case("main"))
                }) {
                    main_seen = true;
                }
                if let Some(id) = tag.attr("id")
                    && !id.trim().is_empty()
                    && !seen_ids.insert(id.to_string())
                    && !reported_ids.contains(id)
                    && rules.is_on(Rule::IdUnique)
                {
                    report.findings.push(HtmlFinding {
                        rule: Rule::IdUnique,
                        message: format!("id \"{id}\" is used more than once on this page"),
                        line: tag.line,
                    });
                }
            }
            Event::End(name) => match name.as_str() {
                "title" => in_title = false,
                "svg" => svg_depth = svg_depth.saturating_sub(1),
                "label" => label_depth = label_depth.saturating_sub(1),
                _ => {}
            },
            Event::Text(text) => {
                if in_title {
                    title_text.push_str(&text);
                }
            }
        }
    }

    if !html_tag_seen || !html_lang_ok {
        page_wide(
            &mut report,
            rules,
            Rule::HtmlLang,
            "the <html> element has no lang attribute",
        );
    }
    if !title_seen || title_text.trim().is_empty() {
        page_wide(
            &mut report,
            rules,
            Rule::PageTitle,
            "the page has no <title> text",
        );
    }
    if h1_lines.len() != 1 && rules.is_on(Rule::SingleH1) {
        report.findings.push(HtmlFinding {
            rule: Rule::SingleH1,
            message: format!("found {} <h1> elements, expected 1", h1_lines.len()),
            line: h1_lines.get(1).copied().unwrap_or(1),
        });
    }
    if !main_seen {
        page_wide(
            &mut report,
            rules,
            Rule::MainLandmark,
            "the page has no <main> landmark",
        );
    }
    if rules.is_on(Rule::FormLabel) {
        for control in unlabelled {
            if control
                .id
                .as_ref()
                .is_some_and(|id| label_targets.contains(id))
            {
                continue;
            }
            report.findings.push(HtmlFinding {
                rule: Rule::FormLabel,
                message: format!("<{}> has no label", control.tag),
                line: control.line,
            });
        }
    }
    report
}

fn page_wide(report: &mut PageReport, rules: &RuleSet, rule: Rule, message: &str) {
    if rules.is_on(rule) {
        report.findings.push(HtmlFinding {
            rule,
            message: message.to_string(),
            line: 1,
        });
    }
}

fn non_blank(tag: &StartTag<()>, attr: &str) -> bool {
    tag.attr(attr).is_some_and(|v| !v.trim().is_empty())
}

fn is_external(href: &str) -> bool {
    matches!(
        crate::links::link_keys("/", "/", href),
        crate::links::LinkKeys::External
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "<!doctype html><html lang=\"en\"><head><title>T</title></head><body><main><h1>T</h1><p id=\"a\">x</p></main></body></html>";

    fn check(html: &str) -> PageReport {
        check_page(html, &RuleSet::all(), &HashSet::new())
    }

    fn rules_of(report: &PageReport) -> Vec<Rule> {
        report.findings.iter().map(|f| f.rule).collect()
    }

    #[test]
    fn a_good_page_has_no_findings() {
        assert_eq!(check(GOOD).findings, []);
    }

    #[test]
    fn page_wide_rules() {
        let report = check("<html><head></head><body><p>x</p></body></html>");
        assert_eq!(
            rules_of(&report),
            [
                Rule::HtmlLang,
                Rule::PageTitle,
                Rule::SingleH1,
                Rule::MainLandmark
            ]
        );
        assert!(report.findings.iter().all(|f| f.line == 1));
        assert_eq!(
            report.findings[2].message,
            "found 0 <h1> elements, expected 1"
        );
    }

    #[test]
    fn blank_lang_and_title_count_as_missing() {
        let report = check("<html lang=\" \"><title> </title><main><h1>x</h1></main>");
        assert_eq!(rules_of(&report), [Rule::HtmlLang, Rule::PageTitle]);
    }

    #[test]
    fn two_h1s_point_at_the_second() {
        let html = GOOD.replace("<h1>T</h1>", "<h1>T</h1>\n\n\n\n<h1>U</h1>");
        let html = html.replace("<main>", "<main>\n\n");
        let report = check(&html);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].rule, Rule::SingleH1);
        assert_eq!(report.findings[0].line, 7);
        assert_eq!(
            report.findings[0].message,
            "found 2 <h1> elements, expected 1"
        );
    }

    #[test]
    fn role_main_counts() {
        let html = GOOD
            .replace("<main>", "<div role=\"main\">")
            .replace("</main>", "</div>");
        assert_eq!(check(&html).findings, []);
    }

    #[test]
    fn duplicate_ids_skip_reported() {
        let html = "<html lang=en><title>T</title><main><h1>T</h1>\n<p id=a></p>\n<p id=a></p>\n<p id=b></p>\n<p id=b></p></main>";
        let reported = HashSet::from(["b".to_string()]);
        let report = check_page(html, &RuleSet::all(), &reported);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].rule, Rule::IdUnique);
        assert!(report.findings[0].message.contains("\"a\""));
        assert_eq!(report.findings[0].line, 3);
    }

    fn with_body(body: &str) -> PageReport {
        check(&GOOD.replace("<p id=\"a\">x</p>", body))
    }

    #[test]
    fn form_labels() {
        assert_eq!(with_body("<input id=q><label for=q>Q</label>").findings, []);
        assert_eq!(with_body("<label>Name <input></label>").findings, []);
        assert_eq!(with_body("<input aria-label=x>").findings, []);
        assert_eq!(
            with_body("<input type=hidden><input type=submit>").findings,
            []
        );
        let report = with_body("<input><select></select><textarea></textarea>");
        assert_eq!(rules_of(&report), [Rule::FormLabel; 3]);
    }

    #[test]
    fn task_list_checkboxes_need_no_label() {
        let tasks = "<ul>\n<li><input type=\"checkbox\" disabled=\"\" /> one</li>\n<li><input type=\"checkbox\" disabled=\"\" checked=\"\" /> two</li>\n</ul>";
        assert_eq!(with_body(tasks).findings, []);
        let enabled = with_body("<input type=\"checkbox\">");
        assert_eq!(rules_of(&enabled), [Rule::FormLabel]);
    }

    #[test]
    fn blank_ids_are_not_duplicates() {
        let report =
            with_body("<p id=\"\"></p><p id=\"\"></p><p id=\"   \"></p><p id=\"   \"></p>");
        assert_eq!(report.findings, []);
    }

    #[test]
    fn hrefs_are_trimmed() {
        let report = with_body("<a href=\" /a/ \">a</a><a href=\" https://x.y/\">x</a>");
        assert_eq!(
            report.links,
            [LocalLink {
                href: "/a/".into(),
                line: 1
            }]
        );
    }

    #[test]
    fn collects_local_links_only() {
        let report = with_body(
            "<a href=\"/a/\">a</a>\n<a href=\"b/\">b</a><a href=\"https://x.y\">x</a><a href=\"#top\">t</a><a href=\"mailto:m@x.y\">m</a>",
        );
        assert_eq!(
            report.links,
            [
                LocalLink {
                    href: "/a/".into(),
                    line: 1
                },
                LocalLink {
                    href: "b/".into(),
                    line: 2
                },
            ]
        );
    }

    #[test]
    fn links_are_not_collected_when_link_target_is_off() {
        let mut rules = RuleSet::all();
        rules.disable(Rule::LinkTarget);
        let html = GOOD.replace("<p id=\"a\">x</p>", "<a href=\"/a/\">a</a>");
        assert!(check_page(&html, &rules, &HashSet::new()).links.is_empty());
    }

    #[test]
    fn an_svg_title_is_not_the_page_title() {
        let html = "<html lang=en><body><main><h1>x</h1><svg><title>Menu</title></svg></main>";
        assert_eq!(rules_of(&check(html)), [Rule::PageTitle]);
        let empty = "<html lang=en><title> </title><body><main><h1>x</h1><svg><title>Menu</title></svg></main>";
        assert_eq!(rules_of(&check(empty)), [Rule::PageTitle]);
    }

    #[test]
    fn an_h1_inside_an_svg_title_does_not_count() {
        let html = GOOD.replace(
            "<p id=\"a\">x</p>",
            "<svg><title><h1>icon</h1></title></svg>",
        );
        assert_eq!(check(&html).findings, []);
    }

    #[test]
    fn comments_and_style_hide_markup() {
        let report = with_body(
            "<!-- <a href=\"/c/\">c</a><h1>c</h1> --><style>/* <a href=\"/s/\"> <h1> */</style><a href=\"/real/\">r</a>",
        );
        assert_eq!(report.findings, []);
        assert_eq!(report.links.len(), 1);
        assert_eq!(report.links[0].href, "/real/");
    }

    #[test]
    fn uppercase_markup_is_treated_like_lowercase() {
        let html = "<HTML LANG=en><TITLE>T</TITLE><MAIN><H1>T</H1><INPUT TYPE=HIDDEN></MAIN>";
        assert_eq!(check(html).findings, []);
        let bare = "<HTML LANG=en><TITLE>T</TITLE><MAIN><H1>T</H1><INPUT></MAIN>";
        assert_eq!(rules_of(&check(bare)), [Rule::FormLabel]);
    }

    #[test]
    fn unclosed_tags_and_unquoted_attributes_do_not_panic() {
        let html = "<html lang=en><title>T<main><h1 class=a>T<p id=x><a href=/y/>y<input type=text";
        let report = check(html);
        assert!(report.findings.iter().all(|f| f.line >= 1));
        let _ = check("<a href=\"/x/");
        let _ = check("<");
        let _ = check("");
    }

    #[test]
    fn skip_links_and_decorated_links() {
        let report = with_body(
            "<a href=\"#main\">skip</a><a href=\"/a/?q=1\">a</a><a href=\"/b/#sec\">b</a>",
        );
        assert_eq!(report.findings, []);
        let hrefs: Vec<&str> = report.links.iter().map(|l| l.href.as_str()).collect();
        assert_eq!(hrefs, ["/a/?q=1", "/b/#sec"]);
    }

    #[test]
    fn script_contents_are_ignored() {
        let html = GOOD.replace(
            "<p id=\"a\">x</p>",
            "<script>document.write('<h1>x</h1><input>')</script>",
        );
        assert_eq!(check(&html).findings, []);
    }
}
