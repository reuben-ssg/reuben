//! Layouts: HTML files whose blocks are all live, resolved site-first and then from the theme.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

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

type Found = Result<Option<PathBuf>, Error>;

/// Layouts found and read during one render, so each is looked up and read once however many pages use it.
/// Errors are kept too, and each page gets its own copy.
pub struct Layouts {
    root: PathBuf,
    theme: Option<PathBuf>,
    /// Keyed by the layout name without `.html`; `None` is the default layout.
    found: Mutex<HashMap<Option<String>, Found>>,
    texts: Mutex<HashMap<PathBuf, Result<Arc<str>, Error>>>,
}

impl Layouts {
    pub fn new(root: &Path, theme: Option<&Path>) -> Self {
        Self {
            root: root.to_path_buf(),
            theme: theme.map(Path::to_path_buf),
            found: Mutex::default(),
            texts: Mutex::default(),
        }
    }

    /// [`find_layout`] in this site and theme.
    pub fn find(&self, name: Option<&str>) -> Result<Option<PathBuf>, Error> {
        // `x` and `x.html` name the same layout.
        let key = name.map(|n| n.strip_suffix(".html").unwrap_or(n).to_owned());
        let mut found = self.found.lock().unwrap_or_else(PoisonError::into_inner);
        found
            .entry(key)
            .or_insert_with(|| find_layout(name, &self.root, self.theme.as_deref()))
            .clone()
    }

    /// The text of the layout at `path`, or the error reading it, which names `path`.
    fn text(&self, path: &Path) -> Result<Arc<str>, Error> {
        let mut texts = self.texts.lock().unwrap_or_else(PoisonError::into_inner);
        texts
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                fs::read_to_string(path)
                    .map(Arc::from)
                    .map_err(|e| Error::new(path, e))
            })
            .clone()
    }
}

pub fn render_layout(
    path: &Path,
    layouts: &Layouts,
    engine: &ScriptEngine,
    scope: &mut rhai::Scope<'static>,
    content: Html,
) -> Result<String, Vec<Error>> {
    let fail = |kind: ErrorKind| vec![Error::new(path, kind)];
    let text = layouts.text(path).map_err(|e| vec![e])?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    #[test]
    fn a_layout_is_found_and_read_once_per_render() {
        let tmp = TempDir::new().unwrap();
        tmp.child("layouts/default.html").write_str("one").unwrap();
        let layouts = Layouts::new(tmp.path(), None);
        let path = layouts.find(None).unwrap().unwrap();
        assert_eq!(&*layouts.text(&path).unwrap(), "one");
        fs::write(&path, "two").unwrap();
        assert_eq!(&*layouts.text(&path).unwrap(), "one");
        fs::remove_file(&path).unwrap();
        assert_eq!(layouts.find(None).unwrap(), Some(path));
        // A fresh render sees the change.
        assert_eq!(Layouts::new(tmp.path(), None).find(None).unwrap(), None);
    }

    #[test]
    fn lookups_are_kept_per_name_with_their_errors() {
        let tmp = TempDir::new().unwrap();
        tmp.child("theme/layouts/x.html").write_str("x").unwrap();
        let theme = tmp.path().join("theme");
        let layouts = Layouts::new(tmp.path(), Some(&theme));
        assert_eq!(
            layouts.find(Some("x")).unwrap(),
            Some(theme.join("layouts/x.html"))
        );
        assert_eq!(layouts.find(None).unwrap(), None);
        for _ in 0..2 {
            let err = layouts.find(Some("nope")).unwrap_err();
            assert!(matches!(&err.kind, ErrorKind::LayoutNotFound(n) if n == "nope"));
            let err = layouts.find(Some("../x")).unwrap_err();
            assert!(matches!(&err.kind, ErrorKind::InvalidLayoutName(n) if n == "../x"));
        }
        tmp.child("layouts/x.html").write_str("site").unwrap();
        assert_eq!(
            layouts.find(Some("x.html")).unwrap(),
            Some(theme.join("layouts/x.html"))
        );
    }

    #[test]
    fn a_read_error_is_kept_and_names_the_layout() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("layouts/gone.html");
        let layouts = Layouts::new(tmp.path(), None);
        for _ in 0..2 {
            let err = layouts.text(&path).unwrap_err();
            assert_eq!(err.path, path);
            assert!(
                matches!(&err.kind, ErrorKind::Io(e) if e.kind() == std::io::ErrorKind::NotFound)
            );
        }
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "late").unwrap();
        assert!(layouts.text(&path).is_err());
    }
}
