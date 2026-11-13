use std::collections::HashSet;

use crate::rules::{Rule, RuleSet};

/// The parts of a document the tree pass looks at, in document order.
/// `O` says where an item came from; the caller decides what that is.
#[derive(Debug, Clone, PartialEq)]
pub enum Item<O> {
    Heading {
        level: u8,
        id: String,
        text: String,
        at: O,
    },
    /// `name` is the link's accessible name.
    Link {
        name: String,
        href: String,
        at: O,
    },
    Image {
        alt: Option<String>,
        at: O,
    },
    Table {
        has_header: bool,
        at: O,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding<O> {
    pub rule: Rule,
    pub message: String,
    pub at: O,
}

const VAGUE_LINK_TEXT: [&str; 6] = [
    "click here",
    "here",
    "read more",
    "more",
    "learn more",
    "link",
];

/// Runs the tree-pass rules over `items`, returning findings in item order.
pub fn check_tree<O: Clone>(items: &[Item<O>], rules: &RuleSet) -> Vec<Finding<O>> {
    let mut findings = Vec::new();
    let mut add = |rule: Rule, message: String, at: &O| {
        if rules.is_on(rule) {
            findings.push(Finding {
                rule,
                message,
                at: at.clone(),
            });
        }
    };

    let mut previous_level: Option<u8> = None;
    let mut seen_ids: HashSet<&str> = HashSet::new();

    for item in items {
        match item {
            Item::Heading { level, id, at, .. } => {
                let allowed = previous_level.map_or(2, |previous| previous + 1);
                if *level > allowed {
                    add(
                        Rule::HeadingOrder,
                        format!("heading level {level} skips level {}", level - 1),
                        at,
                    );
                }
                previous_level = Some(*level);
                if !id.is_empty() && !seen_ids.insert(id) {
                    add(
                        Rule::HeadingIdUnique,
                        format!("heading id \"{id}\" is already used by an earlier heading"),
                        at,
                    );
                }
            }
            Item::Link { name, at, .. } => {
                let normalized = name
                    .trim()
                    .to_lowercase()
                    .trim_end_matches(|c: char| !c.is_alphanumeric())
                    .to_string();
                if normalized.is_empty() {
                    add(Rule::LinkText, "link has no text".to_string(), at);
                } else if VAGUE_LINK_TEXT.contains(&normalized.as_str()) {
                    add(
                        Rule::LinkText,
                        format!("link text \"{normalized}\" doesn't say where the link goes"),
                        at,
                    );
                }
            }
            Item::Image { alt: None, at } => {
                add(Rule::ImgAlt, "image has no alt text".to_string(), at);
            }
            Item::Image { .. } => {}
            Item::Table {
                has_header: false,
                at,
            } => {
                add(
                    Rule::TableHeader,
                    "table has no header cells".to_string(),
                    at,
                );
            }
            Item::Table { .. } => {}
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(level: u8, id: &str, at: usize) -> Item<usize> {
        Item::Heading {
            level,
            id: id.to_string(),
            text: String::new(),
            at,
        }
    }

    fn link(name: &str, at: usize) -> Item<usize> {
        Item::Link {
            name: name.to_string(),
            href: "/x/".to_string(),
            at,
        }
    }

    fn rules_of(findings: &[Finding<usize>]) -> Vec<Rule> {
        findings.iter().map(|f| f.rule).collect()
    }

    #[test]
    fn skipped_heading_level() {
        let all = RuleSet::all();
        let found = check_tree(&[heading(2, "", 0), heading(4, "", 1)], &all);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule, Rule::HeadingOrder);
        assert_eq!(found[0].at, 1);
        assert_eq!(found[0].message, "heading level 4 skips level 3");

        let fine: Vec<_> = [2, 3, 2, 3, 1, 2]
            .iter()
            .enumerate()
            .map(|(i, l)| heading(*l, "", i))
            .collect();
        assert!(check_tree(&fine, &all).is_empty());

        let found = check_tree(&[heading(3, "", 0)], &all);
        assert_eq!(rules_of(&found), [Rule::HeadingOrder]);
    }

    #[test]
    fn vague_and_empty_link_text() {
        let all = RuleSet::all();
        let items = [
            link("Here.", 0),
            link("  click here ", 1),
            link("", 2),
            link("the release notes", 3),
        ];
        let found = check_tree(&items, &all);
        assert_eq!(rules_of(&found), [Rule::LinkText; 3]);
        assert_eq!(
            found[0].message,
            "link text \"here\" doesn't say where the link goes"
        );
        assert_eq!(found[2].message, "link has no text");
        assert_eq!(found[2].at, 2);
    }

    #[test]
    fn unicode_punctuation_is_trimmed_from_link_text() {
        for (text, normalized) in [
            ("Read more…", "read more"),
            ("More →", "more"),
            ("Read more »", "read more"),
        ] {
            let found = check_tree(&[link(text, 0)], &RuleSet::all());
            assert_eq!(rules_of(&found), [Rule::LinkText], "{text}");
            assert_eq!(
                found[0].message,
                format!("link text \"{normalized}\" doesn't say where the link goes")
            );
        }
    }

    #[test]
    fn missing_alt_but_not_empty_alt() {
        let all = RuleSet::all();
        let missing = Item::Image { alt: None, at: 0 };
        let decorative = Item::Image {
            alt: Some(String::new()),
            at: 1,
        };
        let found = check_tree(&[missing, decorative], &all);
        assert_eq!(rules_of(&found), [Rule::ImgAlt]);
        assert_eq!(found[0].at, 0);
        assert_eq!(found[0].message, "image has no alt text");
    }

    #[test]
    fn duplicate_heading_ids() {
        let items = [
            heading(1, "a", 0),
            heading(2, "b", 1),
            heading(2, "a", 2),
            heading(2, "", 3),
            heading(2, "", 4),
        ];
        let found = check_tree(&items, &RuleSet::all());
        assert_eq!(rules_of(&found), [Rule::HeadingIdUnique]);
        assert_eq!(found[0].at, 2);
    }

    #[test]
    fn table_without_header() {
        let items = [
            Item::Table {
                has_header: false,
                at: 0,
            },
            Item::Table {
                has_header: true,
                at: 1,
            },
        ];
        let found = check_tree(&items, &RuleSet::all());
        assert_eq!(rules_of(&found), [Rule::TableHeader]);
        assert_eq!(found[0].at, 0);
    }

    #[test]
    fn disabled_rule_is_silent() {
        let items = [
            heading(3, "a", 0),
            heading(3, "a", 1),
            link("here", 2),
            Item::Image { alt: None, at: 3 },
            Item::Table {
                has_header: false,
                at: 4,
            },
        ];
        let all = RuleSet::all();
        let baseline = rules_of(&check_tree(&items, &all));
        for rule in [
            Rule::HeadingOrder,
            Rule::LinkText,
            Rule::ImgAlt,
            Rule::HeadingIdUnique,
            Rule::TableHeader,
        ] {
            assert!(baseline.contains(&rule), "{rule:?} should fire");
            let mut rules = RuleSet::all();
            rules.disable(rule);
            assert!(!rules_of(&check_tree(&items, &rules)).contains(&rule));
        }
    }
}
