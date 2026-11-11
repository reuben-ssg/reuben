//! A thin wrapper over html5ever's tokenizer that yields the events the
//! accessibility checks need, and nothing else.

use std::cell::{Cell, RefCell};

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};

/// A start tag with its lower-cased name, attributes in source order, and
/// where it was found.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StartTag<S> {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    /// 1-based line in the whole input.
    pub line: usize,
    /// The label of the piece being fed when the tag was emitted.
    pub piece: S,
}

impl<S> StartTag<S> {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(attr, _)| attr == name)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event<S> {
    Start(StartTag<S>),
    /// An end tag's lower-cased name.
    End(String),
    /// A run of text, including the content of `<title>` and `<textarea>`. The content
    /// of `<script>` and `<style>` is dropped.
    Text(String),
}

struct Sink<S> {
    piece: Cell<S>,
    in_code: Cell<bool>,
    events: RefCell<Vec<Event<S>>>,
}

impl<S: Copy> TokenSink for Sink<S> {
    type Handle = ();

    fn process_token(&self, token: Token, line_number: u64) -> TokenSinkResult<()> {
        match token {
            Token::TagToken(Tag {
                kind: TagKind::StartTag,
                name,
                attrs,
                ..
            }) => {
                let name = (*name).to_ascii_lowercase();
                let result = match name.as_str() {
                    "script" => {
                        self.in_code.set(true);
                        TokenSinkResult::RawData(RawKind::ScriptData)
                    }
                    "style" => {
                        self.in_code.set(true);
                        TokenSinkResult::RawData(RawKind::Rawtext)
                    }
                    "title" | "textarea" => TokenSinkResult::RawData(RawKind::Rcdata),
                    _ => TokenSinkResult::Continue,
                };
                self.events.borrow_mut().push(Event::Start(StartTag {
                    name,
                    attrs: attrs
                        .into_iter()
                        .map(|a| ((*a.name.local).to_ascii_lowercase(), a.value.to_string()))
                        .collect(),
                    line: usize::try_from(line_number).unwrap_or(usize::MAX),
                    piece: self.piece.get(),
                }));
                result
            }
            Token::TagToken(Tag {
                kind: TagKind::EndTag,
                name,
                ..
            }) => {
                self.in_code.set(false);
                self.events
                    .borrow_mut()
                    .push(Event::End((*name).to_ascii_lowercase()));
                TokenSinkResult::Continue
            }
            Token::CharacterTokens(text) if !self.in_code.get() => {
                let mut events = self.events.borrow_mut();
                match events.last_mut() {
                    Some(Event::Text(previous)) => previous.push_str(&text),
                    _ => events.push(Event::Text(text.to_string())),
                }
                TokenSinkResult::Continue
            }
            _ => TokenSinkResult::Continue,
        }
    }
}

/// Tokenizes `pieces` as one input, feeding one piece at a time, and returns
/// the events in order. Never fails: malformed HTML just yields fewer events.
pub(crate) fn scan<S: Copy>(pieces: &[(&str, S)]) -> Vec<Event<S>> {
    let Some(&(_, first)) = pieces.first() else {
        return Vec::new();
    };
    let tokenizer = Tokenizer::new(
        Sink {
            piece: Cell::new(first),
            in_code: Cell::new(false),
            events: RefCell::new(Vec::new()),
        },
        TokenizerOpts::default(),
    );
    for &(html, label) in pieces {
        tokenizer.sink.piece.set(label);
        let input = BufferQueue::default();
        input.push_back(StrTendril::from(html));
        let _ = tokenizer.feed(&input);
    }
    tokenizer.end();
    tokenizer.sink.events.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_tags_carry_name_attrs_line_and_piece() {
        let events = scan(&[("<P>x</P>\n<A HREF=/y title=\"t\">", 7)]);
        let Event::Start(a) = &events[4] else {
            panic!("{events:?}");
        };
        assert_eq!(a.name, "a");
        assert_eq!(a.attr("href"), Some("/y"));
        assert_eq!(a.attr("title"), Some("t"));
        assert_eq!(a.attr("class"), None);
        assert_eq!((a.line, a.piece), (2, 7));
    }

    #[test]
    fn script_and_style_content_is_not_markup_or_text() {
        let events = scan(&[(
            "<script>if (a<b) {}</script><style>a{}</style><!-- c -->",
            (),
        )]);
        assert_eq!(
            events
                .iter()
                .map(|e| match e {
                    Event::Start(t) => format!("<{}", t.name),
                    Event::End(n) => format!("</{n}"),
                    Event::Text(t) => format!("text {t}"),
                })
                .collect::<Vec<_>>(),
            ["<script", "</script", "<style", "</style"]
        );
    }

    #[test]
    fn title_content_is_text() {
        let events = scan(&[("<title>a <b> c</title>", ())]);
        assert!(events.contains(&Event::Text("a <b> c".to_string())));
    }
}
