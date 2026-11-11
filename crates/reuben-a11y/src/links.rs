//! Turns an `href` found on a page into the output files that would make it
//! work.

#[derive(Debug, PartialEq)]
pub enum LinkKeys {
    /// Not a link into this site: empty, a fragment, protocol-relative, or a
    /// URL with a scheme.
    External,
    /// Output keys, any one of which existing means the link works. Empty when
    /// the link points outside the site's base path.
    Candidates(Vec<String>),
}

/// `page_url` is the page's site-relative URL (`/blog/x/`), `base_path` the
/// path part of `base_url` (`/` or `/blog/`).
pub fn link_keys(page_url: &str, base_path: &str, href: &str) -> LinkKeys {
    if href.is_empty() || href.starts_with('#') || href.starts_with("//") || has_scheme(href) {
        return LinkKeys::External;
    }
    let path = href.split(['?', '#']).next().unwrap_or("");
    let path = percent_decode(path);

    let full = if path.starts_with('/') {
        path
    } else {
        let dir = &page_url[..page_url.rfind('/').map_or(0, |i| i + 1)];
        format!("/{}{path}", dir.trim_start_matches('/'))
    };
    let resolved = normalize(&full);

    let base = if base_path.ends_with('/') {
        base_path.to_string()
    } else {
        format!("{base_path}/")
    };
    // The base path itself, without its trailing slash, is the site root.
    let rest = if resolved == base.trim_end_matches('/') {
        Some("")
    } else {
        resolved.strip_prefix(base.as_str())
    };
    let Some(rest) = rest else {
        return LinkKeys::Candidates(Vec::new());
    };

    if rest.is_empty() || rest.ends_with('/') {
        LinkKeys::Candidates(vec![format!("{rest}index.html")])
    } else {
        LinkKeys::Candidates(vec![rest.to_string(), format!("{rest}/index.html")])
    }
}

fn has_scheme(href: &str) -> bool {
    let Some((scheme, _)) = href.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Resolves `.` and `..` in an absolute path without climbing above `/`,
/// keeping a trailing slash when the path names a directory.
fn normalize(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    let mut is_dir = path.ends_with('/');
    for segment in path.split('/') {
        match segment {
            "" => {}
            "." => is_dir = true,
            ".." => {
                segments.pop();
                is_dir = true;
            }
            other => {
                segments.push(other);
                is_dir = path.ends_with('/');
            }
        }
    }
    let mut out = format!("/{}", segments.join("/"));
    if is_dir && !out.ends_with('/') {
        out.push('/');
    }
    out
}

/// Decodes `%XX` sequences; if the result isn't valid UTF-8, returns the
/// input unchanged.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = bytes
                .get(i + 1..i + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(page: &str, base: &str, href: &str) -> Vec<String> {
        match link_keys(page, base, href) {
            LinkKeys::Candidates(keys) => keys,
            LinkKeys::External => panic!("{href} was external"),
        }
    }

    #[test]
    fn external_hrefs() {
        for href in [
            "",
            "#x",
            "//cdn.x/a.js",
            "https://x.y/",
            "mailto:a@b.c",
            "tel:1",
        ] {
            assert_eq!(link_keys("/", "/", href), LinkKeys::External, "{href}");
        }
    }

    #[test]
    fn directory_and_file_links() {
        assert_eq!(keys("/blog/x/", "/", "/about/"), ["about/index.html"]);
        assert_eq!(
            keys("/", "/", "/resume.pdf"),
            ["resume.pdf", "resume.pdf/index.html"]
        );
        assert_eq!(keys("/", "/", "/"), ["index.html"]);
    }

    #[test]
    fn relative_links_resolve() {
        assert_eq!(keys("/blog/x/", "/", "../y/"), ["blog/y/index.html"]);
        assert_eq!(
            keys("/blog/x/", "/", "img.png?v=2#a"),
            ["blog/x/img.png", "blog/x/img.png/index.html"]
        );
    }

    #[test]
    fn base_path_is_stripped() {
        assert_eq!(keys("/blog/a/", "/blog/", "/blog/b/"), ["b/index.html"]);
        assert!(keys("/blog/a/", "/blog/", "/other/").is_empty());
    }

    #[test]
    fn cannot_climb_above_root() {
        assert_eq!(keys("/a/", "/", "../../../x/"), ["x/index.html"]);
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(keys("/", "/", "/caf%C3%A9/"), ["café/index.html"]);
        assert_eq!(keys("/", "/", "/a%FF/"), ["a%FF/index.html"]);
    }
}
