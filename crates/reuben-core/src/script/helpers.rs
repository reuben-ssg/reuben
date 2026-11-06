//! Helper functions exposed to scripts: `raw`, `escape`, `markdown` and `load_data`.

use std::path::{Path, PathBuf};

use rhai::{Dynamic, Engine, EvalAltResult};

use super::Html;
use crate::html::{escape_text, render};
use crate::markdown::parse;

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;

pub fn register(engine: &mut Engine, site_root: &Path) {
    let root: PathBuf = site_root.to_path_buf();
    engine
        .register_fn("raw", |s: &str| Html(s.to_string()))
        .register_fn("escape", |s: &str| escape_text(s).into_owned())
        // Never runs `{{ }}` blocks: it only parses and renders Markdown.
        .register_fn("markdown", |s: &str| Html(render(&parse(s, 1))))
        .register_fn("load_data", move |path: &str| load_data(&root, path));
}

fn load_data(root: &Path, path: &str) -> RhaiResult<Dynamic> {
    let fail = |msg: String| -> Box<EvalAltResult> { msg.into() };
    let canonical_root = root
        .canonicalize()
        .map_err(|e| fail(format!("load_data: can't read the site root: {e}")))?;
    let full = canonical_root
        .join(path)
        .canonicalize()
        .map_err(|e| fail(format!("load_data(\"{path}\"): {e}")))?;
    if !full.starts_with(&canonical_root) {
        return Err(fail(format!(
            "load_data(\"{path}\"): path is outside the site"
        )));
    }
    let text =
        std::fs::read_to_string(&full).map_err(|e| fail(format!("load_data(\"{path}\"): {e}")))?;
    let parse_err = |e: String| fail(format!("load_data(\"{path}\"): {e}"));
    let value: serde_json::Value = match full.extension().and_then(|e| e.to_str()) {
        Some("json") => serde_json::from_str(&text).map_err(|e| parse_err(e.to_string()))?,
        Some("toml") => toml::from_str(&text).map_err(|e| parse_err(e.to_string()))?,
        Some("yaml" | "yml") => serde_saphyr::from_str_with_options(
            &text,
            serde_saphyr::options! { strict_booleans: true },
        )
        .map_err(|e| parse_err(e.to_string()))?,
        _ => {
            return Err(fail(format!(
                "load_data(\"{path}\"): only .json, .toml, .yaml and .yml files can be loaded"
            )));
        }
    };
    rhai::serde::to_dynamic(value)
}

#[cfg(test)]
mod tests {
    use assert_fs::prelude::*;
    use rhai::Dynamic;
    use std::path::Path;

    use crate::script::ScriptEngine;

    fn run(engine: &ScriptEngine, src: &str) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
        engine.engine.eval::<Dynamic>(src)
    }

    #[test]
    fn markdown_helper_renders_and_does_not_run_blocks() {
        let engine = ScriptEngine::new(Path::new("."));
        let v = run(&engine, "markdown(\"*a* {{ 1 }}\").to_string()").unwrap();
        assert_eq!(v.into_string().unwrap(), "<p><em>a</em> {{ 1 }}</p>\n");
    }

    #[test]
    fn load_data_reads_inside_root() {
        let dir = assert_fs::TempDir::new().unwrap();
        dir.child("data/x.json").write_str("{\"n\": 1}").unwrap();
        let engine = ScriptEngine::new(dir.path());
        let v = run(&engine, "load_data(\"data/x.json\").n").unwrap();
        assert_eq!(v.as_int().unwrap(), 1);
    }

    #[test]
    fn load_data_yaml_only_treats_true_and_false_as_booleans() {
        let dir = assert_fs::TempDir::new().unwrap();
        dir.child("data/x.yaml").write_str("answer: yes\n").unwrap();
        let engine = ScriptEngine::new(dir.path());
        let v = run(&engine, "load_data(\"data/x.yaml\").answer").unwrap();
        assert_eq!(v.into_string().unwrap(), "yes");
    }

    #[test]
    fn load_data_refuses_outside_root() {
        let dir = assert_fs::TempDir::new().unwrap();
        dir.child("site").create_dir_all().unwrap();
        dir.child("secret.json").write_str("{\"n\": 1}").unwrap();
        let engine = ScriptEngine::new(&dir.path().join("site"));
        let err = run(&engine, "load_data(\"../secret.json\")").unwrap_err();
        assert!(err.to_string().contains("outside the site"), "{err}");
        let abs = dir.path().join("secret.json");
        let src = format!("load_data({:?})", abs.to_str().unwrap());
        let err = run(&engine, &src).unwrap_err();
        assert!(err.to_string().contains("outside the site"), "{err}");
    }
}
