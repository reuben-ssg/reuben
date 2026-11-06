//! Evaluates a page: sidecar script, then its blocks in one scope, spliced into the document tree.

use rhai::serde::to_dynamic;
use rhai::{Dynamic, Map, Scope};

use crate::blocks::{self};
use crate::collections;
use crate::config::Config;
use crate::doc::Document;
use crate::error::{Error, ErrorKind};
use crate::markdown;
use crate::script::ScriptEngine;
use crate::site::Page;
use crate::splice::{self, Output};

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
    map.insert("draft".into(), fm.draft.into());
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
    Ok(map)
}

fn opt<T: Clone + Send + Sync + 'static>(v: Option<T>) -> Dynamic {
    v.map_or(Dynamic::UNIT, Dynamic::from)
}

/// Returns the scope so the layout sees sidecar and setup variables.
/// Every block's error is collected rather than stopping at the first.
pub fn render_body(
    page: &Page,
    engine: &ScriptEngine,
    config: &Config,
    site: &Map,
    data: &Map,
    collections: &Map,
) -> Result<(Document, Scope<'static>), Vec<Error>> {
    let mut scope = Scope::new();
    scope.push_constant("page", page_map(page, config).map_err(|e| vec![e])?);
    scope.push_constant("site", site.clone());
    scope.push_constant("data", data.clone());
    scope.push_constant("collections", collections.clone());

    if let Some(sidecar) = &page.sidecar {
        engine
            .run_file(&mut scope, sidecar)
            .map_err(|e| vec![Error::new(sidecar, ErrorKind::Script(e))])?;
    }

    let skip = markdown::code_ranges(&page.body);
    let extracted = blocks::extract(&page.body, page.body_line, &skip)
        .map_err(|e| vec![Error::new(&page.source, ErrorKind::Block(e))])?;
    let mut doc = markdown::parse(&extracted.text, page.body_line);

    let mut outputs = Vec::with_capacity(extracted.blocks.len());
    let mut errors = Vec::new();
    for block in &extracted.blocks {
        match engine.eval_block(&mut scope, block, &page.source) {
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
    Ok((doc, scope))
}
