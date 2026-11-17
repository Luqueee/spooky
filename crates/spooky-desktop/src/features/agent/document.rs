//! A parsed answer as drawn: paragraphs, headings, code blocks, lists, quotes and tables in
//! the conversation's proportional font, with links that report themselves.

use super::{
    Message as AgentMessage,
    markdown::{Block, Document, Span},
};
use crate::{
    Message,
    gui::{Icon, Tone, View, column, hsla, row},
    terminal::DEFAULT_FAMILY,
};
use gpui::{
    AnyElement, Div, Font, FontFeatures, FontStyle, FontWeight, InteractiveElement,
    InteractiveText, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    StrikethroughStyle, Styled, StyledText, TextRun, UnderlineStyle, div, px,
};
use std::ops::Range;

/// The proportional font of the conversation.
pub(super) const PROSE: &str = ".SystemUIFont";
/// Body text size.
pub(super) const SIZE: f32 = 15.0;

fn font(family: &str, bold: bool, italic: bool) -> Font {
    Font {
        family: SharedString::from(family.to_owned()),
        features: FontFeatures::default(),
        fallbacks: None,
        weight: if bold {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        },
        style: if italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
    }
}

/// Styled text for `spans`, as one element; links are clickable.
fn text(v: &View, id: String, spans: &[Span], size: f32, bold: bool, tone: Tone) -> AnyElement {
    let base = crate::gui::tone(&v.p, tone);
    let mut content = String::new();
    let mut runs = Vec::with_capacity(spans.len());
    let mut links: Vec<(Range<usize>, String)> = Vec::new();
    for span in spans {
        let start = content.len();
        content.push_str(&span.text);
        let link = span.link.is_some();
        let family = if span.style.code {
            DEFAULT_FAMILY
        } else {
            PROSE
        };
        let color = if link { v.p.accent } else { base };
        runs.push(TextRun {
            len: span.text.len(),
            font: font(family, bold || span.style.bold, span.style.italic),
            color: hsla(color),
            background_color: span.style.code.then(|| hsla(v.p.hover)),
            underline: link.then(|| UnderlineStyle {
                color: Some(hsla(color)),
                thickness: px(1.0),
                wavy: false,
            }),
            strikethrough: span.style.strike.then(|| StrikethroughStyle {
                color: Some(hsla(color)),
                thickness: px(1.0),
            }),
        });
        if let Some(target) = &span.link {
            links.push((start..content.len(), target.clone()));
        }
    }
    let styled = StyledText::new(content).with_runs(runs);
    let body = div().text_size(px(if spans.iter().any(|span| span.style.code) {
        size * 0.92
    } else {
        size
    }));
    if links.is_empty() {
        return body.child(styled).into_any_element();
    }
    let send = v.ui.sender();
    let ranges = links.iter().map(|(range, _)| range.clone()).collect();
    let targets: Vec<String> = links.into_iter().map(|(_, target)| target).collect();
    let interactive = InteractiveText::new(SharedString::from(id), styled).on_click(
        ranges,
        move |index, window, cx| {
            if let Some(target) = targets.get(index) {
                send(
                    Message::Agent(AgentMessage::Link(target.clone())),
                    window,
                    cx,
                );
            }
        },
    );
    body.child(interactive).into_any_element()
}

fn block(v: &View, id: &str, item: &Block, tone: Tone) -> AnyElement {
    match item {
        Block::Paragraph(spans) => text(v, id.to_owned(), spans, SIZE, false, tone),
        Block::Heading { level, spans } => {
            let size = match level {
                1 => 22.0,
                2 => 19.0,
                3 => 17.0,
                _ => SIZE,
            };
            text(v, id.to_owned(), spans, size, true, Tone::Text)
        }
        Block::Code { language, text } => code(v, id, language.as_deref(), text),
        Block::List { start, items } => {
            let mut list = column().gap(px(4.));
            for (index, item) in items.iter().enumerate() {
                let marker = match start {
                    Some(first) => format!("{}.", first + index as u64),
                    None => "•".to_owned(),
                };
                let mut body = column().flex_1().gap(px(6.));
                for (position, inner) in item.iter().enumerate() {
                    body = body.child(block(v, &format!("{id}-{index}-{position}"), inner, tone));
                }
                list = list.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_start()
                        .gap(px(8.))
                        .child(div().w(px(22.)).flex().justify_end().child(v.text(
                            marker,
                            SIZE,
                            Tone::Muted,
                        )))
                        .child(body),
                );
            }
            list.into_any_element()
        }
        Block::Quote(blocks) => {
            let mut quote = column()
                .gap(px(6.))
                .pl(px(12.))
                .border_l_2()
                .border_color(hsla(v.p.border));
            for (index, inner) in blocks.iter().enumerate() {
                quote = quote.child(block(v, &format!("{id}-{index}"), inner, Tone::Muted));
            }
            quote.into_any_element()
        }
        Block::Rule => v.separator().into_any_element(),
        Block::Table { header, rows, .. } => {
            let cell = |id: String, spans: &[Span], bold: bool| {
                div().flex_1().min_w_0().px(px(8.)).py(px(4.)).child(text(
                    v,
                    id,
                    spans,
                    13.0,
                    bold,
                    Tone::Text,
                ))
            };
            let mut table = column()
                .w_full()
                .rounded(px(6.))
                .border_1()
                .border_color(hsla(v.p.border));
            let mut head = row().w_full().items_start().bg(hsla(v.p.hover));
            for (column_index, spans) in header.iter().enumerate() {
                head = head.child(cell(format!("{id}-h{column_index}"), spans, true));
            }
            table = table.child(head);
            for (row_index, cells) in rows.iter().enumerate() {
                let mut line = row().w_full().items_start();
                for (column_index, spans) in cells.iter().enumerate() {
                    line = line.child(cell(
                        format!("{id}-{row_index}-{column_index}"),
                        spans,
                        false,
                    ));
                }
                table = table.child(v.separator()).child(line);
            }
            table.into_any_element()
        }
    }
}

/// A code block: its language and a copy button above the code in the terminal's font.
fn code(v: &View, id: &str, language: Option<&str>, source: &str) -> AnyElement {
    let header = row()
        .w_full()
        .justify_between()
        .child(
            v.text(
                language
                    .filter(|name| !name.is_empty())
                    .unwrap_or("code")
                    .to_owned(),
                11.0,
                Tone::Muted,
            ),
        )
        .child(v.action(
            SharedString::from(format!("{id}-copy")),
            Icon::Copy,
            "Copy code",
            Message::Agent(AgentMessage::Copy(source.to_owned())),
        ));
    let mut lines = column().min_w_0();
    for line in source.trim_end_matches('\n').lines() {
        lines = lines.child(
            div()
                .font_family(DEFAULT_FAMILY)
                .text_size(px(13.))
                .whitespace_nowrap()
                .text_color(hsla(v.p.text))
                .min_h(px(18.))
                .child(line.to_owned()),
        );
    }
    column()
        .gap(px(2.))
        .child(header)
        .child(
            div()
                .id(SharedString::from(format!("{id}-code")))
                .w_full()
                .p(px(10.))
                .rounded(px(8.))
                .bg(hsla(v.p.hover))
                .overflow_x_scroll()
                .child(lines),
        )
        .into_any_element()
}

/// A parsed answer, as the column of blocks it is.
pub(super) fn view(v: &View, id: &str, document: &Document) -> Div {
    let mut blocks = column().gap(px(10.));
    for (index, item) in document.blocks().iter().enumerate() {
        blocks = blocks.child(block(v, &format!("{id}-{index}"), item, Tone::Text));
    }
    blocks.font_family(PROSE)
}
