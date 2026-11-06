//! Build-time LaTeX to MathML, with the source kept as an annotation.

use pulldown_latex::config::DisplayMode;
use pulldown_latex::{Parser, RenderConfig, Storage, push_mathml};

use crate::doc::{Document, Node, NodeKind, Span};
use crate::html::escape_text;

pub fn to_mathml(latex: &str, display: bool) -> Result<String, String> {
    let storage = Storage::new();
    // Collected up front so a late parse error can't leave half a formula in the output.
    let events = Parser::new(latex, &storage)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    // The renderer writes the annotation verbatim, so it's escaped here.
    let annotation = escape_text(latex);
    let mut config = RenderConfig::with_annotation(&annotation);
    config.display_mode = if display {
        DisplayMode::Block
    } else {
        DisplayMode::Inline
    };
    let mut out = String::new();
    push_mathml(
        &mut out,
        events.into_iter().map(Ok::<_, std::convert::Infallible>),
        config,
    )
    .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Replaces every math node with its MathML, collecting each failure rather than stopping at the first.
pub fn render_math(doc: &mut Document) -> Result<(), Vec<(Span, String)>> {
    let mut errors = Vec::new();
    walk(doc, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn walk(node: &mut Node, errors: &mut Vec<(Span, String)>) {
    if let NodeKind::Math { display, latex } = &node.kind {
        match to_mathml(latex, *display) {
            Ok(mathml) => node.kind = NodeKind::Raw(mathml),
            Err(message) => errors.push((node.span, message)),
        }
        return;
    }
    for child in &mut node.children {
        walk(child, errors);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_and_display_mathml() {
        let inline = to_mathml("x^2", false).unwrap();
        assert!(
            inline.starts_with("<math")
                && inline.contains("<msup>")
                && inline.contains("application/x-tex")
        );
        assert!(!inline.contains("display=\"block\""));
        assert!(
            to_mathml("x^2", true)
                .unwrap()
                .contains("display=\"block\"")
        );
    }

    #[test]
    fn invalid_latex_is_an_error() {
        assert!(to_mathml("\\frac{1}{", false).is_err());
    }

    #[test]
    fn annotation_is_escaped() {
        let m = to_mathml("a<b", false).unwrap();
        assert!(
            m.contains("<annotation encoding=\"application/x-tex\">a&lt;b</annotation>"),
            "{m}"
        );
    }
}
