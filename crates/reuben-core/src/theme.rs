//! The built-in page shell: plain semantic HTML, no JavaScript. Used until a theme supplies layouts.

use crate::html::{escape_attr, escape_text};

pub struct PageShell<'a> {
    pub title: &'a str,
    pub lang: &'a str,
    pub description: Option<&'a str>,
    pub body: &'a str,
}

const STYLE: &str = "body{font-family:system-ui,sans-serif;line-height:1.6;max-width:70ch;margin:auto;padding:1rem}";

pub fn builtin_page(shell: &PageShell) -> String {
    let description = shell
        .description
        .map(|d| {
            format!(
                "<meta name=\"description\" content=\"{}\">\n",
                escape_attr(d)
            )
        })
        .unwrap_or_default();
    format!(
        "<!doctype html>\n\
         <html lang=\"{lang}\">\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         {description}\
         <title>{title}</title>\n\
         <style>{STYLE}</style>\n\
         </head>\n\
         <body>\n\
         <main>\n\
         {body}\
         </main>\n\
         </body>\n\
         </html>\n",
        lang = escape_attr(shell.lang),
        title = escape_text(shell.title),
        body = shell.body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_has_required_landmarks() {
        let html = builtin_page(&PageShell {
            title: "A & B",
            lang: "en",
            description: None,
            body: "<p>x</p>",
        });
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<html lang=\"en\">"));
        assert!(html.contains("<title>A &amp; B</title>"));
        assert_eq!(html.matches("<main").count(), 1);
        assert!(!html.contains("<script"));
    }
}
