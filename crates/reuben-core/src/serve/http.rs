//! Pure request logic for the dev server: routing onto `Output` keys, MIME types and live-reload injection.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::output::OutputFile;

pub const LIVE_SCRIPT_PATH: &str = "/__reuben/live.js";
pub const LIVE_EVENTS_PATH: &str = "/__reuben/live";
const LIVE_SCRIPT_TAG: &str = "<script src=\"/__reuben/live.js\"></script>";

pub const LIVE_SCRIPT: &str = "new EventSource('/__reuben/live').addEventListener('reload', function () { location.reload(); });\n";

/// Where a request path lands.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// Serve the output entry with this key.
    File(PathBuf),
    /// Send a 301 to this path (a directory URL without its trailing slash).
    Redirect(String),
    NotFound,
    /// The path tried to escape the output or was malformed.
    Rejected,
}

/// Maps a URL path (no query or fragment) onto a key of `files`. Never touches the filesystem.
pub fn route(path: &str, files: &BTreeMap<PathBuf, OutputFile>) -> Route {
    let Some(rest) = path.strip_prefix('/') else {
        return Route::Rejected;
    };
    let Some(decoded) = percent_decode(rest) else {
        return Route::Rejected;
    };
    let mut key = PathBuf::new();
    for seg in decoded.split('/') {
        if seg == ".." || seg.contains(['\0', '\\']) {
            return Route::Rejected;
        }
        if !seg.is_empty() && seg != "." {
            key.push(seg);
        }
    }
    let index = key.join("index.html");
    if path.ends_with('/') || key.as_os_str().is_empty() {
        return found(index, files);
    }
    if files.contains_key(&index) {
        return Route::Redirect(directory_url(&key));
    }
    found(key, files)
}

/// `/seg/seg/`, built from the routed key so a request like `//docs` can't become the protocol-relative `//docs/`.
fn directory_url(key: &Path) -> String {
    let mut url = String::from("/");
    for seg in key.components() {
        for b in seg.as_os_str().to_string_lossy().bytes() {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                url.push(char::from(b));
            } else {
                url.push_str(&format!("%{b:02X}"));
            }
        }
        url.push('/');
    }
    url
}

fn found(key: PathBuf, files: &BTreeMap<PathBuf, OutputFile>) -> Route {
    if files.contains_key(&key) {
        Route::File(key)
    } else {
        Route::NotFound
    }
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The `Content-Type` for an output file, from its extension.
pub fn content_type(key: &Path) -> &'static str {
    let ext = key
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("xml") => "application/xml; charset=utf-8",
        Some("svg") => "image/svg+xml; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Adds the live-reload script before the last `</body>`, or at the end when there is none.
pub fn inject_live_script(html: &[u8]) -> Vec<u8> {
    const CLOSE: &[u8] = b"</body>";
    let at = html
        .windows(CLOSE.len())
        .rposition(|w| w.eq_ignore_ascii_case(CLOSE))
        .unwrap_or(html.len());
    let mut out = Vec::with_capacity(html.len() + LIVE_SCRIPT_TAG.len());
    out.extend_from_slice(&html[..at]);
    out.extend_from_slice(LIVE_SCRIPT_TAG.as_bytes());
    out.extend_from_slice(&html[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(keys: &[&str]) -> BTreeMap<PathBuf, OutputFile> {
        keys.iter()
            .map(|k| (PathBuf::from(k), OutputFile::Bytes(Vec::new())))
            .collect()
    }

    #[test]
    fn route_mapping() {
        let f = files(&["index.html", "a/index.html", "x.css", "b.txt"]);
        assert_eq!(route("/", &f), Route::File("index.html".into()));
        assert_eq!(route("/a/", &f), Route::File("a/index.html".into()));
        assert_eq!(route("/a", &f), Route::Redirect("/a/".into()));
        assert_eq!(route("/x.css", &f), Route::File("x.css".into()));
        assert_eq!(route("/b.txt", &f), Route::File("b.txt".into()));
        assert_eq!(route("/nope", &f), Route::NotFound);
        assert_eq!(route("/nope/", &f), Route::NotFound);
    }

    #[test]
    fn redirect_is_built_from_the_key() {
        let f = files(&["a/index.html", "my dir/index.html"]);
        assert_eq!(route("//a", &f), Route::Redirect("/a/".into()));
        assert_eq!(route("/a//", &f), Route::File("a/index.html".into()));
        assert_eq!(route("/./a", &f), Route::Redirect("/a/".into()));
        assert_eq!(route("/my%20dir", &f), Route::Redirect("/my%20dir/".into()));
    }

    #[test]
    fn double_encoded_traversal_just_misses() {
        let f = files(&["index.html", "secret"]);
        assert_eq!(route("/%252e%252e/secret", &f), Route::NotFound);
        assert_eq!(route("/%252e%252e", &f), Route::NotFound);
    }

    #[test]
    fn route_rejects_traversal() {
        let f = files(&["index.html", "a/index.html"]);
        for p in [
            "/../etc/passwd",
            "/%2e%2e/secret",
            "/a/%2E%2E/b",
            "/a%00b",
            "/a%5Cb",
            "/a%5cb",
            "/%2e%2E/x",
            "/a/%2e%2e",
            "/%zz",
            "/%ff",
        ] {
            assert_eq!(route(p, &f), Route::Rejected, "{p}");
        }
    }

    #[test]
    fn inject_live_script() {
        let out = super::inject_live_script(b"<html><body>x</body></html>");
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("<html><body>x{LIVE_SCRIPT_TAG}</body></html>")
        );
        let out = super::inject_live_script(b"<p>x</p>");
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("<p>x</p>{LIVE_SCRIPT_TAG}")
        );
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type(Path::new("a.css")), "text/css; charset=utf-8");
        assert_eq!(content_type(Path::new("a.woff2")), "font/woff2");
        assert_eq!(content_type(Path::new("a.bin")), "application/octet-stream");
    }
}
