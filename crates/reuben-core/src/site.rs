//! Loading a site from disk and building it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use walkdir::WalkDir;

use std::sync::Arc;

use crate::collections;
use crate::config::{self, Config};
use crate::css::{self, CssInput, CssJob, CssTemplates, is_css, is_partial_css};
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
    /// Per-file problems found while loading. The site holds everything that did load; any entry fails the build.
    pub load_errors: Vec<Error>,
    /// Folders (relative to `content/`) whose `_dir.toml` failed to load. Their pages stay in the site, but their
    /// render errors are dropped because the `_dir.toml` error already explains them.
    broken_dirs: BTreeSet<PathBuf>,
    /// An `includes/*.rhai` file failed, so nothing that runs Rhai is rendered: every call into the includes would
    /// fail and bury the real error.
    includes_broken: bool,
    /// The `site` scope variable, built once from the config.
    site_map: rhai::Map,
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

    /// Reads every page and static file. Only a broken config (`reuben.toml` and its `include` files) stops the
    /// load; a bad page, data file, `_dir.toml` or Rhai include is kept in `load_errors` and the rest still loads.
    pub fn load_with(root: &Path, opts: &LoadOptions) -> Result<Site, Vec<Error>> {
        let mut errors = Vec::new();
        let mut pages = Vec::new();

        let config = config::load(root, opts.env.as_deref()).map_err(|e| vec![e])?;
        let mut engine = ScriptEngine::new(root);
        engine.set_base_url(&config.base_url);
        let include_dirs: Vec<PathBuf> = config
            .theme
            .iter()
            .map(|t| t.join("includes"))
            .chain([root.join("includes")])
            .filter(|d| d.is_dir())
            .collect();
        let include_errors = engine
            .load_includes(&include_dirs)
            .err()
            .unwrap_or_default();
        let includes_broken = !include_errors.is_empty();
        errors.extend(include_errors);
        let site_scope = page::site_map(&config).map_err(|e| vec![e])?;

        let (site_data, mut data_errors) =
            data::load(root, (!includes_broken).then_some(&engine), &site_scope);
        errors.append(&mut data_errors);

        let content = root.join("content");
        let (dirs, mut dir_errors) = dirs::load(&content);
        errors.append(&mut dir_errors);
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
                // The broken `_dir.toml` is already reported, and without its defaults a page would only add
                // misleading errors (a missing title, say). Problems in the file itself are its own.
                Err(f)
                    if f.from_defaults
                        && dirs.folder_broken(rel.parent().unwrap_or(Path::new(""))) => {}
                Err(f) => errors.push(f.error),
            }
        }
        mark_draft_excluded(&mut pages, &dirs);
        let content_assets = asset_rels
            .into_iter()
            .map(|rel| {
                let hidden = asset_drafted(&rel, &pages, &dirs);
                (rel, hidden)
            })
            .collect();

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
                    Err(f) => errors.push(f.error),
                }
            }
        }

        let theme_static_files = match config.theme.as_ref() {
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
            (page.pre_block_toc, page.summary) = page::precompute(
                page.kind,
                page.front_matter.description.as_deref(),
                &page.body,
                page.body_line,
            );
        });

        Ok(Site {
            data: site_data,
            root: root.to_path_buf(),
            pages,
            static_files,
            content_assets,
            theme_static_files,
            config,
            engine: Arc::new(engine),
            load_errors: errors,
            broken_dirs: dirs.broken().clone(),
            includes_broken,
            site_map: site_scope,
        })
    }

    /// Renders the whole site into memory. Nothing touches the disk.
    pub fn render(&self, opts: &RenderOptions) -> Result<(Output, BuildReport), Vec<Error>> {
        let (pages, drafts): (Vec<&Page>, Vec<&Page>) = self
            .pages
            .iter()
            .partition(|p| opts.drafts || !p.draft_excluded);

        // Everything below adds to one list, sorted once at the end, so no stage hides another's errors.
        let mut errors = self.load_errors.clone();
        errors.append(&mut self.check_collisions(&pages, opts.drafts));
        let (collections, mut collection_errors) = collections::build(&pages, &self.config);
        errors.append(&mut collection_errors);

        let mut output = Output::default();
        let scopes = page::SiteScopes {
            site: &self.site_map,
            data: &self.data.value,
            collections: &collections,
        };
        let render_one = |page: &&Page| -> Result<String, Vec<Error>> {
            let html = self.render_page(page, &scopes)?;
            splice::ensure_no_placeholders(&page.source, &html).map_err(|e| vec![e])?;
            Ok(html)
        };
        // A pool that can't start is reported, and the build carries on with rayon's default pool.
        let pool =
            opts.jobs.and_then(
                |n| match rayon::ThreadPoolBuilder::new().num_threads(n).build() {
                    Ok(pool) => Some(pool),
                    Err(e) => {
                        errors.push(Error::new(
                            "reuben.toml",
                            ErrorKind::Config(format!("can't start {n} render threads: {e}")),
                        ));
                        None
                    }
                },
            );
        // Skipped pages count as failed with nothing to add, so templated partials still import as empty.
        let rendered: Vec<Result<String, Vec<Error>>> = if self.includes_broken {
            pages.iter().map(|_| Err(Vec::new())).collect()
        } else {
            in_pool(pool.as_ref(), || pages.par_iter().map(render_one).collect())
        };
        let mut css_jobs: Vec<CssJob> = Vec::new();
        // CSS templates are imported as their rendered text. One that failed to load or render is imported as
        // empty, since its own error is already reported and importers would only add noise.
        let mut templates = CssTemplates {
            rendered: self
                .load_errors
                .iter()
                .filter(|e| is_css(&e.path))
                .map(|e| (css::normalize(&e.path), String::new()))
                .collect(),
            unrendered: drafts
                .iter()
                .filter(|p| is_css(&p.route.file))
                .map(|p| css::normalize(&p.source))
                .collect(),
        };
        let mut partial_pages = 0;
        // Merged in page order so neither output nor errors depend on which thread finished first.
        for (page, result) in pages.iter().zip(rendered) {
            let is_partial = is_partial_css(&page.route.file);
            partial_pages += usize::from(is_partial);
            let result = match result {
                Err(_) if self.in_broken_dir(page) => Err(Vec::new()),
                other => other,
            };
            if is_css(&page.route.file) {
                let text = result
                    .as_ref()
                    .map_or_else(|_| String::new(), String::clone);
                templates
                    .rendered
                    .insert(css::normalize(&page.source), text);
            }
            match result {
                Ok(_) if is_partial => {}
                Ok(html) if is_css(&page.route.file) => {
                    css_jobs.push(self.css_job(
                        &page.route.file,
                        &page.source,
                        page.from_static,
                        CssInput::Text(html),
                    ));
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
            if is_css(rel) {
                css_jobs.push(self.css_job(rel, src, false, CssInput::File));
            } else {
                static_copied += 1;
                output
                    .files
                    .insert(rel.clone(), OutputFile::Copy(src.clone()));
            }
        }
        let processed = in_pool(pool.as_ref(), || {
            css_jobs
                .par_iter()
                .map(|job| job.run(&self.config.css, &templates))
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
        let out_error = check_out_dir(&opts.out).err();
        let rendered = self.render(&RenderOptions {
            drafts: opts.drafts,
            jobs: opts.jobs,
        });
        let (output, report) = match (rendered, out_error) {
            (Ok(done), None) => done,
            (Ok(_), Some(e)) => return Err(vec![e]),
            (Err(mut errors), out_error) => {
                errors.extend(out_error);
                Error::sort(&mut errors);
                return Err(errors);
            }
        };
        write_atomic(&output, &opts.out)?;
        Ok(report)
    }

    fn in_broken_dir(&self, page: &Page) -> bool {
        !page.from_static
            && page
                .rel
                .parent()
                .is_some_and(|dir| dirs::under_broken(&self.broken_dirs, dir))
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
                let page::RenderedBody { doc, scope, .. } =
                    page::render_body(page, &self.engine, &self.config, scopes)?;
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
            (Some(path), _) => {
                layout::render_layout(&path, &self.engine, &mut scope, Html::new(body))
            }
            (None, PageKind::Markdown) => Ok(builtin_page(&PageShell {
                title: &page.front_matter.title,
                lang: "en",
                description: page.front_matter.description.as_deref(),
                body: &body,
            })),
            (None, PageKind::Template { .. }) => Ok(body),
        }
    }

    fn css_job(
        &self,
        rel: &Path,
        src: &Path,
        template_from_static: bool,
        input: CssInput,
    ) -> CssJob {
        let (root, overrides) = css::css_root(
            &self.root,
            self.config.theme.as_deref(),
            src,
            template_from_static,
        );
        CssJob {
            rel: rel.to_path_buf(),
            path: src.to_path_buf(),
            root,
            overrides,
            input,
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
    fn check_collisions(&self, pages: &[&Page], drafts: bool) -> Vec<Error> {
        let sources = pages
            .iter()
            .map(|p| (p.route.file.clone(), p.source.clone()))
            .chain(self.static_sources(drafts));

        // Keyed case-insensitively: on macOS and Windows `Notes/` and `notes/` are the same directory, and a site
        // that relies on the difference can't be deployed portably anyway.
        let mut seen: HashMap<String, PathBuf> = HashMap::new();
        let mut errors = Vec::new();
        for (file, source) in sources {
            if file
                .components()
                .next()
                .is_some_and(|c| c.as_os_str() == "__reuben")
            {
                errors.push(Error::new(
                    &source,
                    ErrorKind::ReservedOutputPath(file.clone()),
                ));
            }
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
        errors
    }
}

fn in_pool<R: Send>(pool: Option<&rayon::ThreadPool>, f: impl FnOnce() -> R + Send) -> R {
    match pool {
        Some(pool) => pool.install(f),
        None => f(),
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

/// Why a page didn't load. `from_defaults` marks a failure in the merged front matter, which a broken `_dir.toml`
/// can cause; reading and parsing the file itself never depends on folder defaults.
struct LoadFailure {
    error: Error,
    from_defaults: bool,
}

impl From<Error> for LoadFailure {
    fn from(error: Error) -> Self {
        Self {
            error,
            from_defaults: false,
        }
    }
}

/// Reads a content or static file. Markdown always becomes a page; other files do only if they start with front
/// matter, and `Ok(None)` means the file is not a template.
fn load_template(
    path: &Path,
    rel: &Path,
    dirs: Option<&DirDefaults>,
) -> Result<Option<Page>, LoadFailure> {
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
) -> Result<Page, LoadFailure> {
    let bytes = fs::read(path).map_err(|e| Error::new(path, e))?;
    load_page_from(path, rel, bytes, dirs, kind)
}

fn load_page_from(
    path: &Path,
    rel: &Path,
    bytes: Vec<u8>,
    dirs: Option<&DirDefaults>,
    kind: PageKind,
) -> Result<Page, LoadFailure> {
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
    let front_matter = frontmatter::from_value(merged.value).map_err(|e| LoadFailure {
        error: Error::new(path, e),
        from_defaults: true,
    })?;
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
