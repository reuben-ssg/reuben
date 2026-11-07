//! Loads `data/` into the nested `data` value shared by every page and layout.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rhai::serde::to_dynamic;
use rhai::{Dynamic, Map, Scope};
use serde_json::Value;
use walkdir::WalkDir;

use crate::error::{Error, ErrorKind};
use crate::merge::{Layered, toml_to_json};
use crate::script::ScriptEngine;
use crate::site::is_junk;

#[derive(Debug, Clone, Default)]
pub struct SiteData {
    pub value: Map,
    /// The file that set each value, keyed by dotted path.
    pub sources: BTreeMap<String, PathBuf>,
}

/// Who owns a key, so a second file claiming it can name the first.
enum Claim {
    File(PathBuf),
    Folder(PathBuf),
}

/// Plain files nest by folder and stem, then each `.rhai` runs once with `data` (plain files only) and `site`.
/// A file that fails is reported and its key left out; the rest still load.
pub fn load(root: &Path, engine: &ScriptEngine, site: &Map) -> (SiteData, Vec<Error>) {
    let dir = root.join("data");
    if !dir.is_dir() {
        return (SiteData::default(), Vec::new());
    }
    let mut errors = Vec::new();
    let mut files = Vec::new();
    // Hidden folders are pruned whole, so nothing inside them is read.
    let visible =
        |e: &walkdir::DirEntry| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.');
    for entry in WalkDir::new(&dir)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(visible)
    {
        match entry {
            Ok(e) if e.file_type().is_file() && !is_junk(e.path()) => files.push(e.into_path()),
            Ok(_) => {}
            Err(e) => {
                let path = e.path().unwrap_or(&dir).to_path_buf();
                errors.push(Error::new(path, ErrorKind::Data(e.to_string())));
            }
        }
    }

    let mut claims: BTreeMap<Vec<String>, Claim> = BTreeMap::new();
    let mut plain = Layered::default();
    let mut scripts = Vec::new();
    for path in files {
        let ext = path.extension().and_then(|e| e.to_str());
        let is_script = ext == Some("rhai");
        if !is_script && !matches!(ext, Some("json" | "toml" | "yaml" | "yml")) {
            continue;
        }
        let keys = key_for(&dir, &path);
        if let Err(message) = claim(&mut claims, &keys, &path) {
            errors.push(Error::new(&path, ErrorKind::Data(message)));
        } else if is_script {
            scripts.push((path, keys));
        } else {
            match parse(&path) {
                Ok(value) => plain.merge(nest(&keys, value), &path),
                Err(message) => errors.push(Error::new(&path, ErrorKind::Data(message))),
            }
        }
    }

    let plain_map = match to_dynamic(Value::Object(plain.value)) {
        Ok(d) => d.cast::<Map>(),
        Err(e) => {
            errors.push(Error::new(&dir, ErrorKind::Data(e.to_string())));
            Map::new()
        }
    };
    let mut value = plain_map.clone();
    let mut sources = plain.sources;
    for (path, keys) in scripts {
        let mut scope = Scope::new();
        scope.push_constant("data", plain_map.clone());
        scope.push_constant("site", site.clone());
        match engine.eval_file(&mut scope, &path) {
            Ok(result) => {
                insert_nested(&mut value, &keys, result);
                sources.insert(keys.join("."), path);
            }
            Err(e) => errors.push(Error::new(&path, ErrorKind::Script(e))),
        }
    }

    (SiteData { value, sources }, errors)
}

/// `data/team/members.toml` becomes `["team", "members"]`.
fn key_for(dir: &Path, path: &Path) -> Vec<String> {
    let rel = path.strip_prefix(dir).unwrap_or(path);
    let mut keys: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if let Some(last) = rel.file_stem() {
        *keys.last_mut().expect("a file has a name") = last.to_string_lossy().into_owned();
    }
    keys
}

fn claim(
    claims: &mut BTreeMap<Vec<String>, Claim>,
    keys: &[String],
    path: &Path,
) -> Result<(), String> {
    let name = |k: &[String]| k.join(".");
    for depth in 1..keys.len() {
        let prefix = &keys[..depth];
        match claims.get(prefix) {
            Some(Claim::File(other)) => {
                return Err(format!(
                    "`{}` is a folder here but a file at {}",
                    name(prefix),
                    other.display()
                ));
            }
            Some(Claim::Folder(_)) => {}
            None => {
                let folder = path.ancestors().nth(keys.len() - depth).unwrap_or(path);
                claims.insert(prefix.to_vec(), Claim::Folder(folder.to_path_buf()));
            }
        }
    }
    match claims.get(keys) {
        Some(Claim::File(other)) => Err(format!(
            "`{}` is already defined by {}",
            name(keys),
            other.display()
        )),
        Some(Claim::Folder(folder)) => Err(format!(
            "`{}` is a file here but a folder at {}",
            name(keys),
            folder.display()
        )),
        None => {
            claims.insert(keys.to_vec(), Claim::File(path.to_path_buf()));
            Ok(())
        }
    }
}

fn parse(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => serde_json::from_str(&text).map_err(|e| e.to_string()),
        Some("toml") => toml::from_str::<toml::Value>(&text)
            .map(toml_to_json)
            .map_err(|e| e.to_string()),
        _ => serde_saphyr::from_str_with_options(
            &text,
            serde_saphyr::options! { strict_booleans: true },
        )
        .map_err(|e| e.to_string()),
    }
}

fn nest(keys: &[String], value: Value) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    match keys {
        [] => {}
        [last] => {
            map.insert(last.clone(), value);
        }
        [first, rest @ ..] => {
            map.insert(first.clone(), Value::Object(nest(rest, value)));
        }
    }
    map
}

/// Every parent key is a folder claim, so it is already a map (or absent) by the time a script lands.
fn insert_nested(map: &mut Map, keys: &[String], value: Dynamic) {
    match keys {
        [] => {}
        [last] => {
            map.insert(last.as_str().into(), value);
        }
        [first, rest @ ..] => {
            let entry = map
                .entry(first.as_str().into())
                .or_insert_with(|| Map::new().into());
            if let Some(mut inner) = entry.write_lock::<Map>() {
                insert_nested(&mut inner, rest, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    fn load_in(dir: &TempDir) -> Result<SiteData, Vec<Error>> {
        match load(dir.path(), &ScriptEngine::new(dir.path()), &Map::new()) {
            (data, errors) if errors.is_empty() => Ok(data),
            (_, errors) => Err(errors),
        }
    }

    #[test]
    fn missing_data_dir_is_empty() {
        let dir = TempDir::new().unwrap();
        assert!(load_in(&dir).unwrap().value.is_empty());
    }

    #[test]
    fn same_stem_in_two_formats_is_an_error_naming_both() {
        let dir = TempDir::new().unwrap();
        dir.child("data/a.json").write_str("{}").unwrap();
        dir.child("data/a.toml").write_str("").unwrap();
        let text = load_in(&dir).unwrap_err()[0].to_string();
        assert!(text.contains("a.toml") && text.contains("a.json"), "{text}");
    }

    #[test]
    fn bad_file_is_named_and_other_types_are_ignored() {
        let dir = TempDir::new().unwrap();
        dir.child("data/bad.json").write_str("{").unwrap();
        dir.child("data/notes.txt").write_str("hi").unwrap();
        let errs = load_in(&dir).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].path.ends_with("bad.json"));
    }

    #[test]
    fn nested_script_lands_beside_plain_data() {
        let dir = TempDir::new().unwrap();
        dir.child("data/team/a.json").write_str("1").unwrap();
        dir.child("data/team/b.rhai").write_str("2").unwrap();
        let d = load_in(&dir).unwrap();
        let team = d.value["team"].clone().cast::<Map>();
        assert_eq!(team["a"].as_int().unwrap(), 1);
        assert_eq!(team["b"].as_int().unwrap(), 2);
    }
}
