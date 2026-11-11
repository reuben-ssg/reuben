//! Helper functions exposed to scripts: `raw`, `escape`, `markdown`, `sort_by`, `load_data`, `url_for` and `format_date`.

use std::path::{Path, PathBuf};

use rhai::{Array, Dynamic, Engine, EvalAltResult, NativeCallContext};

use super::{Html, IncludeFiles, collection};
use crate::doc::Origin;
use crate::html::{escape_text, render};
use crate::markdown::parse;
use crate::math::render_math;

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;

/// The include file and line a native function was called from, or `None` when the caller isn't an include.
fn origin_of(ctx: &NativeCallContext, files: &IncludeFiles) -> Option<Origin> {
    let source = ctx.call_source()?;
    let known = files.read().is_ok_and(|f| f.contains(source));
    known.then(|| Origin {
        file: PathBuf::from(source),
        line: ctx.call_position().line().unwrap_or(0),
    })
}

pub fn register(engine: &mut Engine, site_root: &Path, include_files: &IncludeFiles) {
    let root: PathBuf = site_root.to_path_buf();
    let (raw_files, markdown_files) = (include_files.clone(), include_files.clone());
    engine
        .register_fn("raw", move |ctx: NativeCallContext, s: &str| Html {
            markup: s.to_string(),
            origin: origin_of(&ctx, &raw_files),
        })
        .register_fn("escape", |s: &str| escape_text(s).into_owned())
        // Never runs `{{ }}` blocks: it only parses and renders Markdown.
        .register_fn(
            "markdown",
            move |ctx: NativeCallContext, s: &str| -> RhaiResult<Html> {
                let mut doc = parse(s, 1);
                render_math(&mut doc).map_err(|errors| {
                    let (span, message) = &errors[0];
                    format!("math at {}:{}: {message}", span.line, span.column)
                })?;
                Ok(Html {
                    markup: render(&doc),
                    origin: origin_of(&ctx, &markdown_files),
                })
            },
        )
        .register_fn("sort_by", |items: &mut Array, key: &str| {
            collection::sorted(items, key)
        })
        .register_fn("format_date", format_date)
        .register_fn("load_data", move |path: &str| load_data(&root, path));
    register_url_for(engine, "/");
}

/// Registering again replaces the earlier `url_for`, so this is also how the real `base_url` is applied.
pub fn register_url_for(engine: &mut Engine, base_url: &str) {
    let base = base_url.to_string();
    engine.register_fn("url_for", move |path: &str| url_for(&base, path));
}

pub fn url_for(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn format_date(date: &str, fmt: &str) -> RhaiResult<String> {
    let fail = |e: jiff::Error| -> Box<EvalAltResult> {
        format!("format_date(\"{date}\", \"{fmt}\"): {e}").into()
    };
    let bad = || -> Box<EvalAltResult> {
        format!("format_date: can't read \"{date}\" as an RFC 3339 timestamp, datetime or date")
            .into()
    };
    if let Ok(ts) = date.parse::<jiff::Timestamp>() {
        // Format in the offset the author wrote so the shown date is the written one; `Z` is UTC.
        let offset = jiff::fmt::temporal::Pieces::parse(date)
            .ok()
            .and_then(|p| p.to_numeric_offset())
            .unwrap_or(jiff::tz::Offset::UTC);
        return jiff::fmt::strtime::format(fmt, &ts.to_zoned(jiff::tz::TimeZone::fixed(offset)))
            .map_err(fail);
    }
    if let Ok(dt) = date.parse::<jiff::civil::DateTime>() {
        return jiff::fmt::strtime::format(fmt, dt).map_err(fail);
    }
    if let Ok(d) = date.parse::<jiff::civil::Date>() {
        return jiff::fmt::strtime::format(fmt, d).map_err(fail);
    }
    Err(bad())
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

    #[test]
    fn url_for_joins_base_url() {
        let mut engine = ScriptEngine::new(Path::new("."));
        assert_eq!(
            run(&engine, "url_for(\"/about/\")")
                .unwrap()
                .into_string()
                .unwrap(),
            "/about/"
        );
        engine.set_base_url("https://x.dev/");
        assert_eq!(
            run(&engine, "url_for(\"/about/\")")
                .unwrap()
                .into_string()
                .unwrap(),
            "https://x.dev/about/"
        );
    }

    #[test]
    fn format_date_formats_dates() {
        let engine = ScriptEngine::new(Path::new("."));
        let v = run(&engine, "format_date(\"2024-03-01\", \"%B %Y\")").unwrap();
        assert_eq!(v.into_string().unwrap(), "March 2024");
        let v = run(&engine, "format_date(\"2024-03-01T10:00:00Z\", \"%Y\")").unwrap();
        assert_eq!(v.into_string().unwrap(), "2024");
    }

    #[test]
    fn format_date_keeps_the_offset() {
        let engine = ScriptEngine::new(Path::new("."));
        let v = run(
            &engine,
            "format_date(\"2024-03-01T01:00:00+02:00\", \"%d\")",
        )
        .unwrap();
        assert_eq!(v.into_string().unwrap(), "01");
        let v = run(
            &engine,
            "format_date(\"2024-03-01T23:30:00-05:00\", \"%Y-%m-%d\")",
        )
        .unwrap();
        assert_eq!(v.into_string().unwrap(), "2024-03-01");
    }

    #[test]
    fn format_date_bad_format_names_it() {
        let engine = ScriptEngine::new(Path::new("."));
        let err = run(&engine, "format_date(\"2024-03-01\", \"%Q\")").unwrap_err();
        assert!(err.to_string().contains("%Q"), "{err}");
    }

    #[test]
    fn format_date_rejects_garbage() {
        let engine = ScriptEngine::new(Path::new("."));
        let err = run(&engine, "format_date(\"soon\", \"%Y\")").unwrap_err();
        assert!(err.to_string().contains("soon"), "{err}");
    }
}
