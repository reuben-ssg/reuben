//! Content-hashed names for static files, and the map from source paths to the names they build to.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use xxhash_rust::xxh3::{Xxh3Default, xxh3_64};

/// `rel` with an 8-hex-digit content hash inserted before the final extension.
pub(crate) fn hashed_name(rel: &Path, bytes: &[u8]) -> PathBuf {
    name_with_hash(rel, xxh3_64(bytes))
}

/// [`hashed_name`] for the file at `src`, read in chunks rather than all at once.
pub(crate) fn hashed_file_name(rel: &Path, src: &Path) -> io::Result<PathBuf> {
    let mut file = File::open(src)?;
    let mut hasher = Xxh3Default::new();
    let mut buf = vec![0; 64 * 1024];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(name_with_hash(rel, hasher.digest()))
}

fn name_with_hash(rel: &Path, digest: u64) -> PathBuf {
    let hash = &format!("{digest:016x}")[..8];
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
pub struct AssetMiss {
    pub spec: String,
}

/// Source path to output path for every static file.
#[derive(Debug, Clone)]
pub struct AssetMap {
    enabled: bool,
    base_path: String,
    files: HashMap<PathBuf, PathBuf>,
    /// The URL of every entry in `files`, as [`AssetMap::url`] returns it without a suffix.
    urls: HashSet<String>,
}

impl AssetMap {
    /// `base_path` is `""` or like `/docs`; a trailing slash is ignored.
    pub fn new(enabled: bool, base_path: &str) -> Self {
        Self {
            enabled,
            base_path: base_path.trim_end_matches('/').to_string(),
            files: HashMap::new(),
            urls: HashSet::new(),
        }
    }

    pub fn insert(&mut self, rel: PathBuf, out: PathBuf) {
        let url = self.site_url(&rel, &out);
        if let Some(old) = self.files.insert(rel.clone(), out) {
            let old = self.site_url(&rel, &old);
            self.urls.remove(&old);
        }
        self.urls.insert(url);
    }

    /// Whether `value` is already the URL of a file in the map (as `asset()` returns it), ignoring any `?query` or
    /// `#fragment`.
    pub fn is_url(&self, value: &str) -> bool {
        let split = value.find(['?', '#']).unwrap_or(value.len());
        self.urls.contains(&value[..split])
    }

    fn site_url(&self, rel: &Path, out: &Path) -> String {
        let out = if self.enabled { out } else { rel };
        let out = out.to_string_lossy().replace('\\', "/");
        format!("{}/{}", self.base_path, percent_encode(&out))
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
        Ok(format!("{}{suffix}", self.site_url(&rel, out)))
    }
}

/// Encodes everything but ASCII alphanumerics and `-._~/`, so a path with spaces or non-ASCII is a valid URL.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
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
    fn is_url_knows_output_urls_only() {
        let mut m = map(true, "/docs");
        assert!(m.is_url("/docs/css/a.1234abcd.css"));
        assert!(m.is_url("/docs/css/a.1234abcd.css?v=1#x"));
        assert!(m.is_url("/docs/fonts/My%20Font.1234abcd.woff2"));
        assert!(!m.is_url("/css/a.1234abcd.css"));
        assert!(!m.is_url("/docs/css/a.css"));
        // A replaced entry's old URL is forgotten.
        m.insert("css/a.css".into(), "css/a.ffffffff.css".into());
        assert!(!m.is_url("/docs/css/a.1234abcd.css"));
        assert!(m.is_url("/docs/css/a.ffffffff.css"));
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
            Ok("/fonts/My%20Font.1234abcd.woff2".to_string())
        );
    }

    #[test]
    fn url_keeps_query_and_fragment_suffix_as_given() {
        assert_eq!(
            map(true, "").url("css/a.css?a#b"),
            Ok("/css/a.1234abcd.css?a#b".to_string())
        );
    }

    #[test]
    fn url_fragment_only_is_a_miss() {
        assert!(map(true, "").url("#x").is_err());
    }

    #[test]
    fn url_malformed_escapes_stay_literal() {
        let mut m = map(true, "");
        m.insert("a%zz.css".into(), "a%zz.1.css".into());
        m.insert("a%+f.css".into(), "a%+f.1.css".into());
        assert_eq!(m.url("a%zz.css"), Ok("/a%25zz.1.css".to_string()));
        assert_eq!(m.url("a%+f.css"), Ok("/a%25%2Bf.1.css".to_string()));
    }

    #[test]
    fn hashed_file_name_matches_hashed_name() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let src = tmp.path().join("big.bin");
        // Longer than one read chunk, so the streaming path joins chunks.
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &bytes).unwrap();
        assert_eq!(
            hashed_file_name(Path::new("a/big.bin"), &src).unwrap(),
            hashed_name(Path::new("a/big.bin"), &bytes)
        );
    }

    #[test]
    fn hashed_name_known_answer() {
        assert_eq!(
            hashed_name(Path::new("css/a.css"), b"a{}"),
            PathBuf::from("css/a.b83bfb2b.css")
        );
    }
}
