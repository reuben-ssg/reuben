//! Folder defaults: `_dir.toml` files under `content/` that supply front matter for every page beneath them.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use walkdir::WalkDir;

use crate::error::{Error, ErrorKind};
use crate::merge::{Layered, toml_to_json};

const FILE_NAME: &str = "_dir.toml";

#[derive(Debug, Default)]
pub struct DirDefaults {
    /// Keyed by folder path relative to `content/`; the root folder is the empty path.
    folders: BTreeMap<PathBuf, FolderFile>,
    /// Folders whose `_dir.toml` failed to load, so their pages are missing defaults.
    broken: BTreeSet<PathBuf>,
}

#[derive(Debug)]
struct FolderFile {
    value: Map<String, Value>,
    source: PathBuf,
    drafted: bool,
}

/// Reads every `_dir.toml` under `content`. A bad one is reported and its folder loads without defaults.
pub fn load(content: &Path) -> (DirDefaults, Vec<Error>) {
    let mut folders = BTreeMap::new();
    let mut broken = BTreeSet::new();
    let mut errors = Vec::new();
    if content.is_dir() {
        for entry in WalkDir::new(content).sort_by_file_name() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    let path = e.path().unwrap_or(content).to_path_buf();
                    errors.push(Error::new(path, std::io::Error::from(e)));
                    continue;
                }
            };
            if !entry.file_type().is_file() || entry.file_name() != FILE_NAME {
                continue;
            }
            let path = entry.into_path();
            let dir = path
                .parent()
                .and_then(|p| p.strip_prefix(content).ok())
                .expect("walkdir yields paths under its root")
                .to_path_buf();
            match read_folder_file(&path) {
                Ok(file) => {
                    folders.insert(dir, file);
                }
                Err(e) => {
                    broken.insert(dir);
                    errors.push(e);
                }
            }
        }
    }
    (DirDefaults { folders, broken }, errors)
}

fn read_folder_file(path: &Path) -> Result<FolderFile, Error> {
    let text = fs::read_to_string(path).map_err(|e| Error::new(path, e))?;
    let table: toml::Table =
        toml::from_str(&text).map_err(|e| Error::new(path, ErrorKind::Config(e.to_string())))?;
    let Value::Object(value) = toml_to_json(toml::Value::Table(table)) else {
        unreachable!("a TOML table converts to a JSON object");
    };
    let drafted = match value.get("draft") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => {
            return Err(Error::new(
                path,
                ErrorKind::Config("`draft` must be true or false".to_string()),
            ));
        }
    };
    Ok(FolderFile {
        value,
        source: path.to_path_buf(),
        drafted,
    })
}

impl DirDefaults {
    /// The defaults for pages directly in `rel_dir`: the root's `_dir.toml` first, the deepest folder's last.
    pub fn defaults_for(&self, rel_dir: &Path) -> Layered {
        let mut layered = Layered::default();
        let mut chain: Vec<&Path> = rel_dir.ancestors().collect();
        chain.reverse();
        for dir in chain {
            if let Some(file) = self.folders.get(dir) {
                layered.merge(file.value.clone(), &file.source);
            }
        }
        layered
    }

    pub fn broken(&self) -> &BTreeSet<PathBuf> {
        &self.broken
    }

    /// True when `rel_dir` or a folder above it has a `_dir.toml` that failed to load.
    /// Pages there are reported through that file, not again for what its defaults would have supplied.
    pub fn folder_broken(&self, rel_dir: &Path) -> bool {
        rel_dir.ancestors().any(|dir| self.broken.contains(dir))
    }

    /// True when `rel_dir` or any folder above it has a `_dir.toml` with `draft = true`.
    pub fn folder_drafted(&self, rel_dir: &Path) -> bool {
        rel_dir
            .ancestors()
            .any(|dir| self.folders.get(dir).is_some_and(|f| f.drafted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;
    use serde_json::json;

    fn site(files: &[(&str, &str)]) -> TempDir {
        let tmp = TempDir::new().unwrap();
        for (path, text) in files {
            tmp.child(path).write_str(text).unwrap();
        }
        tmp
    }

    #[test]
    fn deeper_folders_override_and_tables_merge() {
        let tmp = site(&[
            ("_dir.toml", "layout = \"a\"\n[params]\nx = 1\ny = 1\n"),
            ("g/_dir.toml", "layout = \"b\"\n[params]\ny = 2\n"),
        ]);
        let (dirs, errors) = load(tmp.path());
        assert!(errors.is_empty());
        let l = dirs.defaults_for(Path::new("g/sub"));
        assert_eq!(
            Value::Object(l.value),
            json!({"layout": "b", "params": {"x": 1, "y": 2}})
        );
        assert!(l.sources["params.x"].ends_with("_dir.toml"));
        assert!(l.sources["layout"].ends_with("g/_dir.toml"));
        assert_eq!(dirs.defaults_for(Path::new("")).value["layout"], json!("a"));
    }

    #[test]
    fn drafted_folder_covers_descendants_only() {
        let tmp = site(&[("g/old/_dir.toml", "draft = true\n"), ("h/x", "")]);
        let (dirs, errors) = load(tmp.path());
        assert!(errors.is_empty());
        assert!(dirs.folder_drafted(Path::new("g/old")));
        assert!(dirs.folder_drafted(Path::new("g/old/deep")));
        assert!(!dirs.folder_drafted(Path::new("g")));
        assert!(!dirs.folder_drafted(Path::new("")));
    }

    #[test]
    fn bad_toml_and_bad_draft_name_their_files() {
        let tmp = site(&[("a/_dir.toml", "x = [\n"), ("b/_dir.toml", "draft = 1\n")]);
        let (dirs, errors) = load(tmp.path());
        assert!(dirs.folder_broken(Path::new("a/x")));
        assert_eq!(errors.len(), 2);
        assert!(errors[0].path.ends_with("a/_dir.toml"));
        assert!(errors[1].path.ends_with("b/_dir.toml"));
    }

    #[test]
    fn missing_content_dir_is_empty() {
        let tmp = TempDir::new().unwrap();
        let (dirs, errors) = load(&tmp.path().join("content"));
        assert!(errors.is_empty());
        assert!(dirs.defaults_for(Path::new("a")).value.is_empty());
    }
}
