/// An accessibility rule. The ids are stable and appear in config files and
/// in findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    HeadingOrder,
    LinkText,
    ImgAlt,
    HeadingIdUnique,
    TableHeader,
    HtmlLang,
    PageTitle,
    SingleH1,
    MainLandmark,
    IdUnique,
    FormLabel,
    LinkTarget,
}

impl Rule {
    pub const ALL: [Rule; 12] = [
        Rule::HeadingOrder,
        Rule::LinkText,
        Rule::ImgAlt,
        Rule::HeadingIdUnique,
        Rule::TableHeader,
        Rule::HtmlLang,
        Rule::PageTitle,
        Rule::SingleH1,
        Rule::MainLandmark,
        Rule::IdUnique,
        Rule::FormLabel,
        Rule::LinkTarget,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Rule::HeadingOrder => "heading-order",
            Rule::LinkText => "link-text",
            Rule::ImgAlt => "img-alt",
            Rule::HeadingIdUnique => "heading-id-unique",
            Rule::TableHeader => "table-header",
            Rule::HtmlLang => "html-lang",
            Rule::PageTitle => "page-title",
            Rule::SingleH1 => "single-h1",
            Rule::MainLandmark => "main-landmark",
            Rule::IdUnique => "id-unique",
            Rule::FormLabel => "form-label",
            Rule::LinkTarget => "link-target",
        }
    }

    pub fn from_id(s: &str) -> Option<Rule> {
        Rule::ALL.into_iter().find(|rule| rule.id() == s)
    }

    /// One sentence saying what the rule checks, for docs and `--help`.
    pub fn summary(self) -> &'static str {
        match self {
            Rule::HeadingOrder => "Headings don't skip a level on the way down.",
            Rule::LinkText => "Links have text that says where they go.",
            Rule::ImgAlt => "Images have an alt attribute, which may be empty for decoration.",
            Rule::HeadingIdUnique => "No two headings on a page share an id.",
            Rule::TableHeader => "Tables have a header row or column.",
            Rule::HtmlLang => "The html element declares a language.",
            Rule::PageTitle => "The page has a non-empty title.",
            Rule::SingleH1 => "The page has exactly one h1.",
            Rule::MainLandmark => "The page has a main landmark.",
            Rule::IdUnique => "No two elements on a page share an id.",
            Rule::FormLabel => "Form controls have a label.",
            Rule::LinkTarget => "Internal links point at a page or anchor that exists.",
        }
    }
}

/// The rules that are switched on. Starts with every rule on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleSet(u16);

impl RuleSet {
    pub fn all() -> RuleSet {
        RuleSet((1 << Rule::ALL.len()) - 1)
    }

    pub fn disable(&mut self, rule: Rule) {
        self.0 &= !Self::bit(rule);
    }

    pub fn is_on(&self, rule: Rule) -> bool {
        self.0 & Self::bit(rule) != 0
    }

    fn bit(rule: Rule) -> u16 {
        1 << (rule as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for rule in Rule::ALL {
            assert_eq!(Rule::from_id(rule.id()), Some(rule));
        }
        let ids: Vec<&str> = Rule::ALL.iter().map(|r| r.id()).collect();
        assert_eq!(
            ids,
            [
                "heading-order",
                "link-text",
                "img-alt",
                "heading-id-unique",
                "table-header",
                "html-lang",
                "page-title",
                "single-h1",
                "main-landmark",
                "id-unique",
                "form-label",
                "link-target",
            ]
        );
        assert_eq!(Rule::from_id("nope"), None);
    }

    #[test]
    fn ruleset_disables_one_rule() {
        let mut rules = RuleSet::all();
        rules.disable(Rule::LinkText);
        assert!(!rules.is_on(Rule::LinkText));
        assert!(rules.is_on(Rule::ImgAlt));
    }
}
