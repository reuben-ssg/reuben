//! Pulls `{{ … }}` output blocks and `{% … %}` setup blocks out of a page body before Markdown parsing.

use std::fmt;
use std::ops::Range;

use crate::doc::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Output,
    Setup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub source: String,
    /// Position of the first character of `source` in the file.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    /// The input with block `n` replaced by `placeholder(n)`.
    pub text: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockError {
    pub span: Span,
    pub message: String,
}

impl fmt::Display for BlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: {}",
            self.span.line, self.span.column, self.message
        )
    }
}

impl std::error::Error for BlockError {}

/// Private-use characters never occur in normal content and Markdown parsers leave them alone.
pub fn placeholder(n: usize) -> String {
    format!("\u{E000}{n}\u{E001}")
}

/// Removes every placeholder, for text read before the blocks have run.
pub fn strip_placeholders(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_placeholder = false;
    for c in text.chars() {
        match c {
            '\u{E000}' => in_placeholder = true,
            '\u{E001}' => in_placeholder = false,
            _ if !in_placeholder => out.push(c),
            _ => {}
        }
    }
    out
}

/// Extracts blocks from a page body. An opening delimiter that starts inside a `skip` range stays literal.
pub fn extract(
    body: &str,
    body_line: usize,
    skip: &[Range<usize>],
) -> Result<Extracted, BlockError> {
    let bytes = body.as_bytes();
    let mut text = String::with_capacity(body.len());
    let mut blocks = Vec::new();
    let mut literal_from = 0;
    let mut i = 0;
    while i + 1 < bytes.len() {
        let (kind, close) = match (bytes[i], bytes[i + 1]) {
            (b'{', b'{') => (BlockKind::Output, b"}}"),
            (b'{', b'%') => (BlockKind::Setup, b"%}"),
            _ => {
                i += 1;
                continue;
            }
        };
        if skip.iter().any(|r| r.contains(&i)) {
            i += 2;
            continue;
        }
        let code_start = i + 2;
        let Some(code_end) = scan_code(bytes, code_start, Some(close)) else {
            let which = if kind == BlockKind::Output {
                "{{"
            } else {
                "{%"
            };
            return Err(BlockError {
                span: position(body, i, body_line),
                message: format!("unclosed `{which}` block"),
            });
        };
        text.push_str(&body[literal_from..i]);
        text.push_str(&placeholder(blocks.len()));
        blocks.push(Block {
            kind,
            source: body[code_start..code_end].to_string(),
            span: position(body, code_start, body_line),
        });
        i = code_end + 2;
        literal_from = i;
    }
    text.push_str(&body[literal_from..]);
    Ok(Extracted { text, blocks })
}

/// For layouts, which have no Markdown code to protect.
pub fn extract_all(text: &str, first_line: usize) -> Result<Extracted, BlockError> {
    extract(text, first_line, &[])
}

fn position(body: &str, offset: usize, body_line: usize) -> Span {
    let before = &body[..offset];
    let line = body_line + before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |n| n + 1);
    Span {
        line,
        column: before[line_start..].chars().count() + 1,
    }
}

/// Scans Rhai from `i`, returning the index of `close` at brace depth 0. With no `close`, returns the index of
/// the `}` that ends a `${…}` interpolation instead.
fn scan_code(b: &[u8], mut i: usize, close: Option<&[u8]>) -> Option<usize> {
    let mut depth = 0usize;
    while i < b.len() {
        if depth == 0 && close.is_some_and(|c| b[i..].starts_with(c)) {
            return Some(i);
        }
        match b[i] {
            b'{' => depth += 1,
            b'}' if depth == 0 && close.is_none() => return Some(i),
            b'}' => depth = depth.saturating_sub(1),
            b'"' => i = skip_quoted(b, i, b'"')?,
            b'\'' => {
                if let Some(end) = skip_quoted(b, i, b'\'') {
                    i = end;
                }
            }
            b'`' => i = skip_template(b, i)?,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                i = b[i..]
                    .iter()
                    .position(|&c| c == b'\n')
                    .map_or(b.len(), |n| i + n);
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = b[i + 2..].windows(2).position(|w| w == b"*/")? + i + 2 + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Returns the index of the closing quote. A `'` with no closing quote on its line is not a character literal.
fn skip_quoted(b: &[u8], start: usize, quote: u8) -> Option<usize> {
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            c if c == quote => return Some(i),
            b'\n' if quote == b'\'' => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// Returns the index of the closing backtick, recursing into `${…}` interpolations.
fn skip_template(b: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'`' => return Some(i),
            b'$' if b.get(i + 1) == Some(&b'{') => i = scan_code(b, i + 2, None)?,
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_and_setup_blocks() {
        let e = extract("a {{ 1 + 1 }} b\n{% let x = 2; %}\n", 1, &[]).unwrap();
        assert_eq!(
            e.text,
            format!("a {} b\n{}\n", placeholder(0), placeholder(1))
        );
        assert_eq!(e.blocks[0].source, " 1 + 1 ");
        assert!(matches!(e.blocks[1].kind, BlockKind::Setup));
    }

    #[test]
    fn span_counts_front_matter_lines() {
        let e = extract("x\n\n{{\n  page.title\n}}\n", 5, &[]).unwrap();
        assert_eq!(e.blocks[0].span, Span { line: 7, column: 3 });
    }

    #[test]
    fn nested_map_literal_does_not_close_early() {
        let e = extract("{{ #{ a: #{ b: 1 }} }}", 1, &[]).unwrap();
        assert_eq!(e.blocks[0].source, " #{ a: #{ b: 1 }} ");
    }

    #[test]
    fn delimiters_in_strings_and_comments_are_code() {
        let e = extract("{{ \"}}\" + `${ \"%}\" }` // }}\n }}", 1, &[]).unwrap();
        assert_eq!(e.blocks.len(), 1);
        assert!(e.text.ends_with(&placeholder(0)));
    }

    #[test]
    fn blank_lines_inside_a_block_are_fine() {
        let e = extract("{{\n  let a = 1;\n\n  a\n}}\n", 1, &[]).unwrap();
        assert_eq!(e.blocks.len(), 1);
    }

    #[test]
    fn skipped_ranges_stay_literal() {
        let body = "`{{ x }}` and {{ y }}";
        let skip = 0..9;
        let e = extract(body, 1, std::slice::from_ref(&skip)).unwrap();
        assert_eq!(e.blocks.len(), 1);
        assert!(e.text.starts_with("`{{ x }}`"));
    }

    #[test]
    fn unclosed_block_reports_its_opening() {
        let err = extract("ok\n{{ 1 +\n", 3, &[]).unwrap_err();
        assert_eq!(err.span, Span { line: 4, column: 1 });
        assert_eq!(err.message, "unclosed `{{` block");
    }
}
