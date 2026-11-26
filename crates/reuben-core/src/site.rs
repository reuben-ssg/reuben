//! Loading a site from disk and building it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use walkdir::WalkDir;

use std::ops::Range;
use std::sync::{Arc, OnceLock};

use reuben_a11y::{LocalLink, RuleSet};

use crate::assets::{self, AssetMap};
use crate::check::{self, Finding, TreeReport};
use crate::collections;
use crate::config::{self, Config};
use crate::css::{self, CssInput, CssJob, CssTemplates, is_css, is_partial_css};
use crate::data::{self, SiteData};
use crate::dirs::{self, DirDefaults};
use crate::error::{Error, ErrorKind};
use crate::frontmatter::{self, FrontMatter};
use crate::lastmod;
use crate::layout::Layouts;
use crate::links::{AnchorRequest, LinkProblem, PageIndex, Target};
use crate::output::{Output, OutputFile, Stage, check_out_dir, finish_atomic, prepare_staging};
use crate::page::{Failed, PreBlock};
use crate::route::{Route, route_for_page, route_for_template};
use crate::script::helpers::url_for;
use crate::script::{Html, LinkContext, ScriptEngine, take_asset_misses, with_link_context};
use crate::sitemap;
use crate::template::{self, Escape};
use crate::theme::{PageShell, builtin_page};
use crate::{html, layout, markdown, page, splice};

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
    /// The accessibility rules checked on this page: the config's, minus the page's own `a11y.disable`.
    pub rules: RuleSet,
    /// Parsed only when another page may read it before this one renders: see [`Page::pre_block`].
    pub(crate) pre_block: OnceLock<PreBlock>,
    /// See [`Page::code_ranges`].
    pub(crate) code_ranges: OnceLock<Vec<Range<usize>>>,
}

impl Page {
    /// The table of contents before blocks run and the summary, parsed on first use and kept. A render asks for
    /// them only when a script may read them from another page; the page's own render takes its toc from its own
    /// parse.
    pub fn pre_block(&self, math: bool) -> &PreBlock {
        self.pre_block.get_or_init(|| page::pre_block(self, math))
    }

    /// Byte ranges of the body inside code, where `{{` stays literal, found on first use and kept so the early
    /// parse and the render share them. Empty without a block opener.
    pub(crate) fn code_ranges(&self, math: bool) -> &[Range<usize>] {
        self.code_ranges.get_or_init(|| {
            if markdown::has_blocks(&self.body) {
                markdown::code_ranges_with(&self.body, math)
            } else {
                Vec::new()
            }
        })
    }
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
    /// Markdown files under `content/` (relative to it) that failed to load. Links to them resolve, so the
    /// file's own error isn't repeated on every page that links to it.
    failed_markdown: Vec<PathBuf>,
    /// An `includes/*.rhai` file failed, so nothing that runs Rhai is rendered: every call into the includes would
    /// fail and bury the real error.
    pub(crate) includes_broken: bool,
    /// The `site` scope variable, built once from the config.
    pub(crate) site_map: rhai::Map,
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
    /// Accessibility findings, sorted. They never fail a build.
    pub findings: Vec<Finding>,
}

/// A failed render or build: every error found, plus the accessibility findings of the pages that did render.
/// Derefs to the errors, since they are what made it fail.
#[derive(Debug)]
pub struct Failure {
    pub errors: Vec<Error>,
    pub findings: Vec<Finding>,
}

impl std::ops::Deref for Failure {
    type Target = Vec<Error>;

    fn deref(&self) -> &Vec<Error> {
        &self.errors
    }
}

impl From<Vec<Error>> for Failure {
    fn from(errors: Vec<Error>) -> Self {
        Self {
            errors,
            findings: Vec::new(),
        }
    }
}

/// A page's rendered text and what the checks found in it.
struct Rendered {
    html: String,
    findings: Vec<Finding>,
    /// Local links of the page, to be checked once every output file is known.
    links: Vec<LocalLink>,
    /// Every id in the page, for the anchors other pages link to. `None` when the HTML pass was skipped: the ids are
    /// then collected only if a link asks for one of them.
    ids: Option<HashSet<String>>,
    /// The `#anchor`s this page's source links ask for, to be checked against their target's ids.
    anchors: Vec<AnchorRequest>,
}

impl Rendered {
    fn unchecked(html: String) -> Self {
        Self {
            html,
            findings: Vec::new(),
            links: Vec::new(),
            ids: None,
            anchors: Vec::new(),
        }
    }
}

/// What a build has gathered so far, added to in page order so nothing depends on which thread finished first.
struct Merged<'a> {
    output: Output,
    errors: Vec<Error>,
    /// Each stylesheet to process, and whether it gets a hashed name.
    css_jobs: Vec<(CssJob, bool)>,
    templates: CssTemplates,
    partial_pages: usize,
    findings: Vec<Finding>,
    page_links: Vec<(&'a Page, Vec<LocalLink>)>,
    /// Only Markdown pages are anchor targets, keyed by their path under `content/`.
    page_ids: HashMap<&'a Path, HashSet<String>>,
    /// Markdown pages whose ids weren't collected as they rendered, keyed like `page_ids`.
    unscanned: HashMap<&'a Path, &'a Page>,
    page_anchors: Vec<(&'a Page, Vec<AnchorRequest>)>,
    /// HTML pages were written to staging as they rendered.
    staged: bool,
    /// The `keep` globs when hashing is on.
    keep: Option<Vec<glob::Pattern>>,
}

impl<'a> Merged<'a> {
    fn new(site: &Site, errors: Vec<Error>, drafts: &[&Page], staged: bool) -> Self {
        Self {
            output: Output::default(),
            errors,
            css_jobs: Vec::new(),
            // CSS templates are imported as their rendered text. One that failed to load or render is imported as
            // empty, since its own error is already reported and importers would only add noise.
            templates: CssTemplates {
                rendered: site
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
            },
            partial_pages: 0,
            findings: Vec::new(),
            page_links: Vec::new(),
            page_ids: HashMap::new(),
            unscanned: HashMap::new(),
            page_anchors: Vec::new(),
            staged,
            keep: site
                .config
                .assets
                .hash
                .then(|| site.config.assets.keep_patterns()),
        }
    }

    /// Whether the output at `rel` gets a hashed name.
    fn hashable(&self, rel: &Path) -> bool {
        self.keep
            .as_ref()
            .is_some_and(|keep| assets::is_hashable(rel, keep))
    }
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
        engine.set_markdown(&config.markdown);
        let include_errors = engine
            .load_includes(&include_dirs(root, &config))
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
        let mut failed_markdown = Vec::new();
        let (walked, listing) = walk(&content, &mut errors);
        let to_load: Vec<(PathBuf, PathBuf)> = walked
            .into_iter()
            .filter(|(path, rel)| {
                let is_markdown = path.extension().is_some_and(|e| e == "md");
                if !is_markdown && path.file_name().is_some_and(|n| n == "_dir.toml") {
                    return false;
                }
                // Sidecars hold build scripts, which must never be published.
                if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("rhai"))
                {
                    return false;
                }
                !(rel
                    .components()
                    .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
                    || is_junk(rel))
            })
            .collect();
        // Loaded in parallel; zipping the results back with the walk list keeps walk order.
        let loaded_all: Vec<_> = to_load
            .par_iter()
            .map(|(path, rel)| {
                if path.extension().is_some_and(|e| e == "md") {
                    let kind = PageKind::Markdown;
                    load_page(path, rel, Some(&dirs), &listing, kind, config.a11y).map(Some)
                } else {
                    load_template(path, rel, Some(&dirs), &listing, config.a11y)
                }
            })
            .collect();
        for ((path, rel), loaded) in to_load.into_iter().zip(loaded_all) {
            let is_markdown = path.extension().is_some_and(|e| e == "md");
            match loaded {
                Ok(Some(page)) => pages.push(page),
                Ok(None) => asset_rels.push(rel),
                Err(f) => {
                    if is_markdown {
                        failed_markdown.push(rel.clone());
                    }
                    errors.extend(f.errors);
                    // The broken `_dir.toml` is already reported, and without its defaults a page would only add
                    // misleading errors (a missing title, say). Problems in the file itself are its own.
                    if !dirs.folder_broken(rel.parent().unwrap_or(Path::new(""))) {
                        errors.extend(f.from_defaults);
                    }
                }
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
            let (walked, listing) = walk(&static_dir, &mut errors);
            let to_load: Vec<(PathBuf, PathBuf)> = walked
                .into_iter()
                .filter(|(_, rel)| !is_junk(rel))
                .collect();
            let loaded_all: Vec<_> = to_load
                .par_iter()
                .map(|(path, rel)| load_template(path, rel, None, &listing, config.a11y))
                .collect();
            for ((_, rel), loaded) in to_load.into_iter().zip(loaded_all) {
                match loaded {
                    Ok(Some(mut page)) => {
                        page.draft_excluded = page.front_matter.draft;
                        pages.push(page);
                    }
                    Ok(None) => static_files.push(rel),
                    Err(f) => errors.extend(f.errors),
                }
            }
        }

        let theme_static_files = match config.theme.as_ref() {
            Some(theme) if theme.join("static").exists() => {
                walk(&theme.join("static"), &mut errors)
                    .0
                    .into_iter()
                    .map(|(_, rel)| rel)
                    .filter(|rel| !is_junk(rel))
                    .collect()
            }
            _ => Vec::new(),
        };

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
            failed_markdown,
            includes_broken,
            site_map: site_scope,
        })
    }

    /// Every Markdown page a link can name, drafts included but marked as excluded unless `drafts` is set.
    pub(crate) fn page_index(&self, drafts: bool) -> PageIndex {
        let loaded = self
            .pages
            .iter()
            .filter(|p| p.kind == PageKind::Markdown && !p.from_static)
            .map(|p| {
                (
                    p.rel.clone(),
                    Target {
                        url: p.route.url.clone(),
                        excluded_draft: p.draft_excluded && !drafts,
                    },
                )
            });
        let failed = self.failed_markdown.iter().map(|rel| {
            (
                rel.clone(),
                Target {
                    url: route_for_page(rel).url,
                    excluded_draft: false,
                },
            )
        });
        PageIndex::new(loaded.chain(failed))
    }

    /// Whether any script a render of `pages` may run names `collections`: a page's blocks or template body, a
    /// sidecar, a layout or an include. A script reads a scope variable only by naming it, since `eval` is off and
    /// scripts can't list their scope, so when none does no page can read another's summary or toc. A file that
    /// can't be read counts as naming it.
    fn collections_named(&self, pages: &[&Page]) -> bool {
        const NAME: &str = "collections";
        let names = |path: &Path| fs::read_to_string(path).map_or(true, |text| text.contains(NAME));
        let in_dir = |dir: &Path| match fs::read_dir(dir) {
            Ok(mut entries) => entries.any(|entry| match entry {
                Ok(entry) => {
                    let path = entry.path();
                    path.is_file() && names(&path)
                }
                Err(_) => true,
            }),
            Err(e) => e.kind() != io::ErrorKind::NotFound,
        };
        let in_page = |page: &&Page| {
            let blocks = match page.kind {
                PageKind::Markdown => markdown::has_blocks(&page.body),
                PageKind::Template { .. } => true,
            };
            (blocks && page.body.contains(NAME)) || page.sidecar.as_deref().is_some_and(names)
        };
        let layout_dirs = [Some(self.root.as_path()), self.config.theme.as_deref()]
            .into_iter()
            .flatten()
            .map(|dir| dir.join("layouts"));
        pages.iter().any(in_page)
            || layout_dirs
                .chain(include_dirs(&self.root, &self.config))
                .any(|dir| in_dir(&dir))
    }

    /// Renders the whole site into memory. Nothing touches the disk, and no file is `OutputFile::Staged`.
    pub fn render(&self, opts: &RenderOptions) -> Result<(Output, BuildReport), Failure> {
        self.render_into(opts, None)
    }

    /// Renders the site, writing each page except CSS into `stage` as soon as it renders and keeping only its
    /// `OutputFile::Staged` entry. Without a `stage` nothing touches the disk.
    fn render_into(
        &self,
        opts: &RenderOptions,
        stage: Option<&Path>,
    ) -> Result<(Output, BuildReport), Failure> {
        let (pages, drafts): (Vec<&Page>, Vec<&Page>) = self
            .pages
            .iter()
            .partition(|p| opts.drafts || !p.draft_excluded);

        // Everything below adds to one list, sorted once at the end, so no stage hides another's errors.
        let mut errors = self.load_errors.clone();
        let (mut collision_errors, colliding) = self.check_collisions(&pages, opts.drafts);
        errors.append(&mut collision_errors);
        // A pool that can't start is reported, and the build carries on with rayon's default pool.
        let mut pool_error = None;
        let pool =
            opts.jobs.and_then(
                |n| match rayon::ThreadPoolBuilder::new().num_threads(n).build() {
                    Ok(pool) => Some(pool),
                    Err(e) => {
                        pool_error = Some(Error::new(
                            "reuben.toml",
                            ErrorKind::Config(format!("can't start {n} render threads: {e}")),
                        ));
                        None
                    }
                },
            );
        // Collection items carry each member's summary and toc before blocks run, which takes a parse of its own.
        // Only a script that names `collections` can read them, so without one the items go without.
        let read = self.collections_named(&pages);
        if read {
            let math = self.config.markdown.math;
            in_pool(pool.as_ref(), || {
                pages
                    .par_iter()
                    .filter(|p| collections::is_member(p))
                    .for_each(|p| {
                        p.pre_block(math);
                    });
            });
        }
        let (collections, mut collection_errors) =
            collections::build_with(&pages, &self.config, read);
        errors.append(&mut collection_errors);
        errors.extend(pool_error);

        let index = Arc::new(self.page_index(opts.drafts));
        let scopes = page::SiteScopes {
            site: &self.site_map,
            data: &self.data.value,
            collections: &collections,
            pages: &index,
        };
        let stager = stage.map(|dir| Stage::new(dir.to_path_buf()));
        let layouts = Layouts::new(&self.root, self.config.theme.as_deref());
        let render_one = |page: &&Page, assets: &Arc<AssetMap>| -> Result<Rendered, Failed> {
            let (html, tree, anchors) = self.render_page(page, &scopes, assets, &layouts)?;
            splice::ensure_no_placeholders(&page.source, &html)
                .map_err(|e| Failed::from(vec![e]))?;
            let ids_now = !colliding.is_empty()
                && colliding.contains(&page.route.file.to_string_lossy().to_lowercase());
            let mut done = check_html(page, html, tree, anchors, ids_now);
            if let Some(stage) = stager.as_ref().filter(|_| !is_css(&page.route.file)) {
                if let Err(e) = stage.write(&page.route.file, done.html.as_bytes()) {
                    return Err(Failed {
                        errors: vec![e],
                        anchors: done.anchors,
                    });
                }
                done.html = String::new();
            }
            Ok(done)
        };
        // Skipped pages count as failed with nothing to add, so templated partials still import as empty.
        let render_all =
            |batch: &[&Page], assets: &Arc<AssetMap>| -> Vec<Result<Rendered, Failed>> {
                if self.includes_broken {
                    batch.iter().map(|_| Err(Failed::default())).collect()
                } else {
                    in_pool(pool.as_ref(), || {
                        batch.par_iter().map(|p| render_one(p, assets)).collect()
                    })
                }
            };
        let run_css =
            |jobs: &[(CssJob, bool)], templates: &CssTemplates, assets: Option<&AssetMap>| {
                in_pool(pool.as_ref(), || {
                    jobs.par_iter()
                        .map(|(job, _)| job.run(&self.config.css, templates, assets))
                        .collect::<Vec<_>>()
                })
            };
        let mut m = Merged::new(self, errors, &drafts, stage.is_some());
        let statics = self.static_sources(opts.drafts);
        let mut static_copied = 0;
        // The output names that carry a content hash.
        let mut hashed: HashSet<PathBuf> = HashSet::new();
        if self.config.assets.hash {
            // Each step's output names are known before the next step refers to them: non-CSS files, then
            // stylesheets (which may name those files), then HTML pages (which may name either).
            let content_dir = self.root.join("content");
            let mut map = AssetMap::new(true, &check::base_path(&self.config.base_url));
            let mut copies = Vec::new();
            for (rel, src) in &statics {
                if is_partial_css(rel) {
                    continue;
                }
                // Page-bundle assets under `content/` keep their names.
                let hashable = !src.starts_with(&content_dir) && m.hashable(rel);
                if !is_css(rel) {
                    copies.push((rel, src, hashable));
                } else if let Err(e) = fs::File::open(src) {
                    unreadable(&mut m.errors, &mut map, rel, src, e);
                } else {
                    let job = self.css_job(rel, src, false, CssInput::File);
                    m.css_jobs.push((job, hashable));
                }
            }
            let names = in_pool(pool.as_ref(), || {
                copies
                    .par_iter()
                    .map(|&(rel, src, hashable)| match hashable {
                        true => assets::hashed_file_name(rel, src),
                        // Opened read-only now so unreadable sources are reported with the page errors.
                        false => fs::File::open(src).map(|_| rel.clone()),
                    })
                    .collect::<Vec<_>>()
            });
            for ((rel, src, hashable), name) in copies.into_iter().zip(names) {
                match name {
                    Ok(name) => {
                        static_copied += 1;
                        if hashable {
                            hashed.insert(name.clone());
                        }
                        map.insert(rel.clone(), name.clone());
                        m.output.files.insert(name, OutputFile::Copy(src.clone()));
                    }
                    Err(e) => unreadable(&mut m.errors, &mut map, rel, src, e),
                }
            }

            let (css_pages, html_pages): (Vec<&Page>, Vec<&Page>) =
                pages.iter().partition(|p| is_css(&p.route.file));
            // Stylesheets get their names only after this step, so naming one from a stylesheet says why it can't.
            let stylesheets = m.css_jobs.iter().map(|(job, _)| job.rel.clone()).chain(
                css_pages
                    .iter()
                    .filter(|p| !is_partial_css(&p.route.file))
                    .map(|p| p.route.file.clone()),
            );
            for rel in stylesheets {
                map.add_pending(rel);
            }
            let assets = Arc::new(map);
            let rendered = render_all(&css_pages, &assets);
            self.merge_rendered(&mut m, &css_pages, rendered);

            let mut map = Arc::unwrap_or_clone(assets);
            let jobs = std::mem::take(&mut m.css_jobs);
            let processed = run_css(&jobs, &m.templates, Some(&map));
            for ((job, hashable), result) in jobs.into_iter().zip(processed) {
                match result {
                    Ok(css) => {
                        let name = match hashable {
                            true => assets::hashed_name(&job.rel, css.as_bytes()),
                            false => job.rel.clone(),
                        };
                        map.insert(job.rel, name.clone());
                        if hashable {
                            hashed.insert(name.clone());
                        }
                        m.output
                            .files
                            .insert(name, OutputFile::Bytes(css.into_bytes()));
                    }
                    Err(e) => {
                        m.errors.extend(e);
                        map.insert(job.rel.clone(), job.rel);
                    }
                }
            }
            // A stylesheet that failed to render is already reported, so `asset()` on it isn't a second error.
            for page in &css_pages {
                if !is_partial_css(&page.route.file) && !map.knows(&page.route.file) {
                    map.insert(page.route.file.clone(), page.route.file.clone());
                }
            }

            let rendered = render_all(&html_pages, &Arc::new(map));
            self.merge_rendered(&mut m, &html_pages, rendered);
        } else {
            let rendered = render_all(&pages, &self.asset_map(opts.drafts));
            self.merge_rendered(&mut m, &pages, rendered);
            for (rel, src) in &statics {
                if is_partial_css(rel) {
                    continue;
                }
                // Opened read-only now so unreadable sources are reported with the page errors, not at write time.
                if let Err(e) = fs::File::open(src) {
                    m.errors.push(Error::new(src, e));
                    continue;
                }
                if is_css(rel) {
                    let job = self.css_job(rel, src, false, CssInput::File);
                    m.css_jobs.push((job, false));
                } else {
                    static_copied += 1;
                    m.output
                        .files
                        .insert(rel.clone(), OutputFile::Copy(src.clone()));
                }
            }
            let jobs = std::mem::take(&mut m.css_jobs);
            let processed = run_css(&jobs, &m.templates, None);
            for ((job, _), result) in jobs.into_iter().zip(processed) {
                match result {
                    Ok(css) => {
                        m.output
                            .files
                            .insert(job.rel, OutputFile::Bytes(css.into_bytes()));
                    }
                    Err(e) => m.errors.extend(e),
                }
            }
        }
        let Merged {
            mut output,
            mut errors,
            partial_pages,
            mut findings,
            page_links,
            mut page_ids,
            unscanned,
            page_anchors,
            ..
        } = m;

        let mut warnings = Vec::new();
        if self.config.sitemap && !self.user_provides_sitemap(&pages, &statics) {
            if self.config.base_url.starts_with("http://")
                || self.config.base_url.starts_with("https://")
            {
                let listed: Vec<&&Page> = pages
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
                    .collect();
                let sources: Vec<&Path> = listed.iter().map(|p| p.source.as_path()).collect();
                let dates = lastmod::source_dates(&self.root, &sources);
                let mut urls: Vec<(String, Option<jiff::civil::Date>)> = listed
                    .iter()
                    .map(|p| {
                        (
                            url_for(&self.config.base_url, &p.route.url),
                            dates.get(&p.source).copied(),
                        )
                    })
                    .collect();
                urls.sort_by(|a, b| a.0.cmp(&b.0));
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

        if self.config.assets.headers {
            let existing = statics
                .iter()
                .find(|(rel, _)| rel == Path::new("_headers"))
                .map(|(_, src)| (src, fs::read_to_string(src)));
            let existing = match existing {
                Some((_, Ok(text))) => Some(text),
                Some((src, Err(e))) => {
                    errors.push(Error::new(src, e));
                    None
                }
                None => None,
            };
            let files: Vec<(PathBuf, bool)> = output
                .files
                .keys()
                .map(|name| (name.clone(), hashed.contains(name)))
                .collect();
            let base = check::base_path(&self.config.base_url);
            match assets::headers_file(&base, &files, existing.as_deref()) {
                Ok(text) if !text.is_empty() => {
                    output.files.insert(
                        PathBuf::from("_headers"),
                        OutputFile::Bytes(text.into_bytes()),
                    );
                }
                Ok(_) => {}
                Err(msg) => errors.push(Error::new("reuben.toml", ErrorKind::Config(msg))),
            }
        }
        // The ids of a page whose HTML pass was skipped are collected now, and only if a link asks for one.
        // Scanned in parallel, then added in path order, so the errors don't depend on which thread finished first.
        let wanted: Vec<&Page> = page_anchors
            .iter()
            .flat_map(|(_, anchors)| anchors.iter().map(|a| a.target.as_path()))
            .filter_map(|target| unscanned.get(target).copied())
            .map(|page| (page.rel.as_path(), page))
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect();
        let scanned: Vec<_> = in_pool(pool.as_ref(), || {
            wanted
                .par_iter()
                .map(|page| {
                    built_html(&output, stage, page)
                        .map(|html| html.map(|html| check::page_ids(&page.source, &html)))
                })
                .collect()
        });
        for (page, result) in wanted.into_iter().zip(scanned) {
            match result {
                Some(Ok(ids)) => {
                    page_ids.insert(page.rel.as_path(), ids);
                }
                Some(Err(e)) => errors.push(e),
                None => debug_assert!(
                    false,
                    "{} rendered but isn't in the output",
                    page.rel.display()
                ),
            }
        }
        // Unlike `link-target`, this runs on a failed build too: a target that didn't render is skipped, not blamed.
        for (page, anchors) in &page_anchors {
            for anchor in anchors {
                let missing = page_ids
                    .get(anchor.target.as_path())
                    .is_some_and(|ids| !ids.contains(&anchor.fragment));
                if missing {
                    errors.push(Error::new(
                        &page.source,
                        ErrorKind::Link {
                            problem: LinkProblem::NoAnchor {
                                target: anchor.target.clone(),
                                fragment: anchor.fragment.clone(),
                            },
                            location: anchor.location,
                        },
                    ));
                }
            }
        }

        // A failed build is missing pages, so a link to one would be reported as broken for no fault of the page.
        if errors.is_empty() {
            let base = check::base_path(&self.config.base_url);
            for (page, links) in &page_links {
                findings.extend(check::unbuilt_links(
                    &page.source,
                    &page.route.file,
                    &page.route.url,
                    &base,
                    links,
                    |key| output.files.contains_key(Path::new(key)),
                ));
            }
        }
        check::sort(&mut findings);

        if !errors.is_empty() {
            Error::sort(&mut errors);
            return Err(Failure { errors, findings });
        }
        let report = BuildReport {
            pages_written: pages.len() - partial_pages,
            static_copied,
            drafts_skipped: drafts.len(),
            warnings,
            findings,
        };
        Ok((output, report))
    }

    /// Adds the rendered `batch` to `m`: HTML pages to the output, stylesheets to the CSS jobs.
    fn merge_rendered<'a>(
        &self,
        m: &mut Merged<'a>,
        batch: &[&'a Page],
        rendered: Vec<Result<Rendered, Failed>>,
    ) {
        for (&page, result) in batch.iter().zip(rendered) {
            let is_partial = is_partial_css(&page.route.file);
            m.partial_pages += usize::from(is_partial);
            let result = match result {
                Err(_) if self.in_broken_dir(page) => Err(Failed::default()),
                other => other,
            };
            if is_css(&page.route.file) {
                let text = result
                    .as_ref()
                    .map_or_else(|_| String::new(), |r| r.html.clone());
                m.templates
                    .rendered
                    .insert(css::normalize(&page.source), text);
            }
            match result {
                Ok(_) if is_partial => {}
                Ok(done) if is_css(&page.route.file) => {
                    let job = self.css_job(
                        &page.route.file,
                        &page.source,
                        page.from_static,
                        CssInput::Text(done.html),
                    );
                    let hashable = m.hashable(&page.route.file);
                    m.css_jobs.push((job, hashable));
                }
                Ok(done) => {
                    m.findings.extend(done.findings);
                    m.page_links.push((page, done.links));
                    if page.kind == PageKind::Markdown {
                        match done.ids {
                            Some(ids) => {
                                m.page_ids.insert(page.rel.as_path(), ids);
                            }
                            None => {
                                m.unscanned.insert(page.rel.as_path(), page);
                            }
                        }
                    }
                    m.page_anchors.push((page, done.anchors));
                    let file = if m.staged {
                        OutputFile::Staged
                    } else {
                        OutputFile::Bytes(done.html.into_bytes())
                    };
                    m.output.files.insert(page.route.file.clone(), file);
                }
                Err(mut failed) => {
                    m.errors.append(&mut failed.errors);
                    m.page_anchors.push((page, failed.anchors));
                }
            }
        }
    }

    /// Renders the site and swaps it into `opts.out`. A failed build leaves the previous output untouched.
    pub fn build(&self, opts: &BuildOptions) -> Result<BuildReport, Failure> {
        let staging = check_out_dir(&opts.out).and_then(|()| prepare_staging(&opts.out));
        let rendered = self.render_into(
            &RenderOptions {
                drafts: opts.drafts,
                jobs: opts.jobs,
            },
            staging.as_deref().ok(),
        );
        let (output, report, staging) = match (rendered, staging) {
            (Ok((output, report)), Ok(staging)) => (output, report, staging),
            (Ok((_, report)), Err(e)) => {
                return Err(Failure {
                    errors: vec![e],
                    findings: report.findings,
                });
            }
            (Err(mut failure), staging) => {
                match staging {
                    Ok(staging) => {
                        let _ = fs::remove_dir_all(staging);
                    }
                    Err(e) => failure.errors.push(e),
                }
                Error::sort(&mut failure.errors);
                return Err(failure);
            }
        };
        // Pages are already in `staging`; `out` changes only when the swap succeeds.
        finish_atomic(&output, &opts.out, &staging).map_err(|errors| Failure {
            errors,
            findings: report.findings.clone(),
        })?;
        Ok(report)
    }

    /// The page a user means by a source path relative to the site root (`content/blog/x.md`), a path relative to
    /// `content/` (`blog/x.md`) or a URL (`/blog/x/`). Drafts are found too.
    pub fn find_page(&self, query: &str) -> Option<&Page> {
        let query = query.trim();
        let query = query.strip_prefix("./").unwrap_or(query);
        if query.starts_with('/') {
            let url = query.trim_end_matches('/');
            return self
                .pages
                .iter()
                .find(|p| p.route.url == query || p.route.url.trim_end_matches('/') == url);
        }
        let joined = |p: &Path| {
            p.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        };
        self.pages.iter().find(|p| {
            p.source == Path::new(query)
                || p.source
                    .strip_prefix(&self.root)
                    .is_ok_and(|rel| joined(rel) == query)
                || (!p.from_static && joined(&p.rel) == query)
        })
    }

    pub(crate) fn in_broken_dir(&self, page: &Page) -> bool {
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

    /// Renders one page or template to its final output text, layout included. A Markdown page also returns what the
    /// tree pass found in its final body tree. `layouts` is shared by every page of one render.
    pub(crate) fn render_page(
        &self,
        page: &Page,
        scopes: &page::SiteScopes<'_>,
        assets: &Arc<AssetMap>,
        layouts: &Layouts,
    ) -> Result<(String, Option<TreeReport>, Vec<AnchorRequest>), Failed> {
        // Rayon reuses threads, so misses left by an earlier page must not leak into this one.
        take_asset_misses();
        let result = with_link_context(self.link_context(page, scopes.pages, assets), || {
            self.render_page_in_context(page, scopes, layouts)
        });
        // A miss in a layout or include is reported there, once per call site however many pages use it: `Error::sort`
        // drops the repeats other pages add.
        let mut seen = HashSet::new();
        let misses: Vec<Error> = take_asset_misses()
            .into_iter()
            .filter(|m| seen.insert((m.file.clone(), m.location, m.miss.spec.clone())))
            .map(|m| {
                let path = if m.file.as_os_str().is_empty() {
                    page.source.clone()
                } else {
                    m.file
                };
                let kind = ErrorKind::Asset {
                    spec: m.miss.spec,
                    location: m.location,
                    stylesheet: m.miss.stylesheet,
                };
                Error::new(path, kind)
            })
            .collect();
        if misses.is_empty() {
            return result;
        }
        // A page that rendered still hands over its anchors, so its broken anchors are reported in the same run.
        let mut failed = match result {
            Ok((_, _, anchors)) => Failed {
                errors: Vec::new(),
                anchors,
            },
            Err(failed) => failed,
        };
        failed.errors.extend(misses);
        Err(failed)
    }

    /// A map for `asset()` that names every static file and template stylesheet by its source path. A hashing build
    /// makes its own as it goes.
    pub(crate) fn asset_map(&self, drafts: bool) -> Arc<AssetMap> {
        let mut map = AssetMap::new(
            self.config.assets.hash,
            &check::base_path(&self.config.base_url),
        );
        let stylesheets = self
            .pages
            .iter()
            .filter(|p| (drafts || !p.draft_excluded) && is_css(&p.route.file))
            .map(|p| p.route.file.clone());
        let statics = self.static_sources(drafts).into_iter().map(|(rel, _)| rel);
        for rel in statics.chain(stylesheets) {
            if !is_partial_css(&rel) {
                map.insert(rel.clone(), rel);
            }
        }
        Arc::new(map)
    }

    /// What `url_for` and `markdown()` resolve source links against while `page` is being evaluated.
    pub(crate) fn link_context(
        &self,
        page: &Page,
        index: &Arc<PageIndex>,
        assets: &Arc<AssetMap>,
    ) -> LinkContext {
        LinkContext {
            index: Arc::clone(index),
            page_rel: page.rel.clone(),
            base_path: check::base_path(&self.config.base_url),
            markdown: self.config.markdown.clone(),
            assets: Arc::clone(assets),
        }
    }

    fn render_page_in_context(
        &self,
        page: &Page,
        scopes: &page::SiteScopes<'_>,
        layouts: &Layouts,
    ) -> Result<(String, Option<TreeReport>, Vec<AnchorRequest>), Failed> {
        let (body, mut scope, layout_name, tree, anchors) = match page.kind {
            PageKind::Markdown => {
                let page::RenderedBody {
                    doc,
                    scope,
                    outputs,
                    anchors,
                } = page::render_body(page, &self.engine, &self.config, scopes)?;
                let name = page
                    .front_matter
                    .layout
                    .as_deref()
                    .or_else(|| collections::layout_of(&page.rel, &self.config));
                let tree = check::tree_pass(&page.source, &page.rules, &doc, &outputs);
                (
                    html::render_opts(&doc, &html::HtmlOptions::from(&self.config.markdown)),
                    scope,
                    name,
                    Some(tree),
                    anchors,
                )
            }
            PageKind::Template { escape } => {
                let (text, scope) =
                    template::render(page, escape, &self.engine, &self.config, scopes)?;
                // Folder defaults and collection layouts are meant for pages, so only a layout the template names
                // itself wraps it; otherwise its output is written as-is.
                let own = page.front_matter_sources.get("layout") == Some(&page.source);
                let name = page.front_matter.layout.as_deref().filter(|_| own);
                if name.is_none() {
                    return Ok((text, None, Vec::new()));
                }
                (text, scope, name, None, Vec::new())
            }
        };
        let failed = |errors| Failed {
            errors,
            anchors: anchors.clone(),
        };
        let layout = layouts.find(layout_name).map_err(|mut e| {
            e.path.clone_from(&page.source);
            failed(vec![e])
        })?;
        let html = match (layout, page.kind) {
            (Some(path), _) => {
                layout::render_layout(&path, layouts, &self.engine, &mut scope, Html::new(body))
                    .map_err(failed)?
            }
            (None, PageKind::Markdown) => builtin_page(&PageShell {
                title: &page.front_matter.title,
                lang: "en",
                description: page.front_matter.description.as_deref(),
                body: &body,
                body_has_h1: tree.as_ref().is_some_and(|t| t.has_h1),
            }),
            (None, PageKind::Template { .. }) => body,
        };
        Ok((html, tree, anchors))
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

    /// Fails if two sources (pages or static files) would write the same output file. Also returns the colliding
    /// output paths, lowercased.
    fn check_collisions(&self, pages: &[&Page], drafts: bool) -> (Vec<Error>, HashSet<String>) {
        let sources = pages
            .iter()
            .map(|p| (p.route.file.clone(), p.source.clone()))
            .chain(self.static_sources(drafts));

        // Keyed case-insensitively: on macOS and Windows `Notes/` and `notes/` are the same directory, and a site
        // that relies on the difference can't be deployed portably anyway.
        let mut seen: HashMap<String, PathBuf> = HashMap::new();
        let mut errors = Vec::new();
        let mut colliding = HashSet::new();
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
                colliding.insert(key);
            } else {
                seen.insert(key, source);
            }
        }
        (errors, colliding)
    }
}

/// Runs the HTML pass on a page's finished text, with the tree pass's findings. Only HTML pages are checked:
/// Markdown pages and `.html` templates. Other templates are CSS, XML and the like.
///
/// With every HTML-pass rule off the pass has nothing to find, so it is skipped and the page's ids are left to be
/// collected later, only if a link asks for one. `ids_now` collects them anyway, for a page whose output another
/// file may replace before then.
fn check_html(
    page: &Page,
    html: String,
    tree: Option<TreeReport>,
    anchors: Vec<AnchorRequest>,
    ids_now: bool,
) -> Rendered {
    let is_html = match page.kind {
        PageKind::Markdown => true,
        PageKind::Template { .. } => page.route.file.extension().is_some_and(|e| e == "html"),
    };
    if !is_html {
        return Rendered::unchecked(html);
    }
    let (mut findings, reported_ids) =
        tree.map_or_else(Default::default, |t| (t.findings, t.reported_ids));
    if !page.rules.any_html_rule() {
        let ids = ids_now.then(|| check::page_ids(&page.source, &html));
        return Rendered {
            html,
            findings,
            links: Vec::new(),
            ids,
            anchors,
        };
    }
    let report = check::html_pass(
        &page.source,
        &page.route.file,
        &html,
        &page.rules,
        &reported_ids,
    );
    findings.extend(report.findings);
    Rendered {
        html,
        findings,
        links: report.links,
        ids: Some(report.ids),
        anchors,
    }
}

/// The finished HTML of a rendered page: from `output`, or read back from `stage` if it was streamed there. Its
/// output path must be its own, which a page whose path collides doesn't have. `None` if the page isn't in `output`.
fn built_html(output: &Output, stage: Option<&Path>, page: &Page) -> Option<Result<String, Error>> {
    match (output.files.get(&page.route.file), stage) {
        (Some(OutputFile::Bytes(bytes)), _) => {
            Some(Ok(String::from_utf8_lossy(bytes).into_owned()))
        }
        (Some(OutputFile::Staged), Some(stage)) => {
            let path = stage.join(&page.route.file);
            Some(fs::read_to_string(&path).map_err(|e| Error::new(path, e)))
        }
        _ => None,
    }
}

/// The folders of `*.rhai` includes that exist, lowest precedence first: the theme's, then the site's.
pub(crate) fn include_dirs(root: &Path, config: &Config) -> Vec<PathBuf> {
    config
        .theme
        .iter()
        .map(|t| t.join("includes"))
        .chain([root.join("includes")])
        .filter(|d| d.is_dir())
        .collect()
}

/// Reports an unreadable static file and maps it to its own name, so `asset()` on it isn't a second error.
fn unreadable(errors: &mut Vec<Error>, map: &mut AssetMap, rel: &Path, src: &Path, e: io::Error) {
    errors.push(Error::new(src, e));
    map.insert(rel.to_path_buf(), rel.to_path_buf());
}

fn in_pool<R: Send>(pool: Option<&rayon::ThreadPool>, f: impl FnOnce() -> R + Send) -> R {
    match pool {
        Some(pool) => pool.install(f),
        None => f(),
    }
}

/// Every sidecar path the filesystem was asked about, for tests that count them.
#[cfg(test)]
static SIDECAR_PROBES: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// What a walk saw, so a page's sidecar can be found without asking the filesystem about every page.
///
/// A sidecar the walk listed under its exact name is a file. Otherwise the filesystem may still resolve one: through
/// a symlink, or, on a case- or normalization-insensitive filesystem, under another spelling (`x.RHAI` for `x.md`).
/// Only those cases are asked about, so the answer is always the one `is_file()` would give.
#[derive(Default)]
struct Listing {
    /// Every regular file.
    files: HashSet<PathBuf>,
    /// Every regular file with an ASCII name and a `rhai` extension in any case, with its name lowercased.
    rhai_lower: HashSet<PathBuf>,
    /// Folders the walk can't vouch for: they hold a symlink, a file with a non-ASCII name, or something unreadable.
    ask: HashSet<PathBuf>,
}

impl Listing {
    /// The sibling `<stem>.rhai` of `path`, if it is a file.
    fn sidecar(&self, path: &Path) -> Option<PathBuf> {
        let candidate = path.with_extension("rhai");
        if self.files.contains(&candidate) {
            return Some(candidate);
        }
        let (Some(dir), Some(name)) = (candidate.parent(), candidate.file_name()) else {
            return None;
        };
        let ask = self.ask.contains(dir)
            || !name.is_ascii()
            || self
                .rhai_lower
                .contains(&dir.join(name.to_ascii_lowercase()));
        #[cfg(test)]
        if ask {
            SIDECAR_PROBES.lock().unwrap().push(candidate.clone());
        }
        (ask && candidate.is_file()).then_some(candidate)
    }
}

/// Every file under `dir`, sorted, as (full path, path relative to `dir`), and the listing of what was seen.
fn walk(dir: &Path, errors: &mut Vec<Error>) -> (Vec<(PathBuf, PathBuf)>, Listing) {
    let mut files = Vec::new();
    let mut listing = Listing::default();
    for entry in WalkDir::new(dir).sort_by_file_name() {
        match entry {
            Ok(entry) if entry.file_type().is_file() => {
                let path = entry.into_path();
                if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
                    if !name.is_ascii() {
                        listing.ask.insert(parent.to_path_buf());
                    } else if path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("rhai"))
                    {
                        listing
                            .rhai_lower
                            .insert(parent.join(name.to_ascii_lowercase()));
                    }
                }
                listing.files.insert(path.clone());
                let rel = path
                    .strip_prefix(dir)
                    .expect("walkdir yields paths under its root")
                    .to_path_buf();
                files.push((path, rel));
            }
            Ok(entry) if entry.file_type().is_symlink() => {
                if let Some(parent) = entry.path().parent() {
                    listing.ask.insert(parent.to_path_buf());
                }
            }
            Ok(_) => {}
            Err(e) => {
                let path = e.path().unwrap_or(dir).to_path_buf();
                // What the walk couldn't see may hold a sidecar.
                listing.ask.extend(path.parent().map(Path::to_path_buf));
                listing.ask.insert(path.clone());
                errors.push(Error::new(path, std::io::Error::from(e)));
            }
        }
    }
    (files, listing)
}

/// Why a page didn't load. `from_defaults` are failures in the merged front matter, which a broken `_dir.toml` can
/// cause; reading and parsing the file itself never depends on folder defaults, so `errors` are always reported.
struct LoadFailure {
    errors: Vec<Error>,
    from_defaults: Vec<Error>,
}

impl From<Error> for LoadFailure {
    fn from(error: Error) -> Self {
        Self {
            errors: vec![error],
            from_defaults: Vec::new(),
        }
    }
}

/// Reads a content or static file. Markdown always becomes a page; other files do only if they start with front
/// matter, and `Ok(None)` means the file is not a template.
fn load_template(
    path: &Path,
    rel: &Path,
    dirs: Option<&DirDefaults>,
    listing: &Listing,
    rules: RuleSet,
) -> Result<Option<Page>, LoadFailure> {
    let io = |e: std::io::Error| Error::new(path, e);
    // Most static files are binary, so read just enough to rule them out before loading the rest.
    let mut file = fs::File::open(path).map_err(io)?;
    let mut head = [0u8; 8];
    let n = file.read(&mut head).map_err(io)?;
    let start = head[..n]
        .strip_prefix(b"\xef\xbb\xbf")
        .unwrap_or(&head[..n]);
    if !(start.starts_with(b"---") || start.starts_with(b"+++")) {
        return Ok(None);
    }
    let mut bytes = head[..n].to_vec();
    file.read_to_end(&mut bytes).map_err(io)?;
    if !template::starts_with_front_matter(&bytes) {
        return Ok(None);
    }
    let kind = PageKind::Template {
        escape: template::escape_for(rel),
    };
    load_page_from(path, rel, bytes, dirs, listing, kind, rules).map(Some)
}

fn load_page(
    path: &Path,
    rel: &Path,
    dirs: Option<&DirDefaults>,
    listing: &Listing,
    kind: PageKind,
    rules: RuleSet,
) -> Result<Page, LoadFailure> {
    let bytes = fs::read(path).map_err(|e| Error::new(path, e))?;
    load_page_from(path, rel, bytes, dirs, listing, kind, rules)
}

/// `rules` are the accessibility rules the config leaves on; the page's merged `a11y.disable` switches more off.
/// `listing` is the walk that found `path`, for finding its sidecar.
fn load_page_from(
    path: &Path,
    rel: &Path,
    bytes: Vec<u8>,
    dirs: Option<&DirDefaults>,
    listing: &Listing,
    kind: PageKind,
    rules: RuleSet,
) -> Result<Page, LoadFailure> {
    let src = String::from_utf8(bytes).map_err(|_| Error::new(path, ErrorKind::NotUtf8))?;
    let raw = frontmatter::parse_raw(&src).map_err(|e| Error::new(path, e))?;
    let rel_dir = rel.parent().unwrap_or(Path::new(""));
    let mut merged = dirs.map_or_else(Default::default, |d| d.defaults_for(rel_dir));
    let mut layers = dirs.map_or_else(Vec::new, |d| d.layers_for(rel_dir));
    layers.push((&raw.value, path));
    let rules = page_rules(rules, &layers);
    merged.merge(raw.value.clone(), path);
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
    let (front_matter, rules) = match (frontmatter::from_value(merged.value), rules) {
        (Ok(front_matter), Ok(rules)) => (front_matter, rules),
        (front_matter, rules) => {
            return Err(LoadFailure {
                errors: rules.err().unwrap_or_default(),
                from_defaults: front_matter
                    .err()
                    .map(|e| Error::new(path, e))
                    .into_iter()
                    .collect(),
            });
        }
    };
    let sidecar = listing.sidecar(path);
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
        pre_block: OnceLock::new(),
        code_ranges: OnceLock::new(),
        rules,
    })
}

/// The merged front matter's `a11y` table, which may switch accessibility rules off for the page.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PageA11y {
    #[serde(default)]
    disable: Vec<String>,
}

/// The rules on for a page: `base` minus the `a11y.disable` of every layer, from the outermost `_dir.toml` down to
/// the page's own front matter, so a layer adds to the ones above it. Each layer is a front matter map and the file
/// it came from. Problems are blamed on the layer's own file, and every one is reported.
fn page_rules(
    base: RuleSet,
    layers: &[(&serde_json::Map<String, serde_json::Value>, &Path)],
) -> Result<RuleSet, Vec<Error>> {
    let mut rules = base;
    let mut errors = Vec::new();
    for (layer, file) in layers {
        let Some(table) = layer.get("a11y") else {
            continue;
        };
        match serde_json::from_value::<PageA11y>(table.clone()) {
            Ok(parsed) => {
                let (next, unknown) = check::switch_off(rules, &parsed.disable);
                rules = next;
                errors.extend(
                    unknown
                        .into_iter()
                        .map(|id| Error::new(*file, ErrorKind::UnknownRule(id))),
                );
            }
            Err(e) => errors.push(Error::new(
                *file,
                ErrorKind::Config(format!("`a11y` front matter: {e}")),
            )),
        }
    }
    if errors.is_empty() {
        Ok(rules)
    } else {
        Err(errors)
    }
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
            pre_block: OnceLock::new(),
            code_ranges: OnceLock::new(),
            rules: RuleSet::all(),
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

    const ALL_RULES_OFF: &str = "[a11y]\ndisable = [\"heading-order\",\"link-text\",\"img-alt\",\"heading-id-unique\",\"table-header\",\"html-lang\",\"page-title\",\"single-h1\",\"main-landmark\",\"id-unique\",\"form-label\",\"link-target\"]\n";

    fn site_dir(files: &[(&str, &str)]) -> assert_fs::TempDir {
        use assert_fs::prelude::*;
        let tmp = assert_fs::TempDir::new().unwrap();
        for (path, text) in files {
            tmp.child(path).write_str(text).unwrap();
        }
        tmp
    }

    /// How many pages under `dir` had their finished HTML tokenized.
    fn html_scans(dir: &Path) -> usize {
        check::HTML_SCANS
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.starts_with(dir))
            .count()
    }

    fn render_site(dir: &Path) -> Result<(Output, BuildReport), Failure> {
        Site::load(dir)
            .map_err(Failure::from)?
            .render(&RenderOptions {
                drafts: false,
                jobs: None,
            })
    }

    fn no_anchor_fragments(failure: &Failure) -> Vec<String> {
        failure
            .errors
            .iter()
            .filter_map(|e| match &e.kind {
                ErrorKind::Link {
                    problem: LinkProblem::NoAnchor { fragment, .. },
                    ..
                } => Some(fragment.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn html_pass_is_skipped_when_no_rule_or_anchor_needs_it() {
        let tmp = site_dir(&[
            ("reuben.toml", ALL_RULES_OFF),
            (
                "content/index.md",
                "---\ntitle: Home\n---\n# Home\n\n[b](@/b.md)\n",
            ),
            (
                "content/b.md",
                "---\ntitle: B\n---\n# B\n\n<p id=\"x\"></p><p id=\"x\"></p>\n",
            ),
            (
                "content/raw.html",
                "---\ntitle: Raw\n---\n<p id=\"y\"></p><p id=\"y\"></p><input>\n",
            ),
        ]);
        let (output, report) = render_site(tmp.path()).unwrap();
        assert_eq!(html_scans(tmp.path()), 0);
        assert_eq!(report.findings, []);
        let html = |rel: &str| match output.files.get(Path::new(rel)) {
            Some(OutputFile::Bytes(b)) => String::from_utf8(b.clone()).unwrap(),
            other => panic!("{rel}: {other:?}"),
        };
        assert!(html("index.html").contains("href=\"/b/\""));
        assert!(html("b/index.html").contains(">B</h1>"));
        assert!(html("raw/index.html").contains("<input>"));
    }

    #[test]
    fn skipped_html_pass_reports_what_the_full_pass_would() {
        let mut rules = RuleSet::all();
        for rule in reuben_a11y::Rule::ALL {
            rules.disable(rule);
        }
        let mut page = static_template(None);
        page.kind = PageKind::Markdown;
        page.rules = rules;
        let html = "<html><body><p id=a></p><p id=a></p><input><a href=\"/x/\">x</a></body></html>";
        let full = check::html_pass(
            &page.source,
            &page.route.file,
            html,
            &rules,
            &HashSet::new(),
        );
        let skipped = check_html(&page, html.to_string(), None, Vec::new(), false);
        assert_eq!(skipped.findings, full.findings);
        assert_eq!(skipped.links, full.links);
        assert_eq!(skipped.ids, None);
        assert_eq!(check::page_ids(&page.source, html), full.ids);
    }

    #[test]
    fn broken_anchors_are_reported_with_every_rule_off() {
        let files = [
            ("reuben.toml", ALL_RULES_OFF),
            (
                "content/a.md",
                "---\ntitle: A\n---\n# A\n\n[ok](@/b.md#there) [bad](@/b.md#nope)\n",
            ),
            (
                "content/b.md",
                "---\ntitle: B\n---\n# B\n\n## There\n\n<span id=\"raw\"></span>\n",
            ),
            (
                "content/c.md",
                "---\ntitle: C\n---\n# C\n\n[raw](@/b.md#raw)\n",
            ),
        ];
        let tmp = site_dir(&files);
        let failure = render_site(tmp.path()).unwrap_err();
        assert_eq!(no_anchor_fragments(&failure), ["nope"]);
        assert_eq!(failure.errors.len(), 1);
        // Only the page linked with a fragment is scanned.
        assert_eq!(html_scans(tmp.path()), 1);

        // A staged build reads the target back from staging.
        let tmp = site_dir(&files);
        let site = Site::load(tmp.path()).unwrap();
        let failure = site
            .build(&BuildOptions {
                drafts: false,
                out: tmp.path().join("dist"),
                jobs: None,
            })
            .unwrap_err();
        assert_eq!(no_anchor_fragments(&failure), ["nope"]);
        assert_eq!(failure.errors.len(), 1);
        assert_eq!(html_scans(tmp.path()), 1);
    }

    #[test]
    fn a_colliding_page_is_scanned_for_ids_as_it_renders() {
        // `x/index.md` and `x.md` both write `x/index.html`, and x.md's output replaces it. The anchor is still checked
        // against `x/index.md`'s own HTML.
        let tmp = site_dir(&[
            ("reuben.toml", ALL_RULES_OFF),
            (
                "content/a.md",
                "---\ntitle: A\n---\n# A\n\n[y](@/x/index.md#only-y)\n",
            ),
            ("content/x.md", "---\ntitle: X\n---\n# X\n"),
            (
                "content/x/index.md",
                "---\ntitle: Y\n---\n# Y\n\n## Only Y\n",
            ),
        ]);
        let failure = render_site(tmp.path()).unwrap_err();
        assert!(
            failure
                .errors
                .iter()
                .any(|e| matches!(e.kind, ErrorKind::OutputCollision { .. })),
            "{failure:?}"
        );
        assert_eq!(no_anchor_fragments(&failure), Vec::<String>::new());
    }

    /// A site with the HTML rules on, where `lazy.md` (front matter) and `quiet/` (`_dir.toml`) switch them all off
    /// when `quiet` is set. Each page has an unlabelled input, which `form-label` reports.
    fn mixed_site(quiet: bool, broken: bool) -> assert_fs::TempDir {
        let off: Vec<String> = reuben_a11y::Rule::ALL
            .into_iter()
            .filter(|r| r.runs_on_html())
            .map(|r| format!("\"{}\"", r.id()))
            .collect();
        let off = off.join(", ");
        let page_a11y = if quiet {
            format!("a11y:\n  disable: [{off}]\n")
        } else {
            String::new()
        };
        let dir_a11y = if quiet {
            format!("[a11y]\ndisable = [{off}]\n")
        } else {
            String::new()
        };
        let mut links =
            String::from("[e](@/eager.md#e-ok) [l](@/lazy.md#l-ok) [q](@/quiet/q.md#q-ok)\n");
        if broken {
            links
                .push_str("\n[e](@/eager.md#e-bad) [l](@/lazy.md#l-bad) [q](@/quiet/q.md#q-bad)\n");
        }
        let index = format!("---\ntitle: Home\n---\n# Home\n\n{links}");
        let lazy = format!("---\ntitle: L\n{page_a11y}---\n# L\n\n## L ok\n\n<input>\n");
        site_dir(&[
            ("content/index.md", &index),
            (
                "content/eager.md",
                "---\ntitle: E\n---\n# E\n\n## E ok\n\n<input>\n",
            ),
            ("content/lazy.md", &lazy),
            ("content/quiet/_dir.toml", &dir_a11y),
            (
                "content/quiet/q.md",
                "---\ntitle: Q\n---\n# Q\n\n## Q ok\n\n<input>\n",
            ),
        ])
    }

    fn build_site(dir: &Path) -> Result<BuildReport, Failure> {
        Site::load(dir)
            .map_err(Failure::from)?
            .build(&BuildOptions {
                drafts: false,
                out: dir.join("dist"),
                jobs: None,
            })
    }

    fn dist_files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        WalkDir::new(dir.join("dist"))
            .into_iter()
            .map(Result::unwrap)
            .filter(|e| e.file_type().is_file())
            .map(|e| {
                let rel = e
                    .path()
                    .strip_prefix(dir.join("dist"))
                    .unwrap()
                    .to_path_buf();
                (rel, fs::read(e.path()).unwrap())
            })
            .collect()
    }

    #[test]
    fn a_staged_build_checks_anchors_on_eager_and_lazy_pages_alike() {
        let tmp = mixed_site(true, true);
        let failure = build_site(tmp.path()).unwrap_err();
        let mut got: Vec<(PathBuf, String, String)> = failure
            .errors
            .iter()
            .map(|e| match &e.kind {
                ErrorKind::Link {
                    problem: LinkProblem::NoAnchor { target, fragment },
                    ..
                } => (
                    e.path.clone(),
                    target.display().to_string(),
                    fragment.clone(),
                ),
                other => panic!("unexpected error {other:?}"),
            })
            .collect();
        got.sort();
        let index = tmp.path().join("content/index.md");
        let want = [
            ("eager.md", "e-bad"),
            ("lazy.md", "l-bad"),
            ("quiet/q.md", "q-bad"),
        ]
        .map(|(t, f)| (index.clone(), t.to_string(), f.to_string()));
        assert_eq!(got, want);
        assert!(!tmp.path().join("dist").exists());
        // Only the two quiet pages are scanned after rendering; the rest as they render.
        let scans = |rel: &str| {
            check::HTML_SCANS
                .lock()
                .unwrap()
                .iter()
                .filter(|p| *p == &tmp.path().join("content").join(rel))
                .count()
        };
        for rel in ["index.md", "eager.md", "lazy.md", "quiet/q.md"] {
            assert_eq!(scans(rel), 1, "{rel}");
        }

        let mixed = mixed_site(true, false);
        let report = build_site(mixed.path()).unwrap();
        let all_on = mixed_site(false, false);
        let all_report = build_site(all_on.path()).unwrap();
        assert_eq!(dist_files(mixed.path()), dist_files(all_on.path()));
        let sources = |report: &BuildReport, root: &Path| -> Vec<PathBuf> {
            let mut s: Vec<PathBuf> = report
                .findings
                .iter()
                .map(|f| f.path.strip_prefix(root).unwrap().to_path_buf())
                .collect();
            s.dedup();
            s
        };
        assert_eq!(
            sources(&report, mixed.path()),
            [PathBuf::from("content/eager.md")]
        );
        assert_eq!(
            sources(&all_report, all_on.path()),
            ["content/eager.md", "content/lazy.md", "content/quiet/q.md"].map(PathBuf::from)
        );
    }

    #[test]
    fn each_html_rule_fires_alone() {
        let page = "---\ntitle: T\n---\n<html><body><p id=\"d\"></p><p id=\"d\"></p><input><a href=\"/nope/\">x</a></body></html>\n";
        let html_rules = reuben_a11y::Rule::ALL
            .into_iter()
            .filter(|r| r.runs_on_html());
        for rule in html_rules {
            let others: Vec<String> = reuben_a11y::Rule::ALL
                .into_iter()
                .filter(|r| *r != rule)
                .map(|r| format!("\"{}\"", r.id()))
                .collect();
            let config = format!("[a11y]\ndisable = [{}]\n", others.join(","));
            let tmp = site_dir(&[("reuben.toml", &config), ("content/bad.html", page)]);
            let (_, report) = render_site(tmp.path()).unwrap();
            let rules: Vec<_> = report.findings.iter().map(|f| f.rule).collect();
            assert_eq!(rules, [rule], "{}", rule.id());
            assert_eq!(html_scans(tmp.path()), 1, "{}", rule.id());
        }
    }

    fn sidecar_probes(dir: &Path) -> usize {
        SIDECAR_PROBES
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.starts_with(dir))
            .count()
    }

    fn sidecar_of<'a>(site: &'a Site, rel: &str) -> Option<&'a Path> {
        site.pages
            .iter()
            .find(|p| p.rel == Path::new(rel))
            .unwrap_or_else(|| panic!("no page {rel}"))
            .sidecar
            .as_deref()
    }

    #[test]
    fn sidecars_are_found_from_the_walk_without_probing() {
        let page = "---\ntitle: T\n---\nx\n";
        let tmp = site_dir(&[
            ("content/plain.md", page),
            ("content/with.md", page),
            ("content/with.rhai", "let a = 1;"),
            ("content/sub/deep.md", page),
            ("content/sub/deep.rhai", "let a = 1;"),
            ("static/t.css", "---\ntitle: T\n---\nb {}\n"),
            ("static/t.rhai", "let a = 1;"),
            ("static/u.css", "---\ntitle: U\n---\nb {}\n"),
        ]);
        let site = Site::load(tmp.path()).unwrap();
        assert_eq!(sidecar_of(&site, "plain.md"), None);
        let content = tmp.path().join("content");
        assert_eq!(
            sidecar_of(&site, "with.md"),
            Some(content.join("with.rhai").as_path())
        );
        assert_eq!(
            sidecar_of(&site, "sub/deep.md"),
            Some(content.join("sub/deep.rhai").as_path())
        );
        assert_eq!(
            sidecar_of(&site, "t.css"),
            Some(tmp.path().join("static/t.rhai").as_path())
        );
        assert_eq!(sidecar_of(&site, "u.css"), None);
        assert_eq!(sidecar_probes(tmp.path()), 0);
    }

    #[test]
    fn a_sidecar_the_walk_cant_vouch_for_is_probed() {
        let page = "---\ntitle: T\n---\nx\n";
        let tmp = site_dir(&[
            ("content/case.md", page),
            ("content/case.RHAI", "let a = 1;"),
            ("content/other.md", page),
            ("content/intl/café.md", page),
        ]);
        let content = tmp.path().join("content");
        #[cfg(unix)]
        {
            fs::create_dir_all(content.join("linked")).unwrap();
            fs::write(content.join("linked/l.md"), page).unwrap();
            fs::write(tmp.path().join("real.rhai"), "let a = 1;").unwrap();
            std::os::unix::fs::symlink(tmp.path().join("real.rhai"), content.join("linked/l.rhai"))
                .unwrap();
        }
        let site = Site::load(tmp.path()).unwrap();
        // Whatever the filesystem says, as before: found on a case-insensitive one.
        let case = content.join("case.rhai");
        let expected = case.is_file().then_some(case.as_path());
        assert_eq!(sidecar_of(&site, "case.md"), expected);
        assert_eq!(sidecar_of(&site, "other.md"), None);
        #[cfg(unix)]
        assert_eq!(
            sidecar_of(&site, "linked/l.md"),
            Some(content.join("linked/l.rhai").as_path())
        );
        assert_eq!(sidecar_of(&site, "intl/café.md"), None);
        // `case.md`, `café.md` and the page beside the link are asked about; `other.md` isn't.
        let probed = if cfg!(unix) { 3 } else { 2 };
        assert_eq!(sidecar_probes(tmp.path()), probed);
    }

    #[test]
    fn render_into_streams_pages_but_not_css() {
        use assert_fs::TempDir;
        use assert_fs::prelude::*;
        let tmp = TempDir::new().unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sites/templates");
        tmp.copy_from(fixture, &["**"]).unwrap();
        let stage = tmp.path().join("stage");
        fs::create_dir_all(&stage).unwrap();
        let site = Site::load(tmp.path()).unwrap();
        let opts = RenderOptions {
            drafts: false,
            jobs: None,
        };
        let (output, _) = site.render_into(&opts, Some(&stage)).unwrap();
        let about = Path::new("about/index.html");
        assert_eq!(output.files.get(about), Some(&OutputFile::Staged));
        assert!(
            fs::read_to_string(stage.join(about))
                .unwrap()
                .contains("<h1>About</h1>")
        );
        assert!(matches!(
            output.files.get(Path::new("theme.css")),
            Some(OutputFile::Bytes(_))
        ));
        assert!(!stage.join("theme.css").exists());
    }

    /// How many body parses and code-range parses ran on a page whose body holds `marker`.
    fn parses(marker: &str) -> (usize, usize) {
        use crate::markdown;
        let log = markdown::PARSES.lock().unwrap();
        let count = |pass| {
            log.iter()
                .filter(|(p, text)| *p == pass && text.contains(marker))
                .count()
        };
        (
            count(markdown::Pass::Body),
            count(markdown::Pass::CodeRanges),
        )
    }

    fn html_of(output: &Output, file: &str) -> String {
        match output.files.get(Path::new(file)) {
            Some(OutputFile::Bytes(bytes)) => String::from_utf8(bytes.clone()).unwrap(),
            other => panic!("{file}: {other:?}"),
        }
    }

    #[test]
    fn each_page_is_parsed_once_when_no_script_reads_collections() {
        let tmp = site_dir(&[
            (
                "content/index.md",
                "---\ntitle: Home\n---\nparsed-once-home {{ page.toc.len() }}\n",
            ),
            (
                "content/docs/a.md",
                "---\ntitle: A\n---\n# A\n\nparsed-once-blocks `{{ x }}` {{ 1 + 1 }}\n",
            ),
            (
                "content/docs/b.md",
                "---\ntitle: B\n---\n# B\n\nparsed-once-plain text\n",
            ),
        ]);
        render_site(tmp.path()).unwrap();
        assert_eq!(parses("parsed-once-home"), (1, 1));
        assert_eq!(parses("parsed-once-blocks"), (1, 1));
        assert_eq!(parses("parsed-once-plain"), (1, 0));
    }

    #[test]
    fn only_collection_members_are_parsed_early_and_only_when_collections_are_read() {
        let tmp = site_dir(&[
            (
                "content/index.md",
                "---\ntitle: Home\n---\nparsed-early-home {{ collections.docs.map(|p| p.summary) }}\n",
            ),
            (
                "content/docs/a.md",
                "---\ntitle: A\n---\n# A\n\nparsed-early-blocks `{{ x }}` {{ 1 + 1 }}\n",
            ),
            (
                "content/docs/b.md",
                "---\ntitle: B\n---\n# B\n\nparsed-early-plain text\n",
            ),
        ]);
        let (output, _) = render_site(tmp.path()).unwrap();
        // The home page is in no collection, so nothing reads its summary before it renders.
        assert_eq!(parses("parsed-early-home"), (1, 1));
        // Members are parsed once for their summary and table of contents, and again when they render, but their
        // code ranges are found once.
        assert_eq!(parses("parsed-early-blocks"), (2, 1));
        assert_eq!(parses("parsed-early-plain"), (2, 0));
        let home = html_of(&output, "index.html");
        assert!(home.contains("parsed-early-blocks {{ x }}"), "{home}");
        assert!(home.contains("parsed-early-plain text"), "{home}");
    }

    /// Every member reads every member's summary and table of contents, so under parallel rendering some read a page
    /// that hasn't rendered yet. The page's own `page.toc` is the one before blocks run in its sidecar and blocks,
    /// and the final one in its layout.
    #[test]
    fn early_summaries_and_tables_of_contents_are_the_same_everywhere() {
        let mut files = vec![
            (
                "layouts/default.html".to_string(),
                "<html><body>{{ content }}<p>final-toc {{ page.toc.map(|t| t.id) }}</p></body></html>\n"
                    .to_string(),
            ),
            (
                "content/index.md".to_string(),
                "---\ntitle: Home\n---\n{{ collections.docs.map(|p| p.summary + \"|\" + p.toc.map(|t| t.id)) }}\n"
                    .to_string(),
            ),
        ];
        for i in 0..24 {
            files.push((
                format!("content/docs/p{i:02}.md"),
                format!(
                    "---\ntitle: P{i}\n---\n# Head {i}\n\nSummary {i} with *em* and `{{{{ code }}}}`.\n\n## {{{{ page.title }}}}\n\n## Tail\n\nown-toc {{{{ page.toc.map(|t| t.id) }}}} sidecar {{{{ own }}}}\n\nall {{{{ collections.docs.map(|p| p.summary + \"|\" + p.toc.map(|t| t.id)) }}}}\n"
                ),
            ));
            files.push((
                format!("content/docs/p{i:02}.rhai"),
                "let own = page.toc.len();\n".to_string(),
            ));
        }
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        let tmp = site_dir(&refs);
        let site = Site::load(tmp.path()).unwrap();
        for jobs in [Some(1), Some(4), None] {
            let (output, _) = site
                .render(&RenderOptions {
                    drafts: false,
                    jobs,
                })
                .unwrap();
            let mut pages = vec![html_of(&output, "index.html")];
            for i in 0..24 {
                let html = html_of(&output, &format!("docs/p{i:02}/index.html"));
                assert!(
                    html.contains(&format!("own-toc head-{i}tail sidecar 2")),
                    "{html}"
                );
                assert!(
                    html.contains(&format!("final-toc head-{i}p{i}tail</p>")),
                    "{html}"
                );
                pages.push(html);
            }
            for html in &pages {
                for i in 0..24 {
                    let item = format!(
                        "Summary {i} with em and {{{{ code }}}}.|[&quot;head-{i}&quot;, &quot;tail&quot;]"
                    );
                    assert!(html.contains(&item), "{item} in {html}");
                }
            }
        }
    }

    /// Collections are read from a layout, a sidecar or a template page, with no page block naming them.
    #[test]
    fn collections_read_outside_page_blocks_still_see_summaries() {
        let member = (
            "content/docs/a.md",
            "---\ntitle: A\n---\n# Alpha\n\nAlpha summary.\n",
        );
        let read = "{{ collections.docs.map(|p| p.summary + \"|\" + p.toc.map(|t| t.id)) }}";
        let expected = "Alpha summary.|[&quot;alpha&quot;]";

        let layout = format!("<html><body>{{{{ content }}}}{read}</body></html>\n");
        let tmp = site_dir(&[member, ("layouts/default.html", &layout)]);
        let (output, _) = render_site(tmp.path()).unwrap();
        let html = html_of(&output, "docs/a/index.html");
        assert!(html.contains(expected), "{html}");

        let tmp = site_dir(&[
            member,
            ("content/b.md", "---\ntitle: B\n---\n{{ seen }}\n"),
            (
                "content/b.rhai",
                "let seen = collections.docs.map(|p| p.summary + \"|\" + p.toc.map(|t| t.id));\n",
            ),
        ]);
        let (output, _) = render_site(tmp.path()).unwrap();
        let html = html_of(&output, "b/index.html");
        assert!(html.contains(expected), "{html}");

        let template = format!("---\ntitle: L\n---\n{read}\n");
        let tmp = site_dir(&[member, ("static/list.html", &template)]);
        let (output, _) = render_site(tmp.path()).unwrap();
        let html = html_of(&output, "list/index.html");
        assert!(html.contains(expected), "{html}");
    }
}
