//! A deterministic, realistic site generator for benchmarks and scale tests.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

const COLLECTIONS: [&str; 4] = ["guides", "notes", "posts", "refs"];

/// Writes a site with `pages` Markdown pages spread over four collections, plus config, a layout, an include,
/// a data file and a stylesheet that imports a partial. Identical input always yields identical files.
pub fn generate(dir: &Path, pages: usize) -> io::Result<()> {
    let write = |rel: &str, text: &str| -> io::Result<()> {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, text)
    };
    write("reuben.toml", "title = \"Generated\"\n")?;
    write(
        "layouts/default.html",
        "<!doctype html>\n<html><body><main>{{ content }}</main></body></html>\n",
    )?;
    write(
        "includes/card.rhai",
        "fn card(title, url) { raw(\"<a class=\\\"card\\\" href=\\\"\" + escape(url) + \"\\\">\" + escape(title) + \"</a>\") }\n",
    )?;
    write("data/info.toml", "name = \"Rye\"\n")?;
    write("static/_vars.css", ":root {\n  --accent: #336699;\n}\n")?;
    write(
        "static/site.css",
        "@import \"_vars.css\";\nbody {\n  color: var(--accent);\n  margin: 0 auto;\n}\n.card {\n  padding: 4px 8px;\n}\n",
    )?;
    for name in COLLECTIONS {
        write(
            &format!("content/{name}/_dir.toml"),
            &format!("layout = \"default.html\"\n\n[params]\nsection = \"{name}\"\n"),
        )?;
    }
    for i in 0..pages {
        let name = COLLECTIONS[i % COLLECTIONS.len()];
        let extra = 3 + i % 3;
        let mut body = format!("---\ntitle: Page {i}\nweight: {i}\n---\n");
        for h in 0..extra {
            write!(
                body,
                "## Section {h}\n\nProse for page {i}, section {h}, with *emphasis* and a [link](/{name}/).\n\n"
            )
            .unwrap();
        }
        write!(
            body,
            "Inline math $x^{{{i}}} + y$ and a note.[^1]\n\n\
             {{{{ card(page.title, \"/{name}/p{i:05}/\") }}}}\n\n\
             {{{{ data.info.name }}}}\n\n\
             Section: {{{{ page.params.section }}}}\n\n\
             ```rust\nfn main() {{\n    let n = {i};\n    println!(\"{{}}\", n);\n}}\n```\n\n\
             | Key | Value |\n| --- | --- |\n| id | {i} |\n| group | {name} |\n\n\
             [^1]: Footnote text for page {i}.\n"
        )
        .unwrap();
        write(&format!("content/{name}/p{i:05}.md"), &body)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::OutputFile;
    use crate::site::{RenderOptions, Site};
    use assert_fs::TempDir;

    fn text(output: &crate::output::Output, rel: &str) -> String {
        match output
            .files
            .get(Path::new(rel))
            .unwrap_or_else(|| panic!("{rel} missing"))
        {
            OutputFile::Bytes(b) => String::from_utf8(b.clone()).unwrap(),
            OutputFile::Copy(_) => panic!("{rel} is a copy"),
        }
    }

    #[test]
    fn generated_site_builds() {
        let tmp = TempDir::new().unwrap();
        generate(tmp.path(), 50).unwrap();
        let site = Site::load(tmp.path()).unwrap();
        let (output, report) = site
            .render(&RenderOptions {
                drafts: false,
                jobs: None,
            })
            .unwrap();
        assert_eq!(report.pages_written, 50);
        for (i, name) in COLLECTIONS.iter().enumerate() {
            let page = text(&output, &format!("{name}/p{i:05}/index.html"));
            assert!(page.contains("<main>"), "layout missing: {page}");
            assert!(
                page.contains(&format!("Section: {name}")),
                "_dir.toml params not rendered: {page}"
            );
            let h2 = page.matches("<h2").count();
            assert!((3..=5).contains(&h2), "{h2} headings: {page}");
        }
        let page = text(&output, "guides/p00000/index.html");
        assert!(page.contains("hl-"), "{page}");
        assert!(page.contains("<math"), "{page}");
        assert!(page.contains("Footnote text for page 0."), "{page}");
        assert!(
            page.contains("<a class=\"card\" href=\"/guides/p00000/\">Page 0</a>"),
            "{page}"
        );
        assert!(page.contains("<table"), "{page}");
        assert!(page.contains("Rye"), "{page}");
        let css = text(&output, "site.css");
        assert!(
            css.contains("var(--accent)") && !css.contains("@import"),
            "{css}"
        );
        assert!(!output.files.contains_key(Path::new("_vars.css")));
    }
}
