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
        headings::toc_array(&pre_block_toc(page)).into(),
    );
    Ok(map)
}

/// The table of contents before blocks run, so it's what page blocks and collection items see.
/// A body whose blocks can't be extracted has none; the page's own render reports that error.
fn pre_block_toc(page: &Page) -> Vec<headings::TocEntry> {
    if page.kind != PageKind::Markdown {
        return Vec::new();
    }
    let skip = markdown::code_ranges(&page.body);
    let Ok(extracted) = blocks::extract(&page.body, page.body_line, &skip) else {
        return Vec::new();
    };
    headings::assign_ids(&mut markdown::parse(&extracted.text, page.body_line))
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

/// Returns the scope so the layout sees sidecar and setup variables.
/// Every block's error is collected rather than stopping at the first.
pub fn render_body(
    page: &Page,
    engine: &ScriptEngine,
    config: &Config,
    scopes: &SiteScopes<'_>,
) -> Result<(Document, Scope<'static>), Vec<Error>> {
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
    Ok((doc, scope))
}
