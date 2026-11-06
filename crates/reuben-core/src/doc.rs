//! Reuben's typed document tree.
//!
//! Every later stage (Rhai blocks, accessibility checks, HTML output) walks this tree, never the parser's own AST.

/// A 1-based position in the original file, counting front matter lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: NodeKind,
    pub span: Span,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    Document,
    Paragraph,
    Heading {
        level: u8,
    },
    Text(String),
    Emphasis,
    Strong,
    Code(String),
    CodeBlock {
        info: String,
        literal: String,
    },
    Link {
        url: String,
        title: String,
    },
    Image {
        url: String,
        title: String,
    },
    List {
        ordered: bool,
        start: usize,
        tight: bool,
    },
    Item,
    TaskItem {
        checked: bool,
    },
    BlockQuote,
    ThematicBreak,
    SoftBreak,
    LineBreak,
    HtmlBlock(String),
    HtmlInline(String),
    /// Already-rendered HTML for a construct the tree doesn't model yet (tables, footnotes, …), so nothing is dropped.
    Raw(String),
}

pub type Document = Node;
