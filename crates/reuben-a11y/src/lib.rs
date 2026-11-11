//! Accessibility rules for Reuben: the rule catalogue, the checks that run on
//! a parsed tree of headings, links, images and tables, and fragment parsing.

#![forbid(unsafe_code)]

mod fragment;
mod rules;
mod scan;
mod tree;

pub use fragment::fragment_items;
pub use rules::{Rule, RuleSet};
pub use tree::{Finding, Item, check_tree};
