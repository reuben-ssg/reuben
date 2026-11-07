//! Build errors. Every error names the file it came from.

use std::path::PathBuf;

use crate::blocks::BlockError;
use crate::doc::Span;
use crate::frontmatter::FrontMatterError;
use crate::script::ScriptError;

#[derive(Debug, thiserror::Error)]
#[error("{}: {kind}", path.display())]
pub struct Error {
    pub path: PathBuf,
    pub kind: ErrorKind,
}

#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    #[error("front matter: {0}")]
    FrontMatter(#[from] FrontMatterError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Script(ScriptError),
    #[error("{0}")]
    Block(BlockError),
    #[error("math: {message}")]
    Math { message: String, location: Span },
    #[error("css: {message}")]
    Css {
        message: String,
        location: Option<Span>,
    },
    #[error("config: {0}")]
    Config(String),
    #[error("data: {0}")]
    Data(String),
    #[error("layout `{0}` not found in the site or its theme")]
    LayoutNotFound(String),
    #[error("invalid layout name `{0}`: a layout is a plain file name in `layouts/`")]
    InvalidLayoutName(String),
    #[error("internal error: a block placeholder was left in the rendered page")]
    UnsplicedPlaceholder,
    #[error("file is not valid UTF-8")]
    NotUtf8,
    #[error("builds to {} too, which {} already builds to", file.display(), other.display())]
    OutputCollision { other: PathBuf, file: PathBuf },
    #[error(
        "output directory isn't empty and wasn't created by Reuben (no `{}` file), so it won't be cleared",
        crate::site::OUTPUT_MARKER
    )]
    UnsafeOutputDir,
}

impl Error {
    /// Where in `path` the error points, for kinds that carry a position.
    pub fn location(&self) -> Option<Span> {
        match &self.kind {
            ErrorKind::Script(e) => e.location,
            ErrorKind::Block(e) => Some(e.span),
            ErrorKind::Math { location, .. } => Some(*location),
            ErrorKind::Css { location, .. } => *location,
            _ => None,
        }
    }

    /// Orders errors by path, then line and column, with position-less errors first within a path.
    pub fn sort(errors: &mut [Error]) {
        errors.sort_by_cached_key(|e| (e.path.clone(), e.location().map(|s| (s.line, s.column))));
    }

    pub fn new(path: impl Into<PathBuf>, kind: impl Into<ErrorKind>) -> Self {
        Self {
            path: path.into(),
            kind: kind.into(),
        }
    }
}
