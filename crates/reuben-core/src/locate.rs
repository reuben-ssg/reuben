//! Finds the line a key or declaration is written on. Used only by `reuben explain`, never during a build.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Toml,
    Yaml,
    Json,
}

/// The 1-based line where the key that set the value at `dotted_key` is written, or `None` when the text has no
/// such key or doesn't parse. Keys that contain a `.` can't be named.
pub fn line_of(src: &str, format: Format, dotted_key: &str) -> Option<usize> {
    let path: Vec<&str> = dotted_key.split('.').collect();
    match format {
        Format::Toml => toml_line(src, &path),
        Format::Yaml => yaml_line(src, &path),
        Format::Json => Json::new(src).find(&path),
    }
}

fn toml_line(src: &str, path: &[&str]) -> Option<usize> {
    let doc = toml::de::DeTable::parse(src).ok()?;
    let mut table = doc.get_ref();
    for (depth, want) in path.iter().enumerate() {
        let (key, value) = table.iter().find(|(k, _)| k.get_ref().as_ref() == *want)?;
        if depth + 1 == path.len() {
            return Some(line_at(src, key.span().start));
        }
        table = value.get_ref().as_table()?;
    }
    None
}

fn line_at(src: &str, offset: usize) -> usize {
    1 + src.as_bytes()[..offset]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
}

/// Walks block mappings by indentation. Flow mappings (`{a: 1}`) and keys inside sequences aren't entered.
fn yaml_line(src: &str, path: &[&str]) -> Option<usize> {
    // The keys of the mappings that enclose the current line, with the indent each was written at.
    let mut stack: Vec<(usize, String)> = Vec::new();
    // Lines indented deeper than this belong to a block scalar (`key: |`).
    let mut scalar_indent: Option<usize> = None;
    for (index, line) in src.lines().enumerate() {
        let text = line.trim_start_matches(' ');
        let indent = line.len() - text.len();
        if text.is_empty() {
            continue;
        }
        if let Some(parent) = scalar_indent {
            if indent > parent {
                continue;
            }
            scalar_indent = None;
        }
        if text.starts_with('#') || text.starts_with("- ") || text == "-" || text.starts_with("---")
        {
            continue;
        }
        let Some((key, rest)) = yaml_key(text) else {
            continue;
        };
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        let found = stack.len() + 1 == path.len()
            && stack.iter().zip(path).all(|((_, k), p)| k == p)
            && key == path[stack.len()];
        if found {
            return Some(index + 1);
        }
        let rest = rest.trim();
        if rest.is_empty() || rest.starts_with('#') {
            stack.push((indent, key));
        } else if rest.starts_with(['|', '>']) {
            scalar_indent = Some(indent);
        }
    }
    None
}

/// Splits `key: rest` at the colon that ends a (possibly quoted) key.
fn yaml_key(text: &str) -> Option<(String, &str)> {
    if let Some(quote) = text.chars().next().filter(|c| matches!(c, '"' | '\'')) {
        let end = text[1..].find(quote)? + 1;
        let rest = text[end + 1..].strip_prefix(':')?;
        return (rest.is_empty() || rest.starts_with(' '))
            .then(|| (text[1..end].to_string(), rest));
    }
    let end = text.find(':')?;
    let rest = &text[end + 1..];
    (rest.is_empty() || rest.starts_with(' ')).then(|| (text[..end].trim_end().to_string(), rest))
}

/// A cursor over JSON text that counts lines as it moves.
struct Json<'a> {
    bytes: &'a [u8],
    at: usize,
    line: usize,
}

impl<'a> Json<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            bytes: src.as_bytes(),
            at: 0,
            line: 1,
        }
    }

    fn find(mut self, path: &[&str]) -> Option<usize> {
        self.space();
        self.object(path, 0)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.at += 1;
        if b == b'\n' {
            self.line += 1;
        }
        Some(b)
    }

    fn space(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.bump();
        }
    }

    /// Reads a string and returns it with the line it started on.
    fn string(&mut self) -> Option<(String, usize)> {
        let line = self.line;
        self.bump().filter(|b| *b == b'"')?;
        let mut raw = Vec::new();
        loop {
            match self.bump()? {
                b'"' => break,
                b'\\' => {
                    raw.push(b'\\');
                    raw.push(self.bump()?);
                }
                b => raw.push(b),
            }
        }
        let text = String::from_utf8_lossy(&raw).into_owned();
        let decoded = serde_json::from_str::<String>(&format!("\"{text}\"")).unwrap_or(text);
        Some((decoded, line))
    }

    /// Reads the members of an object. `None` means the key at `path[depth..]` wasn't in it.
    fn object(&mut self, path: &[&str], depth: usize) -> Option<usize> {
        self.bump().filter(|b| *b == b'{')?;
        loop {
            self.space();
            match self.peek()? {
                b'}' => {
                    self.bump();
                    return None;
                }
                b',' => {
                    self.bump();
                    continue;
                }
                _ => {}
            }
            let (key, line) = self.string()?;
            self.space();
            self.bump().filter(|b| *b == b':')?;
            self.space();
            if key == path[depth] {
                if depth + 1 == path.len() {
                    return Some(line);
                }
                if self.peek() == Some(b'{') {
                    if let Some(found) = self.object(path, depth + 1) {
                        return Some(found);
                    }
                    continue;
                }
            }
            self.skip_value()?;
        }
    }

    fn skip_value(&mut self) -> Option<()> {
        match self.peek()? {
            b'"' => {
                self.string()?;
            }
            b'{' | b'[' => {
                let mut nesting = 0usize;
                loop {
                    match self.peek()? {
                        b'"' => {
                            self.string()?;
                            continue;
                        }
                        b'{' | b'[' => nesting += 1,
                        b'}' | b']' => nesting -= 1,
                        _ => {}
                    }
                    self.bump();
                    if nesting == 0 {
                        break;
                    }
                }
            }
            _ => {
                while self
                    .peek()
                    .is_some_and(|b| !matches!(b, b',' | b'}' | b']') && !b.is_ascii_whitespace())
                {
                    self.bump();
                }
            }
        }
        Some(())
    }
}

/// A word or one of `(`, `)` found at the top level of a Rhai script, and the line it is on.
struct Token {
    text: String,
    line: usize,
}

/// The words of a Rhai script that sit outside every `{ }`, skipping comments and string and character literals.
fn top_level_tokens(src: &str) -> Vec<Token> {
    let chars: Vec<char> = src.chars().collect();
    let mut tokens = Vec::new();
    let (mut i, mut line, mut depth) = (0, 1, 0usize);
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\n' => line += 1,
            '/' if next == Some('/') => {
                while i + 1 < chars.len() && chars[i + 1] != '\n' {
                    i += 1;
                }
            }
            '/' if next == Some('*') => {
                let mut nesting = 1;
                i += 2;
                while i < chars.len() && nesting > 0 {
                    match (chars[i], chars.get(i + 1)) {
                        ('/', Some('*')) => {
                            nesting += 1;
                            i += 1;
                        }
                        ('*', Some('/')) => {
                            nesting -= 1;
                            i += 1;
                        }
                        ('\n', _) => line += 1,
                        _ => {}
                    }
                    i += 1;
                }
                continue;
            }
            '"' | '\'' | '`' => {
                i += 1;
                while i < chars.len() && chars[i] != c {
                    match chars[i] {
                        '\\' if c != '`' => i += 1,
                        '\n' => line += 1,
                        _ => {}
                    }
                    i += 1;
                }
            }
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '(' | ')' if depth == 0 => tokens.push(Token {
                text: c.to_string(),
                line,
            }),
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i + 1 < chars.len() && (chars[i + 1].is_alphanumeric() || chars[i + 1] == '_')
                {
                    i += 1;
                }
                if depth == 0 {
                    tokens.push(Token {
                        text: chars[start..=i].iter().collect(),
                        line,
                    });
                }
            }
            _ => {}
        }
        i += 1;
    }
    tokens
}

/// The line of the last top-level `let name` or `const name` in a Rhai script.
pub fn let_line(src: &str, name: &str) -> Option<usize> {
    let tokens = top_level_tokens(src);
    tokens
        .windows(2)
        .filter(|w| matches!(w[0].text.as_str(), "let" | "const") && w[1].text == name)
        .map(|w| w[0].line)
        .next_back()
}

/// A callable function an include script declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<String>,
    pub line: usize,
}

/// The public top-level functions of a Rhai script.
pub fn fn_decls(src: &str) -> Vec<FnDecl> {
    let tokens = top_level_tokens(src);
    let mut decls = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if token.text != "fn" || (i > 0 && tokens[i - 1].text == "private") {
            continue;
        }
        let (Some(name), Some(open)) = (tokens.get(i + 1), tokens.get(i + 2)) else {
            continue;
        };
        if open.text != "(" {
            continue;
        }
        let params = tokens[i + 3..]
            .iter()
            .take_while(|t| t.text != ")")
            .map(|t| t.text.clone())
            .collect();
        decls.push(FnDecl {
            name: name.text.clone(),
            params,
            line: token.line,
        });
    }
    decls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_lines() {
        let src = "a = 1\n[t]\nb = 2\nc.d = 3\ne = { f = 4 }";
        let at = |k| line_of(src, Format::Toml, k);
        assert_eq!(at("a"), Some(1));
        assert_eq!(at("t.b"), Some(3));
        assert_eq!(at("t.c.d"), Some(4));
        assert_eq!(at("t.e.f"), Some(5));
        assert_eq!(at("t.missing"), None);
        assert_eq!(at("nope.b"), None);
        assert_eq!(line_of("a = ", Format::Toml, "a"), None);
    }

    #[test]
    fn yaml_lines() {
        let src = "title: x\nparams:\n  deep:\n    k: 1\nlist: [1]";
        let at = |k| line_of(src, Format::Yaml, k);
        assert_eq!(at("title"), Some(1));
        assert_eq!(at("params.deep.k"), Some(4));
        assert_eq!(at("list"), Some(5));
        assert_eq!(at("params.k"), None);
        assert_eq!(at("missing"), None);
    }

    #[test]
    fn yaml_skips_block_scalars_lists_and_comments() {
        let src = "# top\nnote: |\n  title: not a key\ntags:\n  - title: nor this\n\"quoted key\": 1\nafter: 2\n";
        let at = |k| line_of(src, Format::Yaml, k);
        assert_eq!(at("title"), None);
        assert_eq!(at("note"), Some(2));
        assert_eq!(at("quoted key"), Some(6));
        assert_eq!(at("after"), Some(7));
    }

    #[test]
    fn json_lines() {
        let src = "{\n  \"a\": 1,\n  \"t\": {\n    \"b\": [1, {\"a\": 2}],\n    \"c\": \"}\"\n  },\n  \"d\": null\n}";
        let at = |k| line_of(src, Format::Json, k);
        assert_eq!(at("a"), Some(2));
        assert_eq!(at("t.b"), Some(4));
        assert_eq!(at("t.c"), Some(5));
        assert_eq!(at("d"), Some(7));
        assert_eq!(at("t.a"), None);
        assert_eq!(line_of("[1]", Format::Json, "a"), None);
    }

    #[test]
    fn rhai_lets_are_found_at_the_top_level_only() {
        let src = "// let a = 1;\nlet a = \"let b = 2;\";\nif a == 1 {\n    let c = 3;\n}\nconst d = 4;\nlet a = 5;\n";
        assert_eq!(let_line(src, "a"), Some(7));
        assert_eq!(let_line(src, "b"), None);
        assert_eq!(let_line(src, "c"), None);
        assert_eq!(let_line(src, "d"), Some(6));
    }

    #[test]
    fn rhai_functions_have_names_params_and_lines() {
        let src = "/* fn hidden() {} */\nfn card(p, q) {\n    fn inner() {}\n}\nprivate fn secret() {}\nfn none() {}\n";
        let decls = fn_decls(src);
        assert_eq!(
            decls,
            [
                FnDecl {
                    name: "card".into(),
                    params: vec!["p".into(), "q".into()],
                    line: 2
                },
                FnDecl {
                    name: "none".into(),
                    params: vec![],
                    line: 6
                },
            ]
        );
    }
}
