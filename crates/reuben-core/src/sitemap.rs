//! Rendering the `sitemap.xml` file.

use crate::html::escape_text;

/// Lists `urls` in the order given, XML-escaped.
pub fn render(urls: &[String]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for url in urls {
        xml += &format!("  <url><loc>{}</loc></url>\n", escape_text(url));
    }
    xml += "</urlset>\n";
    xml
}

#[cfg(test)]
mod tests {
    use super::render;

    #[test]
    fn sitemap_escapes_and_lists_urls() {
        let xml = render(&["https://x.dev/a?b=1&c=2".into()]);
        assert!(
            xml.contains("<loc>https://x.dev/a?b=1&amp;c=2</loc>"),
            "{xml}"
        );
    }
}
