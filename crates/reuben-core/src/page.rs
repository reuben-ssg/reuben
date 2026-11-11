//! Evaluates a page: sidecar script, then its blocks in one scope, spliced into the document tree.

use rhai::serde::to_dynamic;
use rhai::{Dynamic, Map, Scope};

use crate::blocks::{self};
use crate::collections;
use crate::config::Config;
use crate::doc::Document;
use crate::error::{Error, ErrorKind};
use crate::headings;
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

pub(crate) fn page_map(page: &Page, config: &Config) -> Result<Map, Error> {
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
    map.insert(
        "toc".into(),
        headings::toc_array(&page.pre_block_toc).into(),
    );
    Ok(map)
}

/// From one parse of the body: the table of contents before blocks run, and the summary (front matter
/// `description`, else the plain text of the first paragraph with block placeholders removed). Both are what page
/// blocks and collection items see. A body whose blocks can't be extracted has neither, beyond a `description`;
/// the page's own render reports that error.
pub(crate) fn precompute(
    kind: PageKind,
    description: Option<&str>,
    body: &str,
    body_line: usize,
) -> (Vec<headings::TocEntry>, String) {
    let described = || description.map_or_else(String::new, str::to_string);
    if kind != PageKind::Markdown {
        return (Vec::new(), described());
    }
    let skip = markdown::code_ranges(body);
    let Ok(extracted) = blocks::extract(body, body_line, &skip) else {
        return (Vec::new(), described());
    };
    let mut doc = markdown::parse(&extracted.text, body_line);
    let toc = headings::assign_ids_before_blocks(&mut doc);
    let summary = match description {
        Some(d) => d.to_string(),
        None => first_paragraph_text(&doc),
    };
    (toc, summary)
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
}

/// The scope for a page or template: its constants, then the sidecar script's variables.
pub fn build_scope(
    page: &Page,
    engine: &ScriptEngine,
    config: &Config,
    scopes: &SiteScopes<'_>,
) -> Result<Scope<'static>, Vec<Error>> {
    let mut scope = Scope::new();
    scope.push_constant("page", page_map(page, config).map_err(|e| vec![e])?);
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
}

/// Every block's error is collected rather than stopping at the first.
pub fn render_body(
    page: &Page,
    engine: &ScriptEngine,
    config: &Config,
    scopes: &SiteScopes<'_>,
) -> Result<RenderedBody, Vec<Error>> {
    let mut scope = build_scope(page, engine, config, scopes)?;

    let skip = markdown::code_ranges(&page.body);
    let extracted = blocks::extract(&page.body, page.body_line, &skip)
        .map_err(|e| vec![Error::new(&page.source, ErrorKind::Block(e))])?;
    let mut doc = markdown::parse(&extracted.text, page.body_line);

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
    if !errors.is_empty() {
        return Err(errors);
    }
    splice::splice(&mut doc, &outputs);
    // Ids are assigned first so a heading's slug reads its LaTeX, not the MathML's annotation.
    let toc = headings::toc_array(&headings::assign_ids(&mut doc));
    math::render_math(&mut doc).map_err(|errors| {
        errors
            .into_iter()
            .map(|(location, message)| {
                Error::new(&page.source, ErrorKind::Math { message, location })
            })
            .collect::<Vec<_>>()
    })?;

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
    })
}
