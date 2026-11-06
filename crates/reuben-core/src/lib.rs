//! Core build pipeline for Reuben: front matter, the typed document tree, HTML output and site builds.

#![forbid(unsafe_code)]

pub mod doc;
pub mod error;
pub mod frontmatter;
pub mod html;
pub mod markdown;
pub mod route;
pub mod site;
pub mod theme;

pub use error::{Error, ErrorKind};
