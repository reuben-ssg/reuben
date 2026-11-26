//! Links in Markdown that name `.md` source files, and their resolution to page URLs.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::doc::{Document, Fill, Node, NodeKind, Span};

/// A link destination that names a Markdown source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLink {
    /// Percent-decoded, without the `@/` prefix.
    pub path: String,
    /// Written with `@/`, so it starts at `content/`.
    pub rooted: bool,
    /// The `?query#fragment` part exactly as written; may be empty.
    pub suffix: String,
    /// The decoded text after `#`, when non-empty.
    pub fragment: Option<String>,
}

impl SourceLink {
    fn as_written(&self) -> String {
        if self.rooted {
            format!("@/{}", self.path)
        } else {
            self.path.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkProblem {
    #[error("no page `{}`{}", slashed(.target), did_you_mean(.suggestion))]
    Missing {
        target: PathBuf,
        suggestion: Option<PathBuf>,
    },
    #[error("links to `{}`, a draft that isn't built without --drafts", slashed(.target))]
    Draft { target: PathBuf },
    #[error("link `{link}` points outside content/")]
    OutsideContent { link: String },
    #[error("link `{link}` must name a .md file")]
    NotMarkdown { link: String },
    #[error("links to `#{fragment}` on `{}`, which has no such id", slashed(.target))]
    NoAnchor { target: PathBuf, fragment: String },
}

fn slashed(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn did_you_mean(suggestion: &Option<PathBuf>) -> String {
    suggestion
        .as_ref()
        .map(|s| format!("; did you mean `{}`?", slashed(s)))
        .unwrap_or_default()
}

/// A page a source link can point at, keyed elsewhere by its path under `content/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The site-relative route URL.
    pub url: String,
    pub excluded_draft: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub target: PathBuf,
    pub route_url: String,
    pub suffix: String,
}

fn has_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    for c in chars {
        match c {
            ':' => return true,
            c if c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-') => {}
            _ => return false,
        }
    }
    false
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Classifies a link destination: `None` when it isn't a source link.
pub fn parse_source_link(dest: &str) -> Option<Result<SourceLink, LinkProblem>> {
    let split = dest.find(['?', '#']).unwrap_or(dest.len());
    let (base, suffix) = dest.split_at(split);
    let (rooted, raw) = match base.strip_prefix("@/") {
        Some(rest) => (true, rest),
        None => {
            if base.is_empty()
                || has_scheme(base)
                || base.starts_with('/')
                || !base.ends_with(".md")
            {
                return None;
            }
            (false, base)
        }
    };
    if !raw.ends_with(".md") {
        return Some(Err(LinkProblem::NotMarkdown {
            link: base.to_string(),
        }));
    }
    let fragment = suffix
        .split_once('#')
        .map(|(_, f)| percent_decode(f))
        .filter(|f| !f.is_empty());
    Some(Ok(SourceLink {
        path: percent_decode(raw),
        rooted,
        suffix: suffix.to_string(),
        fragment,
    }))
}

/// Every page under `content/`, by path relative to it.
#[derive(Debug, Default)]
pub struct PageIndex {
    pages: BTreeMap<PathBuf, Target>,
    /// Built by the first plain link that carries a `#fragment`, so a site without one never pays for it.
    by_url: OnceLock<HashMap<String, PathBuf>>,
}

impl PageIndex {
    pub fn new(entries: impl IntoIterator<Item = (PathBuf, Target)>) -> Self {
        PageIndex {
            pages: entries.into_iter().collect(),
            by_url: OnceLock::new(),
        }
    }

    /// The page a plain root-relative `dest` with a `#fragment` points at, and that fragment. `base` is the path of
    /// `base_url` without its trailing slash. Links to anything else, such as static files or other sites, are `None`.
    fn plain_anchor(&self, dest: &str, base: &str) -> Option<(PathBuf, String)> {
        let (before, fragment) = dest.split_once('#')?;
        let fragment = percent_decode(fragment);
        if fragment.is_empty() || !before.starts_with('/') || before.starts_with("//") {
            return None;
        }
        let path = before.split('?').next().unwrap_or(before);
        let path = path.strip_prefix(base)?;
        if !path.is_empty() && !path.starts_with('/') {
            return None;
        }
        let path = percent_decode(path);
        let path = path.strip_suffix("index.html").unwrap_or(&path);
        let by_url = self.by_url.get_or_init(|| {
            self.pages
                .iter()
                .filter(|(_, t)| !t.excluded_draft)
                .rev()
                .map(|(rel, t)| (t.url.clone(), rel.clone()))
                .collect()
        });
        let target = by_url.get(path).or_else(|| {
            let slashed = format!("{path}/");
            by_url.get(slashed.as_str())
        })?;
        Some((target.clone(), fragment))
    }

    /// Resolves `link`, written in the page at `from_rel` (relative to `content/`).
    pub fn resolve(&self, from_rel: &Path, link: &SourceLink) -> Result<Resolved, LinkProblem> {
        let mut parts: Vec<String> = Vec::new();
        if !link.rooted
            && let Some(dir) = from_rel.parent()
        {
            parts.extend(
                dir.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned()),
            );
        }
        for part in link.path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    if parts.pop().is_none() {
                        return Err(LinkProblem::OutsideContent {
                            link: link.as_written(),
                        });
                    }
                }
                p => parts.push(p.to_string()),
            }
        }
        let target: PathBuf = parts.iter().collect();
        match self.pages.get(&target) {
            Some(t) if t.excluded_draft => Err(LinkProblem::Draft { target }),
            Some(t) => Ok(Resolved {
                route_url: t.url.clone(),
                target,
                suffix: link.suffix.clone(),
            }),
            None => {
                let suggestion = self.suggest(&target);
                Err(LinkProblem::Missing { target, suggestion })
            }
        }
    }

    fn suggest(&self, target: &Path) -> Option<PathBuf> {
        let dir = target.parent().unwrap_or(Path::new(""));
        let name = target.file_name()?.to_string_lossy();
        if name == "_index.md" {
            let index = dir.join("index.md");
            if self.pages.contains_key(&index) {
                return Some(index);
            }
        }
        let mut best: Option<(usize, &PathBuf)> = None;
        for path in self.pages.keys() {
            if path.parent().unwrap_or(Path::new("")) != dir {
                continue;
            }
            let Some(other) = path.file_name() else {
                continue;
            };
            let distance = edit_distance(&name, &other.to_string_lossy());
            if distance <= 2 && best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, path));
            }
        }
        best.map(|(_, p)| p.clone())
    }
}

/// A link's `#fragment`, to be checked against the target page's heading ids once every page has rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorRequest {
    pub target: PathBuf,
    pub fragment: String,
    pub location: Span,
    /// Written as a plain root-relative link, not `@/page.md`. A missing anchor is then a `link-target` finding,
    /// which the rule can switch off, instead of an error.
    pub plain: bool,
}

/// Rewrites every source link in `doc` (written in the page at `from_rel`) to its page URL under `base_path`.
/// A link that can't be resolved is left as written and reported with the link's position.
/// Plain root-relative links with a `#fragment` are only looked at when `plain_anchors` is set.
pub fn resolve_tree(
    doc: &mut Document,
    from_rel: &Path,
    index: &PageIndex,
    base_path: &str,
    plain_anchors: bool,
) -> (Vec<(Span, LinkProblem)>, Vec<AnchorRequest>) {
    let mut resolver = Resolver {
        from_rel,
        index,
        base: base_path.trim_end_matches('/'),
        plain_anchors,
        problems: Vec::new(),
        anchors: Vec::new(),
    };
    resolver.walk(doc);
    (resolver.problems, resolver.anchors)
}

/// The destinations of the source links in `doc`, as written.
pub fn source_links(doc: &Document) -> Vec<String> {
    fn collect(node: &Node, out: &mut Vec<String>) {
        if let NodeKind::Link { url, .. } = &node.kind
            && parse_source_link(url).is_some()
        {
            out.push(url.clone());
        }
        for child in &node.children {
            collect(child, out);
        }
    }
    let mut out = Vec::new();
    collect(doc, &mut out);
    out
}

struct Resolver<'a> {
    from_rel: &'a Path,
    index: &'a PageIndex,
    base: &'a str,
    plain_anchors: bool,
    problems: Vec<(Span, LinkProblem)>,
    anchors: Vec<AnchorRequest>,
}

impl Resolver<'_> {
    fn walk(&mut self, node: &mut Node) {
        let span = node.span;
        match &mut node.kind {
            NodeKind::Link { url, .. } => match parse_source_link(url) {
                Some(parsed) => {
                    if let Some((page, suffix)) = self.resolve_dest(parsed, span) {
                        *url = format!("{page}{suffix}");
                    }
                }
                None if self.plain_anchors => {
                    if let Some((target, fragment)) = self.index.plain_anchor(url, self.base) {
                        self.anchors.push(AnchorRequest {
                            target,
                            fragment,
                            location: span,
                            plain: true,
                        });
                    }
                }
                None => {}
            },
            NodeKind::HtmlBlock(html) | NodeKind::HtmlInline(html) => {
                self.resolve_html(html, &mut [], span);
            }
            NodeKind::FilledHtmlBlock { html, fills } => self.resolve_html(html, fills, span),
            _ => {}
        }
        for child in &mut node.children {
            self.walk(child);
        }
    }

    /// The page URL and the `?query#fragment` to write for a source link, or `None` if it didn't resolve and was reported.
    fn resolve_dest(
        &mut self,
        parsed: Result<SourceLink, LinkProblem>,
        location: Span,
    ) -> Option<(String, String)> {
        match parsed.and_then(|link| self.index.resolve(self.from_rel, &link).map(|r| (link, r))) {
            Ok((link, resolved)) => {
                if let Some(fragment) = link.fragment {
                    self.anchors.push(AnchorRequest {
                        target: resolved.target,
                        fragment,
                        location,
                        plain: false,
                    });
                }
                Some((
                    format!("{}{}", self.base, resolved.route_url),
                    resolved.suffix,
                ))
            }
            Err(problem) => {
                self.problems.push((location, problem));
                None
            }
        }
    }

    /// Resolves the `@/` links in the `href` and `src` attributes of raw HTML, which `span` is the start of.
    /// Text a block filled in is left alone, and the fills after a rewrite are moved to where their text went.
    fn resolve_html(&mut self, html: &mut String, fills: &mut [Fill], span: Span) {
        if !html.contains("@/") {
            return;
        }
        let mut out = String::with_capacity(html.len());
        let mut copied = 0;
        let mut shifts: Vec<(usize, isize)> = Vec::new();
        for range in source_attr_values(html) {
            if fills
                .iter()
                .any(|f| f.range.start < range.end && range.start < f.range.end)
            {
                continue;
            }
            let location = html_location(html, fills, span, range.start);
            let Some(parsed) = parse_source_link(&html[range.clone()]) else {
                continue;
            };
            let Some((page, suffix)) = self.resolve_dest(parsed, location) else {
                continue;
            };
            let rewritten = format!("{}{suffix}", escape_attr(&page));
            out.push_str(&html[copied..range.start]);
            out.push_str(&rewritten);
            copied = range.end;
            shifts.push((
                range.end,
                rewritten.len().cast_signed() - range.len().cast_signed(),
            ));
        }
        if shifts.is_empty() {
            return;
        }
        out.push_str(&html[copied..]);
        for fill in fills {
            let by: isize = shifts
                .iter()
                .filter(|(end, _)| *end <= fill.range.start)
                .map(|(_, delta)| delta)
                .sum();
            fill.range = fill.range.start.saturating_add_signed(by)
                ..fill.range.end.saturating_add_signed(by);
        }
        *html = out;
    }
}

/// Where the text at byte `at` of an HTML block that starts at `span` is in the source file.
/// A block's own text keeps its line breaks, and a fill stands for as many as its block spanned.
fn html_location(html: &str, fills: &[Fill], span: Span, at: usize) -> Span {
    let before = &html[..at];
    let from_fills: usize = fills
        .iter()
        .filter(|f| f.range.end <= at)
        .map(|f| f.newlines)
        .sum();
    let lines = before.matches('\n').count() + from_fills;
    let line = span.line + lines;
    let column = match before.rfind('\n') {
        None if from_fills == 0 => span.column + before.chars().count(),
        None => before.chars().count() + 1,
        Some(i) => before[i + 1..].chars().count() + 1,
    };
    Span { line, column }
}

/// The byte range of each `href` or `src` attribute value in `html` that starts with `@/`.
/// Only text inside a start tag counts, so prose that quotes an attribute is left alone.
fn source_attr_values(html: &str) -> Vec<Range<usize>> {
    let bytes = html.as_bytes();
    let end = bytes.len();
    let blank = |b: u8| b.is_ascii_whitespace();
    let mut found = Vec::new();
    let mut i = 0;
    while let Some(at) = html[i..].find('<') {
        i += at + 1;
        if html[i..].starts_with("!--") {
            i = html[i..].find("-->").map_or(end, |e| i + e + 3);
            continue;
        }
        if !bytes.get(i).is_some_and(u8::is_ascii_alphabetic) {
            continue;
        }
        while i < end && !blank(bytes[i]) && !matches!(bytes[i], b'/' | b'>') {
            i += 1;
        }
        loop {
            while i < end && (blank(bytes[i]) || bytes[i] == b'/') {
                i += 1;
            }
            if i >= end || bytes[i] == b'>' {
                i += 1;
                break;
            }
            let name = i;
            while i < end && !blank(bytes[i]) && !matches!(bytes[i], b'=' | b'>' | b'/') {
                i += 1;
            }
            let name = &html[name..i];
            while i < end && blank(bytes[i]) {
                i += 1;
            }
            if bytes.get(i) != Some(&b'=') {
                continue;
            }
            i += 1;
            while i < end && blank(bytes[i]) {
                i += 1;
            }
            let value = match bytes.get(i) {
                Some(&quote @ (b'"' | b'\'')) => {
                    let start = i + 1;
                    let stop = html[start..].find(quote as char).map_or(end, |p| start + p);
                    i = (stop + 1).min(end);
                    start..stop
                }
                _ => {
                    let start = i;
                    while i < end && !blank(bytes[i]) && bytes[i] != b'>' {
                        i += 1;
                    }
                    start..i
                }
            };
            if (name.eq_ignore_ascii_case("href") || name.eq_ignore_ascii_case("src"))
                && html[value.clone()].starts_with("@/")
            {
                found.push(value);
            }
        }
    }
    found
}

fn escape_attr(url: &str) -> String {
    url.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut row = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            row.push((prev[j] + cost).min(prev[j + 1] + 1).min(row[j] + 1));
        }
        prev = row;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(url: &str) -> Target {
        Target {
            url: url.to_string(),
            excluded_draft: false,
        }
    }

    fn index() -> PageIndex {
        PageIndex::new([
            (PathBuf::from("a/b.md"), target("/a/b/")),
            (PathBuf::from("a/index.md"), target("/a/")),
            (PathBuf::from("c.md"), target("/c/")),
        ])
    }

    fn tree(markdown: &str) -> crate::doc::Document {
        crate::markdown::parse(markdown, 1)
    }

    #[test]
    fn code_is_left_alone() {
        let mut doc = tree("`[x](a.md)`\n\n```\n[y](b.md)\n```");
        let before = doc.clone();
        let (problems, anchors) =
            resolve_tree(&mut doc, Path::new("p.md"), &PageIndex::default(), "", true);
        assert!(problems.is_empty() && anchors.is_empty());
        assert_eq!(doc, before);
    }

    #[test]
    fn reference_links_resolve() {
        let mut doc = tree("[x][r]\n\n[r]: ../c.md");
        let (problems, _) = resolve_tree(&mut doc, Path::new("a/b.md"), &index(), "", true);
        assert!(problems.is_empty(), "{problems:?}");
        fn find(n: &crate::doc::Node) -> Option<String> {
            if let crate::doc::NodeKind::Link { url, .. } = &n.kind {
                return Some(url.clone());
            }
            n.children.iter().find_map(find)
        }
        assert_eq!(find(&doc).as_deref(), Some("/c/"));
    }

    fn resolve(dest: &str, from: &str) -> Result<Resolved, LinkProblem> {
        let link = parse_source_link(dest)
            .expect("source link")
            .expect("valid");
        index().resolve(Path::new(from), &link)
    }

    #[test]
    fn rooted_and_relative() {
        assert_eq!(resolve("@/a/b.md", "x.md").unwrap().route_url, "/a/b/");
        assert_eq!(resolve("b.md", "a/index.md").unwrap().route_url, "/a/b/");
        assert_eq!(resolve("../c.md", "a/b.md").unwrap().route_url, "/c/");
        assert_eq!(resolve("./b.md", "a/b.md").unwrap().route_url, "/a/b/");
        let itself = resolve("b.md", "a/b.md").unwrap();
        assert_eq!(itself.route_url, "/a/b/");
        assert_eq!(itself.target, PathBuf::from("a/b.md"));
    }

    #[test]
    fn suffix_is_kept() {
        let link = parse_source_link("@/a/b.md#assumptions").unwrap().unwrap();
        assert_eq!(link.suffix, "#assumptions");
        assert_eq!(link.fragment.as_deref(), Some("assumptions"));
        assert_eq!(link.path, "a/b.md");
        assert!(link.rooted);
        let link = parse_source_link("@/a/b.md?x=1#y").unwrap().unwrap();
        assert_eq!(link.suffix, "?x=1#y");
        assert_eq!(link.fragment.as_deref(), Some("y"));
        let r = resolve("@/a/b.md#z", "x.md").unwrap();
        assert_eq!(r.suffix, "#z");
    }

    #[test]
    fn non_source_links() {
        for dest in [
            "https://x.dev/a.md",
            "mailto:a.md",
            "/a.md",
            "#a.md",
            "a.html",
            "a.md.txt",
            "",
        ] {
            assert!(parse_source_link(dest).is_none(), "{dest}");
        }
    }

    #[test]
    fn percent_decoding() {
        let link = parse_source_link("caf%C3%A9.md").unwrap().unwrap();
        assert_eq!(link.path, "café.md");
        assert!(!link.rooted);
        let link = parse_source_link("my%20page.md").unwrap().unwrap();
        assert_eq!(link.path, "my page.md");
    }

    #[test]
    fn problems() {
        assert_eq!(
            resolve("@/nope.md", "x.md").unwrap_err(),
            LinkProblem::Missing {
                target: PathBuf::from("nope.md"),
                suggestion: None
            }
        );
        assert_eq!(
            resolve("@/a/_index.md", "x.md").unwrap_err(),
            LinkProblem::Missing {
                target: PathBuf::from("a/_index.md"),
                suggestion: Some(PathBuf::from("a/index.md"))
            }
        );
        assert_eq!(
            resolve("@/a/bb.md", "x.md").unwrap_err(),
            LinkProblem::Missing {
                target: PathBuf::from("a/bb.md"),
                suggestion: Some(PathBuf::from("a/b.md"))
            }
        );
        assert_eq!(
            resolve("../../x.md", "a/b.md").unwrap_err(),
            LinkProblem::OutsideContent {
                link: "../../x.md".to_string()
            }
        );
        assert_eq!(
            parse_source_link("@/a/b").unwrap().unwrap_err(),
            LinkProblem::NotMarkdown {
                link: "@/a/b".to_string()
            }
        );
        let drafts = PageIndex::new([(
            PathBuf::from("d.md"),
            Target {
                url: "/d/".to_string(),
                excluded_draft: true,
            },
        )]);
        let link = parse_source_link("@/d.md").unwrap().unwrap();
        assert_eq!(
            drafts.resolve(Path::new("x.md"), &link).unwrap_err(),
            LinkProblem::Draft {
                target: PathBuf::from("d.md")
            }
        );
    }

    #[test]
    fn odd_destinations() {
        assert_eq!(
            resolve("@/../x.md", "x.md").unwrap_err(),
            LinkProblem::OutsideContent {
                link: "@/../x.md".to_string()
            }
        );
        assert_eq!(
            parse_source_link("@/").unwrap().unwrap_err(),
            LinkProblem::NotMarkdown {
                link: "@/".to_string()
            }
        );
        assert!(parse_source_link("?x.md").is_none());
        assert!(parse_source_link("C:x.md").is_none());
        let link = parse_source_link("a%ZZ.md").unwrap().unwrap();
        assert_eq!(link.path, "a%ZZ.md");
    }

    #[test]
    fn messages() {
        let missing = LinkProblem::Missing {
            target: PathBuf::from("a").join("b.md"),
            suggestion: None,
        };
        assert_eq!(missing.to_string(), "no page `a/b.md`");
        let missing = LinkProblem::Missing {
            target: PathBuf::from("a").join("bb.md"),
            suggestion: Some(PathBuf::from("a").join("b.md")),
        };
        assert_eq!(
            missing.to_string(),
            "no page `a/bb.md`; did you mean `a/b.md`?"
        );
        assert_eq!(
            LinkProblem::Draft {
                target: PathBuf::from("d.md")
            }
            .to_string(),
            "links to `d.md`, a draft that isn't built without --drafts"
        );
        assert_eq!(
            LinkProblem::OutsideContent {
                link: "../../x.md".into()
            }
            .to_string(),
            "link `../../x.md` points outside content/"
        );
        assert_eq!(
            LinkProblem::NotMarkdown {
                link: "@/a/b".into()
            }
            .to_string(),
            "link `@/a/b` must name a .md file"
        );
        assert_eq!(
            LinkProblem::NoAnchor {
                target: PathBuf::from("a/b.md"),
                fragment: "zz".into()
            }
            .to_string(),
            "links to `#zz` on `a/b.md`, which has no such id"
        );
    }
}
