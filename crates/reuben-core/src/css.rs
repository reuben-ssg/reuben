//! CSS processing with Lightning CSS: bundling local imports, minifying and lowering for browser targets.

use std::collections::{BTreeMap, HashMap};
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

/// Processes one stylesheet.
///
/// With `bundle` on and an `imports_root`, local `@import`s are inlined, resolved against the importing file and
/// never allowed to leave `imports_root`. Remote imports are left alone.
pub fn process(
    source_path: &Path,
    css: &str,
    imports_root: Option<&Path>,
    cfg: &CssConfig,
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
    let locate = |kind: &dyn Display, loc: Option<&lightningcss::error::ErrorLocation>| {
        let (path, location) = match loc {
            Some(loc) if Path::new(&loc.filename) == entry => (source_path.to_path_buf(), loc),
            Some(loc) if !loc.filename.is_empty() => (PathBuf::from(&loc.filename), loc),
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

    let run = |hoist: bool| -> Result<String, Failure> {
        let bundle_root = imports_root.filter(|_| cfg.bundle);
        let provider = Provider::new(
            normalize(bundle_root.unwrap_or(Path::new(""))),
            entry.clone(),
            css,
            hoist,
        );
        let mut sheet = match bundle_root {
            Some(_) => {
                let options = ParserOptions {
                    filename: entry.to_string_lossy().into_owned(),
                    ..ParserOptions::default()
                };
                let mut bundler = Bundler::new(&provider, None, options);
                let sheet = bundler.bundle(&entry).map_err(|e| {
                    if matches!(e.kind, BundleErrorKind::ExternalImportAfterBundledImport) {
                        Failure::ExternalAfterBundled
                    } else {
                        Failure::Error(locate(&e.kind, e.loc.as_ref()))
                    }
                })?;
                if let Some(cycle) = provider.find_cycle() {
                    return Err(fail(format!("import cycle: {cycle}")).into());
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
        let mut code = printed.code;
        if hoist {
            code = format!("{}{code}", provider.hoisted(cfg.minify));
        }
        Ok(code)
    };
    match run(false) {
        Ok(code) => Ok(code),
        Err(Failure::Error(e)) => Err(e),
        Err(Failure::ExternalAfterBundled) => run(true).map_err(|f| match f {
            Failure::Error(e) => e,
            Failure::ExternalAfterBundled => fail("remote `@import` after a local one".into()),
        }),
    }
}

enum Failure {
    /// Retried with remote imports hoisted to the top.
    ExternalAfterBundled,
    Error(Error),
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Failure::Error(e)
    }
}

#[derive(Debug)]
struct ProviderError(String);

impl Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProviderError {}

/// Serves the entry from memory and its imports from disk, refusing anything outside `root`.
struct Provider {
    root: PathBuf,
    sources: FrozenMap<PathBuf, String>,
    /// Every resolved `@import`, importer to imported, for cycle detection.
    edges: Mutex<Vec<(PathBuf, PathBuf)>>,
    entry: PathBuf,
    /// The bundler refuses a remote import after a local one, so on retry remote imports are lifted out and
    /// re-emitted first. Any media, layer or supports condition on them is not carried over.
    hoist: bool,
    remote: Mutex<Vec<String>>,
}

impl Provider {
    fn new(root: PathBuf, entry: PathBuf, css: &str, hoist: bool) -> Self {
        let sources = FrozenMap::new();
        sources.insert(entry.clone(), css.to_string());
        Self {
            root,
            sources,
            edges: Mutex::new(Vec::new()),
            entry,
            hoist,
            remote: Mutex::new(Vec::new()),
        }
    }

    /// The remote imports moved to the front, as the CSS spec requires of every `@import`.
    fn hoisted(&self, minify: bool) -> String {
        let remote = self.remote.lock().unwrap_or_else(|e| e.into_inner());
        let sep = if minify { "" } else { "\n" };
        remote
            .iter()
            .map(|url| format!("@import url(\"{url}\");{sep}"))
            .collect()
    }

    fn display(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    fn find_cycle(&self) -> Option<String> {
        let edges = self.edges.lock().unwrap_or_else(|e| e.into_inner());
        let mut graph: HashMap<&Path, Vec<&Path>> = HashMap::new();
        for (from, to) in edges.iter() {
            graph.entry(from).or_default().push(to);
        }
        let mut done = std::collections::HashSet::new();
        let mut stack = Vec::new();
        self.visit(&self.entry, &graph, &mut stack, &mut done)
    }

    fn visit<'p>(
        &self,
        node: &'p Path,
        graph: &HashMap<&'p Path, Vec<&'p Path>>,
        stack: &mut Vec<&'p Path>,
        done: &mut std::collections::HashSet<&'p Path>,
    ) -> Option<String> {
        if let Some(at) = stack.iter().position(|p| *p == node) {
            let names: Vec<String> = stack[at..]
                .iter()
                .chain([&node])
                .map(|p| self.display(p))
                .collect();
            return Some(names.join(" -> "));
        }
        if !done.insert(node) {
            return None;
        }
        stack.push(node);
        for next in graph.get(node).into_iter().flatten() {
            if let Some(found) = self.visit(next, graph, stack, done) {
                return Some(found);
            }
        }
        stack.pop();
        None
    }
}

impl SourceProvider for Provider {
    type Error = ProviderError;

    fn read<'a>(&'a self, file: &Path) -> Result<&'a str, ProviderError> {
        if file == self.root.join(".reuben-remote") {
            return Ok("");
        }
        if let Some(text) = self.sources.get(file) {
            return Ok(text);
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
            if self.hoist {
                let mut remote = self.remote.lock().unwrap_or_else(|e| e.into_inner());
                if !remote.iter().any(|u| u == specifier) {
                    remote.push(specifier.to_string());
                }
                drop(remote);
                return Ok(ResolveResult::File(self.root.join(".reuben-remote")));
            }
            return Ok(ResolveResult::External(specifier.to_string()));
        }
        let outside = || {
            ProviderError(format!(
                "import `{specifier}` is outside the stylesheet folder"
            ))
        };
        let target = normalize(&originating.with_file_name(specifier));
        if !target.starts_with(&self.root) {
            return Err(outside());
        }
        // A symlink inside the root must not lead out of it either.
        if let (Ok(real), Ok(root)) = (target.canonicalize(), self.root.canonicalize())
            && !real.starts_with(root)
        {
            return Err(outside());
        }
        self.edges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((originating.to_path_buf(), target.clone()));
        Ok(ResolveResult::File(target))
    }
}

/// Resolves `.` and `..` without touching the disk, so a missing file can still be confined to the root.
fn normalize(path: &Path) -> PathBuf {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

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
        let main = "@import \"_vars.css\";\n@import url(https://fonts.example/x.css);\nbody{color:var(--c)}";
        tmp.child("static/main.css").write_str(main).unwrap();
        tmp.child("static/_vars.css")
            .write_str(":root{--c:red}")
            .unwrap();
        let out = run(&tmp, "main.css", main).unwrap();
        assert!(out.contains("--c:red"), "{out}");
        assert!(out.contains("https://fonts.example/x.css"), "{out}");
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
