//! Core build pipeline for Reuben: front matter, the typed document tree, HTML output and site builds.

#![forbid(unsafe_code)]

pub mod blocks;
pub mod check;
pub mod collections;
pub mod config;
pub mod css;
pub mod data;
pub mod dirs;
pub mod doc;
pub mod error;
pub mod frontmatter;
pub mod headings;
pub mod highlight;
pub mod html;
pub mod layout;
pub mod markdown;
pub mod math;
pub mod merge;
pub mod output;
pub mod page;
pub mod route;
pub mod script;
pub mod serve;
pub mod site;
pub mod sitemap;
pub mod splice;
pub mod template;
#[doc(hidden)]
pub mod testsite;
pub mod theme;

pub use error::{Error, ErrorKind};
