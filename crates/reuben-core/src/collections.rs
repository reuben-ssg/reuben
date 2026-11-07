//! Collections: each top-level `content/` folder as a sorted list of pages and a tree.

use std::collections::BTreeMap;
use std::path::Path;

use rhai::{Array, Dynamic, Map};

use crate::blocks;
use crate::config::Config;
use crate::error::{Error, ErrorKind};
use crate::html::plain_text;
use crate::markdown;
use crate::page::page_map;
use crate::script::collection::{Collection, compare, sort_array};
use crate::site::{Page, PageKind};

/// The first folder under `content/` when the page is inside one; root-level files belong to no collection.
pub fn collection_of(rel: &Path) -> Option<String> {
    let mut parts = rel.components();
    let first = parts.next()?;
    parts.next()?;
    Some(first.as_os_str().to_string_lossy().into_owned())
}

/// The name scripts use for the page's collection, or `""` for none.
pub fn public_name(rel: &Path, config: &Config) -> String {
    collection_of(rel).map_or_else(String::new, |folder| {
        config
            .collections
            .get(&folder)
            .and_then(|c| c.name.clone())
            .unwrap_or(folder)
    })
}

/// The default layout configured for the page's collection.
pub fn layout_of<'a>(rel: &Path, config: &'a Config) -> Option<&'a str> {
    config
        .collections
        .get(&collection_of(rel)?)?
        .layout
        .as_deref()
}

/// Front matter `description`, else the plain text of the first paragraph with block placeholders removed.
/// A body whose blocks can't be extracted has no summary; the page's own render reports that error.
pub(crate) fn compute_summary(
    kind: PageKind,
    description: Option<&str>,
    body: &str,
    body_line: usize,
) -> String {
    if let Some(d) = description {
        return d.to_string();
    }
    if kind != PageKind::Markdown {
        return String::new();
    }
    let skip = markdown::code_ranges(body);
    let Ok(extracted) = blocks::extract(body, body_line, &skip) else {
        return String::new();
    };
    let doc = markdown::parse(&extracted.text, body_line);
    let Some(paragraph) = doc
        .children
        .iter()
        .find(|n| n.kind == crate::doc::NodeKind::Paragraph)
    else {
        return String::new();
    };
    let text = blocks::strip_placeholders(&plain_text(paragraph));
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Markdown pages and `.html` templates under `content/` are collection members; other templates are not.
fn joins_collection(page: &Page) -> bool {
    if page.from_static {
        return false;
    }
    match page.kind {
        PageKind::Markdown => true,
        PageKind::Template { .. } => page.rel.extension().is_some_and(|e| e == "html"),
    }
}

/// Builds every collection from the pages that are in the build. Callers exclude drafts first.
pub fn build(pages: &[&Page], config: &Config) -> Result<Map, Vec<Error>> {
    let mut folders: BTreeMap<String, Vec<&Page>> = BTreeMap::new();
    for page in pages.iter().filter(|p| joins_collection(p)) {
        if let Some(folder) = collection_of(&page.rel) {
            folders.entry(folder).or_default().push(page);
        }
    }

    let mut out = Map::new();
    let mut errors = Vec::new();
    for (folder, group) in folders {
        let settings = config.collections.get(&folder);
        let name = settings
            .and_then(|c| c.name.clone())
            .unwrap_or_else(|| folder.clone());
        let sort = settings.and_then(|c| c.sort.as_deref()).unwrap_or("");

        let mut entries = Vec::with_capacity(group.len());
        for page in group {
            match page_map(page, config) {
                Ok(mut map) => {
                    map.insert("summary".into(), page.summary.clone().into());
                    entries.push((page.rel.as_path(), map));
                }
                Err(e) => errors.push(e),
            }
        }
        let mut items: Array = entries
            .iter()
            .map(|(_, m)| Dynamic::from(m.clone()))
            .collect();
        sort_array(&mut items, sort);
        let tree = tree_node(Path::new(&folder), &entries, sort);

        if out.contains_key(name.as_str()) {
            errors.push(Error::new(
                "reuben.toml",
                ErrorKind::Config(format!(
                    "two collections are named \"{name}\"; rename one under [collections.<folder>]"
                )),
            ));
        } else {
            out.insert(
                name.into(),
                Dynamic::from(Collection::new(items, tree_map(&tree))),
            );
        }
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

struct TreeNode {
    title: String,
    url: String,
    path: String,
    is_dir: bool,
    page: Option<Map>,
    children: Vec<TreeNode>,
}

fn text(map: &Map, key: &str) -> String {
    map.get(key)
        .and_then(|v| v.clone().into_string().ok())
        .unwrap_or_default()
}

fn joined(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// A folder node: taken from its `index.md` when it has one, with the other pages and subfolders as children.
fn tree_node(dir: &Path, entries: &[(&Path, Map)], sort: &str) -> TreeNode {
    let mut index = None;
    let mut children = Vec::new();
    let mut subdirs: BTreeMap<String, Vec<(&Path, Map)>> = BTreeMap::new();
    for (rel, map) in entries {
        let Ok(below) = rel.strip_prefix(dir) else {
            continue;
        };
        let mut parts = below.components();
        let first = parts
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned());
        match (first, parts.next()) {
            (Some(file), None) if file == "index.md" => index = Some(map.clone()),
            (Some(_), None) => children.push(TreeNode {
                title: text(map, "title"),
                url: text(map, "url"),
                path: text(map, "path"),
                is_dir: false,
                page: Some(map.clone()),
                children: Vec::new(),
            }),
            (Some(sub), Some(_)) => subdirs.entry(sub).or_default().push((rel, map.clone())),
            _ => {}
        }
    }
    for (sub, rows) in subdirs {
        children.push(tree_node(&dir.join(sub), &rows, sort));
    }
    children.sort_by(|a, b| compare(sort, a.page.as_ref(), &a.path, b.page.as_ref(), &b.path));

    match index {
        Some(map) => TreeNode {
            title: text(&map, "title"),
            url: text(&map, "url"),
            path: text(&map, "path"),
            is_dir: true,
            page: Some(map),
            children,
        },
        None => TreeNode {
            title: dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            url: String::new(),
            path: joined(dir),
            is_dir: true,
            page: None,
            children,
        },
    }
}

fn tree_map(node: &TreeNode) -> Map {
    let mut map = Map::new();
    map.insert("title".into(), node.title.clone().into());
    map.insert("url".into(), node.url.clone().into());
    map.insert("path".into(), node.path.clone().into());
    map.insert("is_dir".into(), node.is_dir.into());
    let children: Array = node
        .children
        .iter()
        .map(|c| Dynamic::from(tree_map(c)))
        .collect();
    map.insert("children".into(), children.into());
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontmatter::FrontMatter;
    use crate::route::route_for_page;
    use std::path::PathBuf;

    fn page(rel: &str, description: Option<&str>, body: &str) -> Page {
        Page {
            kind: PageKind::Markdown,
            from_static: false,
            source: PathBuf::from("content").join(rel),
            front_matter: FrontMatter {
                title: "T".into(),
                draft: false,
                description: description.map(Into::into),
                date: None,
                layout: None,
                weight: None,
                extra: serde_json::Map::new(),
            },
            body: body.into(),
            body_line: 1,
            rel: PathBuf::from(rel),
            sidecar: None,
            route: route_for_page(Path::new(rel)),
            front_matter_sources: Default::default(),
            draft_excluded: false,
            pre_block_toc: Vec::new(),
            summary: compute_summary(PageKind::Markdown, description, body, 1),
        }
    }

    fn config() -> Config {
        let mut collections = std::collections::BTreeMap::new();
        collections.insert(
            "posts".to_string(),
            crate::config::CollectionConfig {
                name: Some("blog".into()),
                ..Default::default()
            },
        );
        Config {
            title: "T".into(),
            base_url: String::new(),
            theme: None,
            params: toml::Table::new(),
            collections,
            sitemap: false,
            sources: Default::default(),
        }
    }

    #[test]
    fn collection_of_is_first_folder() {
        assert_eq!(
            collection_of(Path::new("posts/a.md")).as_deref(),
            Some("posts")
        );
        assert_eq!(
            collection_of(Path::new("posts/x/a.md")).as_deref(),
            Some("posts")
        );
        assert_eq!(collection_of(Path::new("index.md")), None);
    }

    #[test]
    fn summary_prefers_description_then_first_paragraph() {
        let p = page("guide/v.md", Some("Tone."), "Other.");
        assert_eq!(p.summary, "Tone.");
        let p = page("guide/v.md", None, "Intro {{ x }} text.\n\nMore");
        assert_eq!(p.summary, "Intro text.");
    }

    #[test]
    fn page_knows_its_collection() {
        let p = page("posts/a.md", None, "x");
        let map = crate::page::page_map(&p, &config()).unwrap();
        assert_eq!(map["collection"].clone().into_string().unwrap(), "blog");
        let p = page("index.md", None, "x");
        let map = crate::page::page_map(&p, &config()).unwrap();
        assert_eq!(map["collection"].clone().into_string().unwrap(), "");
    }
}
