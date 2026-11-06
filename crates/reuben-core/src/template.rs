//! Templates: any file with front matter outside Markdown, rendered with the extension's escaping.

use std::path::Path;

use rhai::Scope;

use crate::blocks;
use crate::config::Config;
use crate::error::{Error, ErrorKind};
use crate::page;
use crate::script::ScriptEngine;
use crate::site::Page;
use crate::splice::Output;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Escape {
    Html,
    None,
}

/// Output is HTML-escaped for markup extensions and written as-is for everything else.
pub fn escape_for(path: &Path) -> Escape {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match ext.as_deref() {
        Some("html" | "htm" | "xml" | "svg" | "md") => Escape::Html,
        _ => Escape::None,
    }
}

/// True when the bytes are UTF-8 and, after an optional BOM, open with a `---` or `+++` line that a later line
/// closes. An unclosed opener is just a file that happens to start that way, like a YAML document.
pub fn starts_with_front_matter(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    match lines.next() {
        Some(open @ ("---" | "+++")) => lines.any(|l| l == open),
        _ => false,
    }
}

/// Replaces each block placeholder in one pass, so output that happens to look like a placeholder stays literal.
pub fn substitute(text: &str, outputs: &[Output]) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('\u{E000}') {
        result.push_str(&rest[..start]);
        let after = &rest[start + '\u{E000}'.len_utf8()..];
        let replacement = after.find('\u{E001}').and_then(|end| {
            let n = after[..end].parse::<usize>().ok()?;
            let out = outputs.get(n)?;
            Some((out, end + '\u{E001}'.len_utf8()))
        });
        match replacement {
            Some((out, consumed)) => {
                if let Output::Text(s) | Output::Html(s) = out {
                    result.push_str(s);
                }
                rest = &after[consumed..];
            }
            None => {
                result.push('\u{E000}');
                rest = after;
            }
        }
    }
    result.push_str(rest);
    result
}

/// Evaluates every block in the template body. Returns the text and the scope, so a layout sees its variables.
pub fn render(
    page: &Page,
    escape: Escape,
    engine: &ScriptEngine,
    config: &Config,
    scopes: &page::SiteScopes<'_>,
) -> Result<(String, Scope<'static>), Vec<Error>> {
    let mut scope = page::build_scope(page, engine, config, scopes)?;
    let extracted = blocks::extract_all(&page.body, page.body_line)
        .map_err(|e| vec![Error::new(&page.source, ErrorKind::Block(e))])?;

    let mut outputs = Vec::with_capacity(extracted.blocks.len());
    let mut errors = Vec::new();
    for block in &extracted.blocks {
        match engine.eval_block(&mut scope, block, &page.source, escape) {
            Ok(out) => outputs.push(out),
            Err(e) => {
                errors.push(Error::new(&page.source, ErrorKind::Script(e)));
                outputs.push(Output::Nothing);
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok((substitute(&extracted.text, &outputs), scope))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::placeholder;

    #[test]
    fn front_matter_needs_a_closing_delimiter() {
        assert!(starts_with_front_matter(b"---\ntitle: x\n---\nbody"));
        assert!(starts_with_front_matter(
            b"\xef\xbb\xbf+++\r\na = 1\r\n+++\r\n"
        ));
        assert!(!starts_with_front_matter(b"---\nopenapi: 3.0.0\n"));
        assert!(!starts_with_front_matter(b"---\na: 1\n+++\n"));
    }

    #[test]
    fn front_matter_detection_requires_utf8() {
        assert!(!starts_with_front_matter(b"---\n\xff\n---\n"));
        assert!(starts_with_front_matter(
            b"\xef\xbb\xbf---\ntitle: x\n---\n"
        ));
        assert!(starts_with_front_matter(b"+++\r\n+++\r\n"));
        assert!(!starts_with_front_matter(b"----\n"));
        assert!(!starts_with_front_matter(b"body { }\n"));
    }

    #[test]
    fn substitute_is_single_pass() {
        let text = format!("{}|{}", placeholder(0), placeholder(1));
        let outputs = [Output::Text(placeholder(1)), Output::Html("B".into())];
        assert_eq!(substitute(&text, &outputs), format!("{}|B", placeholder(1)));
    }

    #[test]
    fn escape_follows_extension() {
        for ext in ["html", "HTM", "xml", "svg", "md"] {
            assert_eq!(escape_for(Path::new(&format!("a.{ext}"))), Escape::Html);
        }
        for p in ["a.json", "a.css", "a.txt", "a"] {
            assert_eq!(escape_for(Path::new(p)), Escape::None);
        }
    }
}
