//! The syntax set is process-global, so this check lives in its own test binary.

use reuben_core::highlight::syntaxes_loaded;
use reuben_core::html::render;
use reuben_core::markdown::parse;

#[test]
fn syntax_set_loads_only_when_a_fence_names_a_language() {
    assert!(!syntaxes_loaded());

    render(&parse("# Title\n\nJust prose and `code`.\n", 1));
    assert!(!syntaxes_loaded(), "no code blocks");

    render(&parse("```\nplain\n```\n", 1));
    assert!(!syntaxes_loaded(), "language-less fence");

    render(&parse("```rust\nfn a() {}\n```\n", 1));
    assert!(syntaxes_loaded(), "rust fence");
}
