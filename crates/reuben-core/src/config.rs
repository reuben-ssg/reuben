//! Site configuration from `reuben.toml`.

use std::path::{Path, PathBuf};

use crate::error::{Error, ErrorKind};

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub title: String,
    pub base_url: String,
    pub theme: Option<PathBuf>,
    pub params: toml::Table,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    title: Option<String>,
    base_url: Option<String>,
    theme: Option<PathBuf>,
    #[serde(default)]
    params: toml::Table,
}

/// A missing `reuben.toml` is fine; a present one with unknown keys is an error so typos surface.
pub fn load(root: &Path) -> Result<Config, Error> {
    let path = root.join("reuben.toml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str::<Raw>(&text)
            .map_err(|e| Error::new(&path, ErrorKind::Config(e.to_string())))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Raw {
            title: None,
            base_url: None,
            theme: None,
            params: toml::Table::new(),
        },
        Err(e) => return Err(Error::new(&path, e)),
    };
    Ok(Config {
        title: raw.title.unwrap_or_default(),
        base_url: raw.base_url.unwrap_or_else(|| "/".into()),
        theme: raw.theme.map(|t| root.join(t)),
        params: raw.params,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    #[test]
    fn missing_file_gives_defaults() {
        let tmp = TempDir::new().unwrap();
        let c = load(tmp.path()).unwrap();
        assert_eq!(c.title, "");
        assert_eq!(c.base_url, "/");
        assert_eq!(c.theme, None);
        assert!(c.params.is_empty());
    }

    #[test]
    fn reads_keys_and_resolves_theme() {
        let tmp = TempDir::new().unwrap();
        tmp.child("reuben.toml")
            .write_str("title = \"T\"\nbase_url = \"/x/\"\ntheme = \"themes/a\"\n[params]\nk = 1\n")
            .unwrap();
        let c = load(tmp.path()).unwrap();
        assert_eq!(c.title, "T");
        assert_eq!(c.base_url, "/x/");
        assert_eq!(c.theme, Some(tmp.path().join("themes/a")));
        assert_eq!(c.params["k"].as_integer(), Some(1));
    }

    #[test]
    fn unknown_key_names_it() {
        let tmp = TempDir::new().unwrap();
        tmp.child("reuben.toml")
            .write_str("titel = \"x\"\n")
            .unwrap();
        let err = load(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("titel"), "{err}");
    }
}
