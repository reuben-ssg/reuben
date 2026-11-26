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
    /// Takes a position in `text` back to the input, which has more lines wherever a block spanned several.
    pub map: SourceMap,
}

/// Maps positions in extracted text back to the body it came from. A placeholder is one line, but the block it
/// replaced may have spanned several, so everything after it on its line and on later lines moves. Lines count from
/// the body's first line and columns are 1-based byte columns, like the Markdown parser's source positions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMap {
    /// One per block, in order.
    jumps: Vec<Jump>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Jump {
    /// The placeholder's line in the extracted text, and the columns of its first byte and of the byte after it.
    line: usize,
    start: usize,
    end: usize,
    /// The body positions of the block's opening delimiter and of the byte after its closing one.
    from: Span,
    to: Span,
}

impl SourceMap {
    /// The body position of `line` and `column` in the extracted text. A position inside a placeholder is the
    /// position of its block's opening delimiter.
    pub fn source(&self, line: usize, column: usize) -> Span {
        let before = self
            .jumps
            .partition_point(|j| (j.line, j.start) <= (line, column));
        let Some(jump) = before.checked_sub(1).map(|i| &self.jumps[i]) else {
            return Span { line, column };
        };
        if line != jump.line {
            Span {
                line: line + jump.to.line - jump.line,
                column,
            }
        } else if column < jump.end {
            jump.from
        } else {
            Span {
                line: jump.to.line,
                column: jump.to.column + column - jump.end,
            }
        }
    }

    /// How many line breaks each block spanned, which its placeholder took out.
    pub fn lines(&self) -> Vec<usize> {
        self.jumps.iter().map(|j| j.to.line - j.from.line).collect()
    }
}

/// Counts lines over a text as positions in it are asked for in increasing order.
struct Cursor {
    offset: usize,
    line: usize,
    line_start: usize,
}

impl Cursor {
    fn new() -> Self {
        Self {
            offset: 0,
            line: 1,
            line_start: 0,
        }
    }

    /// The line and byte column of `to`, which is at or after the last position asked for.
    fn at(&mut self, text: &[u8], to: usize) -> Span {
        for (i, b) in text[self.offset..to].iter().enumerate() {
            if *b == b'\n' {
                self.line += 1;
                self.line_start = self.offset + i + 1;
            }
        }
        self.offset = to;
        Span {
            line: self.line,
            column: to - self.line_start + 1,
        }
    }
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
    let mut jumps = Vec::new();
    let (mut in_body, mut in_text) = (Cursor::new(), Cursor::new());
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
        let held = in_text.at(text.as_bytes(), text.len());
        let slot = placeholder(blocks.len());
        text.push_str(&slot);
        blocks.push(Block {
            kind,
            source: body[code_start..code_end].to_string(),
            span: position(body, code_start, body_line),
        });
        jumps.push(Jump {
            line: held.line,
            start: held.column,
            end: held.column + slot.len(),
            from: in_body.at(bytes, i),
            to: in_body.at(bytes, code_end + 2),
        });
        i = code_end + 2;
        literal_from = i;
    }
    text.push_str(&body[literal_from..]);
    Ok(Extracted {
        text,
        blocks,
        map: SourceMap { jumps },
    })
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
    fn positions_without_blocks_are_unchanged() {
        let e = extract("a\nb c\n", 1, &[]).unwrap();
        assert_eq!(e.map.source(2, 3), Span { line: 2, column: 3 });
    }

    #[test]
    fn positions_after_a_multi_line_block_map_to_the_body() {
        // The placeholder is 7 bytes, so the extracted text is `a ⟨0⟩ b` then `c`.
        let e = extract("a {{ x\ny }} b\nc\n", 1, &[]).unwrap();
        assert_eq!(e.text, format!("a {} b\nc\n", placeholder(0)));
        let at = |line, column| e.map.source(line, column);
        assert_eq!(at(1, 1), Span { line: 1, column: 1 });
        // Inside the placeholder is the block's opening.
        assert_eq!(at(1, 3), Span { line: 1, column: 3 });
        assert_eq!(at(1, 9), Span { line: 1, column: 3 });
        // After it, on its line, is after the closing `}}`.
        assert_eq!(at(1, 10), Span { line: 2, column: 5 });
        assert_eq!(at(1, 11), Span { line: 2, column: 6 });
        assert_eq!(at(2, 1), Span { line: 3, column: 1 });
    }

    #[test]
    fn several_blocks_add_up() {
        let body = "{%\nlet a = 1;\n%}\n\nx {{ a +\n1 }} y {{ 2 }} z\n\nw\n";
        let e = extract(body, 1, &[]).unwrap();
        let p = |n| placeholder(n).len();
        // `x ⟨1⟩ y ⟨2⟩ z` is extracted line 3; body line 6 is `1 }} y {{ 2 }} z`.
        let z = 1 + 2 + p(1) + 3 + p(2) + 1;
        assert_eq!(
            e.map.source(3, z),
            Span {
                line: 6,
                column: 16
            }
        );
        assert_eq!(e.map.source(5, 1), Span { line: 8, column: 1 });
        assert_eq!(e.map.lines(), [2, 1, 0]);
    }

    #[test]
    fn blocks_in_skipped_code_shift_nothing() {
        let body = "`{{ a\nb }}`\n{{ c\n}}\nd\n";
        let skip = 0..11;
        let e = extract(body, 1, std::slice::from_ref(&skip)).unwrap();
        assert_eq!(e.blocks.len(), 1);
        assert_eq!(e.map.source(2, 2), Span { line: 2, column: 2 });
        assert_eq!(e.map.source(4, 1), Span { line: 5, column: 1 });
    }

    #[test]
    fn multi_byte_text_counts_bytes_like_the_parser() {
        let e = extract("é {{ x\n}} ü\n", 1, &[]).unwrap();
        let after = "é ".len() + placeholder(0).len() + 2;
        assert_eq!(e.map.source(1, after), Span { line: 2, column: 4 });
    }

    #[test]
    fn unclosed_block_reports_its_opening() {
        let err = extract("ok\n{{ 1 +\n", 3, &[]).unwrap_err();
        assert_eq!(err.span, Span { line: 4, column: 1 });
        assert_eq!(err.message, "unclosed `{{` block");
    }
}
