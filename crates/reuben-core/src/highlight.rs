//! Build-time syntax highlighting for fenced code blocks, emitting `hl-` prefixed CSS classes only.

use std::path::Path;
use std::sync::OnceLock;

use syntect::highlighting::ThemeSet;
use syntect::html::{ClassStyle, ClassedHTMLGenerator, css_for_theme_with_class_style};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

const CLASS_STYLE: ClassStyle = ClassStyle::SpacedPrefixed { prefix: "hl-" };

/// Turns source code into highlighted inner HTML for a `<code>` element.
pub trait Highlighter: Send + Sync {
    /// Returns the highlighted HTML, or `None` when the language is not known.
    fn highlight(&self, lang: &str, code: &str) -> Option<String>;
}

pub struct SyntectHighlighter {
    syntaxes: SyntaxSet,
}

impl SyntectHighlighter {
    pub fn new() -> Self {
        SyntectHighlighter {
            syntaxes: SyntaxSet::load_defaults_newlines(),
        }
    }
}

impl Default for SyntectHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl Highlighter for SyntectHighlighter {
    fn highlight(&self, lang: &str, code: &str) -> Option<String> {
        let token = lang
            .split(|c: char| c == ',' || c.is_whitespace())
            .next()
            .unwrap_or_default()
            .to_lowercase();
        let syntax = self.syntaxes.find_syntax_by_token(&token)?;
        let mut generator =
            ClassedHTMLGenerator::new_with_class_style(syntax, &self.syntaxes, CLASS_STYLE);
        for line in LinesWithEndings::from(code) {
            generator
                .parse_html_for_line_which_includes_newline(line)
                .ok()?;
        }
        Some(generator.finalize())
    }
}

/// The shared highlighter, built on first use because loading the syntax set is slow.
pub fn default_highlighter() -> &'static dyn Highlighter {
    static H: OnceLock<SyntectHighlighter> = OnceLock::new();
    H.get_or_init(SyntectHighlighter::new)
}

/// CSS for the `hl-` classes from a built-in syntect theme name or a `.tmTheme` file path.
pub fn theme_css(theme: &str) -> Result<String, String> {
    let themes = ThemeSet::load_defaults();
    let loaded = match themes.themes.get(theme) {
        Some(t) => t.clone(),
        None if Path::new(theme).is_file() => ThemeSet::get_theme(theme)
            .map_err(|e| format!("cannot load theme file `{theme}`: {e}"))?,
        None => {
            let mut names: Vec<_> = themes.themes.keys().cloned().collect();
            names.sort();
            return Err(format!(
                "unknown theme `{theme}` (built-in themes: {})",
                names.join(", ")
            ));
        }
    };
    css_for_theme_with_class_style(&loaded, CLASS_STYLE)
        .map_err(|e| format!("cannot render theme `{theme}`: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_is_highlighted_with_classes() {
        let h = default_highlighter()
            .highlight("rust", "fn main() {}\n")
            .unwrap();
        assert!(h.contains("class=\"hl-"));
        assert!(!h.contains("style="));
    }

    #[test]
    fn info_string_extras_and_case() {
        for lang in ["rust,ignore", "rust title=\"x\"", "RUST"] {
            assert!(
                default_highlighter()
                    .highlight(lang, "let x = 1;\n")
                    .is_some(),
                "{lang}"
            );
        }
    }

    #[test]
    fn unknown_language_is_none() {
        assert!(
            default_highlighter()
                .highlight("rhai", "let x = 1;\n")
                .is_none()
        );
    }

    #[test]
    fn huge_block_does_not_panic() {
        let code = "let x = 1;\n".repeat(10_000);
        assert!(default_highlighter().highlight("rust", &code).is_some());
    }

    #[test]
    fn theme_css_has_prefixed_classes() {
        assert!(theme_css("base16-ocean.dark").unwrap().contains(".hl-"));
    }

    #[test]
    fn unknown_theme_names_it() {
        assert!(theme_css("nope").unwrap_err().contains("nope"));
    }
}
