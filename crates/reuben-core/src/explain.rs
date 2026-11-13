//! `reuben explain`: every name in scope for one page, with the file and line that set it.
//!
//! Lines are looked up here, when explain runs, so a build never pays for them.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use rhai::{Array, Dynamic, Map};

use crate::collections;
use crate::data;
use crate::error::{Error, ErrorKind};
use crate::locate::{self, Format};
use crate::page::{self, SiteScopes};
use crate::script::collection::Collection;
use crate::script::{helpers, list_rhai};
use crate::site::{Page, Site, include_dirs};

/// Where a name's value was set. A `line` is `None` when the file has no single line for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    FrontMatter { file: PathBuf, line: Option<usize> },
    DirDefaults { file: PathBuf, line: Option<usize> },
    Config { file: PathBuf, line: Option<usize> },
    Data { file: PathBuf, line: Option<usize> },
    Sidecar { file: PathBuf, line: Option<usize> },
    Include { file: PathBuf, line: Option<usize> },
    Computed,
    Collection { name: String },
    Helper,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// The Rhai debug form, cut to fit a table row.
    pub value: String,
    pub source: Source,
}

#[derive(Debug)]
pub struct Explanation {
    pub page: PathBuf,
    pub url: String,
    pub entries: Vec<Entry>,
    /// Errors in the page, its sidecar, its `_dir.toml` files, the includes and the data.
    pub errors: Vec<Error>,
}

const VALUE_WIDTH: usize = 60;

/// `page.*` names Reuben always sets itself, over any front matter of the same name.
const COMPUTED: [&str; 5] = ["url", "path", "slug", "collection", "toc"];

impl Site {
    /// `page` is a source path (`content/blog/x.md`), a path under `content/` (`blog/x.md`) or a URL (`/blog/x/`).
    pub fn explain(&self, page: &str) -> Result<Explanation, Vec<Error>> {
        let Some(found) = self.find_page(page) else {
            return Err(self.not_found(page));
        };
        let mut texts = Texts::default();
        let mut errors = self.errors_for(found);
        let mut entries = Vec::new();

        match page::page_map(found, &self.config) {
            Ok(map) => self.page_entries(found, &map, &mut texts, &mut entries),
            Err(e) => errors.push(e),
        }
        self.site_entries(&mut texts, &mut entries);
        self.data_entries(&mut entries);

        let visible: Vec<&Page> = self.pages.iter().filter(|p| !p.draft_excluded).collect();
        let (collections, _) = collections::build(&visible, &self.config);
        for (name, value) in &collections {
            let count = value
                .clone()
                .try_cast::<Collection>()
                .map_or(0, |c| c.page_count());
            entries.push(Entry {
                name: format!("collections.{name}"),
                value: format!("{count} {}", if count == 1 { "page" } else { "pages" }),
                source: Source::Collection {
                    name: name.to_string(),
                },
            });
        }

        // After a broken include the sidecar and the page's blocks would only fail on the include's account.
        if !self.includes_broken {
            let scopes = SiteScopes {
                site: &self.site_map,
                data: &self.data.value,
                collections: &collections,
            };
            match page::build_scope(found, &self.engine, &self.config, &scopes) {
                // A sidecar that fails fails the render too, with the same error, so only a good one goes on.
                Ok(scope) => {
                    self.sidecar_entries(found, &scope, &mut texts, &mut entries);
                    if let Err(e) = self.render_page(found, &scopes)
                        && !self.in_broken_dir(found)
                    {
                        errors.extend(e);
                    }
                }
                Err(e) => errors.extend(e),
            }
        }
        self.include_entries(&mut entries);
        entries.push(Entry {
            name: "helpers".into(),
            value: helpers::NAMES.join(", "),
            source: Source::Helper,
        });

        Error::sort(&mut errors);
        Ok(Explanation {
            page: found.source.clone(),
            url: found.route.url.clone(),
            entries,
            errors,
        })
    }

    /// A page that failed to load isn't in the site, so what it failed with is the answer.
    fn not_found(&self, query: &str) -> Vec<Error> {
        let query = Path::new(query.strip_prefix("./").unwrap_or(query));
        let candidates = [self.root.join(query), self.root.join("content").join(query)];
        let broken: Vec<Error> = self
            .load_errors
            .iter()
            .filter(|e| candidates.contains(&e.path))
            .cloned()
            .collect();
        if broken.is_empty() {
            vec![Error::new(
                &self.root,
                ErrorKind::PageNotFound(query.display().to_string()),
            )]
        } else {
            broken
        }
    }

    /// The load errors that belong to this page: its own files, the `_dir.toml` files above it, the includes and
    /// the data. Errors in other pages and static files don't.
    fn errors_for(&self, page: &Page) -> Vec<Error> {
        let content = self.root.join("content");
        let rel_dir = page.rel.parent().unwrap_or(Path::new(""));
        let dir_files: Vec<PathBuf> = if page.from_static {
            Vec::new()
        } else {
            rel_dir
                .ancestors()
                .map(|d| content.join(d).join("_dir.toml"))
                .collect()
        };
        let includes = include_dirs(&self.root, &self.config);
        let data = self.root.join("data");
        self.load_errors
            .iter()
            .filter(|e| {
                e.path == page.source
                    || page.sidecar.as_ref() == Some(&e.path)
                    || dir_files.contains(&e.path)
                    || e.path.starts_with(&data)
                    || includes.iter().any(|d| e.path.starts_with(d))
            })
            .cloned()
            .collect()
    }

    /// The front matter names, then the computed ones, as the page's own scope has them. Collections give each page
    /// a `summary` too, but the page's scope only has one its front matter sets.
    fn page_entries(&self, page: &Page, map: &Map, texts: &mut Texts, out: &mut Vec<Entry>) {
        for (key, file) in &page.front_matter_sources {
            let path: Vec<&str> = key.split('.').collect();
            if COMPUTED.contains(&key.as_str()) {
                continue;
            }
            let Some(value) = lookup(map, &path) else {
                continue;
            };
            let own = *file == page.source;
            let line = if own {
                texts.get(file).and_then(|t| front_matter_line(t, key))
            } else {
                texts.line(file, Format::Toml, key)
            };
            let file = file.clone();
            let source = if own {
                Source::FrontMatter { file, line }
            } else {
                Source::DirDefaults { file, line }
            };
            out.push(entry(format!("page.{key}"), &value, source));
        }
        // `draft` is computed only when no front matter sets it: a page can be drafted by its folder.
        let draft = (!page.front_matter_sources.contains_key("draft")).then_some("draft");
        for key in draft.into_iter().chain(COMPUTED) {
            if let Some(value) = lookup(map, &[key]) {
                out.push(entry(format!("page.{key}"), &value, Source::Computed));
            }
        }
    }

    fn site_entries(&self, texts: &mut Texts, out: &mut Vec<Entry>) {
        let params = self
            .config
            .sources
            .keys()
            .filter(|k| *k == "params" || k.starts_with("params."));
        for key in ["title", "base_url"]
            .into_iter()
            .chain(params.map(String::as_str))
        {
            let path: Vec<&str> = key.split('.').collect();
            let Some(value) = lookup(&self.site_map, &path) else {
                continue;
            };
            let source = match self.config.sources.get(key) {
                Some(file) => Source::Config {
                    file: file.clone(),
                    line: texts.line(file, Format::Toml, key),
                },
                // Not set by any file: the built-in default.
                None => Source::Computed,
            };
            out.push(entry(format!("site.{key}"), &value, source));
        }
    }

    /// One entry per data file, named by where the file nests.
    fn data_entries(&self, out: &mut Vec<Entry>) {
        let dir = self.root.join("data");
        let files: BTreeMap<Vec<String>, &PathBuf> = self
            .data
            .sources
            .values()
            .map(|file| (data::key_for(&dir, file), file))
            .collect();
        for (keys, file) in files {
            let path: Vec<&str> = keys.iter().map(String::as_str).collect();
            if let Some(value) = lookup(&self.data.value, &path) {
                let source = Source::Data {
                    file: file.clone(),
                    line: None,
                };
                out.push(entry(format!("data.{}", keys.join(".")), &value, source));
            }
        }
    }

    fn sidecar_entries(
        &self,
        page: &Page,
        scope: &rhai::Scope<'static>,
        texts: &mut Texts,
        out: &mut Vec<Entry>,
    ) {
        let Some(file) = &page.sidecar else {
            return;
        };
        // The scope starts with `page`, `site`, `data` and `collections`. A name declared twice keeps its last value.
        let mut variables: Vec<(&str, Dynamic)> = Vec::new();
        for (name, _, value) in scope.iter().skip(4) {
            variables.retain(|(earlier, _)| *earlier != name);
            variables.push((name, value));
        }
        for (name, value) in variables {
            let line = texts.get(file).and_then(|t| locate::let_line(t, name));
            let source = Source::Sidecar {
                file: file.clone(),
                line,
            };
            out.push(entry(name.to_string(), &value, source));
        }
    }

    /// The functions of the include files, as `name/arity`. A site include replaces a theme one of the same name
    /// and arity, as it does when a page calls it.
    fn include_entries(&self, out: &mut Vec<Entry>) {
        let mut functions: BTreeMap<(String, usize), (PathBuf, locate::FnDecl)> = BTreeMap::new();
        for dir in include_dirs(&self.root, &self.config) {
            let mut files = list_rhai(&dir).unwrap_or_default();
            files.sort();
            for file in files {
                let Ok(text) = fs::read_to_string(&file) else {
                    continue;
                };
                for decl in locate::fn_decls(&text) {
                    functions.insert((decl.name.clone(), decl.params.len()), (file.clone(), decl));
                }
            }
        }
        for ((name, arity), (file, decl)) in functions {
            out.push(Entry {
                name: format!("{name}/{arity}"),
                value: format!("({})", decl.params.join(", ")),
                source: Source::Include {
                    file,
                    line: Some(decl.line),
                },
            });
        }
    }
}

fn entry(name: String, value: &Dynamic, source: Source) -> Entry {
    Entry {
        name,
        value: show(value),
        source,
    }
}

/// The value in Rhai's debug form. One too long for a row is cut, and a long array or map is counted instead.
fn show(value: &Dynamic) -> String {
    let text = format!("{value:?}");
    if text.chars().count() <= VALUE_WIDTH {
        return text;
    }
    if let Some(items) = value.read_lock::<Array>() {
        return format!("[{} items]", items.len());
    }
    if let Some(entries) = value.read_lock::<Map>() {
        return format!("#{{{} keys}}", entries.len());
    }
    text.chars().take(VALUE_WIDTH - 1).chain(['…']).collect()
}

/// The value at `path` inside nested maps.
fn lookup(map: &Map, path: &[&str]) -> Option<Dynamic> {
    let (first, rest) = path.split_first()?;
    let mut value = map.get(*first)?.clone();
    for key in rest {
        value = value.try_cast::<Map>()?.get(*key)?.clone();
    }
    Some(value)
}

/// The line of `key` in a page file, counting the delimiter line the front matter starts after.
fn front_matter_line(src: &str, key: &str) -> Option<usize> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let mut lines = src.lines();
    let delimiter = lines.next()?;
    let format = match delimiter {
        "---" => Format::Yaml,
        "+++" => Format::Toml,
        _ => return None,
    };
    let block: Vec<&str> = lines.take_while(|l| *l != delimiter).collect();
    locate::line_of(&block.join("\n"), format, key).map(|line| line + 1)
}

/// File contents, read once each and only when a line is wanted.
#[derive(Default)]
struct Texts(HashMap<PathBuf, Option<String>>);

impl Texts {
    fn get(&mut self, file: &Path) -> Option<&str> {
        self.0
            .entry(file.to_path_buf())
            .or_insert_with(|| fs::read_to_string(file).ok())
            .as_deref()
    }

    fn line(&mut self, file: &Path, format: Format, key: &str) -> Option<usize> {
        self.get(file)
            .and_then(|text| locate::line_of(text, format, key))
    }
}
