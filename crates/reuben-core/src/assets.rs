//! Content-hashed names for static files, and the map from source paths to the names they build to.
#![allow(dead_code)] // later tasks use these items

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use xxhash_rust::xxh3::xxh3_64;

/// `rel` with an 8-hex-digit content hash inserted before the final extension.
pub(crate) fn hashed_name(rel: &Path, bytes: &[u8]) -> PathBuf {
    let hash = &format!("{:016x}", xxh3_64(bytes))[..8];
    let stem = rel.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let name = match rel.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}.{hash}.{ext}"),
        None => format!("{stem}.{hash}"),
    };
    rel.with_file_name(name)
}

/// Whether `rel` (relative to the static dir) gets a hashed name.
/// Root files, `.well-known/` files and files matching a keep pattern keep their names.
pub(crate) fn is_hashable(rel: &Path, keep: &[glob::Pattern]) -> bool {
    let parts: Vec<_> = rel.components().collect();
    if parts.len() < 2 || parts[0].as_os_str() == ".well-known" {
        return false;
    }
    let text = rel.to_string_lossy().replace('\\', "/");
    !keep.iter().any(|p| p.matches(&text))
}

/// A reference to a static file that does not exist.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AssetMiss {
    pub spec: String,
}

/// Source path to output path for every static file.
#[derive(Debug)]
pub(crate) struct AssetMap {
    enabled: bool,
    base_path: String,
    files: HashMap<PathBuf, PathBuf>,
}

impl AssetMap {
    /// `base_path` is `""` or like `/docs`; a trailing slash is ignored.
    pub fn new(enabled: bool, base_path: &str) -> Self {
        Self {
            enabled,
            base_path: base_path.trim_end_matches('/').to_string(),
            files: HashMap::new(),
        }
    }

    pub fn insert(&mut self, rel: PathBuf, out: PathBuf) {
        self.files.insert(rel, out);
    }

    pub fn knows(&self, rel: &Path) -> bool {
        self.files.contains_key(rel)
    }

    pub fn output(&self, rel: &Path) -> Option<&Path> {
        self.files.get(rel).map(PathBuf::as_path)
    }

    /// The site URL for `spec` (`css/x.css` or `/css/x.css`, with an optional `?query` or `#fragment`).
    pub fn url(&self, spec: &str) -> Result<String, AssetMiss> {
        let split = spec.find(['?', '#']).unwrap_or(spec.len());
        let (path, suffix) = spec.split_at(split);
        let rel = PathBuf::from(percent_decode(path.trim_start_matches('/')));
        let miss = || AssetMiss {
            spec: spec.to_string(),
        };
        let out = self.files.get(&rel).ok_or_else(miss)?;
        let out = if self.enabled { out } else { &rel };
        let out = out.to_string_lossy().replace('\\', "/");
        Ok(format!("{}/{out}{suffix}", self.base_path))
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_hash(s: &str) -> bool {
        s.len() == 8 && s.chars().all(|c| c.is_ascii_hexdigit())
    }

    #[test]
    fn hashed_name_inserts_hash_before_extension() {
        let a = hashed_name(Path::new("css/tokens.css"), b"a{}");
        let s = a.to_string_lossy().into_owned();
        let mid = s
            .strip_prefix("css/tokens.")
            .and_then(|r| r.strip_suffix(".css"))
            .unwrap();
        assert!(is_hash(mid), "{s}");
        assert_eq!(a, hashed_name(Path::new("css/tokens.css"), b"a{}"));
    }

    #[test]
    fn hashed_name_handles_dots_and_no_extension() {
        let s = hashed_name(Path::new("jquery.min.js"), b"x")
            .to_string_lossy()
            .into_owned();
        assert!(is_hash(
            s.strip_prefix("jquery.min.")
                .unwrap()
                .strip_suffix(".js")
                .unwrap()
        ));
        let s = hashed_name(Path::new("docs/LICENSE-x"), b"x")
            .to_string_lossy()
            .into_owned();
        assert!(is_hash(s.strip_prefix("docs/LICENSE-x.").unwrap()));
    }

    #[test]
    fn hashed_name_changes_with_content() {
        assert_ne!(
            hashed_name(Path::new("a/b.css"), b"1"),
            hashed_name(Path::new("a/b.css"), b"2")
        );
    }

    #[test]
    fn root_well_known_and_keep_are_not_hashable() {
        let keep = [glob::Pattern::new("img/keep-*").unwrap()];
        assert!(!is_hashable(Path::new("robots.txt"), &keep));
        assert!(!is_hashable(Path::new(".well-known/security.txt"), &keep));
        assert!(is_hashable(Path::new("img/a.png"), &keep));
        assert!(!is_hashable(Path::new("img/keep-me.png"), &keep));
    }

    fn map(enabled: bool, base: &str) -> AssetMap {
        let mut m = AssetMap::new(enabled, base);
        m.insert("css/a.css".into(), "css/a.1234abcd.css".into());
        m.insert(
            "fonts/My Font.woff2".into(),
            "fonts/My Font.1234abcd.woff2".into(),
        );
        m
    }

    #[test]
    fn url_adds_base_path_and_keeps_suffix() {
        assert_eq!(
            map(true, "/docs").url("/css/a.css?v=2#x"),
            Ok("/docs/css/a.1234abcd.css?v=2#x".to_string())
        );
        assert_eq!(
            map(true, "").url("css/a.css"),
            Ok("/css/a.1234abcd.css".to_string())
        );
        assert_eq!(
            map(true, "/docs/").url("css/a.css"),
            Ok("/docs/css/a.1234abcd.css".to_string())
        );
    }

    #[test]
    fn url_unknown_is_a_miss() {
        assert_eq!(
            map(true, "").url("css/nope.css"),
            Err(AssetMiss {
                spec: "css/nope.css".into()
            })
        );
    }

    #[test]
    fn url_disabled_returns_source_path() {
        assert_eq!(
            map(false, "").url("css/a.css"),
            Ok("/css/a.css".to_string())
        );
    }

    #[test]
    fn url_decodes_percent_escapes() {
        assert_eq!(
            map(true, "").url("fonts/My%20Font.woff2"),
            Ok("/fonts/My Font.1234abcd.woff2".to_string())
        );
    }
}
