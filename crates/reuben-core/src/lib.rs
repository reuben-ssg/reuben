//! Core build pipeline for Reuben: front matter, the typed document tree, HTML output and site builds.

#![forbid(unsafe_code)]

pub mod doc;
pub mod error;
pub mod frontmatter;
pub mod markdown;

pub use error::{Error, ErrorKind};
