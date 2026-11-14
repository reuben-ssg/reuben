//! Build errors. Every error names the file it came from.

use std::path::PathBuf;

use crate::blocks::BlockError;
use crate::doc::Span;
use crate::frontmatter::FrontMatterError;
use crate::script::ScriptError;

#[derive(Debug, Clone, thiserror::Error)]
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
    #[error("unknown accessibility rule `{0}`")]
    UnknownRule(String),
    #[error("layout `{0}` not found in the site or its theme")]
    LayoutNotFound(String),
    #[error("invalid layout name `{0}`: a layout is a plain file name in `layouts/`")]
    InvalidLayoutName(String),
    #[error(
        "no page matches `{0}`: give a path like content/blog/x.md, a path under content/ like blog/x.md, or a URL like /blog/x/"
    )]
    PageNotFound(String),
    #[error("internal error: a block placeholder was left in the rendered page")]
    UnsplicedPlaceholder,
    #[error("file is not valid UTF-8")]
    NotUtf8,
    #[error("builds to {} too, which {} already builds to", file.display(), other.display())]
    OutputCollision { other: PathBuf, file: PathBuf },
    #[error("builds to {}, but `__reuben/` is reserved for the dev server", .0.display())]
    ReservedOutputPath(PathBuf),
    #[error(
        "output directory isn't empty and wasn't created by Reuben (no `{}` file), so it won't be cleared",
        crate::site::OUTPUT_MARKER
    )]
    UnsafeOutputDir,
}

/// `std::io::Error` isn't `Clone`, so an I/O error is rebuilt from its kind and message.
impl Clone for ErrorKind {
    fn clone(&self) -> Self {
        match self {
            Self::FrontMatter(e) => Self::FrontMatter(e.clone()),
            Self::Io(e) => Self::Io(std::io::Error::new(e.kind(), e.to_string())),
            Self::Script(e) => Self::Script(e.clone()),
            Self::Block(e) => Self::Block(e.clone()),
            Self::Math { message, location } => Self::Math {
                message: message.clone(),
                location: *location,
            },
            Self::Css { message, location } => Self::Css {
                message: message.clone(),
                location: *location,
            },
            Self::Config(s) => Self::Config(s.clone()),
            Self::Data(s) => Self::Data(s.clone()),
            Self::UnknownRule(s) => Self::UnknownRule(s.clone()),
            Self::LayoutNotFound(s) => Self::LayoutNotFound(s.clone()),
            Self::InvalidLayoutName(s) => Self::InvalidLayoutName(s.clone()),
            Self::PageNotFound(s) => Self::PageNotFound(s.clone()),
            Self::UnsplicedPlaceholder => Self::UnsplicedPlaceholder,
            Self::NotUtf8 => Self::NotUtf8,
            Self::OutputCollision { other, file } => Self::OutputCollision {
                other: other.clone(),
                file: file.clone(),
            },
            Self::ReservedOutputPath(p) => Self::ReservedOutputPath(p.clone()),
            Self::UnsafeOutputDir => Self::UnsafeOutputDir,
        }
    }
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

    /// Orders errors by path, then line and column, with position-less errors first within a path, and drops repeats.
    /// A problem in a file that several others import is found once per importer but is one problem.
    /// The message is the last key so repeats at one position end up adjacent however the errors arrived.
    pub fn sort(errors: &mut Vec<Error>) {
        errors.sort_by_cached_key(|e| {
            (
                e.path.clone(),
                e.location().map(|s| (s.line, s.column)),
                e.kind.to_string(),
            )
        });
        errors.dedup_by(|a, b| {
            a.path == b.path
                && a.location() == b.location()
                && a.kind.to_string() == b.kind.to_string()
        });
    }

    pub fn new(path: impl Into<PathBuf>, kind: impl Into<ErrorKind>) -> Self {
        Self {
            path: path.into(),
            kind: kind.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_drops_repeats_that_arrived_interleaved() {
        let e = |m: &str| Error::new("a.md", ErrorKind::Config(m.into()));
        let mut errors = vec![e("A"), e("B"), e("A")];
        Error::sort(&mut errors);
        let messages: Vec<_> = errors.iter().map(ToString::to_string).collect();
        assert_eq!(messages, ["a.md: config: A", "a.md: config: B"]);
    }
}
