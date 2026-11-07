//! Loading a site from disk and building it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use walkdir::WalkDir;

use std::sync::Arc;

use crate::collections;
use crate::config::{self, Config};
use crate::data::{self, SiteData};
use crate::dirs::{self, DirDefaults};
use crate::error::{Error, ErrorKind};
use crate::frontmatter::{self, FrontMatter};
use crate::headings::TocEntry;
use crate::output::{Output, OutputFile, check_out_dir, write_atomic};
use crate::route::{Route, route_for_page, route_for_template};
use crate::script::helpers::url_for;
use crate::script::{Html, ScriptEngine};
use crate::sitemap;
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
    /// The table of contents before blocks run, computed once at load.
    pub pre_block_toc: Vec<TocEntry>,
    /// Front matter `description`, else the first paragraph's text, computed once at load.
    pub summary: String,
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
    /// Worker threads for rendering; `None` lets the renderer choose.
    pub jobs: Option<usize>,
}

pub struct RenderOptions {
    pub drafts: bool,
    /// Worker threads for rendering; `None` lets the renderer choose.
    pub jobs: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildReport {
    pub pages_written: usize,
    pub static_copied: usize,
    pub drafts_skipped: usize,
    pub warnings: Vec<String>,
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
            engine.set_base_url(&config.base_url);
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
                    if path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("rhai"))
                    {
                        continue;
                    }
                    if rel
                        .components()
                        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
                        || is_junk(&rel)
                    {
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
                if is_junk(&rel) {
                    continue;
                }
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
                    .filter(|rel| !is_junk(rel))
                    .collect()
            }
            _ => Vec::new(),
        };

        pages.par_iter_mut().for_each(|page| {
            page.pre_block_toc = page::compute_pre_block_toc(page.kind, &page.body, page.body_line);
            page.summary = collections::compute_summary(
                page.kind,
                page.front_matter.description.as_deref(),
                &page.body,
                page.body_line,
            );
        });

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

    /// Renders the whole site into memory. Nothing touches the disk.
    pub fn render(&self, opts: &RenderOptions) -> Result<(Output, BuildReport), Vec<Error>> {
        let (pages, drafts): (Vec<&Page>, Vec<&Page>) = self
            .pages
            .iter()
            .partition(|p| opts.drafts || !p.draft_excluded);

        self.check_collisions(&pages, opts.drafts)?;
        let site_map = page::site_map(&self.config).map_err(|e| vec![e])?;
        let collections = collections::build(&pages, &self.config)?;

        let mut errors = Vec::new();
        let mut output = Output::default();
        let scopes = page::SiteScopes {
            site: &site_map,
            data: &self.data.value,
            collections: &collections,
        };
        let render_one = |page: &&Page| -> Result<String, Vec<Error>> {
            let html = self.render_page(page, &scopes)?;
            splice::ensure_no_placeholders(&page.source, &html).map_err(|e| vec![e])?;
            Ok(html)
        };
        let pool = opts
            .jobs
            .map(|n| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(n)
                    .build()
                    .map_err(|e| {
                        vec![Error::new(
                            "reuben.toml",
                            ErrorKind::Config(format!("can't start {n} render threads: {e}")),
                        )]
                    })
            })
            .transpose()?;
        let rendered: Vec<Result<String, Vec<Error>>> =
            in_pool(pool.as_ref(), || pages.par_iter().map(render_one).collect());
        let mut css_jobs: Vec<CssJob> = Vec::new();
        // Templated partials are imported as their rendered text. A partial that failed to render is imported as
        // empty, since its own error is already reported and importers would only add noise.
        let mut partials: HashMap<PathBuf, String> = HashMap::new();
        let mut partial_pages = 0;
        // Merged in page order so neither output nor errors depend on which thread finished first.
        for (page, result) in pages.iter().zip(rendered) {
            let is_partial = is_partial_css(&page.route.file);
            partial_pages += usize::from(is_partial);
            match result {
                Ok(html) if is_partial => {
                    partials.insert(crate::css::normalize(&page.source), html);
                }
                Err(mut e) if is_partial => {
                    partials.insert(crate::css::normalize(&page.source), String::new());
                    errors.append(&mut e);
                }
                Ok(html) if is_css(&page.route.file) => {
                    css_jobs.push(CssJob {
                        rel: page.route.file.clone(),
                        path: page.source.clone(),
                        root: self.css_root(&page.source, page.from_static),
                        input: CssInput::Text(html),
                    });
                }
                Ok(html) => {
                    output.files.insert(
                        page.route.file.clone(),
                        OutputFile::Bytes(html.into_bytes()),
                    );
                }
                Err(mut e) => errors.append(&mut e),
            }
        }
        let statics = self.static_sources(opts.drafts);
        let mut static_copied = 0;
        for (rel, src) in &statics {
            if is_partial_css(rel) {
                continue;
            }
            // Opened read-only now so unreadable sources are reported with the page errors, not at write time.
            if let Err(e) = fs::File::open(src) {
                errors.push(Error::new(src, e));
                continue;
            }
            static_copied += 1;
            if is_css(rel) {
                css_jobs.push(CssJob {
                    rel: rel.clone(),
                    path: src.clone(),
                    root: self.css_root(src, false),
                    input: CssInput::File,
                });
            } else {
                output
                    .files
                    .insert(rel.clone(), OutputFile::Copy(src.clone()));
            }
        }
        let processed = in_pool(pool.as_ref(), || {
            css_jobs
                .par_iter()
                .map(|job| job.run(&self.config.css, &partials))
                .collect::<Vec<_>>()
        });
        for (job, result) in css_jobs.into_iter().zip(processed) {
            match result {
                Ok(css) => {
                    output
                        .files
                        .insert(job.rel, OutputFile::Bytes(css.into_bytes()));
                }
                Err(e) => errors.push(e),
            }
        }

        let mut warnings = Vec::new();
        if self.config.sitemap && !self.user_provides_sitemap(&pages, &statics) {
            if self.config.base_url.starts_with("http://")
                || self.config.base_url.starts_with("https://")
            {
                let mut urls: Vec<String> = pages
                    .iter()
                    .filter(|p| {
                        p.route.url != "/404.html"
                            && match p.kind {
                                PageKind::Markdown => true,
                                PageKind::Template { .. } => {
                                    p.route.file.extension().is_some_and(|e| e == "html")
                                }
                            }
                    })
                    .map(|p| url_for(&self.config.base_url, &p.route.url))
                    .collect();
                urls.sort();
                output.files.insert(
                    PathBuf::from("sitemap.xml"),
                    OutputFile::Bytes(sitemap::render(&urls).into_bytes()),
                );
            } else {
                warnings.push(
                    "sitemap.xml skipped: base_url must be absolute (http:// or https://)".into(),
                );
            }
        }

        if !errors.is_empty() {
            Error::sort(&mut errors);
            return Err(errors);
        }
        let report = BuildReport {
            pages_written: pages.len() - partial_pages,
            static_copied,
            drafts_skipped: drafts.len(),
            warnings,
        };
        Ok((output, report))
    }

    /// Renders the site and swaps it into `opts.out`. A failed build leaves the previous output untouched.
    pub fn build(&self, opts: &BuildOptions) -> Result<BuildReport, Vec<Error>> {
        // Refuse a foreign directory before spending time rendering.
        check_out_dir(&opts.out).map_err(|e| vec![e])?;
        let (output, report) = self.render(&RenderOptions {
            drafts: opts.drafts,
            jobs: opts.jobs,
        })?;
        write_atomic(&output, &opts.out)?;
        Ok(report)
    }

    /// A page or copied file that already writes `sitemap.xml` (any case) takes the place of the generated one.
    fn user_provides_sitemap(&self, pages: &[&Page], statics: &[(PathBuf, PathBuf)]) -> bool {
        let is_sitemap = |p: &Path| p.to_string_lossy().eq_ignore_ascii_case("sitemap.xml");
        pages.iter().any(|p| is_sitemap(&p.route.file))
            || statics.iter().any(|(rel, _)| is_sitemap(rel))
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
                // Folder defaults and collection layouts are meant for pages, so only a layout the template names
                // itself wraps it; otherwise its output is written as-is.
                let own = page.front_matter_sources.get("layout") == Some(&page.source);
                let name = page.front_matter.layout.as_deref().filter(|_| own);
                if name.is_none() {
                    return Ok(text);
                }
                (text, scope, name)
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

    /// Where `@import`s in the CSS file `src` may reach: the folder it lives in the site, theme or content tree.
    fn css_root(&self, src: &Path, template_from_static: bool) -> PathBuf {
        let site_static = self.root.join("static");
        if template_from_static {
            return site_static;
        }
        let theme_static = self.config.theme.as_ref().map(|t| t.join("static"));
        match theme_static {
            Some(dir) if src.starts_with(&dir) => dir,
            _ if src.starts_with(&site_static) => site_static,
            _ => self.root.join("content"),
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
            .filter(|rel| !is_static_sidecar(&self.pages, &site_dir, rel))
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

fn in_pool<R: Send>(pool: Option<&rayon::ThreadPool>, f: impl FnOnce() -> R + Send) -> R {
    match pool {
        Some(pool) => pool.install(f),
        None => f(),
    }
}

fn is_css(rel: &Path) -> bool {
    rel.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("css"))
}

/// `_name.css` files can be imported but are never written out.
fn is_partial_css(rel: &Path) -> bool {
    is_css(rel)
        && rel
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('_'))
}

enum CssInput {
    /// Template output, already rendered.
    Text(String),
    /// Read from `CssJob::path`.
    File,
}

struct CssJob {
    rel: PathBuf,
    path: PathBuf,
    root: PathBuf,
    input: CssInput,
}

impl CssJob {
    fn run(
        &self,
        cfg: &config::CssConfig,
        partials: &HashMap<PathBuf, String>,
    ) -> Result<String, Error> {
        let text;
        let (css, is_template) = match &self.input {
            CssInput::Text(t) => (t.as_str(), true),
            CssInput::File => {
                let bytes = fs::read(&self.path).map_err(|e| Error::new(&self.path, e))?;
                text = String::from_utf8(bytes)
                    .map_err(|_| Error::new(&self.path, ErrorKind::NotUtf8))?;
                (text.as_str(), false)
            }
        };
        crate::css::process_with(&self.path, css, Some(&self.root), cfg, partials).map_err(
            |mut e| {
                // A template's line numbers describe its rendered text, not the file on disk, so the error drops
                // them and quotes the offending rendered line instead.
                let rendered = if is_template && e.path == self.path {
                    Some(css)
                } else {
                    partials
                        .get(&crate::css::normalize(&e.path))
                        .map(String::as_str)
                };
                if let (Some(rendered), ErrorKind::Css { message, location }) =
                    (rendered, &mut e.kind)
                    && let Some(at) = location.take()
                    && let Some(line) = rendered.lines().nth(at.line.saturating_sub(1))
                {
                    message.push_str(&format!(" (in the rendered output: `{}`)", line.trim()));
                }
                e
            },
        )
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
    // Only Markdown pages need a title; any other template is named after its file.
    let untitled = match merged.value.get("title") {
        None | Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::String(t)) => t.trim().is_empty(),
        Some(_) => false,
    };
    if matches!(kind, PageKind::Template { .. }) && untitled {
        let name = rel
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        merged.value.insert("title".into(), name.into());
    }
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
        pre_block_toc: Vec::new(),
        summary: String::new(),
    })
}

/// A page is excluded when it is a draft itself or sits under a folder drafted by `_dir.toml` or by its `index.md`.
fn mark_draft_excluded(pages: &mut [Page], dirs: &DirDefaults) {
    let drafted_by_index: HashSet<&Path> = pages
        .iter()
        .filter(|p| p.front_matter.draft && p.rel.file_name().is_some_and(|n| n == "index.md"))
        .filter_map(|p| p.rel.parent())
        // The home page's own `draft` hides only itself.
        .filter(|d| !d.as_os_str().is_empty())
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

/// True when `rel` (under `static_dir`) is the sidecar script of a static template, compared case-insensitively
/// because a case-insensitive filesystem resolves the sidecar whatever its case.
fn is_static_sidecar(pages: &[Page], static_dir: &Path, rel: &Path) -> bool {
    let lower = |p: &Path| p.to_string_lossy().to_lowercase();
    let file = lower(&static_dir.join(rel));
    pages
        .iter()
        .any(|p| p.from_static && p.sidecar.as_deref().is_some_and(|s| lower(s) == file))
}

/// OS and editor leftovers that are never published, wherever they sit.
pub(crate) fn is_junk(rel: &Path) -> bool {
    rel.file_name().is_some_and(|n| {
        let n = n.to_string_lossy();
        n == ".DS_Store" || n == "Thumbs.db" || n.ends_with(".swp") || n.ends_with('~')
    })
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
                    .is_some_and(|d| !d.as_os_str().is_empty() && dir.ancestors().any(|a| a == d))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn static_template(sidecar: Option<&str>) -> Page {
        Page {
            kind: PageKind::Template {
                escape: Escape::None,
            },
            from_static: true,
            source: PathBuf::from("/s/static/side.css"),
            front_matter: frontmatter::from_value(
                serde_json::json!({"title": "T"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .unwrap(),
            body: String::new(),
            body_line: 1,
            rel: PathBuf::from("side.css"),
            sidecar: sidecar.map(PathBuf::from),
            route: route_for_template(Path::new("side.css")),
            front_matter_sources: BTreeMap::new(),
            draft_excluded: false,
            pre_block_toc: Vec::new(),
            summary: String::new(),
        }
    }

    #[test]
    fn sidecar_match_ignores_case_without_touching_the_filesystem() {
        let pages = [static_template(Some("/s/static/side.rhai"))];
        let dir = Path::new("/s/static");
        assert!(is_static_sidecar(&pages, dir, Path::new("side.RHAI")));
        assert!(is_static_sidecar(&pages, dir, Path::new("side.rhai")));
        assert!(!is_static_sidecar(&pages, dir, Path::new("other.rhai")));
        let none = [static_template(None)];
        assert!(!is_static_sidecar(&none, dir, Path::new("side.rhai")));
    }
}
