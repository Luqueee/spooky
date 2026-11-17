//! The Markdown of an agent's answer, parsed into blocks of styled text the view draws.
//!
//! Only what an answer needs is kept: paragraphs, headings, code blocks, lists, quotes,
//! rules and tables, with bold, italic, strikethrough, code and links inside text. Raw HTML
//! and images are shown as the text they are written as, never interpreted: an answer is
//! model output, not trusted markup.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// How a run of text is styled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

/// A run of text with one style, and the link it leads to, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Span {
    pub text: String,
    pub style: Style,
    pub link: Option<String>,
}

/// One block of an answer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Block {
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    Paragraph(Vec<Span>),
    Code {
        language: Option<String>,
        text: String,
    },
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Quote(Vec<Block>),
    Rule,
    Table {
        header: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
        alignments: Vec<Alignment>,
    },
}

/// A parsed answer.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Document {
    blocks: Vec<Block>,
}

impl Document {
    pub(crate) fn parse(text: &str) -> Self {
        let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
        let mut builder = Builder::default();
        for event in Parser::new_ext(text, options) {
            builder.event(event);
        }
        Self {
            blocks: builder.finish(),
        }
    }

    pub(crate) fn blocks(&self) -> &[Block] {
        &self.blocks
    }
}

/// What is being built while the parser's events stream by.
enum Open {
    Heading(u8),
    Paragraph,
    Code {
        language: Option<String>,
    },
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Item,
    Quote,
    Table {
        alignments: Vec<Alignment>,
        header: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
    Row {
        header: bool,
        cells: Vec<Vec<Span>>,
    },
    Cell,
}

#[derive(Default)]
struct Builder {
    /// Blocks finished at each nesting level; the last is the one being filled.
    levels: Vec<Vec<Block>>,
    open: Vec<Open>,
    spans: Vec<Span>,
    code: String,
    style: Style,
    links: Vec<String>,
}

impl Builder {
    fn level(&mut self) -> &mut Vec<Block> {
        if self.levels.is_empty() {
            self.levels.push(Vec::new());
        }
        let last = self.levels.len() - 1;
        &mut self.levels[last]
    }

    fn push_block(&mut self, block: Block) {
        self.level().push(block);
    }

    fn text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let link = self.links.last().cloned();
        if let Some(last) = self.spans.last_mut()
            && last.style == self.style
            && last.link == link
        {
            last.text.push_str(text);
            return;
        }
        self.spans.push(Span {
            text: text.to_owned(),
            style: self.style,
            link,
        });
    }

    fn take_spans(&mut self) -> Vec<Span> {
        std::mem::take(&mut self.spans)
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if matches!(self.open.last(), Some(Open::Code { .. })) {
                    self.code.push_str(&text);
                } else {
                    self.text(&text);
                }
            }
            Event::Code(text) => {
                let before = self.style;
                self.style.code = true;
                self.text(&text);
                self.style = before;
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.text("\n"),
            Event::Html(text) | Event::InlineHtml(text) => self.text(&text),
            Event::Rule => {
                self.flush_text();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(done) => self.text(if done { "[x] " } else { "[ ] " }),
            Event::FootnoteReference(name) => self.text(&format!("[{name}]")),
            _ => {}
        }
    }

    /// Text of a tight list item arrives without a paragraph around it: it becomes one when
    /// something else starts after it.
    fn flush_text(&mut self) {
        let spans = self.take_spans();
        if !spans.is_empty() {
            self.push_block(Block::Paragraph(spans));
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        if matches!(
            tag,
            Tag::List(_) | Tag::BlockQuote(_) | Tag::CodeBlock(_) | Tag::Table(_)
        ) {
            self.flush_text();
        }
        match tag {
            Tag::Paragraph => self.open.push(Open::Paragraph),
            Tag::Heading { level, .. } => self.open.push(Open::Heading(match level {
                HeadingLevel::H1 => 1,
                HeadingLevel::H2 => 2,
                HeadingLevel::H3 => 3,
                HeadingLevel::H4 => 4,
                HeadingLevel::H5 => 5,
                HeadingLevel::H6 => 6,
            })),
            Tag::CodeBlock(kind) => {
                self.code.clear();
                self.open.push(Open::Code {
                    language: match kind {
                        CodeBlockKind::Fenced(language) if !language.is_empty() => {
                            Some(language.to_string())
                        }
                        _ => None,
                    },
                });
            }
            Tag::List(start) => {
                self.open.push(Open::List {
                    start,
                    items: Vec::new(),
                });
            }
            Tag::Item => {
                self.levels.push(Vec::new());
                self.open.push(Open::Item);
            }
            Tag::BlockQuote(_) => {
                self.levels.push(Vec::new());
                self.open.push(Open::Quote);
            }
            Tag::Table(alignments) => self.open.push(Open::Table {
                alignments,
                header: Vec::new(),
                rows: Vec::new(),
            }),
            Tag::TableHead => self.open.push(Open::Row {
                header: true,
                cells: Vec::new(),
            }),
            Tag::TableRow => self.open.push(Open::Row {
                header: false,
                cells: Vec::new(),
            }),
            Tag::TableCell => self.open.push(Open::Cell),
            Tag::Emphasis => self.style.italic = true,
            Tag::Strong => self.style.bold = true,
            Tag::Strikethrough => self.style.strike = true,
            Tag::Link { dest_url, .. } => self.links.push(dest_url.to_string()),
            // An image is shown as its alternative text, and where it points.
            Tag::Image { dest_url, .. } => self.links.push(dest_url.to_string()),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => {
                let spans = self.take_spans();
                let block = match self.open.pop() {
                    Some(Open::Heading(level)) => Block::Heading { level, spans },
                    _ => Block::Paragraph(spans),
                };
                // A tight list item holds its text without a paragraph of its own.
                if !spans_empty(&block) {
                    self.push_block(block);
                }
            }
            TagEnd::CodeBlock => {
                let language = match self.open.pop() {
                    Some(Open::Code { language }) => language,
                    _ => None,
                };
                let text = std::mem::take(&mut self.code);
                self.push_block(Block::Code {
                    language,
                    text: text.trim_end_matches('\n').to_owned(),
                });
            }
            TagEnd::Item => {
                // Text of a tight item was never closed by a paragraph.
                let spans = self.take_spans();
                if !spans.is_empty() {
                    self.push_block(Block::Paragraph(spans));
                }
                let blocks = self.levels.pop().unwrap_or_default();
                self.open.pop();
                if let Some(Open::List { items, .. }) = self.open.last_mut() {
                    items.push(blocks);
                }
            }
            TagEnd::List(_) => {
                if let Some(Open::List { start, items }) = self.open.pop() {
                    self.push_block(Block::List { start, items });
                }
            }
            TagEnd::BlockQuote(_) => {
                let blocks = self.levels.pop().unwrap_or_default();
                self.open.pop();
                self.push_block(Block::Quote(blocks));
            }
            TagEnd::TableCell => {
                let spans = self.take_spans();
                self.open.pop();
                if let Some(Open::Row { cells, .. }) = self.open.last_mut() {
                    cells.push(spans);
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(Open::Row { header, cells }) = self.open.pop()
                    && let Some(Open::Table {
                        header: head, rows, ..
                    }) = self.open.last_mut()
                {
                    if header {
                        *head = cells;
                    } else {
                        rows.push(cells);
                    }
                }
            }
            TagEnd::Table => {
                if let Some(Open::Table {
                    alignments,
                    header,
                    rows,
                }) = self.open.pop()
                {
                    self.push_block(Block::Table {
                        header,
                        rows,
                        alignments,
                    });
                }
            }
            TagEnd::Emphasis => self.style.italic = false,
            TagEnd::Strong => self.style.bold = false,
            TagEnd::Strikethrough => self.style.strike = false,
            TagEnd::Link | TagEnd::Image => {
                self.links.pop();
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<Block> {
        let spans = self.take_spans();
        if !spans.is_empty() {
            self.push_block(Block::Paragraph(spans));
        }
        self.levels.into_iter().next().unwrap_or_default()
    }
}

fn spans_empty(block: &Block) -> bool {
    matches!(block, Block::Paragraph(spans) | Block::Heading { spans, .. } if spans.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Span {
        Span {
            text: text.into(),
            style: Style::default(),
            link: None,
        }
    }

    #[test]
    fn headings_and_paragraphs_become_blocks_in_order() {
        let doc = Document::parse("# Title\n\nfirst line\nsecond line\n\nnext");
        assert_eq!(
            doc.blocks(),
            [
                Block::Heading {
                    level: 1,
                    spans: vec![plain("Title")]
                },
                Block::Paragraph(vec![plain("first line second line")]),
                Block::Paragraph(vec![plain("next")]),
            ]
        );
    }

    #[test]
    fn inline_styles_and_links_split_the_text_into_spans() {
        let doc =
            Document::parse("a **bold** and *it* and `code` and ~~gone~~ and [site](https://x.y)");
        let [Block::Paragraph(spans)] = doc.blocks() else {
            panic!("one paragraph, got {:?}", doc.blocks());
        };
        let bold = spans.iter().find(|s| s.text == "bold").map(|s| s.style);
        assert_eq!(bold.map(|s| s.bold), Some(true));
        assert_eq!(
            spans
                .iter()
                .find(|s| s.text == "it")
                .map(|s| s.style.italic),
            Some(true)
        );
        assert_eq!(
            spans
                .iter()
                .find(|s| s.text == "code")
                .map(|s| s.style.code),
            Some(true)
        );
        assert_eq!(
            spans
                .iter()
                .find(|s| s.text == "gone")
                .map(|s| s.style.strike),
            Some(true)
        );
        let link = spans.iter().find(|s| s.text == "site");
        assert_eq!(link.and_then(|s| s.link.as_deref()), Some("https://x.y"));
        assert!(
            spans.iter().filter(|s| s.link.is_some()).count() == 1,
            "only the link text leads somewhere"
        );
    }

    #[test]
    fn a_fenced_block_keeps_its_language_and_text_untouched() {
        let doc = Document::parse("```rust\nfn main() {}\n  indented\n```\n");
        assert_eq!(
            doc.blocks(),
            [Block::Code {
                language: Some("rust".into()),
                text: "fn main() {}\n  indented".into()
            }]
        );
    }

    #[test]
    fn lists_nest_and_keep_their_numbering_start() {
        let doc = Document::parse("3. one\n4. two\n   - inner\n");
        let [Block::List { start, items }] = doc.blocks() else {
            panic!("one list, got {:?}", doc.blocks());
        };
        assert_eq!(*start, Some(3));
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], [Block::Paragraph(vec![plain("one")])]);
        assert!(matches!(
            &items[1][..],
            [Block::Paragraph(_), Block::List { start: None, .. }]
        ));
    }

    #[test]
    fn quotes_rules_and_tables_are_blocks() {
        let doc = Document::parse("> quoted\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(matches!(&doc.blocks()[0], Block::Quote(inner) if inner.len() == 1));
        assert_eq!(doc.blocks()[1], Block::Rule);
        let Block::Table { header, rows, .. } = &doc.blocks()[2] else {
            panic!("a table, got {:?}", doc.blocks());
        };
        assert_eq!(header, &[vec![plain("a")], vec![plain("b")]]);
        assert_eq!(rows, &[vec![vec![plain("1")], vec![plain("2")]]]);
    }

    #[test]
    fn html_is_text_and_never_markup() {
        let doc = Document::parse("<script>alert(1)</script>\n");
        let text: String = doc
            .blocks()
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(spans) => {
                    Some(spans.iter().map(|s| s.text.as_str()).collect::<String>())
                }
                _ => None,
            })
            .collect();
        assert!(text.contains("<script>"), "shown as written: {text:?}");
    }

    #[test]
    fn an_unfinished_answer_still_parses() {
        let doc = Document::parse("text with **half");
        assert_eq!(doc.blocks().len(), 1);
        assert!(Document::parse("").blocks().is_empty());
    }
}
