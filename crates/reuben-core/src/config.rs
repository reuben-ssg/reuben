//! Site configuration from `reuben.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use reuben_a11y::RuleSet;
use serde_json::{Map, Value};

use crate::error::{Error, ErrorKind};
use crate::merge::{Layered, toml_to_json};

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub title: String,
    pub base_url: String,
    pub theme: Option<PathBuf>,
    pub params: toml::Table,
    pub collections: BTreeMap<String, CollectionConfig>,
    pub sitemap: bool,
    pub css: CssConfig,
    pub markdown: MarkdownConfig,
    pub assets: AssetsConfig,
    /// The accessibility rules that are on for every page, before a page's own switches.
    pub a11y: RuleSet,
    pub sources: BTreeMap<String, PathBuf>,
}

/// The `[markdown]` table: every byte Reuben adds beyond the Markdown's own HTML is opt-in, except the accessibility
/// features `footnote_backlinks` and `focusable_scroll`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MarkdownConfig {
    pub highlight: bool,
    pub math: bool,
    /// Turning this off removes an accessibility feature.
    pub footnote_backlinks: bool,
    /// Makes each code block's `<pre>`, and a `<div class="table-wrap">` around each table, a named
    /// `role="region"` with `tabindex="0"`, so keyboard users can scroll them in every browser. Turning this off
    /// removes an accessibility feature.
    pub focusable_scroll: bool,
}

impl Default for MarkdownConfig {
    fn default() -> Self {
        Self {
            highlight: false,
            math: false,
            footnote_backlinks: true,
            focusable_scroll: true,
        }
    }
}

/// The `[a11y]` table: rules switched off site-wide, by id.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct A11yConfig {
    pub disable: Vec<String>,
}

/// The `[css]` table: how every CSS file in the output is processed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CssConfig {
    pub minify: bool,
    /// A Browserslist query naming the browsers CSS is lowered for.
    pub targets: String,
    /// Inline local `@import`s.
    pub bundle: bool,
}

impl Default for CssConfig {
    fn default() -> Self {
        Self {
            minify: true,
            targets: "defaults".into(),
            bundle: true,
        }
    }
}

/// The `[assets]` table: opt-in content-hashed names for static files.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AssetsConfig {
    pub hash: bool,
    /// Emit a `_headers` file with long-lived caching for hashed files; needs `hash`.
    pub headers: bool,
    /// Globs of files that keep their original names.
    pub keep: Vec<String>,
}

impl AssetsConfig {
    /// The `keep` globs that parse; `load` reports the ones that don't.
    pub fn keep_patterns(&self) -> Vec<glob::Pattern> {
        self.keep
            .iter()
            .filter_map(|g| glob::Pattern::new(g).ok())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionConfig {
    pub name: Option<String>,
    pub layout: Option<String>,
    pub sort: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    title: Option<String>,
    base_url: Option<String>,
    theme: Option<PathBuf>,
    #[serde(default)]
    params: toml::Table,
    #[serde(default)]
    collections: BTreeMap<String, CollectionConfig>,
    sitemap: Option<bool>,
    #[serde(default)]
    css: CssConfig,
    #[serde(default)]
    markdown: MarkdownConfig,
    #[serde(default)]
    assets: AssetsConfig,
    #[serde(default)]
    a11y: A11yConfig,
}

fn read_layer(path: &Path) -> Result<Option<Map<String, Value>>, Error> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::new(path, e)),
    };
    let table = toml::from_str::<toml::Table>(&text)
        .map_err(|e| Error::new(path, ErrorKind::Config(e.to_string())))?;
    match toml_to_json(toml::Value::Table(table)) {
        Value::Object(map) => Ok(Some(map)),
        _ => unreachable!("a TOML table converts to a JSON object"),
    }
}

/// Merges `reuben.toml`, its `include` globs, then `reuben.<env>.toml`.
/// A missing `reuben.toml` or env file is fine; unknown keys in the merged result are errors so typos surface.
pub fn load(root: &Path, env: Option<&str>) -> Result<Config, Error> {
    let path = root.join("reuben.toml");
    let mut layered = Layered::default();
    let mut includes = Vec::new();

    if let Some(mut base) = read_layer(&path)? {
        if let Some(globs) = base.remove("include") {
            includes = serde_json::from_value::<Vec<String>>(globs).map_err(|e| {
                Error::new(
                    &path,
                    ErrorKind::Config(format!("`include` must be a list of globs: {e}")),
                )
            })?;
        }
        layered.merge(base, &path);
    }

    for pattern in &includes {
        for file in expand_glob(root, pattern, &path)? {
            merge_layer(&mut layered, &file)?;
        }
    }
    if let Some(env) = env {
        merge_layer(&mut layered, &root.join(format!("reuben.{env}.toml")))?;
    }

    let raw: Raw = serde_json::from_value(Value::Object(layered.value.clone())).map_err(|e| {
        let blamed = blame(&layered, &e.to_string()).unwrap_or_else(|| path.clone());
        Error::new(blamed, ErrorKind::Config(e.to_string()))
    })?;
    let theme = raw.theme.map(|t| root.join(t));
    if let Some(theme) = &theme
        && !theme.is_dir()
    {
        return Err(Error::new(
            &path,
            ErrorKind::Config(format!("theme `{}` is not a directory", theme.display())),
        ));
    }
    if let Err(message) = crate::css::targets(&raw.css.targets) {
        let blamed = layered.sources.get("css.targets").unwrap_or(&path);
        return Err(Error::new(
            blamed,
            ErrorKind::Config(format!("css.targets: {message}")),
        ));
    }
    let (a11y, unknown) = crate::check::switch_off(RuleSet::all(), &raw.a11y.disable);
    if !unknown.is_empty() {
        let blamed = layered.sources.get("a11y.disable").unwrap_or(&path);
        let message = unknown
            .iter()
            .map(|id| format!("unknown accessibility rule `{id}`"))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Error::new(blamed, ErrorKind::Config(message)));
    }
    let mut problems = Vec::new();
    if raw.assets.headers && !raw.assets.hash {
        problems.push("assets: `headers = true` needs `hash = true`".to_string());
    }
    if raw.assets.hash && !raw.css.bundle {
        problems.push("assets: `hash = true` needs `[css] bundle = true`".to_string());
    }
    for g in &raw.assets.keep {
        if let Err(e) = glob::Pattern::new(g) {
            problems.push(format!("assets.keep: invalid glob `{g}`: {e}"));
        }
    }
    if !problems.is_empty() {
        let blamed = layered
            .sources
            .get("assets.keep")
            .or_else(|| layered.sources.get("assets.headers"))
            .or_else(|| layered.sources.get("assets.hash"))
            .unwrap_or(&path);
        return Err(Error::new(blamed, ErrorKind::Config(problems.join("; "))));
    }
    Ok(Config {
        title: raw.title.unwrap_or_default(),
        base_url: raw.base_url.unwrap_or_else(|| "/".into()),
        theme,
        params: raw.params,
        collections: raw.collections,
        sitemap: raw.sitemap.unwrap_or(false),
        css: raw.css,
        markdown: raw.markdown,
        assets: raw.assets,
        a11y,
        sources: layered.sources,
    })
}

/// Finds the file that set the key an "unknown field" error names, at the top level or inside any table, including a collection's.
fn blame(layered: &Layered, message: &str) -> Option<PathBuf> {
    let key = message.strip_prefix("unknown field `")?.split('`').next()?;
    let dotted = if layered.value.contains_key(key) {
        key.to_string()
    } else {
        nested_path(&layered.value, key)?
    };
    let nested = format!("{dotted}.");
    layered
        .sources
        .iter()
        .find(|(k, _)| **k == dotted || k.starts_with(&nested))
        .map(|(_, file)| file.clone())
}

fn nested_path(value: &Map<String, Value>, key: &str) -> Option<String> {
    for (table, body) in value {
        let Some(body) = body.as_object() else {
            continue;
        };
        if table == "collections" {
            if let Some((name, _)) = body.iter().find(|(_, t)| t.get(key).is_some()) {
                return Some(format!("collections.{name}.{key}"));
            }
        } else if table != "params" && body.contains_key(key) {
            return Some(format!("{table}.{key}"));
        }
    }
    None
}

fn merge_layer(layered: &mut Layered, file: &Path) -> Result<(), Error> {
    if let Some(layer) = read_layer(file)? {
        if layer.contains_key("include") {
            return Err(Error::new(
                file,
                ErrorKind::Config("`include` is only allowed in `reuben.toml`".into()),
            ));
        }
        layered.merge(layer, file);
    }
    Ok(())
}

/// Matches are sorted by path so merge order doesn't depend on the filesystem.
fn expand_glob(root: &Path, pattern: &str, config: &Path) -> Result<Vec<PathBuf>, Error> {
    let bad = |msg: String| Error::new(config, ErrorKind::Config(msg));
    // Only the user's pattern is a glob; the root may contain `[` or `*`.
    let full = format!(
        "{}/{pattern}",
        glob::Pattern::escape(&root.to_string_lossy())
    );
    let paths =
        glob::glob(&full).map_err(|e| bad(format!("invalid include glob `{pattern}`: {e}")))?;
    let mut files = Vec::new();
    for entry in paths {
        let file = entry.map_err(|e| bad(format!("reading include `{pattern}`: {e}")))?;
        if file.is_file() {
            files.push(file);
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    #[test]
    fn css_config_defaults_and_validation() {
        let tmp = TempDir::new().unwrap();
        let c = load(tmp.path(), None).unwrap();
        assert_eq!(c.css, CssConfig::default());
        assert!(c.css.minify && c.css.bundle);
        assert_eq!(c.css.targets, "defaults");

        tmp.child("reuben.toml")
            .write_str("[css]\nminify = false\ntargets = \"last 2 versions\"\n")
            .unwrap();
        let c = load(tmp.path(), None).unwrap();
        assert!(!c.css.minify && c.css.bundle);
        assert_eq!(c.css.targets, "last 2 versions");

        tmp.child("reuben.toml")
            .write_str("[css]\ntargets = \"not a query ###\"\n")
            .unwrap();
        let err = load(tmp.path(), None).unwrap_err();
        assert!(err.path.ends_with("reuben.toml"), "{err}");
        assert!(err.to_string().contains("targets"), "{err}");

        tmp.child("reuben.toml")
            .write_str("[css]\nminfy = true\n")
            .unwrap();
        assert!(load(tmp.path(), None).is_err());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let tmp = TempDir::new().unwrap();
        let c = load(tmp.path(), None).unwrap();
        assert_eq!(c.title, "");
        assert_eq!(c.base_url, "/");
        assert_eq!(c.theme, None);
        assert!(c.params.is_empty());
    }

    #[test]
    fn reads_keys_and_resolves_theme() {
        let tmp = TempDir::new().unwrap();
        tmp.child("themes/a").create_dir_all().unwrap();
        tmp.child("reuben.toml")
            .write_str("title = \"T\"\nbase_url = \"/x/\"\ntheme = \"themes/a\"\n[params]\nk = 1\n")
            .unwrap();
        let c = load(tmp.path(), None).unwrap();
        assert_eq!(c.title, "T");
        assert_eq!(c.base_url, "/x/");
        assert_eq!(c.theme, Some(tmp.path().join("themes/a")));
        assert_eq!(c.params["k"].as_integer(), Some(1));
    }

    #[test]
    fn unknown_key_names_it() {
        let tmp = TempDir::new().unwrap();
        tmp.child("reuben.toml")
            .write_str("titel = \"x\"\n")
            .unwrap();
        let err = load(tmp.path(), None).unwrap_err();
        assert!(err.to_string().contains("titel"), "{err}");
    }

    fn write(tmp: &TempDir, name: &str, text: &str) {
        tmp.child(name).write_str(text).unwrap();
    }

    #[test]
    fn includes_merge_in_order_after_base() {
        let tmp = TempDir::new().unwrap();
        write(
            &tmp,
            "reuben.toml",
            "title = \"A\"\ninclude = [\"config/*.toml\"]\n[params]\na = 1\n",
        );
        write(&tmp, "config/1.toml", "[params]\na = 2\nb = 1\n");
        write(&tmp, "config/2.toml", "[params]\nb = 2\n");
        let c = load(tmp.path(), None).unwrap();
        assert_eq!(c.params["a"].as_integer(), Some(2));
        assert_eq!(c.params["b"].as_integer(), Some(2));
        assert!(c.sources["params.a"].ends_with("config/1.toml"));
        assert!(c.sources["params.b"].ends_with("config/2.toml"));
    }

    #[test]
    fn env_overlay_merges_last() {
        let tmp = TempDir::new().unwrap();
        write(&tmp, "reuben.toml", "base_url = \"/\"\n");
        write(
            &tmp,
            "reuben.production.toml",
            "base_url = \"https://x.dev/\"\n",
        );
        assert_eq!(
            load(tmp.path(), Some("production")).unwrap().base_url,
            "https://x.dev/"
        );
        assert_eq!(load(tmp.path(), None).unwrap().base_url, "/");
    }

    #[test]
    fn missing_env_file_is_fine() {
        let tmp = TempDir::new().unwrap();
        write(&tmp, "reuben.toml", "title = \"A\"\n");
        assert!(load(tmp.path(), Some("staging")).is_ok());
    }

    #[test]
    fn include_in_included_file_is_an_error() {
        let tmp = TempDir::new().unwrap();
        write(&tmp, "reuben.toml", "include = [\"config/*.toml\"]\n");
        write(&tmp, "config/1.toml", "include = [\"other/*.toml\"]\n");
        let err = load(tmp.path(), None).unwrap_err();
        assert!(err.path.ends_with("config/1.toml"), "{err}");
    }

    #[test]
    fn collection_settings_parse() {
        let tmp = TempDir::new().unwrap();
        write(
            &tmp,
            "reuben.toml",
            "[collections.writing-guide]\nname = \"guide\"\nlayout = \"doc\"\nsort = \"-date\"\n",
        );
        let c = load(tmp.path(), None).unwrap();
        let g = &c.collections["writing-guide"];
        assert_eq!(g.name.as_deref(), Some("guide"));
        assert_eq!(g.layout.as_deref(), Some("doc"));
        assert_eq!(g.sort.as_deref(), Some("-date"));
    }

    #[test]
    fn unknown_collection_key_is_an_error() {
        let tmp = TempDir::new().unwrap();
        write(&tmp, "reuben.toml", "[collections.blog]\nsrot = \"date\"\n");
        let err = load(tmp.path(), None).unwrap_err();
        assert!(err.to_string().contains("srot"), "{err}");
    }

    #[test]
    fn glob_metacharacters_in_the_root_are_literal() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.child("my[site]");
        root.create_dir_all().unwrap();
        root.child("reuben.toml")
            .write_str("include = [\"config/*.toml\"]\n")
            .unwrap();
        root.child("config/1.toml")
            .write_str("[params]\na = 1\n")
            .unwrap();
        let c = load(root.path(), None).unwrap();
        assert_eq!(c.params["a"].as_integer(), Some(1));
    }

    #[test]
    fn unknown_key_in_included_file_blames_that_file() {
        let tmp = TempDir::new().unwrap();
        write(&tmp, "reuben.toml", "include = [\"config/*.toml\"]\n");
        write(&tmp, "config/1.toml", "titel = \"x\"\n");
        let err = load(tmp.path(), None).unwrap_err();
        assert!(err.path.ends_with("config/1.toml"), "{err}");
        assert!(err.to_string().contains("titel"), "{err}");
    }

    #[test]
    fn unknown_collection_key_in_env_file_blames_that_file() {
        let tmp = TempDir::new().unwrap();
        write(&tmp, "reuben.toml", "title = \"A\"\n");
        write(
            &tmp,
            "reuben.production.toml",
            "[collections.blog]\nsrot = \"date\"\n",
        );
        let err = load(tmp.path(), Some("production")).unwrap_err();
        assert!(err.path.ends_with("reuben.production.toml"), "{err}");
        assert!(err.to_string().contains("srot"), "{err}");
    }

    #[test]
    fn sitemap_defaults_off() {
        let tmp = TempDir::new().unwrap();
        assert!(!load(tmp.path(), None).unwrap().sitemap);
        write(&tmp, "reuben.toml", "sitemap = true\n");
        assert!(load(tmp.path(), None).unwrap().sitemap);
    }

    #[test]
    fn markdown_defaults() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(
            load(tmp.path(), None).unwrap().markdown,
            MarkdownConfig {
                highlight: false,
                math: false,
                footnote_backlinks: true,
                focusable_scroll: true,
            }
        );
    }

    #[test]
    fn markdown_table_parses() {
        let tmp = TempDir::new().unwrap();
        write(
            &tmp,
            "reuben.toml",
            "[markdown]\nhighlight = true\nmath = true\nfootnote_backlinks = false\nfocusable_scroll = false\n",
        );
        assert_eq!(
            load(tmp.path(), None).unwrap().markdown,
            MarkdownConfig {
                highlight: true,
                math: true,
                footnote_backlinks: false,
                focusable_scroll: false,
            }
        );
    }

    #[test]
    fn markdown_unknown_key_is_an_error() {
        let tmp = TempDir::new().unwrap();
        write(&tmp, "reuben.toml", "[markdown]\nmaths = true\n");
        let err = load(tmp.path(), None).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::Config(_)), "{err}");
        assert!(err.path.ends_with("reuben.toml"), "{err}");
        assert!(err.to_string().contains("maths"), "{err}");
    }

    #[test]
    fn unknown_markdown_key_in_env_file_blames_that_file() {
        let tmp = TempDir::new().unwrap();
        write(&tmp, "reuben.toml", "title = \"A\"\n");
        write(&tmp, "reuben.production.toml", "[markdown]\nmaths = true\n");
        let err = load(tmp.path(), Some("production")).unwrap_err();
        assert!(err.path.ends_with("reuben.production.toml"), "{err}");
        assert!(err.to_string().contains("maths"), "{err}");
    }

    #[test]
    fn unknown_markdown_key_in_included_file_blames_that_file() {
        let tmp = TempDir::new().unwrap();
        write(&tmp, "reuben.toml", "include = [\"config/*.toml\"]\n");
        write(&tmp, "config/1.toml", "[markdown]\nmaths = true\n");
        let err = load(tmp.path(), None).unwrap_err();
        assert!(err.path.ends_with("config/1.toml"), "{err}");
    }

    #[test]
    fn assets_defaults_off() {
        let tmp = TempDir::new().unwrap();
        let c = load(tmp.path(), None).unwrap();
        assert!(!c.assets.hash && !c.assets.headers && c.assets.keep.is_empty());
        assert!(c.assets.keep_patterns().is_empty());
    }

    #[test]
    fn assets_table_parses() {
        let tmp = TempDir::new().unwrap();
        write(
            &tmp,
            "reuben.toml",
            "[assets]\nhash = true\nheaders = true\nkeep = [\"favicon.*\"]\n",
        );
        let c = load(tmp.path(), None).unwrap();
        assert!(c.assets.hash && c.assets.headers);
        assert_eq!(c.assets.keep_patterns().len(), 1);
        assert!(c.assets.keep_patterns()[0].matches("favicon.ico"));
    }

    #[test]
    fn assets_headers_without_hash_is_an_error() {
        let tmp = TempDir::new().unwrap();
        write(&tmp, "reuben.toml", "[assets]\nheaders = true\n");
        let err = load(tmp.path(), None).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::Config(_)), "{err}");
        assert!(
            err.to_string()
                .contains("`headers = true` needs `hash = true`"),
            "{err}"
        );
    }

    #[test]
    fn assets_hash_needs_css_bundle() {
        let tmp = TempDir::new().unwrap();
        write(
            &tmp,
            "reuben.toml",
            "[css]\nbundle = false\n[assets]\nhash = true\n",
        );
        let err = load(tmp.path(), None).unwrap_err();
        assert!(
            err.to_string()
                .contains("`hash = true` needs `[css] bundle = true`"),
            "{err}"
        );
    }

    #[test]
    fn assets_bad_keep_glob_is_an_error() {
        let tmp = TempDir::new().unwrap();
        write(&tmp, "reuben.toml", "[assets]\nkeep = [\"[x\"]\n");
        let err = load(tmp.path(), None).unwrap_err();
        assert!(err.to_string().contains("[x"), "{err}");
    }

    #[test]
    fn assets_errors_are_all_reported() {
        let tmp = TempDir::new().unwrap();
        write(
            &tmp,
            "reuben.toml",
            "[assets]\nheaders = true\nkeep = [\"[x\"]\n",
        );
        let text = load(tmp.path(), None).unwrap_err().to_string();
        assert!(
            text.contains("`headers = true` needs `hash = true`"),
            "{text}"
        );
        assert!(text.contains("[x"), "{text}");
    }

    #[test]
    fn unknown_css_and_a11y_keys_blame_the_file_that_set_them() {
        let tmp = TempDir::new().unwrap();
        write(&tmp, "reuben.toml", "title = \"A\"\n");
        write(&tmp, "reuben.production.toml", "[css]\ntargts = []\n");
        let err = load(tmp.path(), Some("production")).unwrap_err();
        assert!(err.path.ends_with("reuben.production.toml"), "{err}");
        write(&tmp, "reuben.production.toml", "[a11y]\ndisabel = []\n");
        let err = load(tmp.path(), Some("production")).unwrap_err();
        assert!(err.path.ends_with("reuben.production.toml"), "{err}");
    }
}
