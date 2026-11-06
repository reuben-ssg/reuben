//! Loading a site from disk and building it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::doc::Document;
use crate::error::{Error, ErrorKind};
use crate::frontmatter::{self, FrontMatter};
use crate::route::{Route, route_for_page};
use crate::theme::{PageShell, builtin_page};
use crate::{html, markdown};

/// Written into every output directory Reuben builds. Only a directory containing it is ever cleared.
pub const OUTPUT_MARKER: &str = ".reuben-output";

#[derive(Debug)]
pub struct Page {
    pub source: PathBuf,
    pub front_matter: FrontMatter,
    pub doc: Document,
    pub route: Route,
}

#[derive(Debug)]
pub struct Site {
    pub root: PathBuf,
    pub pages: Vec<Page>,
    /// Paths relative to `static/`.
    pub static_files: Vec<PathBuf>,
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
    /// Reads every page and static file, reporting all bad files at once rather than stopping at the first.
    pub fn load(root: &Path) -> Result<Site, Vec<Error>> {
        let mut errors = Vec::new();
        let mut pages = Vec::new();

        let content = root.join("content");
        for (path, rel) in walk(&content, &mut errors) {
            if path.extension().is_some_and(|e| e == "md") {
                match load_page(&path, &rel) {
                    Ok(page) => pages.push(page),
                    Err(e) => errors.push(e),
                }
            }
        }

        let static_dir = root.join("static");
        let static_files = if static_dir.exists() {
            walk(&static_dir, &mut errors)
                .into_iter()
                .map(|(_, rel)| rel)
                .collect()
        } else {
            Vec::new()
        };

        if errors.is_empty() {
            Ok(Site {
                root: root.to_path_buf(),
                pages,
                static_files,
            })
        } else {
            Err(errors)
        }
    }

    pub fn build(&self, opts: &BuildOptions) -> Result<BuildReport, Vec<Error>> {
        let (pages, drafts): (Vec<&Page>, Vec<&Page>) = self
            .pages
            .iter()
            .partition(|p| opts.drafts || !p.front_matter.draft);

        self.check_collisions(&pages)?;
        prepare_out_dir(&opts.out).map_err(|e| vec![e])?;

        let mut errors = Vec::new();
        for page in &pages {
            let body = html::render(&page.doc);
            let shell = PageShell {
                title: &page.front_matter.title,
                lang: "en",
                description: page.front_matter.description.as_deref(),
                body: &body,
            };
            let dest = opts.out.join(&page.route.file);
            if let Err(e) = write(&dest, builtin_page(&shell).as_bytes()) {
                errors.push(Error::new(&dest, e));
            }
        }
        for rel in &self.static_files {
            let dest = opts.out.join(rel);
            let copied = dest
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|()| fs::copy(self.root.join("static").join(rel), &dest));
            if let Err(e) = copied {
                errors.push(Error::new(&dest, e));
            }
        }

        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(BuildReport {
            pages_written: pages.len(),
            static_copied: self.static_files.len(),
            drafts_skipped: drafts.len(),
        })
    }

    /// Fails if two sources (pages or static files) would write the same output file.
    fn check_collisions(&self, pages: &[&Page]) -> Result<(), Vec<Error>> {
        let static_dir = self.root.join("static");
        let sources = pages
            .iter()
            .map(|p| (p.route.file.clone(), p.source.clone()))
            .chain(
                self.static_files
                    .iter()
                    .map(|rel| (rel.clone(), static_dir.join(rel))),
            );

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

fn load_page(path: &Path, rel: &Path) -> Result<Page, Error> {
    let bytes = fs::read(path).map_err(|e| Error::new(path, e))?;
    let src = String::from_utf8(bytes).map_err(|_| Error::new(path, ErrorKind::NotUtf8))?;
    let parsed = frontmatter::parse(&src).map_err(|e| Error::new(path, e))?;
    Ok(Page {
        source: path.to_path_buf(),
        doc: markdown::parse(parsed.body, parsed.body_line),
        front_matter: parsed.front_matter,
        route: route_for_page(rel),
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
