//! Where each page is written and the URL it's served at.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Site-relative URL, always starting with `/`.
    pub url: String,
    /// Output file, relative to the output directory.
    pub file: PathBuf,
}

/// Routes a page from its path relative to `content/`. Pages get trailing-slash URLs, except `404.md`.
pub fn route_for_page(rel: &Path) -> Route {
    let mut parts: Vec<String> = rel
        .with_extension("")
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();

    if parts == ["404"] {
        return Route {
            url: "/404.html".into(),
            file: "404.html".into(),
        };
    }
    if parts.last().is_some_and(|p| p == "index") {
        parts.pop();
    }

    let mut file: PathBuf = parts.iter().collect();
    file.push("index.html");
    let url = if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}/", parts.join("/"))
    };
    Route { url, file }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn routes() {
        assert_eq!(
            route_for_page(Path::new("index.md")),
            Route {
                url: "/".into(),
                file: "index.html".into()
            }
        );
        assert_eq!(
            route_for_page(Path::new("a/b.md")),
            Route {
                url: "/a/b/".into(),
                file: "a/b/index.html".into()
            }
        );
        assert_eq!(
            route_for_page(Path::new("a/index.md")),
            Route {
                url: "/a/".into(),
                file: "a/index.html".into()
            }
        );
        assert_eq!(
            route_for_page(Path::new("404.md")),
            Route {
                url: "/404.html".into(),
                file: "404.html".into()
            }
        );
    }
}
