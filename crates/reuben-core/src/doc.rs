//! Reuben's typed document tree.
//!
//! Every later stage (Rhai blocks, accessibility checks, HTML output) walks this tree, never the parser's own AST.

/// A 1-based position in the original file, counting front matter lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: usize,
    pub column: usize,
}

/// A line in an `includes/*.rhai` file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    pub file: std::path::PathBuf,
    pub line: usize,
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
    /// `id` is empty until `headings::assign_ids` runs on the final tree.
    Heading {
        level: u8,
        id: String,
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
    /// An `HtmlBlock` after its placeholders were filled with block output. It renders exactly like `HtmlBlock`;
    /// `fills` says which bytes of `html` each block wrote, so later stages can trace them.
    FilledHtmlBlock {
        html: String,
        fills: Vec<Fill>,
    },
    HtmlInline(String),
    /// The HTML that block number `block` (its index in the page's extracted blocks) produced, spliced at block
    /// level or inline. It renders exactly like `HtmlBlock`/`HtmlInline`; the index lets later stages find the
    /// block's `Markup` and so the include that wrote it.
    Generated {
        html: String,
        block: usize,
        inline: bool,
    },
    Table {
        alignments: Vec<Alignment>,
    },
    TableRow {
        header: bool,
    },
    TableCell,
    Strikethrough,
    /// `index` is the 1-based number the footnote gets, in order of first reference.
    FootnoteReference {
        label: String,
        index: usize,
    },
    /// All footnote definitions, appended to the document in reference order.
    FootnoteSection,
    FootnoteDefinition {
        label: String,
        index: usize,
    },
    /// Becomes `Raw` MathML in `math::render_math`.
    Math {
        display: bool,
        latex: String,
    },
    /// Already-rendered HTML for a construct the tree doesn't model yet, so nothing is dropped.
    Raw(String),
}

/// The output of block number `block` in a `FilledHtmlBlock`: the byte range of the block's `html` it fills, and the
/// 1-based line of the HTML block its placeholder was on.
#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    pub range: std::ops::Range<usize>,
    pub block: usize,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Alignment {
    None,
    Left,
    Center,
    Right,
}

pub type Document = Node;
