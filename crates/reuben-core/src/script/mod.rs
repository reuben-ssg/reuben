//! The sandboxed Rhai engine that evaluates template blocks and sidecar scripts.

pub mod collection;
mod helpers;
mod includes;

use std::path::{Path, PathBuf};

use crate::blocks::{Block, BlockKind};
use crate::doc::Span;
use crate::html::escape_output;
use crate::splice::Output;
use crate::template::Escape;

#[derive(Clone)]
pub struct Html(pub String);

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub function: String,
    pub file: PathBuf,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptError {
    pub message: String,
    pub location: Option<Span>,
    pub stack: Vec<Frame>,
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub struct ScriptEngine {
    engine: rhai::Engine,
}

impl std::fmt::Debug for ScriptEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScriptEngine")
    }
}

thread_local! {
    static CURRENT_FILE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

impl ScriptEngine {
    pub fn new(site_root: &Path) -> Self {
        let mut engine = rhai::Engine::new();
        engine.set_max_call_levels(32);
        engine.set_max_operations(1_000_000);
        engine.set_max_expr_depths(64, 32);
        engine.set_max_string_size(10_000_000);
        engine.set_max_array_size(100_000);
        engine.set_max_map_size(100_000);
        engine.disable_symbol("eval");
        engine.set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new());
        engine.on_print(|text| {
            CURRENT_FILE.with(|f| eprintln!("{}: {text}", f.borrow()));
        });
        engine.on_debug(|text, source, _| {
            let prefix = source.map(str::to_owned);
            CURRENT_FILE.with(|f| {
                let file = f.borrow();
                eprintln!("{}: {text}", prefix.as_deref().unwrap_or(&file));
            });
        });
        engine
            .register_type_with_name::<Html>("Html")
            .register_fn("to_string", |h: &mut Html| h.0.clone());
        collection::register(&mut engine);
        helpers::register(&mut engine, site_root);
        Self { engine }
    }

    pub fn eval_block(
        &self,
        scope: &mut rhai::Scope<'static>,
        block: &Block,
        file: &Path,
        escape: Escape,
    ) -> Result<Output, ScriptError> {
        let file_str = file.display().to_string();
        CURRENT_FILE.with(|f| f.borrow_mut().clone_from(&file_str));
        let map_pos = |pos: rhai::Position| {
            let (line, col) = (pos.line()?, pos.position().unwrap_or(1));
            Some(Span {
                line: block.span.line + line - 1,
                column: if line == 1 {
                    block.span.column + col - 1
                } else {
                    col
                },
            })
        };
        let mut ast = self
            .engine
            .compile(&block.source)
            .map_err(|e| ScriptError {
                message: e.err_type().to_string(),
                location: map_pos(e.position()),
                stack: Vec::new(),
            })?;
        ast.set_source(file_str);
        let runtime_err = |e: Box<rhai::EvalAltResult>| runtime_error(*e, map_pos);
        match block.kind {
            BlockKind::Setup => {
                self.engine
                    .run_ast_with_scope(scope, &ast)
                    .map_err(runtime_err)?;
                Ok(Output::Nothing)
            }
            BlockKind::Output => {
                let value = self
                    .engine
                    .eval_ast_with_scope::<rhai::Dynamic>(scope, &ast)
                    .map_err(runtime_err)?;
                to_output(value, escape).map_err(|message| ScriptError {
                    message,
                    location: Some(block.span),
                    stack: Vec::new(),
                })
            }
        }
    }

    pub fn run_file(
        &self,
        scope: &mut rhai::Scope<'static>,
        path: &Path,
    ) -> Result<(), ScriptError> {
        let ast = self.compile_file(path)?;
        self.engine
            .run_ast_with_scope(scope, &ast)
            .map_err(|e| runtime_error(*e, file_span))
    }

    /// Runs a whole file and returns its final expression.
    pub fn eval_file(
        &self,
        scope: &mut rhai::Scope<'static>,
        path: &Path,
    ) -> Result<rhai::Dynamic, ScriptError> {
        let ast = self.compile_file(path)?;
        self.engine
            .eval_ast_with_scope::<rhai::Dynamic>(scope, &ast)
            .map_err(|e| runtime_error(*e, file_span))
    }

    fn compile_file(&self, path: &Path) -> Result<rhai::AST, ScriptError> {
        let path_str = path.display().to_string();
        CURRENT_FILE.with(|f| f.borrow_mut().clone_from(&path_str));
        let plain = |message: String, pos: rhai::Position| ScriptError {
            message,
            location: file_span(pos),
            stack: Vec::new(),
        };
        let src = std::fs::read_to_string(path)
            .map_err(|e| plain(e.to_string(), rhai::Position::NONE))?;
        let mut ast = self
            .engine
            .compile(&src)
            .map_err(|e| plain(e.err_type().to_string(), e.position()))?;
        ast.set_source(path_str);
        Ok(ast)
    }
}

fn file_span(pos: rhai::Position) -> Option<Span> {
    pos.line().map(|line| Span {
        line,
        column: pos.position().unwrap_or(1),
    })
}

/// Splits a runtime error into the failing call site, the innermost message and the include call chain.
/// `map_pos` maps the outermost position (the caller's own source); frame positions are within include files.
fn runtime_error(
    mut err: rhai::EvalAltResult,
    map_pos: impl Fn(rhai::Position) -> Option<Span>,
) -> ScriptError {
    let location = map_pos(err.take_position());
    let mut calls: Vec<(String, String, rhai::Position)> = Vec::new();
    let mut inner = err;
    while let rhai::EvalAltResult::ErrorInFunctionCall(name, source, next, _) = inner {
        calls.push((name, source, next.position()));
        inner = *next;
    }
    let overflowed = matches!(inner, rhai::EvalAltResult::ErrorStackOverflow(_));
    let stack = if overflowed {
        Vec::new()
    } else {
        calls
            .into_iter()
            .map(|(function, file, pos)| Frame {
                function,
                file: PathBuf::from(file),
                line: pos.line().unwrap_or(0),
                column: pos.position().unwrap_or(1),
            })
            .collect()
    };
    let message = match inner {
        rhai::EvalAltResult::ErrorStackOverflow(_) => {
            "include call depth limit (32) exceeded; an include probably calls itself".to_string()
        }
        other => other.to_string(),
    };
    ScriptError {
        message,
        location,
        stack,
    }
}

/// Applies the output rules: strings are escaped unless `escape` is `None`, `Html` is raw, arrays concatenate,
/// maps are an error.
pub fn to_output(value: rhai::Dynamic, escape: Escape) -> Result<Output, String> {
    if value.is_unit() {
        return Ok(Output::Nothing);
    }
    let mut out = String::new();
    let mut html = false;
    write_value(value, escape, &mut out, &mut html)?;
    Ok(if html {
        Output::Html(out)
    } else {
        Output::Text(out)
    })
}

/// `html` is set when any `Html` value contributes, which makes the whole output markup.
fn write_value(
    value: rhai::Dynamic,
    escape: Escape,
    out: &mut String,
    html: &mut bool,
) -> Result<(), String> {
    let text = |s: String| match escape {
        Escape::Html => escape_output(&s),
        Escape::None => s,
    };
    if value.is_unit() {
        Ok(())
    } else if value.is_string() {
        out.push_str(&text(value.into_string()?));
        Ok(())
    } else if value.is::<Html>() {
        *html = true;
        out.push_str(&value.cast::<Html>().0);
        Ok(())
    } else if value.is_array() {
        let items = value.into_array().map_err(|t| t.to_string())?;
        items
            .into_iter()
            .try_for_each(|item| write_value(item, escape, out, html))
    } else if value.is_map() {
        Err("can't output a map; output one of its fields".to_string())
    } else if value.is_int() || value.is_float() || value.is_bool() || value.is_char() {
        out.push_str(&text(value.to_string()));
        Ok(())
    } else {
        Err(format!(
            "can't output a value of type {}",
            value.type_name()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(src: &str) -> Result<Output, ScriptError> {
        let engine = ScriptEngine::new(Path::new("."));
        let mut scope = rhai::Scope::new();
        let block = Block {
            kind: BlockKind::Output,
            source: src.into(),
            span: Span {
                line: 10,
                column: 4,
            },
        };
        engine.eval_block(&mut scope, &block, Path::new("page.md"), Escape::Html)
    }
    #[test]
    fn no_escape_writes_strings_and_scalars_raw() {
        let engine = ScriptEngine::new(Path::new("."));
        let mut scope = rhai::Scope::new();
        let block = Block {
            kind: BlockKind::Output,
            source: "[\"<a>\", 1, raw(\"<b>\")]".into(),
            span: Span::default(),
        };
        assert_eq!(
            engine
                .eval_block(&mut scope, &block, Path::new("p.json"), Escape::None)
                .unwrap(),
            Output::Html("<a>1<b>".into())
        );
        let block = Block {
            source: "\"<a>\"".into(),
            ..block
        };
        assert_eq!(
            engine
                .eval_block(&mut scope, &block, Path::new("p.json"), Escape::None)
                .unwrap(),
            Output::Text("<a>".into())
        );
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(eval("\"<a>\"").unwrap(), Output::Text("&lt;a&gt;".into()));
    }
    #[test]
    fn single_quotes_are_escaped() {
        assert_eq!(eval("\"it's\"").unwrap(), Output::Text("it&#39;s".into()));
    }
    #[test]
    fn html_is_raw() {
        assert_eq!(eval("raw(\"<a>\")").unwrap(), Output::Html("<a>".into()));
    }
    #[test]
    fn arrays_concatenate() {
        assert_eq!(
            eval("[\"a\", raw(\"<b>\"), 3]").unwrap(),
            Output::Html("a<b>3".into())
        );
        assert_eq!(eval("[\"a\", 3]").unwrap(), Output::Text("a3".into()));
    }
    #[test]
    fn unit_outputs_nothing() {
        assert_eq!(eval("()").unwrap(), Output::Nothing);
    }
    #[test]
    fn map_is_an_error() {
        assert!(
            eval("#{ a: 1 }")
                .unwrap_err()
                .message
                .contains("can't output a map")
        );
    }
    #[test]
    fn setup_variables_persist() {
        let engine = ScriptEngine::new(Path::new("."));
        let mut scope = rhai::Scope::new();
        let setup = Block {
            kind: BlockKind::Setup,
            source: "let x = 41;".into(),
            span: Span::default(),
        };
        let out = Block {
            kind: BlockKind::Output,
            source: "x + 1".into(),
            span: Span::default(),
        };
        assert_eq!(
            engine
                .eval_block(&mut scope, &setup, Path::new("p.md"), Escape::Html)
                .unwrap(),
            Output::Nothing
        );
        assert_eq!(
            engine
                .eval_block(&mut scope, &out, Path::new("p.md"), Escape::Html)
                .unwrap(),
            Output::Text("42".into())
        );
    }
    #[test]
    fn error_location_maps_into_the_file() {
        let err = eval("\n  undefined_thing").unwrap_err();
        assert_eq!(
            err.location,
            Some(Span {
                line: 11,
                column: 3
            })
        );
    }
    #[test]
    fn runaway_loop_is_stopped() {
        assert!(eval("loop {}").is_err());
    }
    #[test]
    fn eval_is_disabled() {
        assert!(eval("eval(\"1\")").is_err());
    }
    #[test]
    fn import_is_disabled() {
        use assert_fs::prelude::*;
        let dir = assert_fs::TempDir::new().unwrap();
        dir.child("m.rhai").write_str("fn f() { 1 }").unwrap();
        let module = dir.path().join("m");
        let src = format!("import {:?} as m; m::f()", module.to_str().unwrap());
        assert!(eval(&src).is_err());
    }
}
