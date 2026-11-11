use crate::scan::{Event, StartTag, scan};
use crate::tree::Item;

/// Collects headings, links, images and tables from HTML given as consecutive
/// labelled pieces, in document order. The pieces are tokenized as one input.
/// Each item is tagged with the label of the piece being fed when its start
/// tag was emitted, and the 1-based line of that tag in the whole input.
pub fn fragment_items<S: Copy>(pieces: &[(&str, S)]) -> Vec<Item<(S, usize)>> {
    let mut items: Vec<Item<(S, usize)>> = Vec::new();
    // Indexes into `items` of the elements still open, innermost last. Each
    // open heading or link collects its text here until it closes.
    let mut open_headings: Vec<(usize, String)> = Vec::new();
    let mut open_links: Vec<OpenLink> = Vec::new();
    let mut open_tables: Vec<usize> = Vec::new();

    for event in scan(pieces) {
        match event {
            Event::Start(tag) => {
                let at = (tag.piece, tag.line);
                match tag.name.as_str() {
                    name @ ("h1" | "h2" | "h3" | "h4" | "h5" | "h6") => {
                        open_headings.push((items.len(), String::new()));
                        items.push(Item::Heading {
                            level: name.as_bytes()[1] - b'0',
                            id: tag.attr("id").unwrap_or_default().to_string(),
                            text: String::new(),
                            at,
                        });
                    }
                    "a" => {
                        if let Some(href) = tag.attr("href") {
                            let label = tag
                                .attr("aria-label")
                                .filter(|l| !l.trim().is_empty())
                                .map(str::to_string);
                            open_links.push(OpenLink {
                                index: items.len(),
                                text: String::new(),
                                label,
                            });
                            items.push(Item::Link {
                                name: String::new(),
                                href: href.to_string(),
                                at,
                            });
                        }
                    }
                    "img" => {
                        let alt = alt_of(&tag);
                        append_all(&mut open_headings, &mut open_links, alt.as_deref());
                        items.push(Item::Image { alt, at });
                    }
                    "table" => {
                        open_tables.push(items.len());
                        items.push(Item::Table {
                            has_header: false,
                            at,
                        });
                    }
                    "th" => {
                        if let Some(&index) = open_tables.last()
                            && let Item::Table { has_header, .. } = &mut items[index]
                        {
                            *has_header = true;
                        }
                    }
                    _ => {}
                }
            }
            Event::End(name) => match name.as_str() {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    if let Some(open) = open_headings.pop() {
                        close_heading(&mut items, open);
                    }
                }
                "a" => {
                    if let Some(open) = open_links.pop() {
                        close_link(&mut items, open);
                    }
                }
                "table" => {
                    open_tables.pop();
                }
                _ => {}
            },
            Event::Text(text) => append_all(&mut open_headings, &mut open_links, Some(&text)),
        }
    }

    for open in open_headings {
        close_heading(&mut items, open);
    }
    for open in open_links {
        close_link(&mut items, open);
    }
    items
}

fn alt_of<S>(tag: &StartTag<S>) -> Option<String> {
    tag.attr("alt").map(str::to_string)
}

struct OpenLink {
    index: usize,
    text: String,
    /// A non-blank `aria-label`, which wins over the link's content.
    label: Option<String>,
}

/// Adds `text` to every open heading and link.
fn append_all(headings: &mut [(usize, String)], links: &mut [OpenLink], text: Option<&str>) {
    let Some(text) = text else { return };
    for (_, buffer) in headings.iter_mut() {
        buffer.push_str(text);
    }
    for link in links.iter_mut() {
        link.text.push_str(text);
    }
}

fn close_heading<S>(items: &mut [Item<S>], (index, buffer): (usize, String)) {
    if let Item::Heading { text, .. } = &mut items[index] {
        *text = collapse(&buffer);
    }
}

fn close_link<S>(items: &mut [Item<S>], link: OpenLink) {
    if let Item::Link { name, .. } = &mut items[link.index] {
        *name = collapse(link.label.as_deref().unwrap_or(&link.text));
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(html: &str) -> Vec<Item<((), usize)>> {
        fragment_items(&[(html, ())])
    }

    #[test]
    fn finds_items_with_positions() {
        let items = one(
            "<p>x</p>\n<h2 id=\"a\">A <em>b</em></h2>\n<a href=\"/x/\"><img src=\"i.png\" alt=\"Logo\"></a>",
        );
        assert_eq!(
            items,
            [
                Item::Heading {
                    level: 2,
                    id: "a".into(),
                    text: "A b".into(),
                    at: ((), 2)
                },
                Item::Link {
                    name: "Logo".into(),
                    href: "/x/".into(),
                    at: ((), 3)
                },
                Item::Image {
                    alt: Some("Logo".into()),
                    at: ((), 3)
                },
            ]
        );
    }

    #[test]
    fn pieces_label_their_items() {
        let items = fragment_items(&[
            ("<a href=\"/x/\">", 0),
            ("<b>T</b><h3>H</h3>", 1),
            ("</a>", 2),
        ]);
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            Item::Link {
                name: "TH".into(),
                href: "/x/".into(),
                at: (0, 1)
            }
        );
        assert_eq!(
            items[1],
            Item::Heading {
                level: 3,
                id: String::new(),
                text: "H".into(),
                at: (1, 1)
            }
        );
    }

    #[test]
    fn aria_label_names_a_link() {
        let items = one("<a href=\"/x/\" aria-label=\"Home\"><svg></svg></a>");
        assert_eq!(
            items,
            [Item::Link {
                name: "Home".into(),
                href: "/x/".into(),
                at: ((), 1)
            }]
        );
    }

    #[test]
    fn aria_label_wins_over_link_content() {
        let items = one("<a href=\"/x/\" aria-label=\"Home\">here</a>");
        assert!(matches!(&items[0], Item::Link { name, .. } if name == "Home"));
    }

    #[test]
    fn table_header_detection() {
        let with = one("<table><tr><th>A</th></tr></table>");
        let without = one("<table><tr><td>A</td></tr></table>");
        assert!(matches!(
            with[0],
            Item::Table {
                has_header: true,
                ..
            }
        ));
        assert!(matches!(
            without[0],
            Item::Table {
                has_header: false,
                ..
            }
        ));
    }

    #[test]
    fn ignores_script_style_comments() {
        let items =
            one("<script>\"<a href=x>here</a>\"</script><style>a{}</style><!-- <img src=x> -->");
        assert!(items.is_empty(), "{items:?}");
    }

    #[test]
    fn tolerates_messy_html() {
        let items = one("<H2 ID=x>T<a HREF=/y>more");
        assert_eq!(
            items,
            [
                Item::Heading {
                    level: 2,
                    id: "x".into(),
                    text: "Tmore".into(),
                    at: ((), 1)
                },
                Item::Link {
                    name: "more".into(),
                    href: "/y".into(),
                    at: ((), 1)
                },
            ]
        );
    }

    #[test]
    fn img_without_alt_attribute() {
        assert_eq!(
            one("<img src=\"a.png\">"),
            [Item::Image {
                alt: None,
                at: ((), 1)
            }]
        );
    }
}
