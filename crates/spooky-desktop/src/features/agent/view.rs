//! An agent pane as drawn: the conversation and the composer. The look follows a chat
//! app: the user's messages in a rounded bubble on the right, the agent's answers as
//! Markdown in a centred column with a row of actions under them, tool calls and
//! reasoning as folded cards, and a rounded composer with the model and the agent's state
//! underneath. Everything the agent wrote is drawn, never acted on.

use super::{
    Agents, Attachment, Menu, Message as AgentMessageEvent,
    commands::Choice,
    document::{self, PROSE, SIZE},
    model_id, provider,
};
use crate::{
    Message,
    gui::{Icon, Look, Spec, Tone, View, column, hsla, icon, row},
    keys::{Key, Named, Press},
    shortcuts,
    terminal::DEFAULT_FAMILY,
    ui::Color,
};
use gpui::{
    AnyElement, Div, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, img, px,
};
use spooky_core::{
    Agent, AgentDialog, AgentMessage, AgentRole, AgentTool, DialogAnswer, DialogKind, ToolState,
};
use std::rc::Rc;

/// The id of the model search box.
pub(crate) const SEARCH: &str = "agent-model-search";
/// The id of the composer field.
pub(crate) const FIELD: &str = "agent-input";
/// The ids of the answer fields of dialogs that ask for text.
const ANSWERS: [&str; 4] = [
    "agent-answer-0",
    "agent-answer-1",
    "agent-answer-2",
    "agent-answer-3",
];

/// Width of the conversation column.
const COLUMN_WIDTH: f32 = 760.0;
/// Widest a user's bubble grows.
const BUBBLE_WIDTH: f32 = 520.0;
/// Commands drawn at once; the list scrolls through what is offered.
const MOST_COMMANDS: usize = 50;

/// The list of choices is a shade darker than the composer, with an edge to stand out of
/// the page.
const POPUP: Color = Color::from_rgb8(36, 40, 50);
const POPUP_EDGE: Color = Color::from_rgb8(66, 72, 88);
const POPUP_ITEM: Color = Color::from_rgb8(51, 56, 68);

/// Width of the thinking button, so the lists below can be put right under their buttons.
const THINKING_WIDTH: f32 = 88.0;
/// Space between footer items.
const FOOTER_GAP: f32 = 8.0;
/// Left padding of the footer, aligning it with the text of the composer.
const FOOTER_INSET: f32 = 11.0;
/// From the bottom of the pane to the top of a one-line composer: a list opens above it.
const FOOTER_CLEARANCE: f32 = 90.0;

/// The message a control sends.
fn tell(message: AgentMessageEvent) -> Message {
    Message::Agent(message)
}

pub(crate) fn view(
    v: &mut View,
    agents: &Agents,
    pane: u64,
    agent: &Agent,
    can_zoom: bool,
) -> AnyElement {
    let mut conversation = column().w_full().gap(px(22.));
    if agent.dropped > 0 {
        conversation = conversation.child(v.text(
            format!("{} earlier entries not shown", agent.dropped),
            11.0,
            Tone::Muted,
        ));
    }
    if agent.messages.is_empty() && agent.ended.is_none() {
        conversation = conversation.child(
            div()
                .w_full()
                .flex()
                .justify_center()
                .py(px(40.))
                .child(v.text("What can I help with?", 16.0, Tone::Muted)),
        );
    }
    let last = agent.messages.len().saturating_sub(1);
    for (index, entry) in agent.messages.iter().enumerate() {
        let streaming = agent.busy && index == last;
        conversation = conversation.child(entry_view(v, agents, pane, entry, streaming));
    }

    let live = agent.ended.is_none();
    let handle = v.ui.scroll(pane);
    // Stay at the bottom while the conversation grows, unless it was scrolled back.
    let offset = handle.offset().y;
    let most = handle.max_offset().y;
    if most <= px(1.) || -offset >= most - px(2.) {
        handle.scroll_to_bottom();
    }
    let scroll = div()
        .id(("agent-conversation", pane))
        .w_full()
        .flex_1()
        .overflow_y_scroll()
        .track_scroll(&handle)
        .child(centred(
            div().w_full().px(px(18.)).py(px(24.)).child(conversation),
        ));
    let mut bottom = column().w_full().gap(px(8.)).px(px(18.)).py(px(6.));
    for (index, dialog) in agent.dialogs.iter().enumerate() {
        bottom = bottom.child(dialog_view(v, agents, pane, dialog, index));
    }
    bottom = bottom.child(composer(v, agents, pane, agent, live, can_zoom));
    bottom = bottom.child(footer(v, agents, pane, agent));
    let mut page = column()
        .size_full()
        .font_family(PROSE)
        .child(scroll)
        .child(centred(bottom))
        .child(div().h(px(6.)).flex_none());
    // A press anywhere that no control took closes an open list.
    if let Some(kind) = agents.menu(pane) {
        page = page
            .child(v.dismiss_layer("agent-menu-dismiss", tell(AgentMessageEvent::CloseMenu)))
            .child(menu_layer(v, agents, pane, kind, agent));
    }
    page.into_any_element()
}

/// A column of the conversation's width, in the middle.
fn centred(content: impl IntoElement) -> Div {
    div()
        .w_full()
        .flex()
        .justify_center()
        .child(div().w_full().max_w(px(COLUMN_WIDTH)).child(content))
}

/// One entry of the conversation.
fn entry_view(
    v: &mut View,
    agents: &Agents,
    pane: u64,
    entry: &AgentMessage,
    streaming: bool,
) -> AnyElement {
    match (entry.role, &entry.tool) {
        (AgentRole::User, _) => row()
            .w_full()
            .justify_end()
            .child(
                div()
                    .max_w(px(BUBBLE_WIDTH))
                    .px(px(16.))
                    .py(px(9.))
                    .rounded(px(14.))
                    .bg(hsla(v.p.selected))
                    .child(v.text(entry.text.clone(), SIZE, Tone::Text)),
            )
            .into_any_element(),
        (AgentRole::Assistant, _) => {
            let mut block = column().w_full().gap(px(8.));
            block = block.child(match agents.markdown(pane, entry.id) {
                Some(content) => document::view(v, &format!("answer-{pane}-{}", entry.id), content)
                    .into_any_element(),
                None => v
                    .text(entry.text.clone(), SIZE, Tone::Text)
                    .into_any_element(),
            });
            // Actions appear once the answer is complete.
            if !streaming {
                block = block.child(div().child(v.action(
                    ("agent-copy", entry.id as usize),
                    Icon::Copy,
                    "Copy answer",
                    tell(AgentMessageEvent::Copy(entry.text.clone())),
                )));
            }
            block.into_any_element()
        }
        (AgentRole::Thinking, _) => thinking_view(v, agents, pane, entry, streaming),
        (AgentRole::Tool, Some(tool)) => tool_view(v, agents, pane, entry, tool),
        (AgentRole::Tool | AgentRole::Notice, _) => v
            .text(entry.text.clone(), 13.0, Tone::Muted)
            .into_any_element(),
    }
}

/// A header that folds or unfolds its entry.
fn fold_header(v: &View, pane: u64, entry: u64, unfolded: bool, content: impl IntoElement) -> Div {
    div().child(
        v.button(
            SharedString::from(format!("fold-{pane}-{entry}")),
            row()
                .gap(px(8.))
                .child(v.icon(
                    if unfolded {
                        Icon::Chevron
                    } else {
                        Icon::ChevronRight
                    },
                    Tone::Muted,
                ))
                .child(content),
            Look::Quiet,
            Some(tell(AgentMessageEvent::Toggle(pane, entry))),
        )
        .px(px(6.))
        .py(px(3.)),
    )
}

/// The agent's reasoning: a muted line that unfolds to the text.
fn thinking_view(
    v: &View,
    agents: &Agents,
    pane: u64,
    entry: &AgentMessage,
    streaming: bool,
) -> AnyElement {
    let unfolded = agents.is_unfolded(pane, entry.id);
    let label = if streaming { "Thinking…" } else { "Thought" };
    let mut block = column().w_full().gap(px(4.)).child(fold_header(
        v,
        pane,
        entry.id,
        unfolded,
        v.text(label, 12.0, Tone::Muted),
    ));
    if unfolded {
        block = block.child(
            div()
                .px(px(22.))
                .child(v.text(entry.text.clone(), 13.0, Tone::Muted)),
        );
    }
    block.into_any_element()
}

/// A tool call as a card: what it is, how far it got, and its output when unfolded.
fn tool_view(
    v: &View,
    agents: &Agents,
    pane: u64,
    entry: &AgentMessage,
    tool: &AgentTool,
) -> AnyElement {
    let unfolded = agents.is_unfolded(pane, entry.id);
    let color = match tool.state {
        ToolState::Running => v.p.accent,
        ToolState::Done => v.p.success,
        ToolState::Failed => v.p.danger,
    };
    let mut title =
        row()
            .gap(px(8.))
            .child(v.dot(color))
            .child(v.text(tool.name.clone(), 12.0, Tone::Text));
    if !entry.text.is_empty() {
        title = title.child(v.text(entry.text.clone(), 12.0, Tone::Muted));
    }
    if !tool.detail.is_empty() {
        title = title.child(
            v.text(tool.detail.clone(), 11.0, Tone::Muted)
                .font_family(DEFAULT_FAMILY),
        );
    }
    let mut card = column()
        .w_full()
        .gap(px(4.))
        .child(fold_header(v, pane, entry.id, unfolded, title));
    if unfolded {
        card = card.child(
            div()
                .w_full()
                .p(px(10.))
                .rounded(px(8.))
                .bg(hsla(v.p.hover))
                .child(output(v, tool)),
        );
    }
    div()
        .w_full()
        .p(px(4.))
        .rounded(px(10.))
        .border_1()
        .border_color(hsla(v.p.border))
        .child(card)
        .into_any_element()
}

/// What a tool printed, in the terminal's font, or what an edit changed; added and
/// removed lines are coloured.
fn output(v: &View, tool: &AgentTool) -> AnyElement {
    let (shown, diff) = if tool.diff.is_empty() {
        (&tool.output, false)
    } else {
        (&tool.diff, true)
    };
    if shown.is_empty() {
        return v
            .text(
                if tool.state == ToolState::Running {
                    "Running…"
                } else {
                    "No output"
                },
                12.0,
                Tone::Muted,
            )
            .into_any_element();
    }
    let mut lines = column();
    for line in shown.lines() {
        let tone = match line.chars().next() {
            Some('+') if diff => Tone::Success,
            Some('-') if diff => Tone::Danger,
            _ => Tone::Text,
        };
        lines = lines.child(
            v.text(line.to_owned(), 12.0, tone)
                .font_family(DEFAULT_FAMILY)
                .min_h(px(16.)),
        );
    }
    lines.into_any_element()
}

/// How a dialog's choice looks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Chip {
    Plain,
    Strong,
    Danger,
}

/// A bordered, rounded button: the look of a dialog's choices.
fn chip(
    v: &View,
    id: SharedString,
    label: impl Into<SharedString>,
    chip: Chip,
    message: Message,
) -> impl IntoElement {
    let ink = if chip == Chip::Danger {
        Tone::Danger
    } else {
        Tone::Text
    };
    let button = v
        .button(
            id,
            v.text(label, 13.0, ink),
            if chip == Chip::Strong {
                Look::Primary
            } else {
                Look::Quiet
            },
            Some(message),
        )
        .px(px(14.))
        .py(px(6.))
        .rounded(px(8.))
        .border_1()
        .border_color(hsla(if chip == Chip::Strong {
            v.p.background.with_alpha(0.0)
        } else {
            v.p.border
        }));
    if chip == Chip::Strong {
        button.bg(hsla(v.p.background.with_alpha(1.0)))
    } else {
        button
    }
}

/// A question or approval the agent waits on. Only a click here answers it.
fn dialog_view(
    v: &mut View,
    agents: &Agents,
    pane: u64,
    dialog: &AgentDialog,
    index: usize,
) -> AnyElement {
    let answer =
        |what: DialogAnswer| tell(AgentMessageEvent::Answer(pane, dialog.id.clone(), what));
    let key = |name: &str| SharedString::from(format!("dialog-{pane}-{index}-{name}"));
    let mut card =
        column()
            .w_full()
            .gap(px(8.))
            .child(v.text(dialog.title.clone(), 14.0, Tone::Text));
    if !dialog.message.is_empty() {
        card = card.child(v.text(dialog.message.clone(), 12.0, Tone::Muted));
    }
    let mut buttons = row().gap(px(8.));
    match dialog.kind {
        DialogKind::Confirm => {
            buttons = buttons
                .child(chip(
                    v,
                    key("allow"),
                    "Allow",
                    Chip::Strong,
                    answer(DialogAnswer::Confirm(true)),
                ))
                .child(chip(
                    v,
                    key("deny"),
                    "Deny",
                    Chip::Danger,
                    answer(DialogAnswer::Confirm(false)),
                ));
        }
        DialogKind::Select => {
            for (position, option) in dialog.options.iter().enumerate() {
                buttons = buttons.child(chip(
                    v,
                    key(&format!("option-{position}")),
                    option.clone(),
                    Chip::Plain,
                    answer(DialogAnswer::Value(option.clone())),
                ));
            }
        }
        DialogKind::Input => {
            let typed = agents.answer_text(pane, &dialog.id).to_owned();
            let id = dialog.id.clone();
            let spec = v.line_spec(
                &typed,
                "Answer",
                13.0,
                move |text| tell(AgentMessageEvent::DialogText(pane, id.clone(), text)),
                Some(answer(DialogAnswer::Value(typed.clone()))),
            );
            let field = v.boxed_field(ANSWERS[index.min(ANSWERS.len() - 1)], spec, (8.0, 6.0));
            buttons = buttons.child(div().flex_1().child(field)).child(chip(
                v,
                key("submit"),
                "Submit",
                Chip::Strong,
                answer(DialogAnswer::Value(typed)),
            ));
        }
    }
    buttons = buttons.child(chip(
        v,
        key("dismiss"),
        "Dismiss",
        Chip::Plain,
        answer(DialogAnswer::Cancel),
    ));
    div()
        .w_full()
        .p(px(14.))
        .rounded(px(14.))
        .border_1()
        .border_color(hsla(v.p.accent))
        .bg(hsla(v.p.selected))
        .child(card.child(buttons))
        .into_any_element()
}

/// The key a composer hands to the application before it edits: the commands list's
/// navigation, the zoom chord and sending.
fn composer_key(
    pane: u64,
    can_zoom: bool,
    count: usize,
    chosen: Option<String>,
    written: bool,
) -> impl Fn(&Press) -> Option<Message> + 'static {
    move |press: &Press| {
        let write = |name: &String| Some(tell(AgentMessageEvent::WriteCommand(pane, name.clone())));
        let move_to = |down: bool| {
            Some(tell(AgentMessageEvent::MoveCommand {
                down,
                offered: count,
            }))
        };
        let shift = press.modifiers.shift;
        match press.key {
            Key::Named(Named::Enter)
                if can_zoom
                    && shortcuts::primary(press.modifiers)
                    && !press.modifiers.alt
                    && (!cfg!(target_os = "macos") || shift) =>
            {
                Some(Message::TogglePaneZoom(pane))
            }
            // While a command is being typed, the list takes the keys that move through
            // it and the ones that pick one.
            Key::Named(Named::ArrowDown) if count > 0 => move_to(true),
            Key::Named(Named::ArrowUp) if count > 0 => move_to(false),
            Key::Named(Named::Escape) if count > 0 => Some(tell(AgentMessageEvent::HideCommands)),
            Key::Named(Named::Tab) if count > 0 && !shift => chosen.as_ref().and_then(write),
            Key::Named(Named::Enter) if count > 0 && !written && !shift => {
                chosen.as_ref().and_then(write)
            }
            _ => None,
        }
    }
}

/// The multiline composer: Enter sends, Shift+Enter starts a new line.
fn composer(
    v: &mut View,
    agents: &Agents,
    pane: u64,
    agent: &Agent,
    live: bool,
    can_zoom: bool,
) -> AnyElement {
    let Some(content) = agents.composer(pane) else {
        return div().into_any_element();
    };
    // What the program offers after a slash, narrowed to what is typed.
    let offered = super::commands::typing(content)
        .filter(|_| live && !agents.commands_hidden())
        .map(|query| super::commands::matching(super::commands::choices(&agent.commands), query))
        .unwrap_or_default();
    let at = agents.command_at(offered.len());
    let chosen = offered.get(at).map(|choice| choice.name.clone());
    // A command already written whole is sent, not written again.
    let written = chosen.as_ref().is_some_and(|name| {
        super::commands::typing(content).is_some_and(|query| query.trim_end() == name)
    });
    let count = chosen.as_ref().map_or(0, |_| offered.len());
    let spec = Spec {
        value: content.to_owned(),
        placeholder: if live { "Reply" } else { "The agent has ended" },
        size: SIZE,
        rows: (1, 9),
        submit_on_enter: true,
        on_input: Rc::new(move |text| tell(AgentMessageEvent::Draft(pane, text))),
        on_submit: live.then(|| tell(AgentMessageEvent::Send(pane))),
        on_key: live.then(|| {
            Rc::new(composer_key(pane, can_zoom, count, chosen, written))
                as Rc<dyn Fn(&Press) -> Option<Message>>
        }),
        colors: v.p,
    };
    let field = v.field(FIELD, spec);
    let mut controls = row()
        .w_full()
        .items_end()
        .gap(px(8.))
        .child(div().flex_1().min_w_0().px(px(12.)).py(px(6.)).child(field));
    if agent.busy {
        controls = controls.child(
            v.button(
                "agent-stop",
                div().text_size(px(12.)).child("Stop"),
                Look::Destructive,
                Some(tell(AgentMessageEvent::Abort(pane))),
            )
            .h(px(32.))
            .px(px(14.)),
        );
    }
    if live {
        // The arrow is lit only when there is something to send.
        let ready = !content.trim().is_empty() || !agents.attachments(pane).is_empty();
        let mut arrow = div()
            .id("agent-send")
            .size(px(32.))
            .flex_none()
            .rounded(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(if ready { v.p.text } else { v.p.hover }))
            .child(icon(
                Icon::Send,
                if ready { Color::BLACK } else { v.p.muted },
                14.0,
            ));
        if ready {
            arrow = arrow
                .cursor_pointer()
                .on_click(v.on_click(tell(AgentMessageEvent::Send(pane))));
        }
        controls = controls.child(arrow);
    }
    // Lighter than the page, as a raised field, with no outline.
    let raised = div()
        .w_full()
        .px(px(10.))
        .py(px(6.))
        .rounded(px(22.))
        .bg(hsla(v.p.selected))
        .child(controls);
    let files = agents.attachments(pane);
    let mut stack = column().w_full();
    if !offered.is_empty() {
        stack = stack.child(
            div()
                .w_full()
                .pb(px(6.))
                .child(command_list(v, pane, &offered, at)),
        );
    }
    if !files.is_empty() {
        stack = stack.child(div().w_full().pb(px(6.)).child(attachments(v, pane, files)));
    }
    stack.child(raised).into_any_element()
}

/// What the next message carries: a small picture of each attached image, or its name,
/// each with a button to drop it.
fn attachments(v: &View, pane: u64, files: &[Attachment]) -> AnyElement {
    let mut chips = row().flex_wrap().gap(px(8.));
    for (index, file) in files.iter().enumerate() {
        let shown = match &file.preview {
            Some(preview) => {
                let image = gpui::Image::from_bytes(gpui::ImageFormat::Png, preview.png.to_vec());
                column()
                    .items_center()
                    .gap(px(4.))
                    .child(img(std::sync::Arc::new(image)).h(px(74.)))
                    .child(v.text(file.name.clone(), 11.0, Tone::Muted))
                    .into_any_element()
            }
            None => v
                .text(file.name.clone(), 12.0, Tone::Text)
                .into_any_element(),
        };
        let drop = v
            .button(
                ("agent-unattach", index),
                v.text("×", 13.0, Tone::Muted),
                Look::Quiet,
                Some(tell(AgentMessageEvent::Unattach(pane, file.named.clone()))),
            )
            .px(px(4.));
        chips = chips.child(
            row()
                .items_start()
                .gap(px(6.))
                .px(px(8.))
                .py(px(6.))
                .rounded(px(10.))
                .border_1()
                .border_color(hsla(v.p.border))
                .bg(hsla(v.p.selected))
                .child(shown)
                .child(drop),
        );
    }
    chips.into_any_element()
}

/// The commands offered after a slash: the name, what it takes, and what it does. Their
/// text comes from the agent's program; picking one only writes it in the composer.
fn command_list(v: &View, pane: u64, offered: &[Choice<'_>], at: usize) -> AnyElement {
    let mut items = column().gap(px(2.));
    for (index, choice) in offered.iter().take(MOST_COMMANDS).enumerate() {
        let mut line = row().w_full().gap(px(8.)).overflow_hidden().child(
            v.text(format!("/{}", choice.name), 12.5, Tone::Text)
                .font_family(DEFAULT_FAMILY)
                .whitespace_nowrap(),
        );
        if !choice.hint.is_empty() {
            line = line.child(
                v.text(choice.hint.to_owned(), 11.5, Tone::Accent)
                    .font_family(DEFAULT_FAMILY)
                    .whitespace_nowrap(),
            );
        }
        // One line each: a long description is cut, not wrapped into a tall row.
        line = line.child(v.line(choice.description.to_owned(), 12.0, Tone::Muted));
        items = items.child(choice_button(
            v,
            SharedString::from(format!("command-{index}")),
            line,
            index == at,
            tell(AgentMessageEvent::WriteCommand(pane, choice.name.clone())),
        ));
    }
    popup(v)
        .id("agent-commands")
        .w_full()
        .max_h(px(220.))
        .overflow_y_scroll()
        .p(px(6.))
        .child(items)
        .into_any_element()
}

/// A choice in a list: lit while chosen or under the pointer.
fn choice_button(
    v: &View,
    id: SharedString,
    content: impl IntoElement,
    chosen: bool,
    message: Message,
) -> impl IntoElement {
    let lit = crate::gui::hsla(POPUP_ITEM);
    let mut button = v
        .button(id, content, Look::Quiet, Some(message))
        .w_full()
        .px(px(8.))
        .py(px(4.))
        .rounded(px(8.))
        .hover(move |style| style.bg(lit))
        .text_color(hsla(if chosen { v.p.text } else { v.p.muted }));
    if chosen {
        button = button.bg(lit);
    }
    button
}

/// The frame of a list of choices.
fn popup(v: &View) -> Div {
    let _ = v;
    div()
        .rounded(px(12.))
        .border_1()
        .border_color(hsla(POPUP_EDGE))
        .bg(hsla(POPUP))
        .shadow_lg()
}

/// A button that opens or closes the list of choices of `kind`.
fn chooser(v: &View, pane: u64, kind: Menu, open: bool, selected: &str) -> impl IntoElement {
    v.button(
        SharedString::from(format!("chooser-{pane}-{kind:?}")),
        row()
            .gap(px(6.))
            .child(v.text(
                selected.to_owned(),
                12.0,
                if open { Tone::Text } else { Tone::Muted },
            ))
            .child(v.icon(
                if open { Icon::ChevronUp } else { Icon::Chevron },
                Tone::Muted,
            )),
        if open { Look::Selected } else { Look::Quiet },
        Some(tell(AgentMessageEvent::Menu(pane, kind))),
    )
    .px(px(8.))
    .py(px(3.))
}

/// The thinking levels: a short list.
fn thinking_panel(v: &View, pane: u64, agent: &Agent) -> Div {
    let mut items = column();
    for (index, level) in agent.levels.iter().enumerate() {
        let chosen = agent.thinking.as_deref() == Some(level.as_str());
        items = items.child(
            choice_button(
                v,
                SharedString::from(format!("level-{index}")),
                v.text(level.clone(), 12.0, Tone::Text),
                chosen,
                tell(AgentMessageEvent::SetThinking(pane, level.clone())),
            )
            .into_any_element(),
        );
    }
    popup(v).w(px(110.)).p(px(4.)).child(items)
}

/// A small tag with a role's name.
fn role_chip(v: &View, name: &str) -> Div {
    div()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(6.))
        .border_1()
        .border_color(hsla(POPUP_EDGE))
        .child(v.text(name.to_owned(), 10.0, Tone::Accent))
}

/// The roles the program gives its models: the role, its model and level. A role whose
/// model the agent does not offer cannot be used and is dimmed.
fn role_list(v: &View, pane: u64, agent: &Agent) -> Div {
    let mut list = column().gap(px(2.));
    for (index, role) in agent.roles.iter().enumerate() {
        let offered = agent.models.is_empty() || agent.models.contains(&role.model);
        let current = agent.model.as_deref() == Some(role.model.as_str());
        let mut line = row()
            .w_full()
            .gap(px(8.))
            .child(div().w(px(76.)).child(role_chip(v, &role.role)))
            .child(div().flex_1().child(v.text(
                role.model.clone(),
                12.0,
                if offered { Tone::Text } else { Tone::Muted },
            )));
        if let Some(level) = &role.thinking {
            line = line.child(v.text(level.clone(), 11.0, Tone::Muted));
        }
        let id = SharedString::from(format!("role-{index}"));
        list = list.child(if offered {
            choice_button(
                v,
                id,
                line,
                current,
                tell(AgentMessageEvent::SetRole(
                    pane,
                    role.model.clone(),
                    role.thinking.clone(),
                )),
            )
            .into_any_element()
        } else {
            div()
                .w_full()
                .px(px(8.))
                .py(px(4.))
                .child(line)
                .into_any_element()
        });
    }
    list
}

/// A list that scrolls with the wheel and draws no bar.
fn quiet_scroll(id: &'static str, content: impl IntoElement) -> Div {
    div().child(div().id(id).size_full().overflow_y_scroll().child(content))
}

/// The models: a search box on top, the providers down the left, and the models of the
/// chosen provider (or all) on the right, each provider under its own heading.
fn model_panel(v: &mut View, agents: &Agents, pane: u64, agent: &Agent) -> Div {
    let spec = v.line_spec(
        agents.model_search(),
        "Search models",
        12.0,
        |text| tell(AgentMessageEvent::MenuSearch(text)),
        None,
    );
    let search = v.boxed_field(SEARCH, spec, (10.0, 7.0));

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
    let entry = |label: &str, count: usize, name: Option<&str>, id: SharedString| {
        let lit = !agents.shows_roles() && chosen == name;
        choice_button(
            v,
            id,
            row()
                .w_full()
                .gap(px(6.))
                .child(
                    div()
                        .flex_1()
                        .child(v.text(label.to_owned(), 12.0, Tone::Text)),
                )
                .child(v.text(count.to_string(), 11.0, Tone::Muted)),
            lit,
            tell(AgentMessageEvent::MenuProvider(name.map(str::to_owned))),
        )
    };
    let mut sidebar = column().gap(px(2.));
    if !agent.roles.is_empty() {
        sidebar = sidebar.child(choice_button(
            v,
            SharedString::from("provider-roles"),
            row()
                .w_full()
                .gap(px(6.))
                .child(div().flex_1().child(v.text("Roles", 12.0, Tone::Text)))
                .child(v.text(agent.roles.len().to_string(), 11.0, Tone::Muted)),
            agents.shows_roles(),
            tell(AgentMessageEvent::MenuRoles),
        ));
    }
    sidebar = sidebar.child(entry(
        "All",
        matching.len(),
        None,
        SharedString::from("provider-all"),
    ));
    for (index, (name, count)) in counts.iter().enumerate() {
        sidebar = sidebar.child(entry(
            name,
            *count,
            Some(name),
            SharedString::from(format!("provider-{index}")),
        ));
    }

    let shown = if agents.shows_roles() {
        Vec::new()
    } else {
        agents.shown(&agent.models)
    };
    let mut ordered = shown.clone();
    ordered.sort_by_key(|model| provider(model));
    let mut list = if agents.shows_roles() {
        role_list(v, pane, agent)
    } else {
        column().gap(px(2.))
    };
    let mut current = None;
    for (index, model) in ordered.into_iter().enumerate() {
        if chosen.is_none() && current != Some(provider(model)) {
            current = Some(provider(model));
            list = list.child(div().px(px(10.)).py(px(8.)).child(v.text(
                provider(model).to_owned(),
                11.0,
                Tone::Accent,
            )));
        }
        let lit = agent.model.as_deref() == Some(model.as_str());
        let mut label = row()
            .w_full()
            .gap(px(6.))
            .child(
                div()
                    .flex_1()
                    .child(v.text(model_id(model).to_owned(), 12.0, Tone::Text)),
            );
        // The roles this model plays.
        for role in agent.roles.iter().filter(|r| &r.model == model) {
            label = label.child(role_chip(v, &role.role));
        }
        list = list.child(choice_button(
            v,
            SharedString::from(format!("model-{index}")),
            label,
            lit,
            tell(AgentMessageEvent::SetModel(pane, model.clone())),
        ));
    }
    if shown.is_empty() && !agents.shows_roles() {
        list = list.child(
            div()
                .p(px(10.))
                .child(v.text("No model matches", 12.0, Tone::Muted)),
        );
    }

    let body = row()
        .w_full()
        .flex_1()
        .min_h_0()
        .items_start()
        .gap(px(6.))
        .child(
            quiet_scroll("model-providers", sidebar)
                .w(px(150.))
                .h_full(),
        )
        .child(div().w(px(1.)).h_full().bg(hsla(POPUP_EDGE)))
        .child(quiet_scroll("model-list", list).flex_1().h_full());
    popup(v)
        .w(px(540.))
        .h(px(300.))
        .p(px(4.))
        .flex()
        .flex_col()
        .child(div().w_full().pb(px(6.)).child(search))
        .child(body)
}

/// The open list of choices, drawn over the pane just above its button. It is a layer of
/// its own, so opening it moves nothing.
fn menu_layer(v: &mut View, agents: &Agents, pane: u64, kind: Menu, agent: &Agent) -> AnyElement {
    if kind == Menu::Usage {
        // Usage is a dialog: centred over the pane on a dimmed backdrop. A press on the
        // panel itself is its own; one on the backdrop closes it.
        return div()
            .id("agent-usage-layer")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(Color::BLACK.with_alpha(0.5)))
            .on_mouse_down(
                gpui::MouseButton::Left,
                v.on_mouse_down(|_| tell(AgentMessageEvent::CloseMenu)),
            )
            .child(
                usage_panel(v, pane, agent)
                    .id("agent-usage-panel")
                    .occlude()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation()),
            )
            .into_any_element();
    }
    let (panel, indent) = match kind {
        Menu::Thinking => (thinking_panel(v, pane, agent), 0.0),
        _ => (
            model_panel(v, agents, pane, agent),
            THINKING_WIDTH + FOOTER_GAP,
        ),
    };
    let panel = panel
        .id("agent-menu-panel")
        .occlude()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation());
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .flex_col()
        .justify_end()
        .child(centred(
            div()
                .w_full()
                .px(px(18.))
                .pl(px(18.0 + FOOTER_INSET + indent))
                .flex()
                .child(panel),
        ))
        .child(div().h(px(FOOTER_CLEARANCE)))
        .into_any_element()
}

/// A bar of how much of an allowance is left: lit in proportion, red when nearly gone.
fn allowance_bar(v: &View, left: u8) -> Div {
    let color = if left <= 10 { v.p.danger } else { v.p.accent };
    let left = f32::from(left.min(100)) / 100.0;
    div()
        .w_full()
        .h(px(6.))
        .rounded(px(3.))
        .bg(hsla(POPUP_EDGE))
        .child(
            div()
                .h_full()
                .w(gpui::relative(left.max(0.01)))
                .rounded(px(3.))
                .bg(hsla(color)),
        )
}

/// What the providers report about the remaining usage: per provider, how much of each
/// allowance each account has left and when it renews.
fn usage_panel(v: &View, pane: u64, agent: &Agent) -> gpui::Stateful<Div> {
    let refresh = v
        .button(
            "usage-refresh",
            v.text("Refresh", 12.0, Tone::Text),
            Look::Quiet,
            Some(tell(AgentMessageEvent::RefreshUsage(pane))),
        )
        .px(px(12.))
        .py(px(4.))
        .rounded(px(8.))
        .border_1()
        .border_color(hsla(v.p.border));
    let mut body = column().w_full().gap(px(12.)).child(
        row()
            .w_full()
            .justify_between()
            .child(v.text("Usage", 14.0, Tone::Text))
            .child(refresh),
    );
    if let Some(context) = agent.context {
        body = body.child(v.text(
            format!("Context window: {context}% used"),
            12.0,
            Tone::Muted,
        ));
    }
    match &agent.usage {
        None => body = body.child(v.text("Reading usage…", 12.0, Tone::Muted)),
        Some(providers) if providers.is_empty() => {
            body = body.child(v.text("No usage data", 12.0, Tone::Muted));
        }
        Some(providers) => {
            for provider in providers {
                let mut block = column().w_full().gap(px(6.)).child(v.text(
                    provider.name.clone(),
                    13.0,
                    Tone::Text,
                ));
                for note in &provider.notes {
                    block = block.child(v.text(note.clone(), 11.0, Tone::Success));
                }
                // The account is named only when a provider has more than one.
                let several = provider
                    .windows
                    .iter()
                    .any(|w| w.account != provider.windows[0].account);
                for window in &provider.windows {
                    let mut name =
                        column()
                            .flex_1()
                            .child(v.text(window.name.clone(), 12.0, Tone::Text));
                    if several {
                        name = name.child(v.text(window.account.clone(), 10.0, Tone::Muted));
                    }
                    block = block.child(
                        row()
                            .w_full()
                            .gap(px(10.))
                            .child(name)
                            .child(div().flex_1().child(allowance_bar(v, window.left)))
                            .child(div().w(px(70.)).flex().justify_end().child(v.text(
                                format!("{}% left", window.left),
                                12.0,
                                if window.left <= 10 {
                                    Tone::Danger
                                } else {
                                    Tone::Text
                                },
                            )))
                            .child(div().w(px(96.)).flex().justify_end().child(v.text(
                                if window.resets.is_empty() {
                                    String::new()
                                } else {
                                    format!("resets in {}", window.resets)
                                },
                                11.0,
                                Tone::Muted,
                            ))),
                    );
                }
                body = body.child(block);
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
    popup(v)
        .w(px(560.))
        .h(px(fitted))
        .child(quiet_scroll("usage-scroll", div().p(px(10.)).child(body)).size_full())
        .id("usage-popup")
}

/// How much of the context window is used: a short bar and the figure.
fn context_meter(v: &View, percent: u8) -> impl IntoElement {
    let color = if percent >= 90 {
        v.p.danger
    } else {
        v.p.accent
    };
    let filled = 36.0 * f32::from(percent.min(100)) / 100.0;
    row()
        .gap(px(6.))
        .child(
            div()
                .w(px(36.))
                .h(px(4.))
                .rounded(px(2.))
                .bg(hsla(v.p.border))
                .child(
                    div()
                        .w(px(filled))
                        .h(px(4.))
                        .rounded(px(2.))
                        .bg(hsla(color)),
                ),
        )
        .child(v.text(format!("{percent}%"), 12.0, Tone::Muted))
}

/// Under the composer: the model and thinking level, the context in use, the state.
fn footer(v: &View, agents: &Agents, pane: u64, agent: &Agent) -> AnyElement {
    let open = agents.menu(pane);
    let level_line = |item: Div| div().h(px(26.)).flex().items_center().child(item);
    let mut line = row()
        .w_full()
        .gap(px(FOOTER_GAP))
        .pl(px(FOOTER_INSET))
        .pr(px(10.));
    if let (Some(level), false) = (&agent.thinking, agent.levels.is_empty()) {
        line = line.child(div().w(px(THINKING_WIDTH)).child(chooser(
            v,
            pane,
            Menu::Thinking,
            open == Some(Menu::Thinking),
            level,
        )));
    }
    if let Some(model) = &agent.model {
        line = line.child(if agent.models.is_empty() {
            level_line(v.text(model.clone(), 12.0, Tone::Muted)).into_any_element()
        } else {
            chooser(v, pane, Menu::Model, open == Some(Menu::Model), model).into_any_element()
        });
    }
    line = line.child(div().flex_1());
    // The context meter opens the usage the providers report.
    let usage_open = open == Some(Menu::Usage);
    line = line.child(
        v.button(
            "agent-usage",
            match agent.context {
                Some(context) => context_meter(v, context).into_any_element(),
                None => v.text("Usage", 12.0, Tone::Muted).into_any_element(),
            },
            if usage_open {
                Look::Selected
            } else {
                Look::Quiet
            },
            Some(tell(AgentMessageEvent::Menu(pane, Menu::Usage))),
        )
        .px(px(8.))
        .py(px(3.)),
    );
    line = line.child(level_line(v.text(agent.backend.clone(), 12.0, Tone::Text)));
    line = line.child(level_line(
        match (&agent.ended, agent.busy, agent.dialogs.is_empty()) {
            (Some(reason), _, _) => v.text(reason.clone(), 12.0, Tone::Danger),
            (None, _, false) => v.text("Waiting", 12.0, Tone::Accent),
            (None, true, true) => v.text("Working…", 12.0, Tone::Accent),
            (None, false, true) => v.text("Ready", 12.0, Tone::Muted),
        },
    ));
    line.into_any_element()
}
