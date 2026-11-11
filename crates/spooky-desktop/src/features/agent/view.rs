//! An agent pane as drawn: the conversation and the composer. The look follows a chat
//! app: the user's messages in a rounded bubble on the right, the agent's answers as
//! Markdown in a centred column with a row of actions under them, tool calls and
//! reasoning as folded cards, and a rounded composer with the model and the agent's state
//! underneath. Everything the agent wrote is drawn, never acted on.

use super::{Agents, Attachment, Menu, Message as AgentMessageEvent, model_id, provider};
use crate::{Message, ui::*};
use iced::{
    Alignment, Background, Border, Element, Font, Length, font, keyboard,
    widget::{
        Space, button, column, container, image, markdown, mouse_area, opaque, row, scrollable,
        stack, text, text_editor, text_input,
    },
};
use spooky_core::{
    Agent, AgentDialog, AgentMessage, AgentRole, AgentTool, DialogAnswer, DialogKind, ToolState,
};

/// The id of the model search box.
pub(crate) const SEARCH: &str = "agent-model-search";
/// The id of the composer field.
pub(crate) const FIELD: &str = "agent-input";

/// The conversation is prose, so it uses the system's proportional font, not the terminal's.
const FONT: Font = Font {
    family: font::Family::SansSerif,
    ..Font::DEFAULT
};
/// Width of the conversation column.
const COLUMN_WIDTH: f32 = 760.0;
/// Widest a user's bubble grows.
const BUBBLE_WIDTH: f32 = 520.0;

/// The message a control sends.
fn tell(message: AgentMessageEvent) -> Message {
    Message::Agent(message)
}

/// How Markdown in an answer is drawn: links report themselves, and code blocks carry a
/// copy button.
struct Viewer;

impl<'a> markdown::Viewer<'a, Message> for Viewer {
    fn on_link_click(url: markdown::Uri) -> Message {
        tell(AgentMessageEvent::Link(url))
    }

    fn code_block(
        &self,
        settings: markdown::Settings,
        language: Option<&'a str>,
        code: &'a str,
        lines: &'a [markdown::Text],
    ) -> Element<'a, Message> {
        let header = row![
            text(language.filter(|l| !l.is_empty()).unwrap_or("code"))
                .size(11)
                .color(MUTED),
            Space::new().width(Length::Fill),
            action(
                Icon::Copy,
                "Copy code",
                tell(AgentMessageEvent::Copy(code.to_owned()))
            ),
        ]
        .align_y(Alignment::Center);
        column![
            header,
            markdown::code_block(settings, lines, Self::on_link_click)
        ]
        .spacing(2)
        .into()
    }
}

/// The conversation of the agent pane `pane`, and its composer.
pub(crate) fn view<'a>(agents: &'a Agents, pane: u64, agent: &'a Agent) -> Element<'a, Message> {
    let mut conversation = column![].spacing(22).width(Length::Fill);
    if agent.dropped > 0 {
        conversation = conversation.push(
            text(format!("{} earlier entries not shown", agent.dropped))
                .size(11)
                .color(MUTED),
        );
    }
    if agent.messages.is_empty() && agent.ended.is_none() {
        conversation = conversation.push(
            container(
                text("What can I help with?")
                    .size(16)
                    .font(FONT)
                    .color(MUTED),
            )
            .padding([40, 0])
            .center_x(Length::Fill),
        );
    }
    let last = agent.messages.len().saturating_sub(1);
    for (index, entry) in agent.messages.iter().enumerate() {
        let streaming = agent.busy && index == last;
        conversation = conversation.push(entry_view(agents, pane, entry, streaming));
    }

    let live = agent.ended.is_none();
    let mut body = column![
        scrollable(centred(container(conversation).padding([24, 18]).into()))
            .anchor_bottom()
            .width(Length::Fill)
            .height(Length::Fill),
    ];
    let mut bottom = column![].spacing(8).padding([6, 18]);
    for dialog in &agent.dialogs {
        bottom = bottom.push(dialog_view(agents, pane, dialog));
    }
    bottom = bottom.push(composer(agents, pane, agent, live));
    bottom = bottom.push(footer(agents, pane, agent));
    body = body
        .push(centred(bottom.into()))
        .push(Space::new().height(6));
    let page = container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| surface(BACKGROUND));
    // A press anywhere that no control took closes an open list.
    if let Some(kind) = agents.menu(pane) {
        return mouse_area(stack![page, menu_layer(agents, pane, kind, agent)])
            .on_press(tell(AgentMessageEvent::CloseMenu))
            .into();
    }
    page.into()
}

/// A column of the conversation's width, in the middle.
fn centred<'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    container(container(content).max_width(COLUMN_WIDTH))
        .center_x(Length::Fill)
        .width(Length::Fill)
        .into()
}

/// One entry of the conversation.
fn entry_view<'a>(
    agents: &'a Agents,
    pane: u64,
    entry: &'a AgentMessage,
    streaming: bool,
) -> Element<'a, Message> {
    match (entry.role, &entry.tool) {
        (AgentRole::User, _) => row![
            Space::new().width(Length::Fill),
            container(text(entry.text.as_str()).size(15).font(FONT).color(TEXT))
                .padding([9, 16])
                .max_width(BUBBLE_WIDTH)
                .style(|_| container::Style {
                    background: Some(Background::Color(SELECTED)),
                    border: Border {
                        radius: 14.0.into(),
                        ..Border::default()
                    },
                    text_color: Some(TEXT),
                    ..container::Style::default()
                }),
        ]
        .into(),
        (AgentRole::Assistant, _) => {
            let mut block = column![].spacing(8);
            block = block.push(match agents.markdown(pane, entry.id) {
                Some(content) => markdown::view_with(content.items(), settings(), &Viewer),
                None => text(entry.text.as_str())
                    .size(15)
                    .font(FONT)
                    .color(TEXT)
                    .into(),
            });
            // Actions appear once the answer is complete.
            if !streaming {
                block = block.push(action(
                    Icon::Copy,
                    "Copy answer",
                    tell(AgentMessageEvent::Copy(entry.text.clone())),
                ));
            }
            block.into()
        }
        (AgentRole::Thinking, _) => thinking_view(agents, pane, entry, streaming),
        (AgentRole::Tool, Some(tool)) => tool_view(agents, pane, entry, tool),
        (AgentRole::Tool | AgentRole::Notice, _) => {
            Element::from(text(entry.text.as_str()).size(13).font(FONT).color(MUTED))
        }
    }
}

/// How Markdown is sized and coloured.
fn settings() -> markdown::Settings {
    let mut style = markdown::Style::from_palette(theme().palette());
    style.font = FONT;
    style.code_block_font = Font::MONOSPACE;
    style.inline_code_font = Font::MONOSPACE;
    markdown::Settings::with_text_size(15, style)
}

/// A header that folds or unfolds its entry.
fn fold_header<'a>(
    pane: u64,
    entry: u64,
    unfolded: bool,
    content: Element<'a, Message>,
) -> Element<'a, Message> {
    button(
        row![
            icon(
                if unfolded {
                    Icon::Chevron
                } else {
                    Icon::ChevronRight
                },
                MUTED
            ),
            content,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([3, 6])
    .style(quiet)
    .on_press(tell(AgentMessageEvent::Toggle(pane, entry)))
    .into()
}

/// The agent's reasoning: a muted line that unfolds to the text.
fn thinking_view<'a>(
    agents: &'a Agents,
    pane: u64,
    entry: &'a AgentMessage,
    streaming: bool,
) -> Element<'a, Message> {
    let unfolded = agents.is_unfolded(pane, entry.id);
    let label = if streaming { "Thinking…" } else { "Thought" };
    let mut block = column![fold_header(
        pane,
        entry.id,
        unfolded,
        text(label).size(12).font(FONT).color(MUTED).into()
    )]
    .spacing(4);
    if unfolded {
        block = block.push(
            container(text(entry.text.as_str()).size(13).font(FONT).color(MUTED)).padding([0, 22]),
        );
    }
    block.into()
}

/// A tool call as a card: what it is, how far it got, and its output when unfolded.
fn tool_view<'a>(
    agents: &'a Agents,
    pane: u64,
    entry: &'a AgentMessage,
    tool: &'a AgentTool,
) -> Element<'a, Message> {
    let unfolded = agents.is_unfolded(pane, entry.id);
    let color = match tool.state {
        ToolState::Running => ACCENT,
        ToolState::Done => SUCCESS,
        ToolState::Failed => DANGER,
    };
    let mut title = row![
        dot(color),
        text(tool.name.as_str()).size(12).font(FONT).color(TEXT)
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if !entry.text.is_empty() {
        title = title.push(text(entry.text.as_str()).size(12).font(FONT).color(MUTED));
    }
    if !tool.detail.is_empty() {
        title = title.push(
            text(tool.detail.as_str())
                .size(11)
                .font(Font::MONOSPACE)
                .color(MUTED),
        );
    }
    let mut card = column![fold_header(pane, entry.id, unfolded, title.into())].spacing(4);
    if unfolded {
        card = card.push(
            container(output(tool))
                .padding(10)
                .width(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(HOVER)),
                    border: Border {
                        radius: 8.0.into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                }),
        );
    }
    container(card)
        .padding(4)
        .width(Length::Fill)
        .style(|_| container::Style {
            border: Border {
                color: BORDER,
                width: 1.0,
                radius: 10.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// What a tool printed, in the terminal's font, or what an edit changed; added and
/// removed lines are coloured.
fn output(tool: &AgentTool) -> Element<'_, Message> {
    let (shown, diff) = if tool.diff.is_empty() {
        (&tool.output, false)
    } else {
        (&tool.diff, true)
    };
    if shown.is_empty() {
        return text(if tool.state == ToolState::Running {
            "Running…"
        } else {
            "No output"
        })
        .size(12)
        .color(MUTED)
        .into();
    }
    column(shown.lines().map(|line| {
        let color = match line.chars().next() {
            Some('+') if diff => SUCCESS,
            Some('-') if diff => DANGER,
            _ => TEXT,
        };
        text(line)
            .size(12)
            .font(Font::MONOSPACE)
            .color(color)
            .into()
    }))
    .into()
}

/// How a dialog's choice looks.
#[derive(Clone, Copy)]
enum Look {
    Plain,
    Strong,
    Danger,
}

/// A question or approval the agent waits on. Only a click here answers it.
fn dialog_view<'a>(agents: &'a Agents, pane: u64, dialog: &'a AgentDialog) -> Element<'a, Message> {
    let answer =
        |what: DialogAnswer| tell(AgentMessageEvent::Answer(pane, dialog.id.clone(), what));
    let choice = |label: &'a str, what: DialogAnswer, look: Look| {
        let style = match look {
            Look::Plain => chip(None, TEXT),
            Look::Strong => chip(Some(TEXT), BACKGROUND),
            Look::Danger => chip(None, DANGER),
        };
        button(text(label).size(13).font(FONT))
            .padding([6, 14])
            .style(style)
            .on_press(answer(what))
    };
    let mut card = column![text(dialog.title.as_str()).size(14).font(FONT).color(TEXT)].spacing(8);
    if !dialog.message.is_empty() {
        card = card.push(
            text(dialog.message.as_str())
                .size(12)
                .font(FONT)
                .color(MUTED),
        );
    }
    let mut buttons = row![].spacing(8).align_y(Alignment::Center);
    match dialog.kind {
        DialogKind::Confirm => {
            buttons = buttons
                .push(choice("Allow", DialogAnswer::Confirm(true), Look::Strong))
                .push(choice("Deny", DialogAnswer::Confirm(false), Look::Danger));
        }
        DialogKind::Select => {
            for option in &dialog.options {
                buttons = buttons.push(choice(
                    option.as_str(),
                    DialogAnswer::Value(option.clone()),
                    Look::Plain,
                ));
            }
        }
        DialogKind::Input => {
            let typed = agents.answer_text(pane, &dialog.id);
            let id = dialog.id.clone();
            let field = text_input("Answer", typed)
                .font(FONT)
                .size(13)
                .padding([6, 8])
                .on_input(move |text| tell(AgentMessageEvent::DialogText(pane, id.clone(), text)))
                .on_submit(answer(DialogAnswer::Value(typed.to_owned())));
            buttons = buttons.push(field).push(choice(
                "Submit",
                DialogAnswer::Value(typed.to_owned()),
                Look::Strong,
            ));
        }
    }
    buttons = buttons.push(choice("Dismiss", DialogAnswer::Cancel, Look::Plain));
    container(card.push(buttons))
        .padding(14)
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(SELECTED)),
            border: Border {
                color: ACCENT,
                width: 1.0,
                radius: 14.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// The multiline composer: Enter sends, Shift+Enter starts a new line.
fn composer<'a>(
    agents: &'a Agents,
    pane: u64,
    agent: &'a Agent,
    live: bool,
) -> Element<'a, Message> {
    let Some(content) = agents.composer(pane) else {
        return Space::new().into();
    };
    let mut field = text_editor(content)
        .id(FIELD)
        .placeholder(if live { "Reply" } else { "The agent has ended" })
        .font(FONT)
        .size(15)
        .padding([6, 12])
        .min_height(20)
        .max_height(220)
        .style(|theme, status| {
            let mut style = text_editor::default(theme, status);
            style.background = Background::Color(iced::Color::TRANSPARENT);
            style.border = Border::default();
            style
        });
    if live {
        field = field
            .on_action(move |action| tell(AgentMessageEvent::Edit(pane, action)))
            .key_binding(move |press| match press.key.as_ref() {
                keyboard::Key::Named(keyboard::key::Named::Enter) if !press.modifiers.shift() => {
                    Some(text_editor::Binding::Custom(tell(AgentMessageEvent::Send(
                        pane,
                    ))))
                }
                keyboard::Key::Named(keyboard::key::Named::Enter) => {
                    Some(text_editor::Binding::Enter)
                }
                _ => text_editor::Binding::from_key_press(press),
            });
    }
    let mut controls = row![field].spacing(8).align_y(Alignment::End);
    if agent.busy {
        controls = controls.push(
            button(container(text("Stop").size(12)).center_y(Length::Fill))
                .height(32)
                .padding([0, 14])
                .style(destructive)
                .on_press(tell(AgentMessageEvent::Abort(pane))),
        );
    }
    if live {
        // The arrow is lit only when there is something to send.
        let ready = !content.text().trim().is_empty() || !agents.attachments(pane).is_empty();
        let arrow = button(
            container(icon(Icon::Send, if ready { BACKGROUND } else { MUTED }))
                .center(Length::Fill),
        )
        .width(32)
        .height(32)
        .padding(0)
        .style(move |_, _| button::Style {
            background: Some(Background::Color(if ready { TEXT } else { HOVER })),
            border: Border {
                radius: 16.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        });
        controls = controls.push(if ready {
            arrow.on_press(tell(AgentMessageEvent::Send(pane)))
        } else {
            arrow
        });
    }
    // Lighter than the page, as a raised field, with no outline.
    let field = container(controls)
        .padding([6, 10])
        .style(|_| container::Style {
            background: Some(Background::Color(SELECTED)),
            border: Border {
                radius: 22.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        });
    let files = agents.attachments(pane);
    if files.is_empty() {
        return field.into();
    }
    column![attachments(pane, files), field].spacing(6).into()
}

/// What the next message carries: a small picture of each attached image, or its name,
/// each with a button to drop it.
fn attachments<'a>(pane: u64, files: &'a [Attachment]) -> Element<'a, Message> {
    let chips = files.iter().map(|file| {
        let shown: Element<'_, Message> = match &file.preview {
            Some(preview) => column![
                image(image::Handle::from_bytes(preview.png.clone()))
                    .width(Length::Shrink)
                    .height(74),
                text(file.name.as_str()).size(11).color(MUTED),
            ]
            .spacing(4)
            .align_x(Alignment::Center)
            .into(),
            None => text(file.name.as_str()).size(12).into(),
        };
        let drop = button(text("×").size(13).color(MUTED))
            .padding([0, 4])
            .style(quiet)
            .on_press(tell(AgentMessageEvent::Unattach(pane, file.named.clone())));
        container(row![shown, drop].spacing(6).align_y(Alignment::Start))
            .padding([6, 8])
            .style(|_| container::Style {
                background: Some(Background::Color(SELECTED)),
                border: Border {
                    radius: 10.0.into(),
                    width: 1.0,
                    color: BORDER,
                },
                ..container::Style::default()
            })
            .into()
    });
    row(chips).spacing(8).wrap().into()
}

/// The list of choices is a shade darker than the composer, with an edge and a shadow to stand out of the page.
const POPUP: iced::Color = iced::Color::from_rgb8(36, 40, 50);
const POPUP_EDGE: iced::Color = iced::Color::from_rgb8(66, 72, 88);
const POPUP_ITEM: iced::Color = iced::Color::from_rgb8(51, 56, 68);

/// Width of the thinking button, so the lists below can be put right under their buttons.
const THINKING_WIDTH: f32 = 88.0;
/// Space between footer items.
const FOOTER_GAP: f32 = 8.0;
/// Left padding of the footer, aligning it with the text of the composer.
const FOOTER_INSET: f32 = 11.0;
/// From the bottom of the pane to the top of a one-line composer: a list opens above it.
const FOOTER_CLEARANCE: f32 = 90.0;

/// A button that opens or closes the list of choices of `kind`.
fn chooser<'a>(pane: u64, kind: Menu, open: bool, selected: &'a str) -> Element<'a, Message> {
    button(
        row![
            text(selected)
                .font(FONT)
                .size(12)
                .color(if open { TEXT } else { MUTED }),
            icon(if open { Icon::ChevronUp } else { Icon::Chevron }, MUTED),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([3, 8])
    .style(move |theme, status| selectable(open, theme, status))
    .on_press(tell(AgentMessageEvent::Menu(pane, kind)))
    .into()
}

/// A choice in a list: lit while chosen or under the pointer.
fn choice_style(chosen: bool) -> impl Fn(&iced::Theme, button::Status) -> button::Style {
    move |_, status| button::Style {
        background: match (chosen, status) {
            (true, _) | (false, button::Status::Hovered | button::Status::Pressed) => {
                Some(Background::Color(POPUP_ITEM))
            }
            _ => None,
        },
        text_color: if chosen { TEXT } else { MUTED },
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

/// A scrollable that scrolls with the wheel and draws no bar.
fn quiet_scroll<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    let hidden = scrollable::Scrollbar::new()
        .width(0)
        .scroller_width(0)
        .margin(0);
    scrollable(content)
        .direction(scrollable::Direction::Vertical(hidden))
        .width(Length::Fill)
        .into()
}

/// The frame of a list of choices.
fn popup<'a>(content: Element<'a, Message>, width: f32) -> container::Container<'a, Message> {
    container(content)
        .padding(4)
        .width(Length::Fixed(width))
        .style(|_| container::Style {
            background: Some(Background::Color(POPUP)),
            border: Border {
                color: POPUP_EDGE,
                width: 1.0,
                radius: 12.0.into(),
            },
            shadow: iced::Shadow {
                color: iced::Color::from_rgba8(0, 0, 0, 0.45),
                offset: iced::Vector::new(0.0, 6.0),
                blur_radius: 18.0,
            },
            ..container::Style::default()
        })
}

/// The thinking levels: a short list.
fn thinking_panel<'a>(pane: u64, agent: &'a Agent) -> container::Container<'a, Message> {
    let items = column(agent.levels.iter().map(|level| {
        let chosen = agent.thinking.as_deref() == Some(level.as_str());
        button(text(level.as_str()).font(FONT).size(12))
            .width(Length::Fill)
            .padding([5, 10])
            .style(choice_style(chosen))
            .on_press(tell(AgentMessageEvent::SetThinking(pane, level.clone())))
            .into()
    }));
    popup(items.into(), 110.0)
}

/// A small tag with a role's name.
fn role_chip<'a>(name: &str) -> Element<'a, Message> {
    container(text(name.to_owned()).font(FONT).size(10).color(ACCENT))
        .padding([1, 6])
        .style(|_| container::Style {
            border: Border {
                color: POPUP_EDGE,
                width: 1.0,
                radius: 6.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// The roles the program gives its models: the role, its model and level. A role whose
/// model the agent does not offer cannot be used and is dimmed.
fn role_list<'a>(pane: u64, agent: &'a Agent) -> iced::widget::Column<'a, Message> {
    let mut list = column![].spacing(2);
    for role in &agent.roles {
        let offered = agent.models.is_empty() || agent.models.contains(&role.model);
        let current = agent.model.as_deref() == Some(role.model.as_str());
        let mut line = row![
            container(role_chip(&role.role)).width(76),
            text(role.model.as_str())
                .font(FONT)
                .size(12)
                .width(Length::Fill)
                .color(if offered { TEXT } else { MUTED }),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if let Some(level) = &role.thinking {
            line = line.push(text(level.as_str()).font(FONT).size(11).color(MUTED));
        }
        let choice = button(line)
            .width(Length::Fill)
            .padding([6, 10])
            .style(choice_style(current));
        list = list.push(if offered {
            choice.on_press(tell(AgentMessageEvent::SetRole(
                pane,
                role.model.clone(),
                role.thinking.clone(),
            )))
        } else {
            choice
        });
    }
    list
}

/// The models: a search box on top, the providers down the left, and the models of the
/// chosen provider (or all) on the right, each provider under its own heading.
fn model_panel<'a>(
    agents: &'a Agents,
    pane: u64,
    agent: &'a Agent,
) -> container::Container<'a, Message> {
    let search = text_input("Search models", agents.model_search())
        .id(SEARCH)
        .font(FONT)
        .size(12)
        .padding([7, 10])
        .on_input(move |text| tell(AgentMessageEvent::MenuSearch(text)))
        .style(|theme, status| {
            let mut style = text_input::default(theme, status);
            style.background = Background::Color(iced::Color::TRANSPARENT);
            style.border = Border {
                color: POPUP_EDGE,
                width: 1.0,
                radius: 8.0.into(),
            };
            style.value = TEXT;
            style.placeholder = MUTED;
            style
        });

    // Providers, with how many of their models match the search.
    let mut counts: Vec<(&str, usize)> = Vec::new();
    let matching = agents.matching(&agent.models);
    for model in &matching {
        let name = provider(model);
        match counts.iter_mut().find(|(known, _)| *known == name) {
            Some((_, count)) => *count += 1,
            None => counts.push((name, 1)),
        }
    }
    counts.sort_by_key(|(name, _)| *name);
    let chosen = agents.model_provider();
    let entry = |label: &str, count: usize, name: Option<&str>| {
        let lit = !agents.shows_roles() && chosen == name;
        button(
            row![
                text(label.to_owned())
                    .font(FONT)
                    .size(12)
                    .width(Length::Fill),
                text(count.to_string()).font(FONT).size(11).color(MUTED),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding([5, 8])
        .style(choice_style(lit))
        .on_press(tell(AgentMessageEvent::MenuProvider(
            name.map(str::to_owned),
        )))
    };
    let mut sidebar = column![].spacing(2);
    if !agent.roles.is_empty() {
        sidebar = sidebar.push(
            button(
                row![
                    text("Roles").font(FONT).size(12).width(Length::Fill),
                    text(agent.roles.len().to_string())
                        .font(FONT)
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([5, 8])
            .style(choice_style(agents.shows_roles()))
            .on_press(tell(AgentMessageEvent::MenuRoles)),
        );
    }
    sidebar = sidebar.push(entry("All", matching.len(), None));
    for (name, count) in &counts {
        sidebar = sidebar.push(entry(name, *count, Some(name)));
    }

    let shown = if agents.shows_roles() {
        Vec::new()
    } else {
        agents.shown(&agent.models)
    };
    let mut ordered = shown.clone();
    ordered.sort_by_key(|model| provider(model));
    let mut list = if agents.shows_roles() {
        role_list(pane, agent)
    } else {
        column![].spacing(2)
    };
    let mut current = None;
    for model in ordered {
        if chosen.is_none() && current != Some(provider(model)) {
            current = Some(provider(model));
            list = list.push(
                container(text(provider(model)).font(FONT).size(11).color(ACCENT)).padding([8, 10]),
            );
        }
        let lit = agent.model.as_deref() == Some(model.as_str());
        let mut label = row![
            text(model_id(model))
                .font(FONT)
                .size(12)
                .width(Length::Fill)
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        // The roles this model plays.
        for role in agent.roles.iter().filter(|r| &r.model == model) {
            label = label.push(role_chip(&role.role));
        }
        list = list.push(
            button(label)
                .width(Length::Fill)
                .padding([5, 10])
                .style(choice_style(lit))
                .on_press(tell(AgentMessageEvent::SetModel(pane, model.clone()))),
        );
    }
    if shown.is_empty() && !agents.shows_roles() {
        list = list
            .push(container(text("No model matches").font(FONT).size(12).color(MUTED)).padding(10));
    }

    let body = row![
        container(quiet_scroll(sidebar)).width(150),
        container(Space::new().width(1).height(Length::Fill)).style(|_| surface(POPUP_EDGE)),
        quiet_scroll(list),
    ]
    .spacing(6)
    .height(Length::Fill);
    popup(
        column![
            container(search).padding(iced::Padding {
                top: 2.0,
                right: 2.0,
                bottom: 6.0,
                left: 2.0,
            }),
            body
        ]
        .into(),
        540.0,
    )
    .height(300)
}

/// The open list of choices, drawn over the pane just above its button. It is a layer of
/// its own, so opening it moves nothing.
fn menu_layer<'a>(
    agents: &'a Agents,
    pane: u64,
    kind: Menu,
    agent: &'a Agent,
) -> Element<'a, Message> {
    let (panel, indent) = match kind {
        Menu::Usage => (usage_panel(pane, agent), 0.0),
        Menu::Thinking => (thinking_panel(pane, agent), 0.0),
        Menu::Model => (
            model_panel(agents, pane, agent),
            THINKING_WIDTH + FOOTER_GAP,
        ),
    };
    if kind == Menu::Usage {
        // Usage is a dialog: centred over the pane on a dimmed backdrop. A press on the
        // panel itself is its own; one on the backdrop closes it.
        return container(opaque(panel))
            .center(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(iced::Color::from_rgba8(0, 0, 0, 0.5))),
                ..container::Style::default()
            })
            .into();
    }
    let placed = row![Space::new().width(FOOTER_INSET + indent), panel];
    column![
        Space::new().height(Length::Fill),
        centred(placed.width(Length::Fill).padding([0, 18]).into()),
        Space::new().height(FOOTER_CLEARANCE),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// A bar of how much of an allowance is left: lit in proportion, red when nearly gone.
fn allowance_bar<'a>(left: u8) -> Element<'a, Message> {
    let color = if left <= 10 { DANGER } else { ACCENT };
    let left = u16::from(left.min(100));
    container(row![
        container(Space::new())
            .width(Length::FillPortion(left.max(1)))
            .height(6)
            .style(move |_| container::Style {
                background: Some(Background::Color(color)),
                border: Border {
                    radius: 3.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            }),
        Space::new().width(Length::FillPortion(101 - left)),
    ])
    .width(Length::Fill)
    .height(6)
    .style(|_| container::Style {
        background: Some(Background::Color(POPUP_EDGE)),
        border: Border {
            radius: 3.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    })
    .into()
}

/// What the providers report about the remaining usage: per provider, how much of each
/// allowance each account has left and when it renews.
fn usage_panel<'a>(pane: u64, agent: &'a Agent) -> container::Container<'a, Message> {
    let refresh = button(text("Refresh").font(FONT).size(12))
        .padding([4, 12])
        .style(chip(None, TEXT))
        .on_press(tell(AgentMessageEvent::RefreshUsage(pane)));
    let mut body = column![
        row![
            text("Usage").font(FONT).size(14).color(TEXT),
            Space::new().width(Length::Fill),
            refresh,
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(12);
    if let Some(context) = agent.context {
        body = body.push(
            text(format!("Context window: {context}% used"))
                .font(FONT)
                .size(12)
                .color(MUTED),
        );
    }
    match &agent.usage {
        None => {
            body = body.push(text("Reading usage…").font(FONT).size(12).color(MUTED));
        }
        Some(providers) if providers.is_empty() => {
            body = body.push(text("No usage data").font(FONT).size(12).color(MUTED));
        }
        Some(providers) => {
            for provider in providers {
                let mut block =
                    column![text(provider.name.as_str()).font(FONT).size(13).color(TEXT)]
                        .spacing(6);
                for note in &provider.notes {
                    block = block.push(text(note.as_str()).font(FONT).size(11).color(SUCCESS));
                }
                // The account is named only when a provider has more than one.
                let several = provider
                    .windows
                    .iter()
                    .any(|w| w.account != provider.windows[0].account);
                for window in &provider.windows {
                    block = block.push(
                        row![
                            column![text(window.name.as_str()).font(FONT).size(12).color(TEXT)]
                                .extend(several.then(|| {
                                    Element::from(
                                        text(window.account.as_str())
                                            .font(FONT)
                                            .size(10)
                                            .color(MUTED),
                                    )
                                }))
                                .width(Length::FillPortion(3)),
                            container(allowance_bar(window.left)).width(Length::FillPortion(3)),
                            text(format!("{}% left", window.left))
                                .font(FONT)
                                .size(12)
                                .color(if window.left <= 10 { DANGER } else { TEXT })
                                .width(70)
                                .align_x(iced::alignment::Horizontal::Right),
                            text(if window.resets.is_empty() {
                                String::new()
                            } else {
                                format!("resets in {}", window.resets)
                            })
                            .font(FONT)
                            .size(11)
                            .color(MUTED)
                            .width(96)
                            .align_x(iced::alignment::Horizontal::Right),
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center),
                    );
                }
                body = body.push(block);
            }
        }
    }
    // Tall enough for what there is, up to what fits; a long report scrolls.
    let fitted = (match &agent.usage {
        Some(providers) => providers
            .iter()
            .map(|p| 34 + 16 * p.notes.len() + 24 * p.windows.len())
            .sum::<usize>(),
        None => 20,
    } as f32
        + 110.0)
        .clamp(150.0, 360.0);
    popup(
        container(quiet_scroll(container(body).padding(10)))
            .height(Length::Fill)
            .into(),
        560.0,
    )
    .height(Length::Fixed(fitted))
}

/// How much of the context window is used: a short bar and the figure.
fn context_meter<'a>(percent: u8) -> Element<'a, Message> {
    let color = if percent >= 90 { DANGER } else { ACCENT };
    let filled = 36.0 * f32::from(percent.min(100)) / 100.0;
    row![
        container(
            container(Space::new())
                .width(Length::Fixed(filled))
                .height(4)
                .style(move |_| surface(color))
        )
        .width(36)
        .height(4)
        .style(|_| container::Style {
            background: Some(Background::Color(BORDER)),
            border: Border {
                radius: 2.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        }),
        text(format!("{percent}%")).font(FONT).size(12).color(MUTED),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

/// Under the composer: the model and thinking level, the context in use, the state.
fn footer<'a>(agents: &'a Agents, pane: u64, agent: &'a Agent) -> Element<'a, Message> {
    let open = agents.menu(pane);
    let mut line = row![]
        .spacing(FOOTER_GAP)
        .align_y(Alignment::Center)
        .padding(iced::Padding {
            left: FOOTER_INSET,
            right: 10.0,
            ..iced::Padding::ZERO
        });
    if let (Some(level), false) = (&agent.thinking, agent.levels.is_empty()) {
        line = line.push(
            container(chooser(
                pane,
                Menu::Thinking,
                open == Some(Menu::Thinking),
                level,
            ))
            .width(THINKING_WIDTH),
        );
    }
    if let Some(model) = &agent.model {
        line = line.push(if agent.models.is_empty() {
            level_line(text(model.as_str()).font(FONT).size(12).color(MUTED).into())
        } else {
            chooser(pane, Menu::Model, open == Some(Menu::Model), model)
        });
    }
    line = line.push(Space::new().width(Length::Fill));
    // The context meter opens the usage the providers report.
    let usage_open = open == Some(Menu::Usage);
    line = line.push(
        button(match agent.context {
            Some(context) => context_meter(context),
            None => text("Usage").font(FONT).size(12).color(MUTED).into(),
        })
        .padding([3, 8])
        .style(move |theme, status| selectable(usage_open, theme, status))
        .on_press(tell(AgentMessageEvent::Menu(pane, Menu::Usage))),
    );
    line = line.push(level_line(
        text(agent.backend.as_str())
            .font(FONT)
            .size(12)
            .color(TEXT)
            .into(),
    ));
    line.push(level_line(
        match (&agent.ended, agent.busy, agent.dialogs.is_empty()) {
            (Some(reason), _, _) => text(reason.as_str()).font(FONT).size(12).color(DANGER),
            (None, _, false) => text("Waiting").font(FONT).size(12).color(ACCENT),
            (None, true, true) => text("Working…").font(FONT).size(12).color(ACCENT),
            (None, false, true) => text("Ready").font(FONT).size(12).color(MUTED),
        }
        .into(),
    ))
    .into()
}

/// Centres a footer item on the height of a chooser button.
fn level_line<'a>(item: Element<'a, Message>) -> Element<'a, Message> {
    container(item)
        .height(26)
        .center_y(Length::Fixed(26.0))
        .into()
}

/// A bordered, rounded button: the look of a dialog's choices.
fn chip(
    fill: Option<iced::Color>,
    ink: iced::Color,
) -> impl Fn(&iced::Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(match (fill, hovered) {
                (Some(color), _) => color,
                (None, true) => HOVER,
                (None, false) => iced::Color::TRANSPARENT,
            })),
            text_color: ink,
            border: Border {
                color: if fill.is_some() {
                    iced::Color::TRANSPARENT
                } else {
                    BORDER
                },
                width: 1.0,
                radius: 8.0.into(),
            },
            ..button::Style::default()
        }
    }
}
