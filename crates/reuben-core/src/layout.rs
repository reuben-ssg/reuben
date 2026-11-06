//! Layouts: HTML files whose blocks are all live, resolved site-first and then from the theme.

use std::fs;
use std::path::{Path, PathBuf};

use crate::blocks;
use crate::error::{Error, ErrorKind};
use crate::script::{Html, ScriptEngine};
use crate::splice::Output;
use crate::template::{Escape, substitute};

/// `name` is front matter `layout`, with or without `.html`. Only an explicit name that can't be found is an error;
/// a missing `default` means the built-in shell.
pub fn find_layout(
    name: Option<&str>,
    root: &Path,
    theme: Option<&Path>,
) -> Result<Option<PathBuf>, Error> {
    let stem = name.map(|n| n.strip_suffix(".html").unwrap_or(n));
    if let Some(stem) = stem
        && (stem.is_empty() || stem.contains(['/', '\\']) || stem.contains(".."))
    {
        return Err(Error::new(
            "",
            ErrorKind::InvalidLayoutName(stem.to_string()),
        ));
    }
    let file = format!("{}.html", stem.unwrap_or("default"));
    let found = [Some(root), theme]
        .into_iter()
        .flatten()
        .map(|dir| dir.join("layouts").join(&file))
        .find(|p| p.is_file());
    match (found, stem) {
        (Some(path), _) => Ok(Some(path)),
        (None, None) => Ok(None),
        (None, Some(stem)) => Err(Error::new("", ErrorKind::LayoutNotFound(stem.to_string()))),
    }
}

pub fn render_layout(
    path: &Path,
    engine: &ScriptEngine,
    scope: &mut rhai::Scope<'static>,
    content: Html,
) -> Result<String, Vec<Error>> {
    let fail = |kind: ErrorKind| vec![Error::new(path, kind)];
    let text = fs::read_to_string(path).map_err(|e| fail(e.into()))?;
    scope.push_constant("content", content);
    let extracted = blocks::extract_all(&text, 1).map_err(|e| fail(ErrorKind::Block(e)))?;

    let mut outputs = Vec::with_capacity(extracted.blocks.len());
    let mut errors = Vec::new();
    for block in &extracted.blocks {
        match engine.eval_block(scope, block, path, Escape::Html) {
            Ok(out) => outputs.push(out),
            Err(e) => {
                errors.push(Error::new(path, ErrorKind::Script(e)));
                outputs.push(Output::Nothing);
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(substitute(&extracted.text, &outputs))
}
