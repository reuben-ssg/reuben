//! Rendering the `sitemap.xml` file.

use jiff::civil::Date;

use crate::html::escape_text;

/// Lists `urls` in the order given, XML-escaped, each with its `<lastmod>` when known.
pub fn render(urls: &[(String, Option<Date>)]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for (url, lastmod) in urls {
        xml += &format!("  <url><loc>{}</loc>", escape_text(url));
        if let Some(date) = lastmod {
            xml += &format!("<lastmod>{date}</lastmod>");
        }
        xml += "</url>\n";
    }
    xml += "</urlset>\n";
    xml
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::render;

    #[test]
    fn sitemap_escapes_and_lists_urls() {
        let xml = render(&[("https://x.dev/a?b=1&c=2".into(), None)]);
        assert!(
            xml.contains("<loc>https://x.dev/a?b=1&amp;c=2</loc>"),
            "{xml}"
        );
    }

    #[test]
    fn sitemap_writes_lastmod() {
        let xml = render(&[("https://x.dev/".into(), Some(date(2024, 3, 5)))]);
        assert!(
            xml.contains("<url><loc>https://x.dev/</loc><lastmod>2024-03-05</lastmod></url>"),
            "{xml}"
        );
    }
}
