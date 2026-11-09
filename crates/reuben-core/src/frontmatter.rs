//! Front matter: YAML between `---` lines or TOML between `+++` lines, only at the very start of a file.

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::merge::toml_to_json;

#[derive(Debug, Clone, PartialEq)]
pub struct FrontMatter {
    pub title: String,
    pub draft: bool,
    pub description: Option<String>,
    pub date: Option<String>,
    pub layout: Option<String>,
    pub weight: Option<i64>,
    /// Every field Reuben doesn't recognize, passed through to templates untouched.
    pub extra: Map<String, Value>,
}

/// `title` is optional here only so a missing title can get its own error instead of a generic one.
#[derive(Deserialize)]
struct RawFrontMatter {
    title: Option<String>,
    #[serde(default)]
    draft: bool,
    description: Option<String>,
    date: Option<String>,
    layout: Option<String>,
    weight: Option<i64>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Debug, PartialEq)]
pub struct Parsed<'a> {
    pub front_matter: FrontMatter,
    pub body: &'a str,
    /// 1-based line in the original file where `body` starts.
    pub body_line: usize,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum FrontMatterError {
    #[error("file must start with front matter between `---` (YAML) or `+++` (TOML) lines")]
    Missing,
    #[error("no closing `{delimiter}` line")]
    Unterminated { delimiter: &'static str },
    #[error("invalid YAML: {0}")]
    Yaml(String),
    #[error("invalid TOML: {0}")]
    Toml(String),
    #[error("invalid front matter: {0}")]
    Invalid(String),
    #[error("`title` is required")]
    MissingTitle,
}

#[derive(Clone, Copy)]
enum Format {
    Yaml,
    Toml,
}

/// Front matter as an untyped map, before folder defaults are layered under it.
#[derive(Debug, PartialEq)]
pub struct RawParsed<'a> {
    pub value: Map<String, Value>,
    pub body: &'a str,
    /// 1-based line in the original file where `body` starts.
    pub body_line: usize,
}

pub fn parse(src: &str) -> Result<Parsed<'_>, FrontMatterError> {
    let raw = parse_raw(src)?;
    Ok(Parsed {
        front_matter: from_value(raw.value)?,
        body: raw.body,
        body_line: raw.body_line,
    })
}

pub fn parse_raw(src: &str) -> Result<RawParsed<'_>, FrontMatterError> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let mut lines = LineIter { src, pos: 0 };

    let (first, _) = lines.next().ok_or(FrontMatterError::Missing)?;
    let (delimiter, format) = match first {
        "---" => ("---", Format::Yaml),
        "+++" => ("+++", Format::Toml),
        _ => return Err(FrontMatterError::Missing),
    };

    let start = lines.pos;
    let mut line_no = 1;
    for (line, line_start) in lines.by_ref() {
        line_no += 1;
        if line == delimiter {
            let value = to_map(&src[start..line_start], format)?;
            return Ok(RawParsed {
                value,
                body: &src[lines.pos..],
                body_line: line_no + 1,
            });
        }
    }
    Err(FrontMatterError::Unterminated { delimiter })
}

/// Yields each line without its `\n` or `\r\n`, plus the byte offset where it starts.
struct LineIter<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Iterator for LineIter<'a> {
    type Item = (&'a str, usize);

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.src.len() {
            return None;
        }
        let start = self.pos;
        let rest = &self.src[start..];
        let (line, advance) = match rest.find('\n') {
            Some(i) => (&rest[..i], i + 1),
            None => (rest, rest.len()),
        };
        self.pos += advance;
        Some((line.strip_suffix('\r').unwrap_or(line), start))
    }
}

fn to_map(text: &str, format: Format) -> Result<Map<String, Value>, FrontMatterError> {
    let err = |e: String| match format {
        Format::Yaml => FrontMatterError::Yaml(e),
        Format::Toml => FrontMatterError::Toml(e),
    };
    let value = match format {
        Format::Yaml => serde_saphyr::from_str_with_options::<Value>(
            text,
            serde_saphyr::options! { strict_booleans: true },
        )
        .map_err(|e| err(e.to_string()))?,
        Format::Toml => toml_to_json(toml::from_str(text).map_err(|e| err(e.to_string()))?),
    };
    match value {
        Value::Null => Ok(Map::new()),
        Value::Object(map) => Ok(map),
        _ => Err(err("front matter must be a mapping".to_string())),
    }
}

/// Types a merged map. The title check lives here so a title supplied only by a folder's `_dir.toml` counts.
pub fn from_value(value: Map<String, Value>) -> Result<FrontMatter, FrontMatterError> {
    let raw: RawFrontMatter = serde_json::from_value(Value::Object(value))
        .map_err(|e| FrontMatterError::Invalid(e.to_string()))?;
    Ok(FrontMatter {
        title: raw
            .title
            .filter(|t| !t.trim().is_empty())
            .ok_or(FrontMatterError::MissingTitle)?,
        draft: raw.draft,
        description: raw.description,
        date: raw.date,
        layout: raw.layout,
        weight: raw.weight,
        extra: raw.extra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml() {
        let p = parse("---\ntitle: Hi\ndraft: true\ntags: [a]\n---\n# Body\n").unwrap();
        assert_eq!(p.front_matter.title, "Hi");
        assert!(p.front_matter.draft);
        assert_eq!(p.front_matter.extra["tags"], serde_json::json!(["a"]));
        assert_eq!(p.body, "# Body\n");
        assert_eq!(p.body_line, 6);
    }

    #[test]
    fn toml() {
        let p = parse("+++\ntitle = \"Hi\"\nweight = 3\n+++\nText\n").unwrap();
        assert_eq!(p.front_matter.weight, Some(3));
        assert_eq!(p.body_line, 5);
    }

    #[test]
    fn crlf_and_bom() {
        let p = parse("\u{feff}---\r\ntitle: Hi\r\n---\r\nText\r\n").unwrap();
        assert_eq!(p.front_matter.title, "Hi");
        assert_eq!(p.body_line, 4);
    }

    #[test]
    fn later_thematic_break_is_not_front_matter() {
        assert!(matches!(
            parse("Intro\n\n---\ntitle: x\n---\n"),
            Err(FrontMatterError::Missing)
        ));
    }

    #[test]
    fn unterminated() {
        assert!(matches!(
            parse("---\ntitle: Hi\n"),
            Err(FrontMatterError::Unterminated { delimiter: "---" })
        ));
    }

    #[test]
    fn missing_title() {
        assert!(matches!(
            parse("---\ndraft: true\n---\n"),
            Err(FrontMatterError::MissingTitle)
        ));
    }

    #[test]
    fn bad_yaml_is_an_error_not_a_panic() {
        assert!(matches!(
            parse("---\ntitle: [\n---\n"),
            Err(FrontMatterError::Yaml(_))
        ));
    }
}

#[cfg(test)]
mod raw_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn yaml_only_treats_true_and_false_as_booleans() {
        let p = parse("---\ntitle: No\ntags: [on, off]\n---\n").unwrap();
        assert_eq!(p.front_matter.title, "No");
        assert_eq!(p.front_matter.extra["tags"], json!(["on", "off"]));
    }

    #[test]
    fn parse_raw_does_not_validate() {
        let raw = parse_raw("---\ndraft: true\n---\nBody\n").unwrap();
        assert_eq!(raw.value["draft"], json!(true));
        assert!(!raw.value.contains_key("title"));
        assert_eq!(raw.body, "Body\n");
        assert_eq!(raw.body_line, 4);
    }

    #[test]
    fn from_value_requires_a_title() {
        let map = |v: Value| v.as_object().unwrap().clone();
        assert!(matches!(
            from_value(map(json!({"draft": true}))),
            Err(FrontMatterError::MissingTitle)
        ));
        let fm = from_value(map(json!({"title": "Hi"}))).unwrap();
        assert_eq!(fm.title, "Hi");
    }

    #[test]
    fn from_value_rejects_wrong_types() {
        let map = json!({"title": 5}).as_object().unwrap().clone();
        assert!(matches!(from_value(map), Err(FrontMatterError::Invalid(_))));
    }
}

#[cfg(test)]
mod review_fixes {
    use super::*;

    #[test]
    fn blank_title_is_missing() {
        assert!(matches!(
            parse("---\ntitle: \"  \"\n---\n"),
            Err(FrontMatterError::MissingTitle)
        ));
    }
}
