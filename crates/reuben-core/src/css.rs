//! CSS processing with Lightning CSS: bundling local imports, minifying and lowering for browser targets.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Display;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use elsa::sync::FrozenMap;
use lightningcss::bundler::{BundleErrorKind, Bundler, ResolveResult, SourceProvider};
use lightningcss::stylesheet::{MinifyOptions, ParserOptions, PrinterOptions, StyleSheet};
use lightningcss::targets::{Browsers, Targets};

use crate::config::CssConfig;
use crate::doc::Span;
use crate::error::{Error, ErrorKind};

/// Resolves a Browserslist query, caching the answer because every CSS file in a build asks for the same one.
pub(crate) fn targets(query: &str) -> Result<Targets, String> {
    static CACHE: Mutex<BTreeMap<String, Result<Targets, String>>> = Mutex::new(BTreeMap::new());
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(hit) = cache.get(query) {
        return hit.clone();
    }
    let resolved = Browsers::from_browserslist([query])
        .map(|browsers| browsers.map(Targets::from).unwrap_or_default())
        .map_err(|e| e.to_string());
    cache.insert(query.to_string(), resolved.clone());
    resolved
}

/// The CSS templates of a build, keyed by [`normalize`]d source path, so an import gets a template's rendered
/// output instead of its file.
#[derive(Default)]
pub(crate) struct CssTemplates {
    pub(crate) rendered: HashMap<PathBuf, String>,
    /// Templates this build leaves out (drafts). Importing one is an error, since its file isn't CSS.
    pub(crate) unrendered: HashSet<PathBuf>,
}

/// Processes one stylesheet.
///
/// With `bundle` on and an `imports_root`, local `@import`s are inlined, resolved against the importing file and
/// never allowed to leave `imports_root`. Remote imports are left alone.
///
/// A file in `overrides` replaces the one at the same relative path under `imports_root`, the way a site's static
/// file replaces its theme's. Imports may reach either folder but nothing outside them.
pub(crate) fn process_with(
    source_path: &Path,
    css: &str,
    imports_root: Option<&Path>,
    overrides: Option<&Path>,
    cfg: &CssConfig,
    templates: &CssTemplates,
) -> Result<String, Error> {
    let fail = |message: String| {
        Error::new(
            source_path,
            ErrorKind::Css {
                message,
                location: None,
            },
        )
    };
    let targets = targets(&cfg.targets).map_err(|m| fail(format!("css.targets: {m}")))?;
    let entry = normalize(source_path);
    let bundle_root = imports_root.filter(|_| cfg.bundle);
    let provider = Provider {
        root: bundle_root.map(Base::new).unwrap_or_default(),
        overrides: overrides.map(Base::new),
        templates,
        sources: FrozenMap::new(),
        edges: Mutex::new(Vec::new()),
        entry: entry.clone(),
        entry_css: css,
    };
    let locate = |kind: &dyn Display, loc: Option<&lightningcss::error::ErrorLocation>| {
        let (path, location) = match loc {
            Some(loc) if Path::new(&loc.filename) == entry => (source_path.to_path_buf(), loc),
            Some(loc) if !loc.filename.is_empty() => {
                (provider.spelled(Path::new(&loc.filename)), loc)
            }
            Some(loc) => (source_path.to_path_buf(), loc),
            None => return fail(kind.to_string()),
        };
        Error::new(
            path,
            ErrorKind::Css {
                message: kind.to_string(),
                // Lightning CSS counts lines from 0 and columns from 1.
                location: Some(Span {
                    line: location.line as usize + 1,
                    column: location.column as usize,
                }),
            },
        )
    };

    let mut sheet = match bundle_root {
        Some(_) => {
            let options = ParserOptions {
                filename: entry.to_string_lossy().into_owned(),
                ..ParserOptions::default()
            };
            let mut bundler = Bundler::new(&provider, None, options);
            let sheet = bundler.bundle(&entry).map_err(|e| {
                if matches!(e.kind, BundleErrorKind::ExternalImportAfterBundledImport) {
                    let message = "remote `@import` must come before local `@import`s";
                    locate(&message, e.loc.as_ref())
                } else {
                    locate(&e.kind, e.loc.as_ref())
                }
            })?;
            if let Some(cycle) = provider.find_cycle() {
                let path = if cycle.importer == entry {
                    source_path.to_path_buf()
                } else {
                    provider.spelled(&cycle.importer)
                };
                return Err(Error::new(
                    path,
                    ErrorKind::Css {
                        message: format!("import cycle: {}", cycle.names),
                        location: cycle.location,
                    },
                ));
            }
            sheet
        }
        None => {
            let options = ParserOptions {
                filename: source_path.to_string_lossy().into_owned(),
                ..ParserOptions::default()
            };
            StyleSheet::parse(css, options).map_err(|e| locate(&e.kind, e.loc.as_ref()))?
        }
    };
    sheet
        .minify(MinifyOptions {
            targets,
            ..MinifyOptions::default()
        })
        .map_err(|e| locate(&e.kind, e.loc.as_ref()))?;
    let printed = sheet
        .to_css(PrinterOptions {
            minify: cfg.minify,
            targets,
            ..PrinterOptions::default()
        })
        .map_err(|e| locate(&e.kind, e.loc.as_ref()))?;
    Ok(printed.code)
}

#[derive(Debug)]
struct ProviderError(String);

impl Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProviderError {}

/// An `@import` loop, with the import that closes it.
struct Cycle {
    names: String,
    importer: PathBuf,
    location: Option<Span>,
}

/// One resolved local `@import`.
struct Edge {
    from: PathBuf,
    to: PathBuf,
    specifier: String,
}

/// A folder imports may come from, normalized for resolving and as the caller spelled it for error paths.
#[derive(Default)]
struct Base {
    norm: PathBuf,
    given: PathBuf,
}

impl Base {
    fn new(given: &Path) -> Self {
        Self {
            norm: normalize(given),
            given: given.to_path_buf(),
        }
    }
}

/// Serves the entry and rendered templates from memory and other imports from disk, refusing anything outside
/// `root` and `overrides`.
struct Provider<'a> {
    root: Base,
    /// Files here take the place of the same relative path under `root`.
    overrides: Option<Base>,
    templates: &'a CssTemplates,
    /// Files read from disk.
    sources: FrozenMap<PathBuf, String>,
    /// Every resolved `@import`, for cycle detection. Collected in parallel, so sorted before use.
    edges: Mutex<Vec<Edge>>,
    entry: PathBuf,
    entry_css: &'a str,
}

impl Provider<'_> {
    /// The folders imports may come from, the deepest first so a nested one wins.
    fn bases(&self) -> Vec<&Base> {
        let mut bases: Vec<&Base> = self.overrides.iter().collect();
        bases.push(&self.root);
        bases.sort_by_key(|b| std::cmp::Reverse(b.norm.components().count()));
        bases
    }

    fn display(&self, path: &Path) -> String {
        self.bases()
            .into_iter()
            .find_map(|b| path.strip_prefix(&b.norm).ok())
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// `path` (normalized) spelled the way the build walked it, so one file never shows up under two names.
    fn spelled(&self, path: &Path) -> PathBuf {
        self.bases()
            .into_iter()
            .find_map(|b| Some(b.given.join(path.strip_prefix(&b.norm).ok()?)))
            .unwrap_or_else(|| path.to_path_buf())
    }

    /// The text of a file already in memory.
    fn text(&self, file: &Path) -> Option<&str> {
        if file == self.entry {
            return Some(self.entry_css);
        }
        self.templates
            .rendered
            .get(file)
            .map(String::as_str)
            .or_else(|| self.sources.get(file))
    }

    fn find_cycle(&self) -> Option<Cycle> {
        let mut edges = self.edges.lock().unwrap_or_else(|e| e.into_inner());
        edges.sort_by(|a, b| (&a.from, &a.to, &a.specifier).cmp(&(&b.from, &b.to, &b.specifier)));
        let mut graph: HashMap<&Path, Vec<&Edge>> = HashMap::new();
        for edge in edges.iter() {
            graph.entry(&edge.from).or_default().push(edge);
        }
        let mut done = HashSet::new();
        let mut stack = Vec::new();
        self.visit(&self.entry, None, &graph, &mut stack, &mut done)
    }

    fn visit<'p>(
        &self,
        node: &'p Path,
        via: Option<&'p Edge>,
        graph: &HashMap<&'p Path, Vec<&'p Edge>>,
        stack: &mut Vec<&'p Path>,
        done: &mut HashSet<&'p Path>,
    ) -> Option<Cycle> {
        if let Some(at) = stack.iter().position(|p| *p == node) {
            let names: Vec<String> = stack[at..]
                .iter()
                .chain([&node])
                .map(|p| self.display(p))
                .collect();
            let closing = via?;
            return Some(Cycle {
                names: names.join(" -> "),
                importer: closing.from.clone(),
                location: self.import_location(&closing.from, &closing.specifier),
            });
        }
        if !done.insert(node) {
            return None;
        }
        stack.push(node);
        for edge in graph.get(node).into_iter().flatten() {
            if let Some(found) = self.visit(&edge.to, Some(edge), graph, stack, done) {
                return Some(found);
            }
        }
        stack.pop();
        None
    }

    /// The first `@import` line of `file` that mentions `specifier`.
    fn import_location(&self, file: &Path, specifier: &str) -> Option<Span> {
        let text = self.text(file)?;
        text.lines().enumerate().find_map(|(i, line)| {
            let at = line.find("@import")?;
            line.contains(specifier).then_some(Span {
                line: i + 1,
                column: line[..at].chars().count() + 1,
            })
        })
    }
}

impl SourceProvider for Provider<'_> {
    type Error = ProviderError;

    fn read<'s>(&'s self, file: &Path) -> Result<&'s str, ProviderError> {
        if let Some(text) = self.text(file) {
            return Ok(text);
        }
        if self.templates.unrendered.contains(file) {
            return Err(ProviderError(format!(
                "imports `{}`, a template that isn't rendered (draft)",
                self.display(file)
            )));
        }
        let text = std::fs::read_to_string(file)
            .map_err(|e| ProviderError(format!("can't read `{}`: {e}", self.display(file))))?;
        Ok(self.sources.insert(file.to_path_buf(), text))
    }

    fn resolve(&self, specifier: &str, originating: &Path) -> Result<ResolveResult, ProviderError> {
        let lower = specifier.to_ascii_lowercase();
        if ["http://", "https://", "//", "data:"]
            .iter()
            .any(|p| lower.starts_with(p))
        {
            return Ok(ResolveResult::External(specifier.to_string()));
        }
        let outside = || {
            ProviderError(format!(
                "import `{specifier}` is outside the stylesheet folder"
            ))
        };
        let resolved = normalize(&originating.with_file_name(specifier));
        let rel = self
            .bases()
            .into_iter()
            .find_map(|b| resolved.strip_prefix(&b.norm).ok())
            .ok_or_else(outside)?;
        let (target, base) = match &self.overrides {
            Some(over) if over.norm.join(rel).is_file() => (over.norm.join(rel), &over.norm),
            _ => (self.root.norm.join(rel), &self.root.norm),
        };
        // A symlink inside a root must not lead out of it either.
        if let (Ok(real), Ok(base)) = (target.canonicalize(), base.canonicalize())
            && !real.starts_with(base)
        {
            return Err(outside());
        }
        self.edges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Edge {
                from: originating.to_path_buf(),
                to: target.clone(),
                specifier: specifier.to_string(),
            });
        Ok(ResolveResult::File(target))
    }
}

/// Resolves `.` and `..` without touching the disk, so a missing file can still be confined to the root.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

pub(crate) fn is_css(rel: &Path) -> bool {
    rel.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("css"))
}

/// `_name.css` files can be imported but are never written out.
pub(crate) fn is_partial_css(rel: &Path) -> bool {
    is_css(rel)
        && rel
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('_'))
}

/// Where `@import`s in the CSS file `src` may reach: the folder it lives in the site, theme or content tree.
/// A theme stylesheet also gets the site's `static/` as overrides, since a site file replaces the theme's.
pub(crate) fn css_root(
    site_root: &Path,
    theme: Option<&Path>,
    src: &Path,
    template_from_static: bool,
) -> (PathBuf, Option<PathBuf>) {
    let site_static = site_root.join("static");
    if template_from_static {
        return (site_static, None);
    }
    let theme_static = theme.map(|t| t.join("static"));
    match theme_static {
        Some(dir) if src.starts_with(&dir) => (dir, Some(site_static)),
        _ if src.starts_with(&site_static) => (site_static, None),
        _ => (site_root.join("content"), None),
    }
}

pub(crate) enum CssInput {
    /// Template output, already rendered.
    Text(String),
    /// Read from `CssJob::path`.
    File,
}

/// One stylesheet to process during a build.
pub(crate) struct CssJob {
    pub(crate) rel: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) overrides: Option<PathBuf>,
    pub(crate) input: CssInput,
}

impl CssJob {
    pub(crate) fn run(&self, cfg: &CssConfig, templates: &CssTemplates) -> Result<String, Error> {
        let text;
        let (css, is_template) = match &self.input {
            CssInput::Text(t) => (t.as_str(), true),
            CssInput::File => {
                let bytes = std::fs::read(&self.path).map_err(|e| Error::new(&self.path, e))?;
                text = String::from_utf8(bytes)
                    .map_err(|_| Error::new(&self.path, ErrorKind::NotUtf8))?;
                (text.as_str(), false)
            }
        };
        process_with(
            &self.path,
            css,
            Some(&self.root),
            self.overrides.as_deref(),
            cfg,
            templates,
        )
        .map_err(|mut e| {
            // A template's line numbers describe its rendered text, not the file on disk, so the error drops
            // them and quotes the offending rendered line instead.
            let rendered = if is_template && e.path == self.path {
                Some(css)
            } else {
                templates
                    .rendered
                    .get(&normalize(&e.path))
                    .map(String::as_str)
            };
            if let (Some(rendered), ErrorKind::Css { message, location }) = (rendered, &mut e.kind)
                && let Some(at) = location.take()
                && let Some(line) = rendered.lines().nth(at.line.saturating_sub(1))
            {
                message.push_str(&format!(" (in the rendered output: `{}`)", line.trim()));
            }
            e
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    fn process(
        source_path: &Path,
        css: &str,
        imports_root: Option<&Path>,
        cfg: &CssConfig,
    ) -> Result<String, Error> {
        process_with(
            source_path,
            css,
            imports_root,
            None,
            cfg,
            &CssTemplates::default(),
        )
    }

    fn run(tmp: &TempDir, rel: &str, css: &str) -> Result<String, Error> {
        process(
            &tmp.path().join("static").join(rel),
            css,
            Some(&tmp.path().join("static")),
            &CssConfig::default(),
        )
    }

    fn css_error(err: &Error) -> (String, Option<(usize, usize)>) {
        match &err.kind {
            ErrorKind::Css { message, location } => {
                (message.clone(), location.map(|s| (s.line, s.column)))
            }
            other => panic!("not a CSS error: {other:?}"),
        }
    }

    #[test]
    fn minifies_and_lowers_nesting() {
        let input = ".a { color: red; .b { color: blue } }";
        let out = process(
            Path::new("a.css"),
            input,
            None,
            &CssConfig {
                targets: "chrome 90".into(),
                ..CssConfig::default()
            },
        )
        .unwrap();
        assert!(out.contains(".a .b"), "{out}");
        assert!(!out.contains('\n') && !out.contains("  "), "{out}");
        assert!(out.len() < input.len(), "{out}");
    }

    #[test]
    fn bundles_local_imports_and_skips_remote() {
        let tmp = TempDir::new().unwrap();
        let main = "@import url(https://fonts.example/x.css) print;\n@import \"_vars.css\";\nbody{color:var(--c)}";
        tmp.child("static/main.css").write_str(main).unwrap();
        tmp.child("static/_vars.css")
            .write_str(":root{--c:red}")
            .unwrap();
        let out = run(&tmp, "main.css", main).unwrap();
        assert!(out.contains("--c:red"), "{out}");
        let remote = out.split(';').next().unwrap();
        assert!(
            remote.starts_with("@import") && remote.contains("https://fonts.example/x.css"),
            "{out}"
        );
        assert!(remote.ends_with("print"), "{out}");
    }

    #[test]
    fn remote_import_after_a_local_one_is_a_located_error() {
        let tmp = TempDir::new().unwrap();
        tmp.child("static/_vars.css")
            .write_str(":root{--c:red}")
            .unwrap();
        let main = "@import \"_vars.css\";\n@import url(https://fonts.example/x.css);\nbody{}";
        let err = run(&tmp, "main.css", main).unwrap_err();
        assert!(err.path.ends_with("main.css"), "{err}");
        let (message, location) = css_error(&err);
        assert_eq!(
            message,
            "remote `@import` must come before local `@import`s"
        );
        assert_eq!(location.map(|l| l.0), Some(2));
    }

    #[test]
    fn rendered_partials_replace_the_file_on_disk() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("static");
        tmp.child("static/_vars.css")
            .write_str("---\n---\n:root{--w:{{ 1 + 1 }}px}")
            .unwrap();
        let templates = CssTemplates {
            rendered: HashMap::from([(
                normalize(&dir.join("_vars.css")),
                ":root{--w:2px}".to_string(),
            )]),
            unrendered: HashSet::new(),
        };
        let out = process_with(
            &dir.join("main.css"),
            "@import \"_vars.css\";\na{width:var(--w)}",
            Some(&dir),
            None,
            &CssConfig::default(),
            &templates,
        )
        .unwrap();
        assert!(out.contains("--w:2px") && !out.contains("{{"), "{out}");
    }

    #[test]
    fn import_cycle_is_an_error() {
        let tmp = TempDir::new().unwrap();
        let a = "@import \"b.css\";\na{color:red}";
        tmp.child("static/a.css").write_str(a).unwrap();
        tmp.child("static/b.css")
            .write_str("@import \"a.css\";\nb{color:red}")
            .unwrap();
        let err = run(&tmp, "a.css", a).unwrap_err();
        let (message, _) = css_error(&err);
        assert!(message.contains("cycle"), "{message}");
        assert!(
            message.contains("a.css") && message.contains("b.css"),
            "{message}"
        );
    }

    #[test]
    fn import_cycle_points_at_the_closing_import() {
        let tmp = TempDir::new().unwrap();
        let a = "@import \"b.css\";\na{color:red}";
        tmp.child("static/a.css").write_str(a).unwrap();
        tmp.child("static/b.css")
            .write_str("/* b */\n  @import \"a.css\";\nb{color:red}")
            .unwrap();
        let err = run(&tmp, "a.css", a).unwrap_err();
        assert!(err.path.ends_with("b.css"), "{err}");
        assert_eq!(css_error(&err).1, Some((2, 3)), "{err}");
    }

    #[test]
    fn missing_import_is_located() {
        let tmp = TempDir::new().unwrap();
        let err = run(&tmp, "main.css", "/* c */\n@import \"nope.css\";\n").unwrap_err();
        assert!(err.path.ends_with("main.css"), "{err}");
        let (message, location) = css_error(&err);
        assert!(message.contains("nope.css"), "{message}");
        assert_eq!(location.map(|l| l.0), Some(2), "{err}");
    }

    #[test]
    fn import_outside_root_is_refused() {
        let tmp = TempDir::new().unwrap();
        tmp.child("secret.css").write_str("s{color:red}").unwrap();
        let err = run(
            &tmp,
            "main.css",
            "@import \"../secret.css\";\na{color:blue}",
        )
        .unwrap_err();
        let (message, location) = css_error(&err);
        assert!(message.contains("outside"), "{message}");
        assert_eq!(location.map(|l| l.0), Some(1));
    }

    #[test]
    fn overrides_replace_imports_but_widen_nothing_else() {
        let tmp = TempDir::new().unwrap();
        tmp.child("t/static/_v.css")
            .write_str("a{color:red}")
            .unwrap();
        tmp.child("t/static/_w.css")
            .write_str("b{color:red}")
            .unwrap();
        tmp.child("s/static/_v.css")
            .write_str("a{color:blue}")
            .unwrap();
        tmp.child("secret.css").write_str("s{color:red}").unwrap();
        let theme = tmp.path().join("t/static");
        let site = tmp.path().join("s/static");
        let run = |css: &str| {
            process_with(
                &theme.join("main.css"),
                css,
                Some(&theme),
                Some(&site),
                &CssConfig::default(),
                &CssTemplates::default(),
            )
        };
        let out = run("@import \"_v.css\";\n@import \"_w.css\";\n").unwrap();
        assert!(
            out.contains("a{color:#00f}") && out.contains("b{color:red}"),
            "{out}"
        );
        let err = run("@import \"../../secret.css\";\n").unwrap_err();
        assert!(css_error(&err).0.contains("outside"), "{err}");
    }

    #[test]
    fn invalid_css_is_located() {
        let tmp = TempDir::new().unwrap();
        let err = run(&tmp, "bad.css", "a {\n  color: red;\n}\n}}} b {{\n").unwrap_err();
        assert!(err.path.ends_with("bad.css"), "{err}");
        assert!(css_error(&err).1.is_some(), "{err}");
    }

    #[test]
    fn error_in_an_imported_file_names_that_file() {
        let tmp = TempDir::new().unwrap();
        tmp.child("static/_bad.css")
            .write_str("a { color: red; }\n}}} b {{\n")
            .unwrap();
        let err = run(&tmp, "main.css", "@import \"_bad.css\";\n").unwrap_err();
        assert!(err.path.ends_with("_bad.css"), "{err}");
        assert_eq!(css_error(&err).1.map(|l| l.0), Some(2), "{err}");
    }

    #[test]
    fn bundle_off_leaves_imports() {
        let tmp = TempDir::new().unwrap();
        let out = process(
            &tmp.path().join("static/m.css"),
            "@import \"_v.css\";\na{color:red}",
            Some(&tmp.path().join("static")),
            &CssConfig {
                bundle: false,
                ..CssConfig::default()
            },
        )
        .unwrap();
        assert!(out.contains("@import"), "{out}");
    }
}
