//! Loads `includes/*.rhai` as global Rhai modules so pages and includes can call them.

use std::path::PathBuf;

use std::path::Path;

use super::ScriptEngine;
use crate::doc::Span;
use crate::error::{Error, ErrorKind};
use crate::script::ScriptError;

impl ScriptEngine {
    /// Registers every `*.rhai` file directly in each dir, lowest precedence first.
    /// All files are compiled and registered before any page runs, so includes can call one another across files.
    pub fn load_includes(&mut self, dirs: &[PathBuf]) -> Result<(), Vec<Error>> {
        let mut errors = Vec::new();
        let mut modules = Vec::new();
        for dir in dirs {
            let mut files = match list_rhai(dir) {
                Ok(files) => files,
                Err(e) => {
                    errors.push(Error::new(dir, e));
                    continue;
                }
            };
            files.sort();
            for path in files {
                match self.compile_include(&path) {
                    Ok(module) => modules.push(module),
                    Err(e) => errors.push(e),
                }
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        for module in modules {
            self.engine.register_global_module(module);
        }
        Ok(())
    }

    fn compile_include(&self, path: &Path) -> Result<rhai::Shared<rhai::Module>, Error> {
        let fail = |message: String, pos: rhai::Position| {
            Error::new(
                path,
                ErrorKind::Script(ScriptError {
                    message,
                    location: pos.line().map(|line| Span {
                        line,
                        column: pos.position().unwrap_or(1),
                    }),
                    stack: Vec::new(),
                }),
            )
        };
        let src = std::fs::read_to_string(path).map_err(|e| Error::new(path, e))?;
        if let Ok(mut files) = self.include_files.write() {
            files.insert(path.display().to_string());
        }
        let mut ast = self
            .engine
            .compile(&src)
            .map_err(|e| fail(e.err_type().to_string(), e.position()))?;
        ast.set_source(path.display().to_string());
        let module = rhai::Module::eval_ast_as_new(rhai::Scope::new(), &ast, &self.engine)
            .map_err(|mut e| {
                let pos = e.take_position();
                fail(e.to_string(), pos)
            })?;
        Ok(module.into())
    }
}

pub(crate) fn list_rhai(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "rhai") {
            files.push(path);
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::blocks::{Block, BlockKind};
    use crate::doc::Span;
    use crate::error::ErrorKind;
    use crate::script::ScriptError;
    use crate::splice::Output;
    use crate::template::Escape;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/includes")
            .join(name)
    }

    fn eval_with(dirs: &[&str], src: &str) -> Result<Output, ScriptError> {
        let mut engine = ScriptEngine::new(Path::new("."));
        let dirs: Vec<PathBuf> = dirs.iter().map(|d| fixture(d)).collect();
        engine.load_includes(&dirs).unwrap();
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
    fn includes_call_each_other_across_files() {
        let Output::Html(m) = eval_with(&["theme"], "card(\"<x>\")").unwrap() else {
            panic!("not html")
        };
        assert_eq!(m.html, "<article><b>theme &lt;x&gt;</b></article>");
    }

    #[test]
    fn site_include_overrides_theme() {
        match eval_with(&["theme", "site"], "card(\"y\")").unwrap() {
            Output::Html(m) => assert!(m.html.contains("<b>site y</b>"), "{m:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn mutual_recursion_is_a_clean_error() {
        let err = eval_with(&["loop"], "ping(0)").unwrap_err();
        assert_eq!(
            err.message,
            "include call depth limit (32) exceeded; an include probably calls itself"
        );
        assert_eq!(err.location.unwrap().line, 10);
    }

    #[test]
    fn include_syntax_error_names_file_and_line() {
        let mut engine = ScriptEngine::new(Path::new("."));
        let errs = engine.load_includes(&[fixture("broken")]).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].path.ends_with("bad.rhai"));
        match &errs[0].kind {
            ErrorKind::Script(e) => assert_eq!(e.location.unwrap().line, 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn runtime_error_in_include_has_a_frame() {
        let err = eval_with(&["errors"], "fail(1)").unwrap_err();
        assert_eq!(err.location.unwrap().line, 10);
        let frame = &err.stack[0];
        assert_eq!(frame.function, "fail");
        assert!(frame.file.ends_with("fail.rhai"));
        assert_eq!(frame.line, 3);
    }
}
