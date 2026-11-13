//! Rust-style rendering of build errors with source snippets.

use std::path::Path;

use reuben_core::check::Finding;
use reuben_core::doc::Span;
use reuben_core::explain::{Entry, Explanation, Source};
use reuben_core::script::Frame;
use reuben_core::{Error, ErrorKind};

/// `path` as it is printed in a diagnostic: relative to the site `root`, or in full when it is outside it.
fn shown(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Formats `error`; `source` is the text of `error.path`, when it could be read. Files are named relative to the
/// site `root`.
pub fn render(error: &Error, source: Option<&str>, root: &Path) -> String {
    let (message, span, stack): (&str, Span, &[Frame]) = match &error.kind {
        ErrorKind::Script(e) => match e.location {
            Some(span) => (&e.message, span, &e.stack),
            None => return plain(error, root),
        },
        ErrorKind::Block(e) => (&e.message, e.span, &[]),
        ErrorKind::Math { message, location } => (message, *location, &[]),
        ErrorKind::Css {
            message,
            location: Some(location),
        } => (message, *location, &[]),
        _ => return plain(error, root),
    };
    let (mut out, pad) = located("error", message, &error.path, span, source, root);
    for f in stack {
        out += &format!(
            "{pad}= in include `{}` ({}:{}:{})\n",
            f.function,
            shown(&f.file, root),
            f.line,
            f.column
        );
    }
    out
}

/// The heading, `-->` line and source snippet shared by errors and findings, plus the gutter padding for lines
/// that follow.
fn located(
    kind: &str,
    message: &str,
    path: &Path,
    span: Span,
    source: Option<&str>,
    root: &Path,
) -> (String, String) {
    let width = span.line.to_string().len();
    let pad = " ".repeat(width + 1);
    let mut out = format!(
        "{kind}: {message}\n{pad}--> {}:{}:{}\n",
        shown(path, root),
        span.line,
        span.column
    );
    let text = source.and_then(|s| s.lines().nth(span.line.saturating_sub(1)));
    if let Some(text) = text {
        out += &format!("{pad}|\n{} | {text}\n", span.line);
        out += &format!("{pad}| {}^\n", " ".repeat(span.column.saturating_sub(1)));
    }
    (out, pad)
}

/// Formats an accessibility finding as a warning; `source` is the text of `finding.path`, when it could be read.
/// Files are named relative to the site `root`.
pub fn render_finding(finding: &Finding, source: Option<&str>, root: &Path) -> String {
    let kind = format!("warning[{}]", finding.rule.id());
    let (mut out, pad) = match finding.location {
        Some(span) => located(&kind, &finding.message, &finding.path, span, source, root),
        None => {
            let mut out = format!(
                "{kind}: {}\n  --> {}",
                finding.message,
                shown(&finding.path, root)
            );
            if let Some((built, line)) = &finding.built {
                out += &format!(" (built as {}, line {line})", shown(built, root));
            }
            (out + "\n", "  ".to_string())
        }
    };
    if let Some(origin) = &finding.include {
        out += &format!(
            "{pad}= note: from {}:{}\n",
            shown(&origin.file, root),
            origin.line
        );
    }
    out
}

/// The page, a blank line, then one row per name: name, value and where it was set, in aligned columns.
/// Files are named relative to the site `root`.
pub fn render_explanation(explanation: &Explanation, root: &Path) -> String {
    let relative = |file: &Path| shown(file, root);
    let origin = |source: &Source| match source {
        Source::FrontMatter { file, line }
        | Source::DirDefaults { file, line }
        | Source::Config { file, line }
        | Source::Data { file, line }
        | Source::Sidecar { file, line }
        | Source::Include { file, line } => match line {
            Some(line) => format!("{}:{line}", relative(file)),
            None => relative(file),
        },
        Source::Computed => "computed".to_string(),
        Source::Collection { name } => format!("collection {name}"),
        Source::Helper => "built-in".to_string(),
    };
    let width = |column: fn(&Entry) -> &str| {
        explanation
            .entries
            .iter()
            .map(|e| column(e).chars().count())
            .max()
            .unwrap_or(0)
    };
    let (names, values) = (width(|e| &e.name), width(|e| &e.value));
    let mut out = format!("{} ({})\n\n", relative(&explanation.page), explanation.url);
    for e in &explanation.entries {
        out += &format!(
            "{:<names$}  {:<values$}  {}\n",
            e.name,
            e.value,
            origin(&e.source)
        );
    }
    out
}

fn plain(error: &Error, root: &Path) -> String {
    format!("error: {}: {}\n", shown(&error.path, root), error.kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reuben_core::doc::Span;
    use reuben_core::script::{Frame, ScriptError};

    #[test]
    fn renders_snippet_with_caret() {
        let err = Error::new(
            "site/content/broken.md",
            ErrorKind::Script(ScriptError {
                message: "unexpected end of input".into(),
                location: Some(Span { line: 3, column: 4 }),
                stack: vec![],
            }),
        );
        assert_eq!(
            render(&err, Some("a\nb\n{{ nope( }}\n"), Path::new("site")),
            "error: unexpected end of input\n  --> content/broken.md:3:4\n  |\n3 | {{ nope( }}\n  |    ^\n"
        );
    }

    #[test]
    fn renders_math_errors_with_snippet() {
        let err = Error::new(
            "content/m.md",
            ErrorKind::Math {
                message: "bad".into(),
                location: Span { line: 2, column: 1 },
            },
        );
        assert_eq!(
            render(&err, Some("a\n$\\frac{1}{$\n"), Path::new("")),
            "error: bad\n  --> content/m.md:2:1\n  |\n2 | $\\frac{1}{$\n  | ^\n"
        );
    }

    #[test]
    fn renders_css_errors_with_snippet() {
        let err = Error::new(
            "static/a.css",
            ErrorKind::Css {
                message: "bad".into(),
                location: Some(Span { line: 2, column: 3 }),
            },
        );
        assert_eq!(
            render(&err, Some("a {\n  color: ;\n"), Path::new("")),
            "error: bad\n  --> static/a.css:2:3\n  |\n2 |   color: ;\n  |   ^\n"
        );
        let unlocated = Error::new(
            "static/a.css",
            ErrorKind::Css {
                message: "bad".into(),
                location: None,
            },
        );
        assert_eq!(
            render(&unlocated, None, Path::new("")),
            "error: static/a.css: css: bad\n"
        );
    }

    #[test]
    fn renders_include_frames() {
        let err = Error::new(
            "content/p.md",
            ErrorKind::Script(ScriptError {
                message: "boom".into(),
                location: Some(Span { line: 1, column: 1 }),
                stack: vec![Frame {
                    function: "fail".into(),
                    file: "/site/includes/fail.rhai".into(),
                    line: 3,
                    column: 5,
                }],
            }),
        );
        assert!(
            render(&err, Some("x\n"), Path::new("/site"))
                .ends_with("  = in include `fail` (includes/fail.rhai:3:5)\n")
        );
    }

    #[test]
    fn renders_findings_with_snippet_note_and_built_location() {
        use reuben_a11y::Rule;
        use reuben_core::doc::Origin;

        let tree = Finding {
            path: "content/p.md".into(),
            location: Some(Span { line: 2, column: 1 }),
            rule: Rule::LinkText,
            message: "bad link".into(),
            include: Some(Origin {
                file: "/site/includes/card.rhai".into(),
                line: 2,
            }),
            built: None,
        };
        assert_eq!(
            render_finding(&tree, Some("x\n{{ card() }}\n"), Path::new("/site")),
            "warning[link-text]: bad link\n  --> content/p.md:2:1\n  |\n2 | {{ card() }}\n  | ^\n  = note: from includes/card.rhai:2\n"
        );
        let html = Finding {
            location: None,
            include: None,
            built: Some(("a/index.html".into(), 14)),
            ..tree
        };
        assert_eq!(
            render_finding(&html, None, Path::new("/site")),
            "warning[link-text]: bad link\n  --> content/p.md (built as a/index.html, line 14)\n"
        );
    }

    #[test]
    fn renders_explanation_rows_with_root_relative_origins() {
        let entry = |name: &str, value: &str, source| Entry {
            name: name.into(),
            value: value.into(),
            source,
        };
        let explanation = Explanation {
            page: "/site/content/a.md".into(),
            url: "/a/".into(),
            entries: vec![
                entry(
                    "page.x",
                    "1",
                    Source::DirDefaults {
                        file: "/site/content/_dir.toml".into(),
                        line: Some(3),
                    },
                ),
                entry("page.url", "\"/a/\"", Source::Computed),
                entry(
                    "collections.blog",
                    "2 pages",
                    Source::Collection {
                        name: "blog".into(),
                    },
                ),
                entry("helpers", "raw", Source::Helper),
                entry(
                    "data.d",
                    "#{}",
                    Source::Data {
                        file: "/elsewhere/d.json".into(),
                        line: None,
                    },
                ),
            ],
            errors: Vec::new(),
        };
        assert_eq!(
            render_explanation(&explanation, Path::new("/site")),
            "content/a.md (/a/)\n\
             \n\
             page.x            1        content/_dir.toml:3\n\
             page.url          \"/a/\"    computed\n\
             collections.blog  2 pages  collection blog\n\
             helpers           raw      built-in\n\
             data.d            #{}      /elsewhere/d.json\n"
        );
    }

    #[test]
    fn no_location_keeps_m1_format() {
        let err = Error::new(
            "content/x.md",
            ErrorKind::Io(std::io::Error::other("disk gone")),
        );
        assert_eq!(
            render(&err, None, Path::new("")),
            "error: content/x.md: disk gone\n"
        );
    }

    #[test]
    fn caret_counts_characters_not_bytes() {
        let err = Error::new(
            "p.md",
            ErrorKind::Script(ScriptError {
                message: "m".into(),
                location: Some(Span { line: 1, column: 3 }),
                stack: vec![],
            }),
        );
        assert!(render(&err, Some("éé{{\n"), Path::new("")).contains("1 | éé{{\n  |   ^\n"));
    }
}
