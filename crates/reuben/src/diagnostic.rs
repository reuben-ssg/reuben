//! Rust-style rendering of build errors with source snippets.

use reuben_core::doc::Span;
use reuben_core::script::Frame;
use reuben_core::{Error, ErrorKind};

/// Formats `error`; `source` is the text of `error.path`, when it could be read.
pub fn render(error: &Error, source: Option<&str>) -> String {
    let (message, span, stack): (&str, Span, &[Frame]) = match &error.kind {
        ErrorKind::Script(e) => match e.location {
            Some(span) => (&e.message, span, &e.stack),
            None => return plain(error),
        },
        ErrorKind::Block(e) => (&e.message, e.span, &[]),
        ErrorKind::Math { message, location } => (message, *location, &[]),
        ErrorKind::Css {
            message,
            location: Some(location),
        } => (message, *location, &[]),
        _ => return plain(error),
    };
    let width = span.line.to_string().len();
    let pad = " ".repeat(width + 1);
    let mut out = format!(
        "error: {message}\n{pad}--> {}:{}:{}\n",
        error.path.display(),
        span.line,
        span.column
    );
    let text = source.and_then(|s| s.lines().nth(span.line.saturating_sub(1)));
    if let Some(text) = text {
        out += &format!("{pad}|\n{} | {text}\n", span.line);
        out += &format!("{pad}| {}^\n", " ".repeat(span.column.saturating_sub(1)));
    }
    for f in stack {
        out += &format!(
            "{pad}= in include `{}` ({}:{}:{})\n",
            f.function,
            f.file.display(),
            f.line,
            f.column
        );
    }
    out
}

fn plain(error: &Error) -> String {
    format!("error: {error}\n")
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
            render(&err, Some("a\nb\n{{ nope( }}\n")),
            "error: unexpected end of input\n  --> site/content/broken.md:3:4\n  |\n3 | {{ nope( }}\n  |    ^\n"
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
            render(&err, Some("a\n$\\frac{1}{$\n")),
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
            render(&err, Some("a {\n  color: ;\n")),
            "error: bad\n  --> static/a.css:2:3\n  |\n2 |   color: ;\n  |   ^\n"
        );
        let unlocated = Error::new(
            "static/a.css",
            ErrorKind::Css {
                message: "bad".into(),
                location: None,
            },
        );
        assert_eq!(render(&unlocated, None), "error: static/a.css: css: bad\n");
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
                    file: "includes/fail.rhai".into(),
                    line: 3,
                    column: 5,
                }],
            }),
        );
        assert!(
            render(&err, Some("x\n")).ends_with("  = in include `fail` (includes/fail.rhai:3:5)\n")
        );
    }

    #[test]
    fn no_location_keeps_m1_format() {
        let err = Error::new(
            "content/x.md",
            ErrorKind::Io(std::io::Error::other("disk gone")),
        );
        assert_eq!(render(&err, None), "error: content/x.md: disk gone\n");
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
        assert!(render(&err, Some("éé{{\n")).contains("1 | éé{{\n  |   ^\n"));
    }
}
