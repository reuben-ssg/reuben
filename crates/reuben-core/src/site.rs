//! Loading a site from disk and building it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use std::sync::Arc;

use crate::collections;
use crate::config::{self, Config};
use crate::data::{self, SiteData};
use crate::dirs::{self, DirDefaults};
use crate::error::{Error, ErrorKind};
use crate::frontmatter::{self, FrontMatter};
use crate::route::{Route, route_for_page, route_for_template};
use crate::script::{Html, ScriptEngine};
use crate::template::{self, Escape};
use crate::theme::{PageShell, builtin_page};
use crate::{html, layout, page, splice};

/// Written into every output directory Reuben builds. Only a directory containing it is ever cleared.
pub const OUTPUT_MARKER: &str = ".reuben-output";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PageKind {
    Markdown,
    Template { escape: Escape },
}

#[derive(Debug)]
pub struct Page {
    pub kind: PageKind,
    /// Found under `static/`: no folder defaults, and never part of a collection.
    pub from_static: bool,
    pub source: PathBuf,
    pub front_matter: FrontMatter,
    /// Raw Markdown after the front matter, with blocks unevaluated.
    pub body: String,
    /// 1-based line in `source` where `body` starts.
    pub body_line: usize,
    /// Path relative to `content/`.
    pub rel: PathBuf,
    /// The sibling `<stem>.rhai`, if present.
    pub sidecar: Option<PathBuf>,
    pub route: Route,
    /// The file that set each front matter value, keyed by dotted path.
    pub front_matter_sources: BTreeMap<String, PathBuf>,
    /// Hidden from normal builds: the page or a folder above it is a draft.
    pub draft_excluded: bool,
}

#[derive(Debug)]
pub struct Site {
    pub root: PathBuf,
    pub pages: Vec<Page>,
    /// Paths relative to `static/`.
    pub static_files: Vec<PathBuf>,
    /// Non-page files in `content/`, relative to it, with whether a drafted folder hides them.
    pub content_assets: Vec<(PathBuf, bool)>,
    /// Paths relative to the theme's `static/`, including ones the site overrides.
    pub theme_static_files: Vec<PathBuf>,
    pub config: Config,
    pub engine: Arc<ScriptEngine>,
    pub data: SiteData,
}

#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Merges `reuben.<env>.toml` over the base config when it exists.
    pub env: Option<String>,
}

pub struct BuildOptions {
    pub drafts: bool,
    pub out: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildReport {
    pub pages_written: usize,
    pub static_copied: usize,
    pub drafts_skipped: usize,
}

impl Site {
    pub fn load(root: &Path) -> Result<Site, Vec<Error>> {
        Self::load_with(root, &LoadOptions::default())
    }

    /// Reads every page and static file, reporting all bad files at once rather than stopping at the first.
    pub fn load_with(root: &Path, opts: &LoadOptions) -> Result<Site, Vec<Error>> {
        let mut errors = Vec::new();
        let mut pages = Vec::new();

        let config = match config::load(root, opts.env.as_deref()) {
            Ok(c) => Some(c),
            Err(e) => {
                errors.push(e);
                None
            }
        };
        let mut engine = ScriptEngine::new(root);
        if let Some(config) = &config {
            let dirs: Vec<PathBuf> = config
                .theme
                .iter()
                .map(|t| t.join("includes"))
                .chain([root.join("includes")])
                .filter(|d| d.is_dir())
                .collect();
            if let Err(mut e) = engine.load_includes(&dirs) {
                errors.append(&mut e);
            }
        }

        let site_data = config.as_ref().and_then(|config| {
            let result = page::site_map(config)
                .map_err(|e| vec![e])
                .and_then(|site| data::load(root, &engine, &site));
            match result {
                Ok(d) => Some(d),
                Err(mut e) => {
                    errors.append(&mut e);
                    None
                }
            }
        });

        let content = root.join("content");
        let mut content_assets = Vec::new();
        // Pages without their folder defaults would report misleading missing-title errors, so skip them.
        match dirs::load(&content) {
            Ok(dirs) => {
                let mut asset_rels = Vec::new();
                for (path, rel) in walk(&content, &mut errors) {
                    let is_markdown = path.extension().is_some_and(|e| e == "md");
                    if !is_markdown && path.file_name().is_some_and(|n| n == "_dir.toml") {
                        continue;
                    }
                    // Sidecars hold build scripts, which must never be published.
                    if path.extension().is_some_and(|e| e == "rhai") {
                        continue;
                    }
                    let loaded = if is_markdown {
                        load_page(&path, &rel, Some(&dirs), PageKind::Markdown).map(Some)
                    } else {
                        load_template(&path, &rel, Some(&dirs))
                    };
                    match loaded {
                        Ok(Some(page)) => pages.push(page),
                        Ok(None) => asset_rels.push(rel),
                        Err(e) => errors.push(e),
                    }
                }
                mark_draft_excluded(&mut pages, &dirs);
                content_assets = asset_rels
                    .into_iter()
                    .map(|rel| {
                        let hidden = asset_drafted(&rel, &pages, &dirs);
                        (rel, hidden)
                    })
                    .collect();
            }
            Err(mut e) => errors.append(&mut e),
        }

        let static_dir = root.join("static");
        let mut static_files = Vec::new();
        if static_dir.exists() {
            for (path, rel) in walk(&static_dir, &mut errors) {
                match load_template(&path, &rel, None) {
                    Ok(Some(mut page)) => {
                        page.draft_excluded = page.front_matter.draft;
                        pages.push(page);
                    }
                    Ok(None) => static_files.push(rel),
                    Err(e) => errors.push(e),
                }
            }
        }

        let theme_static_files = match config.as_ref().and_then(|c| c.theme.as_ref()) {
            Some(theme) if theme.join("static").exists() => {
                walk(&theme.join("static"), &mut errors)
                    .into_iter()
                    .map(|(_, rel)| rel)
                    .collect()
            }
            _ => Vec::new(),
        };

        match config {
            Some(config) if errors.is_empty() => Ok(Site {
                data: site_data.unwrap_or_default(),
                root: root.to_path_buf(),
                pages,
                static_files,
                content_assets,
                theme_static_files,
                config,
                engine: Arc::new(engine),
            }),
            _ => Err(errors),
        }
    }

    pub fn build(&self, opts: &BuildOptions) -> Result<BuildReport, Vec<Error>> {
        let (pages, drafts): (Vec<&Page>, Vec<&Page>) = self
            .pages
            .iter()
            .partition(|p| opts.drafts || !p.draft_excluded);

        self.check_collisions(&pages, opts.drafts)?;
        prepare_out_dir(&opts.out).map_err(|e| vec![e])?;

        let mut errors = Vec::new();
        let site_map = page::site_map(&self.config).map_err(|e| vec![e])?;
        let collections = collections::build(&pages, &self.config)?;
        let scopes = page::SiteScopes {
            site: &site_map,
            data: &self.data.value,
            collections: &collections,
        };
        for page in &pages {
            let output = match self.render_page(page, &scopes) {
                Ok(output) => output,
                Err(mut e) => {
                    errors.append(&mut e);
                    continue;
                }
            };
            if let Err(e) = splice::ensure_no_placeholders(&page.source, &output) {
                errors.push(e);
                continue;
            }
            let dest = opts.out.join(&page.route.file);
            if let Err(e) = write(&dest, output.as_bytes()) {
                errors.push(Error::new(&dest, e));
            }
        }
        let statics = self.static_sources(opts.drafts);
        for (rel, src) in &statics {
            let dest = opts.out.join(rel);
            let copied = dest
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|()| fs::copy(src, &dest));
            if let Err(e) = copied {
                errors.push(Error::new(&dest, e));
            }
        }

        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(BuildReport {
            pages_written: pages.len(),
            static_copied: statics.len(),
            drafts_skipped: drafts.len(),
        })
    }

    /// Renders one page or template to its final output text, layout included.
    fn render_page(
        &self,
        page: &Page,
        scopes: &page::SiteScopes<'_>,
    ) -> Result<String, Vec<Error>> {
        let (body, mut scope, layout_name) = match page.kind {
            PageKind::Markdown => {
                let (doc, scope) = page::render_body(page, &self.engine, &self.config, scopes)?;
                let name = page
                    .front_matter
                    .layout
                    .as_deref()
                    .or_else(|| collections::layout_of(&page.rel, &self.config));
                (html::render(&doc), scope, name)
            }
            PageKind::Template { escape } => {
                let (text, scope) =
                    template::render(page, escape, &self.engine, &self.config, scopes)?;
                (text, scope, page.front_matter.layout.as_deref())
            }
        };
        let layout = layout::find_layout(layout_name, &self.root, self.config.theme.as_deref())
            .map_err(|mut e| {
                e.path.clone_from(&page.source);
                vec![e]
            })?;
        match (layout, page.kind) {
            (Some(path), _) => layout::render_layout(&path, &self.engine, &mut scope, Html(body)),
            (None, PageKind::Markdown) => Ok(builtin_page(&PageShell {
                title: &page.front_matter.title,
                lang: "en",
                description: page.front_matter.description.as_deref(),
                body: &body,
            })),
            (None, PageKind::Template { .. }) => Ok(body),
        }
    }

    /// Every file to copy as (path relative to the output, source). Theme files come first and a site file at the
    /// same path replaces one, so only two files within the site can collide.
    /// Content assets are included unless a drafted folder hides them and `drafts` is off.
    fn static_sources(&self, drafts: bool) -> Vec<(PathBuf, PathBuf)> {
        let theme_dir = self.config.theme.as_ref().map(|t| t.join("static"));
        let templated: HashSet<&Path> = self
            .pages
            .iter()
            .filter(|p| p.from_static)
            .map(|p| p.rel.as_path())
            .collect();
        let theme = theme_dir.iter().flat_map(|dir| {
            self.theme_static_files
                .iter()
                .filter(|rel| {
                    !self.static_files.contains(rel) && !templated.contains(rel.as_path())
                })
                .map(|rel| (rel.clone(), dir.join(rel)))
        });
        let site_dir = self.root.join("static");
        let site = self
            .static_files
            .iter()
            // A static template's sidecar script is source, not an asset.
            .filter(|rel| {
                !self
                    .pages
                    .iter()
                    .any(|p| p.from_static && p.rel.with_extension("rhai") == **rel)
            })
            .map(|rel| (rel.clone(), site_dir.join(rel)));
        let content_dir = self.root.join("content");
        let assets = self
            .content_assets
            .iter()
            .filter(move |(_, hidden)| drafts || !hidden)
            .map(move |(rel, _)| (rel.clone(), content_dir.join(rel)));
        theme.chain(site).chain(assets).collect()
    }

    /// Fails if two sources (pages or static files) would write the same output file.
    fn check_collisions(&self, pages: &[&Page], drafts: bool) -> Result<(), Vec<Error>> {
        let sources = pages
            .iter()
            .map(|p| (p.route.file.clone(), p.source.clone()))
            .chain(self.static_sources(drafts));

        // Keyed case-insensitively: on macOS and Windows `Notes/` and `notes/` are the same directory, and a site
        // that relies on the difference can't be deployed portably anyway.
        let mut seen: HashMap<String, PathBuf> = HashMap::new();
        let mut errors = Vec::new();
        for (file, source) in sources {
            let key = file.to_string_lossy().to_lowercase();
            if let Some(other) = seen.get(&key) {
                errors.push(Error::new(
                    &source,
                    ErrorKind::OutputCollision {
                        other: other.clone(),
                        file: file.clone(),
                    },
                ));
            } else {
                seen.insert(key, source);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Every file under `dir`, sorted, as (full path, path relative to `dir`).
fn walk(dir: &Path, errors: &mut Vec<Error>) -> Vec<(PathBuf, PathBuf)> {
    let mut files = Vec::new();
    for entry in WalkDir::new(dir).sort_by_file_name() {
        match entry {
            Ok(entry) if entry.file_type().is_file() => {
                let path = entry.into_path();
                let rel = path
                    .strip_prefix(dir)
                    .expect("walkdir yields paths under its root")
                    .to_path_buf();
                files.push((path, rel));
            }
            Ok(_) => {}
            Err(e) => {
                let path = e.path().unwrap_or(dir).to_path_buf();
                errors.push(Error::new(path, std::io::Error::from(e)));
            }
        }
    }
    files
}

/// Reads a content or static file. Markdown always becomes a page; other files do only if they start with front
/// matter, and `Ok(None)` means the file is not a template.
fn load_template(
    path: &Path,
    rel: &Path,
    dirs: Option<&DirDefaults>,
) -> Result<Option<Page>, Error> {
    let io = |e: std::io::Error| Error::new(path, e);
    // Most static files are binary, so read just enough to rule them out before loading the rest.
    let mut file = fs::File::open(path).map_err(io)?;
    let mut head = [0u8; 8];
    let n = file.read(&mut head).map_err(io)?;
    let head = head[..n]
        .strip_prefix(b"\xef\xbb\xbf")
        .unwrap_or(&head[..n]);
    if !(head.starts_with(b"---") || head.starts_with(b"+++")) {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(io)?;
    if !template::starts_with_front_matter(&bytes) {
        return Ok(None);
    }
    let kind = PageKind::Template {
        escape: template::escape_for(rel),
    };
    load_page_from(path, rel, bytes, dirs, kind).map(Some)
}

fn load_page(
    path: &Path,
    rel: &Path,
    dirs: Option<&DirDefaults>,
    kind: PageKind,
) -> Result<Page, Error> {
    let bytes = fs::read(path).map_err(|e| Error::new(path, e))?;
    load_page_from(path, rel, bytes, dirs, kind)
}

fn load_page_from(
    path: &Path,
    rel: &Path,
    bytes: Vec<u8>,
    dirs: Option<&DirDefaults>,
    kind: PageKind,
) -> Result<Page, Error> {
    let src = String::from_utf8(bytes).map_err(|_| Error::new(path, ErrorKind::NotUtf8))?;
    let raw = frontmatter::parse_raw(&src).map_err(|e| Error::new(path, e))?;
    let mut merged = dirs.map_or_else(Default::default, |d| {
        d.defaults_for(rel.parent().unwrap_or(Path::new("")))
    });
    merged.merge(raw.value, path);
    let front_matter = frontmatter::from_value(merged.value).map_err(|e| Error::new(path, e))?;
    let sidecar = Some(path.with_extension("rhai")).filter(|p| p.is_file());
    let route = match kind {
        PageKind::Markdown => route_for_page(rel),
        PageKind::Template { .. } => route_for_template(rel),
    };
    Ok(Page {
        kind,
        from_static: dirs.is_none(),
        source: path.to_path_buf(),
        body: raw.body.to_string(),
        body_line: raw.body_line,
        rel: rel.to_path_buf(),
        sidecar,
        front_matter,
        route,
        front_matter_sources: merged.sources,
        draft_excluded: false,
    })
}

/// A page is excluded when it is a draft itself or sits under a folder drafted by `_dir.toml` or by its `index.md`.
fn mark_draft_excluded(pages: &mut [Page], dirs: &DirDefaults) {
    let drafted_by_index: HashSet<&Path> = pages
        .iter()
        .filter(|p| p.front_matter.draft && p.rel.file_name().is_some_and(|n| n == "index.md"))
        .filter_map(|p| p.rel.parent())
        .collect();
    let excluded: Vec<bool> = pages
        .iter()
        .map(|p| {
            let dir = p.rel.parent().unwrap_or(Path::new(""));
            p.front_matter.draft
                || dirs.folder_drafted(dir)
                || dir.ancestors().any(|d| drafted_by_index.contains(d))
        })
        .collect();
    for (page, excluded) in pages.iter_mut().zip(excluded) {
        page.draft_excluded = excluded;
    }
}

/// Whether a drafted folder (by `_dir.toml` or a draft `index.md`) hides the asset at `rel`.
fn asset_drafted(rel: &Path, pages: &[Page], dirs: &DirDefaults) -> bool {
    let dir = rel.parent().unwrap_or(Path::new(""));
    dirs.folder_drafted(dir)
        || pages.iter().any(|p| {
            p.front_matter.draft
                && !p.from_static
                && p.rel.file_name().is_some_and(|n| n == "index.md")
                && p.rel
                    .parent()
                    .is_some_and(|d| dir.ancestors().any(|a| a == d))
        })
}

/// Creates the output directory, or clears it only if a previous Reuben build created it.
fn prepare_out_dir(out: &Path) -> Result<(), Error> {
    let io = |e: std::io::Error| Error::new(out, e);
    if out.exists() {
        let entries: Vec<_> = fs::read_dir(out)
            .map_err(io)?
            .collect::<Result<_, _>>()
            .map_err(io)?;
        if !entries.is_empty() {
            if !out.join(OUTPUT_MARKER).is_file() {
                return Err(Error::new(out, ErrorKind::UnsafeOutputDir));
            }
            for entry in entries {
                let path = entry.path();
                let removed = if entry.file_type().map_err(io)?.is_dir() {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
                removed.map_err(|e| Error::new(&path, e))?;
            }
        }
    } else {
        fs::create_dir_all(out).map_err(io)?;
    }
    fs::write(out.join(OUTPUT_MARKER), "").map_err(io)
}

fn write(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(dest, bytes)
}
