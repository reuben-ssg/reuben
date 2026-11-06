//! Build errors. Every error names the file it came from.

use std::path::PathBuf;

use crate::blocks::BlockError;
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
    #[error("config: {0}")]
    Config(String),
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
    pub fn new(path: impl Into<PathBuf>, kind: impl Into<ErrorKind>) -> Self {
        Self {
            path: path.into(),
            kind: kind.into(),
        }
    }
}
