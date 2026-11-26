//! Evaluates a page: sidecar script, then its blocks in one scope, spliced into the document tree.

use std::sync::Arc;

use rhai::serde::to_dynamic;
use rhai::{Dynamic, Map, Scope};

use crate::blocks::{self};
use crate::check;
use crate::collections;
use crate::config::Config;
use crate::doc::Document;
use crate::error::{Error, ErrorKind};
use crate::headings::{self, TocEntry};
use crate::links::{self, AnchorRequest, PageIndex};
use crate::markdown;
use crate::math;
use crate::script::ScriptEngine;
use crate::site::{Page, PageKind};
use crate::splice::{self, Output};
use crate::template::Escape;

/// The `site` scope variable: `title`, `base_url` and `params`.
pub fn site_map(config: &Config) -> Result<Map, Error> {
    let mut map = Map::new();
    map.insert("title".into(), config.title.clone().into());
    map.insert("base_url".into(), config.base_url.clone().into());
    let params = to_dynamic(&config.params).map_err(|e| {
        Error::new(
            "reuben.toml",
            ErrorKind::Config(format!("params can't be passed to scripts: {e}")),
        )
    })?;
    map.insert("params".into(), params);
    Ok(map)
}

/// `toc` is the page's table of contents before blocks run.
pub(crate) fn page_map(page: &Page, config: &Config, toc: &[TocEntry]) -> Result<Map, Error> {
    let fm = &page.front_matter;
    let fail = |e: Box<rhai::EvalAltResult>| {
        Error::new(
            &page.source,
            ErrorKind::Config(format!("front matter can't be passed to scripts: {e}")),
        )
    };
    let mut map = Map::new();
    for (key, value) in &fm.extra {
        map.insert(key.as_str().into(), to_dynamic(value).map_err(fail)?);
    }
    map.insert("title".into(), fm.title.clone().into());
    map.insert("draft".into(), page.draft_excluded.into());
    map.insert("description".into(), opt(fm.description.clone()));
    map.insert("date".into(), opt(fm.date.clone()));
    map.insert("layout".into(), opt(fm.layout.clone()));
    map.insert("weight".into(), opt(fm.weight));
    map.insert("url".into(), page.route.url.clone().into());
    let path = page
        .rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    map.insert("path".into(), path.into());
    let slug = page
        .route
        .url
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    map.insert("slug".into(), slug.to_string().into());
    map.insert(
        "collection".into(),
        collections::public_name(&page.rel, config).into(),
    );
    map.insert("toc".into(), headings::toc_array(toc).into());
    Ok(map)
}

/// What another page sees of this one before it renders: its collection item's `toc` and `summary`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreBlock {
    /// The table of contents before blocks run, which is also the page's own `page.toc` in its sidecar and blocks.
    pub toc: Vec<TocEntry>,
    /// Front matter `description`, else the plain text of the first paragraph with block placeholders removed.
    pub summary: String,
}

/// From one parse of the body. A body whose blocks can't be extracted has neither a toc nor a summary beyond its
/// `description`; the page's own render reports that error.
pub(crate) fn pre_block(page: &Page, math: bool) -> PreBlock {
    let description = page.front_matter.description.as_deref();
    let described = || PreBlock {
        toc: Vec::new(),
        summary: description.map_or_else(String::new, str::to_string),
    };
    if page.kind != PageKind::Markdown {
        return described();
    }
    let Some(doc) = parse_body(page, math) else {
        return described();
    };
    PreBlock {
        toc: headings::toc_before_blocks(&doc),
        summary: match description {
            Some(d) => d.to_string(),
            None => first_paragraph_text(&doc),
        },
    }
}

/// The body with its blocks cut out and parsed, or `None` when the blocks can't be extracted.
fn parse_body(page: &Page, math: bool) -> Option<Document> {
    let extracted = blocks::extract(&page.body, page.body_line, page.code_ranges(math)).ok()?;
    Some(markdown::parse_extracted(&extracted, page.body_line, math))
}

fn first_paragraph_text(doc: &Document) -> String {
    let Some(paragraph) = doc
        .children
        .iter()
        .find(|n| n.kind == crate::doc::NodeKind::Paragraph)
    else {
        return String::new();
    };
    let text = blocks::strip_placeholders(&crate::html::plain_text(paragraph));
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn opt<T: Clone + Send + Sync + 'static>(v: Option<T>) -> Dynamic {
    v.map_or(Dynamic::UNIT, Dynamic::from)
}

/// The site-wide scope variables every page and template sees.
pub struct SiteScopes<'a> {
    pub site: &'a Map,
    pub data: &'a Map,
    pub collections: &'a Map,
    pub pages: &'a Arc<PageIndex>,
}

/// The scope for a page or template: its constants, then the sidecar script's variables.
pub fn build_scope(
    page: &Page,
    engine: &ScriptEngine,
    config: &Config,
    scopes: &SiteScopes<'_>,
) -> Result<Scope<'static>, Vec<Error>> {
    let toc = &page.pre_block(config.markdown.math).toc;
    scope_with_toc(page, engine, config, scopes, toc)
}

/// [`build_scope`] with the page's table of contents before blocks run already in hand.
fn scope_with_toc(
    page: &Page,
    engine: &ScriptEngine,
    config: &Config,
    scopes: &SiteScopes<'_>,
    toc: &[TocEntry],
) -> Result<Scope<'static>, Vec<Error>> {
    let mut scope = Scope::new();
    scope.push_constant("page", page_map(page, config, toc).map_err(|e| vec![e])?);
    scope.push_constant("site", scopes.site.clone());
    scope.push_constant("data", scopes.data.clone());
    scope.push_constant("collections", scopes.collections.clone());

    if let Some(sidecar) = &page.sidecar {
        engine
            .run_file(&mut scope, sidecar)
            .map_err(|e| vec![Error::new(sidecar, ErrorKind::Script(e))])?;
    }
    Ok(scope)
}

/// What `render_body` produces. `scope` is for the layout, which sees sidecar and setup variables. `outputs` are the
/// blocks' outputs in block order, so a `Generated { block, .. }` node maps back to the `Markup` it came from.
pub struct RenderedBody {
    pub doc: Document,
    pub scope: Scope<'static>,
    pub outputs: Vec<Output>,
    /// The `#fragment`s of resolved source links, to check once every page has rendered.
    pub anchors: Vec<AnchorRequest>,
}

/// Why a page didn't render. A page that failed after its links were resolved still hands over its anchor requests,
/// so the pages it links to are checked in the same run.
#[derive(Debug, Default)]
pub struct Failed {
    pub errors: Vec<Error>,
    pub anchors: Vec<AnchorRequest>,
}

impl From<Vec<Error>> for Failed {
    fn from(errors: Vec<Error>) -> Self {
        Self {
            errors,
            anchors: Vec::new(),
        }
    }
}

/// Every block's error is collected rather than stopping at the first.
pub fn render_body(
    page: &Page,
    engine: &ScriptEngine,
    config: &Config,
    scopes: &SiteScopes<'_>,
) -> Result<RenderedBody, Failed> {
    let math = config.markdown.math;
    // The page's own parse gives its toc before blocks run, so its scope needs no parse of its own. A sidecar
    // error still comes before an extraction error, as the sidecar runs first.
    let extracted = blocks::extract(&page.body, page.body_line, page.code_ranges(math));
    let parsed = extracted
        .as_ref()
        .ok()
        .map(|e| markdown::parse_extracted(e, page.body_line, math));
    let pre_block_toc = parsed
        .as_ref()
        .map_or_else(Vec::new, headings::toc_before_blocks);
    let mut scope = scope_with_toc(page, engine, config, scopes, &pre_block_toc)?;
    let extracted = extracted.map_err(|e| vec![Error::new(&page.source, ErrorKind::Block(e))])?;
    let mut doc = parsed.expect("an extracted body is parsed");

    let mut outputs = Vec::with_capacity(extracted.blocks.len());
    let mut errors = Vec::new();
    for block in &extracted.blocks {
        match engine.eval_block(&mut scope, block, &page.source, Escape::Html) {
            Ok(out) => outputs.push(out),
            Err(e) => {
                errors.push(Error::new(&page.source, ErrorKind::Script(e)));
                outputs.push(Output::Nothing);
            }
        }
    }
    // A failed block is spliced as `Output::Nothing`, so link and math errors on the page still surface.
    splice::splice(&mut doc, &outputs, &extracted.map.lines());
    // Ids are assigned first so a heading's slug reads its LaTeX, not the MathML's annotation.
    let toc = headings::toc_array(&headings::assign_ids(&mut doc));
    let (link_problems, anchors) = links::resolve_tree(
        &mut doc,
        &page.rel,
        scopes.pages,
        &check::base_path(&config.base_url),
    );
    errors.extend(link_problems.into_iter().map(|(location, problem)| {
        Error::new(&page.source, ErrorKind::Link { problem, location })
    }));
    if math && let Err(math_errors) = math::render_math(&mut doc) {
        errors.extend(math_errors.into_iter().map(|(location, message)| {
            Error::new(&page.source, ErrorKind::Math { message, location })
        }));
    }
    if !errors.is_empty() {
        return Err(Failed { errors, anchors });
    }

    // Constants can't be reassigned, so the layout's `page` is swapped for a copy carrying the final toc.
    if let Some(mut page_map) = scope.get_value::<Map>("page") {
        page_map.insert("toc".into(), toc.into());
        let _ = scope.remove::<Map>("page");
        scope.push_constant("page", page_map);
    }
    Ok(RenderedBody {
        doc,
        scope,
        outputs,
        anchors,
    })
}
