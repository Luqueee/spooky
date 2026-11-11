//! The conversation of one agent, built from [`Event`]s and bounded in size.

use super::Event;
use spooky_core::{
    Agent as AgentSnapshot, AgentDialog, AgentMessage, AgentRole, AgentTool, DialogAnswer,
    DialogKind, ModelRole, ToolState, UsageProvider,
};
use std::collections::HashMap;

/// Entries kept, oldest dropped first.
pub(super) const MAX_MESSAGES: usize = 200;
/// Text kept across all entries.
pub(super) const MAX_TOTAL_BYTES: usize = 128 * 1024;
/// Text kept in one entry.
pub(super) const MAX_ENTRY_BYTES: usize = 32 * 1024;
/// Output kept for one tool call.
pub(super) const MAX_TOOL_OUTPUT_BYTES: usize = 8 * 1024;
/// Diff kept for one tool call.
pub(super) const MAX_TOOL_DIFF_BYTES: usize = 16 * 1024;
/// A tool's purpose and main argument.
const MAX_TOOL_LABEL_BYTES: usize = 512;
/// Marks text cut at its bound.
pub(super) const TRUNCATED: &str = "\n[truncated]";
/// Dialogs held at once; a further one replaces nothing and is dropped.
const MAX_DIALOGS: usize = 8;
/// Options of one dialog, and the text of each.
const MAX_OPTIONS: usize = 64;
const MAX_OPTION_BYTES: usize = 256;
const MAX_DIALOG_TEXT_BYTES: usize = 2048;
/// Models offered, and the length of each name.
const MAX_MODELS: usize = 400;
/// Most model roles kept, and the longest name or model of one.
const MAX_ROLES: usize = 32;
const MAX_ROLE_BYTES: usize = 256;
/// Most providers, allowances and notes of a usage report, and the longest text in one.
const MAX_USAGE_PROVIDERS: usize = 16;
const MAX_USAGE_ITEMS: usize = 24;
const MAX_USAGE_TEXT: usize = 200;
const MAX_MODEL_BYTES: usize = 128;
/// Tool calls remembered so their results find them.
const MAX_REMEMBERED_CALLS: usize = 512;

#[derive(Default)]
pub(crate) struct Transcript {
    busy: bool,
    ended: Option<String>,
    dropped: u32,
    messages: Vec<AgentMessage>,
    /// The role of the last entry while it is still being streamed.
    streaming: Option<AgentRole>,
    next_id: u64,
    /// Tool call identity to the entry that shows it.
    calls: HashMap<String, u64>,
    dialogs: Vec<AgentDialog>,
    model: Option<String>,
    thinking: Option<String>,
    models: Vec<String>,
    levels: Vec<String>,
    context: Option<u8>,
    roles: Vec<ModelRole>,
    usage: Option<Vec<UsageProvider>>,
}

impl Transcript {
    /// Applies one event. Returns whether anything a client shows changed.
    pub(crate) fn apply(&mut self, event: Event) -> bool {
        match event {
            Event::TurnStarted => {
                self.busy = true;
                true
            }
            Event::TurnEnded => {
                self.busy = false;
                self.streaming = None;
                true
            }
            Event::BlockStarted(role) => {
                self.push(role, String::new());
                self.streaming = Some(role);
                true
            }
            Event::BlockDelta(delta) => {
                if self.streaming.is_none() {
                    // Joined mid-block: start the entry the missing start would have.
                    self.push(AgentRole::Assistant, String::new());
                    self.streaming = Some(AgentRole::Assistant);
                }
                if let Some(last) = self.messages.last_mut() {
                    append_bounded(&mut last.text, &delta);
                }
                self.trim();
                true
            }
            Event::BlockDone { role, text } => {
                let streaming = self.streaming.take() == Some(role)
                    && self.messages.last().is_some_and(|m| m.role == role);
                if streaming {
                    if text.is_empty() {
                        if self.messages.last().is_some_and(|m| m.text.is_empty()) {
                            self.messages.pop();
                        }
                    } else if let Some(last) = self.messages.last_mut() {
                        last.text = bounded(text, MAX_ENTRY_BYTES);
                    }
                    true
                } else if text.is_empty() {
                    false
                } else {
                    self.push(role, text);
                    true
                }
            }
            Event::User(text) => {
                self.push(AgentRole::User, text);
                true
            }
            Event::ToolStarted {
                call,
                name,
                intent,
                detail,
            } => {
                if self.calls.contains_key(&call) {
                    return false;
                }
                let id = self.push(AgentRole::Tool, bounded(intent, MAX_TOOL_LABEL_BYTES));
                if let Some(last) = self.messages.last_mut() {
                    last.tool = Some(AgentTool {
                        name: bounded(name, MAX_TOOL_LABEL_BYTES),
                        detail: bounded(detail, MAX_TOOL_LABEL_BYTES),
                        state: ToolState::Running,
                        output: String::new(),
                        diff: String::new(),
                    });
                }
                if self.calls.len() >= MAX_REMEMBERED_CALLS {
                    self.calls.clear();
                }
                self.calls.insert(call, id);
                true
            }
            Event::ToolFinished {
                call,
                output,
                diff,
                failed,
            } => {
                let Some(id) = self.calls.get(&call).copied() else {
                    return false;
                };
                let Some(tool) = self
                    .messages
                    .iter_mut()
                    .find(|m| m.id == id)
                    .and_then(|m| m.tool.as_mut())
                else {
                    return false;
                };
                tool.state = if failed {
                    ToolState::Failed
                } else {
                    ToolState::Done
                };
                // An edit's own output is the whole file again; its diff says what it did.
                tool.output = if diff.is_empty() {
                    bounded(output, MAX_TOOL_OUTPUT_BYTES)
                } else {
                    String::new()
                };
                tool.diff = bounded(diff, MAX_TOOL_DIFF_BYTES);
                true
            }
            Event::Failure(error) => {
                let text = format!("error: {}", bounded(error, MAX_ENTRY_BYTES));
                self.push(AgentRole::Notice, text);
                true
            }
            Event::Dialog(dialog) => self.open_dialog(dialog),
            Event::DialogClosed(id) => self.close_dialog(&id),
            Event::Model(model) => replace(&mut self.model, Some(bounded(model, MAX_MODEL_BYTES))),
            Event::Thinking(level) => {
                replace(&mut self.thinking, Some(bounded(level, MAX_MODEL_BYTES)))
            }
            Event::Models(models) => {
                let models: Vec<String> = models
                    .into_iter()
                    .take(MAX_MODELS)
                    .map(|m| bounded(m, MAX_MODEL_BYTES))
                    .collect();
                replace_value(&mut self.models, models)
            }
            Event::Levels(levels) => {
                let levels = levels
                    .into_iter()
                    .take(MAX_OPTIONS)
                    .map(|l| bounded(l, MAX_MODEL_BYTES))
                    .collect();
                replace_value(&mut self.levels, levels)
            }
            Event::Context(percent) => replace(&mut self.context, Some(percent)),
            Event::Usage(providers) => {
                self.usage = Some(bounded_usage(providers));
                true
            }
        }
    }

    /// Records that the agent process is gone.
    pub(crate) fn end(&mut self, reason: String) {
        self.busy = false;
        self.streaming = None;
        self.dialogs.clear();
        self.ended = Some(reason);
    }

    /// Whether a turn is running.
    pub(crate) fn is_busy(&self) -> bool {
        self.busy
    }

    /// Whether the agent process is gone.
    pub(crate) fn has_ended(&self) -> bool {
        self.ended.is_some()
    }

    /// Whether `model` is one the agent listed.
    pub(crate) fn offers_model(&self, model: &str) -> bool {
        self.models.iter().any(|m| m == model)
    }

    /// Whether `level` is one the agent listed.
    pub(crate) fn offers_level(&self, level: &str) -> bool {
        self.levels.iter().any(|l| l == level)
    }

    /// Keeps the roles the program gives its models, dropping what is oversize or empty.
    pub(crate) fn set_roles(&mut self, roles: Vec<ModelRole>) {
        self.roles = roles
            .into_iter()
            .filter(|r| {
                !r.role.is_empty()
                    && !r.model.is_empty()
                    && r.role.len() <= MAX_ROLE_BYTES
                    && r.model.len() <= MAX_ROLE_BYTES
                    && !r.role.chars().chain(r.model.chars()).any(char::is_control)
            })
            .take(MAX_ROLES)
            .collect();
    }

    /// Checks an answer against the dialog it is for, and closes that dialog.
    pub(crate) fn take_dialog(&mut self, id: &str, answer: &DialogAnswer) -> Result<(), String> {
        let dialog = self
            .dialogs
            .iter()
            .find(|d| d.id == id)
            .ok_or("that question is no longer open")?;
        match (dialog.kind, answer) {
            (_, DialogAnswer::Cancel)
            | (DialogKind::Confirm, DialogAnswer::Confirm(_))
            | (DialogKind::Input, DialogAnswer::Value(_)) => {}
            (DialogKind::Select, DialogAnswer::Value(value)) => {
                if !dialog.options.contains(value) {
                    return Err("that is not one of the options".into());
                }
            }
            _ => return Err("that answer does not fit the question".into()),
        }
        self.close_dialog(id);
        Ok(())
    }

    pub(super) fn snapshot(&self, backend: &str) -> AgentSnapshot {
        AgentSnapshot {
            backend: backend.to_owned(),
            busy: self.busy,
            ended: self.ended.clone(),
            dropped: self.dropped,
            messages: self.messages.clone(),
            dialogs: self.dialogs.clone(),
            model: self.model.clone(),
            thinking: self.thinking.clone(),
            models: self.models.clone(),
            levels: self.levels.clone(),
            context: self.context,
            roles: self.roles.clone(),
            usage: self.usage.clone(),
        }
    }

    /// Adds an entry and returns its identity.
    pub(super) fn push(&mut self, role: AgentRole, text: String) -> u64 {
        self.streaming = None;
        self.next_id += 1;
        let id = self.next_id;
        self.messages.push(AgentMessage {
            id,
            role,
            text: bounded(text, MAX_ENTRY_BYTES),
            tool: None,
        });
        self.trim();
        id
    }

    fn open_dialog(&mut self, mut dialog: AgentDialog) -> bool {
        dialog.title = bounded(dialog.title, MAX_DIALOG_TEXT_BYTES);
        dialog.message = bounded(dialog.message, MAX_DIALOG_TEXT_BYTES);
        dialog.options = dialog
            .options
            .into_iter()
            .take(MAX_OPTIONS)
            .map(|o| bounded(o, MAX_OPTION_BYTES))
            .collect();
        if let Some(open) = self.dialogs.iter_mut().find(|d| d.id == dialog.id) {
            *open = dialog;
            return true;
        }
        if self.dialogs.len() >= MAX_DIALOGS {
            return false;
        }
        self.dialogs.push(dialog);
        true
    }

    fn close_dialog(&mut self, id: &str) -> bool {
        let before = self.dialogs.len();
        self.dialogs.retain(|d| d.id != id);
        self.dialogs.len() != before
    }

    /// Drops the oldest entries until the conversation fits its bounds.
    fn trim(&mut self) {
        let size = |m: &AgentMessage| {
            m.text.len()
                + m.tool
                    .as_ref()
                    .map_or(0, |t| t.output.len() + t.detail.len())
        };
        let mut total: usize = self.messages.iter().map(size).sum();
        while self.messages.len() > 1
            && (self.messages.len() > MAX_MESSAGES || total > MAX_TOTAL_BYTES)
        {
            total -= size(&self.messages.remove(0));
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}

/// Sets `slot`, returning whether it changed.
fn replace<T: PartialEq>(slot: &mut Option<T>, value: Option<T>) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

fn replace_value<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

/// `text` cut to `limit` bytes on a character boundary, marked as cut.
pub(super) fn bounded(mut text: String, limit: usize) -> String {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str(TRUNCATED);
    }
    text
}

/// Appends to an entry while it fits; past the bound the entry is marked and left.
fn append_bounded(text: &mut String, more: &str) {
    if text.ends_with(TRUNCATED) {
        return;
    }
    text.push_str(more);
    if text.len() > MAX_ENTRY_BYTES {
        *text = bounded(std::mem::take(text), MAX_ENTRY_BYTES);
    }
}

/// Keeps a usage report within bounds: few entries, short text without control characters.
fn bounded_usage(mut providers: Vec<UsageProvider>) -> Vec<UsageProvider> {
    let clean = |text: &mut String| {
        text.retain(|c| !c.is_control());
        if text.len() > MAX_USAGE_TEXT {
            let mut end = MAX_USAGE_TEXT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
    };
    providers.truncate(MAX_USAGE_PROVIDERS);
    for provider in &mut providers {
        clean(&mut provider.name);
        provider.notes.truncate(MAX_USAGE_ITEMS);
        provider.notes.iter_mut().for_each(clean);
        provider.windows.truncate(MAX_USAGE_ITEMS);
        for window in &mut provider.windows {
            clean(&mut window.name);
            clean(&mut window.account);
            clean(&mut window.resets);
            window.left = window.left.min(100);
        }
    }
    providers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(transcript: &mut Transcript, events: Vec<Event>) {
        for event in events {
            transcript.apply(event);
        }
    }

    fn done(role: AgentRole, text: &str) -> Event {
        Event::BlockDone {
            role,
            text: text.into(),
        }
    }

    fn select(id: &str) -> AgentDialog {
        AgentDialog {
            id: id.into(),
            kind: DialogKind::Select,
            title: "Which?".into(),
            message: String::new(),
            options: vec!["Red".into(), "Blue".into()],
        }
    }

    #[test]
    fn a_turn_becomes_a_user_entry_and_a_streamed_assistant_entry() {
        let mut transcript = Transcript::default();
        apply(
            &mut transcript,
            vec![
                Event::TurnStarted,
                Event::User("hi".into()),
                Event::BlockStarted(AgentRole::Assistant),
                Event::BlockDelta("Hel".into()),
            ],
        );
        let shown = transcript.snapshot("x");
        assert!(shown.busy);
        assert_eq!(shown.messages.len(), 2);
        assert_eq!(shown.messages[0].role, AgentRole::User);
        assert_eq!(shown.messages[1].text, "Hel", "text grows while streaming");
        apply(
            &mut transcript,
            vec![
                Event::BlockDelta("lo".into()),
                done(AgentRole::Assistant, "Hello"),
                Event::TurnEnded,
            ],
        );
        let shown = transcript.snapshot("x");
        assert!(!shown.busy);
        assert_eq!(shown.messages[1].text, "Hello");
        assert_ne!(shown.messages[0].id, shown.messages[1].id);
    }

    #[test]
    fn thinking_and_answer_are_separate_entries_in_order() {
        let mut transcript = Transcript::default();
        apply(
            &mut transcript,
            vec![
                Event::BlockStarted(AgentRole::Thinking),
                Event::BlockDelta("hmm".into()),
                done(AgentRole::Thinking, "hmm, so"),
                Event::BlockStarted(AgentRole::Assistant),
                done(AgentRole::Assistant, "Done."),
            ],
        );
        let shown = transcript.snapshot("x");
        let roles: Vec<_> = shown
            .messages
            .iter()
            .map(|m| (m.role, m.text.as_str()))
            .collect();
        assert_eq!(
            roles,
            [
                (AgentRole::Thinking, "hmm, so"),
                (AgentRole::Assistant, "Done.")
            ]
        );
    }

    #[test]
    fn a_block_that_ends_empty_leaves_no_entry_and_a_whole_block_is_shown_once() {
        let mut transcript = Transcript::default();
        apply(
            &mut transcript,
            vec![
                Event::BlockStarted(AgentRole::Assistant),
                done(AgentRole::Assistant, ""),
            ],
        );
        assert!(transcript.snapshot("x").messages.is_empty());
        assert!(transcript.apply(done(AgentRole::Assistant, "whole")));
        assert!(!transcript.apply(done(AgentRole::Assistant, "")));
        assert_eq!(transcript.snapshot("x").messages.len(), 1);
    }

    #[test]
    fn a_tool_call_is_shown_running_then_with_its_result() {
        let mut transcript = Transcript::default();
        let start = || Event::ToolStarted {
            call: "c1".into(),
            name: "read".into(),
            intent: "Reading hosts".into(),
            detail: "/etc/hosts".into(),
        };
        assert!(transcript.apply(start()));
        assert!(
            !transcript.apply(start()),
            "the same call is not shown twice"
        );
        let tool = transcript.snapshot("x").messages[0]
            .tool
            .clone()
            .expect("tool");
        assert_eq!((tool.state, tool.output.as_str()), (ToolState::Running, ""));
        assert!(transcript.apply(Event::ToolFinished {
            call: "c1".into(),
            output: "127.0.0.1 localhost".into(),
            diff: String::new(),
            failed: false,
        }));
        let shown = transcript.snapshot("x");
        let tool = shown.messages[0].tool.clone().expect("tool");
        assert_eq!(shown.messages[0].text, "Reading hosts");
        assert_eq!(tool.detail, "/etc/hosts");
        assert_eq!(
            (tool.state, tool.output.as_str()),
            (ToolState::Done, "127.0.0.1 localhost")
        );
        assert!(!transcript.apply(Event::ToolFinished {
            call: "unknown".into(),
            output: String::new(),
            diff: String::new(),
            failed: true,
        }));
    }

    #[test]
    fn a_failed_tool_and_a_failed_command_are_marked() {
        let mut transcript = Transcript::default();
        transcript.apply(Event::ToolStarted {
            call: "c".into(),
            name: "bash".into(),
            intent: String::new(),
            detail: "false".into(),
        });
        transcript.apply(Event::ToolFinished {
            call: "c".into(),
            output: "exit 1".into(),
            diff: String::new(),
            failed: true,
        });
        transcript.apply(Event::Failure("no model".into()));
        let shown = transcript.snapshot("x");
        assert_eq!(
            shown.messages[0].tool.as_ref().map(|t| t.state),
            Some(ToolState::Failed)
        );
        assert_eq!(shown.messages[1].role, AgentRole::Notice);
        assert_eq!(shown.messages[1].text, "error: no model");
    }

    #[test]
    fn a_dialog_waits_until_it_is_answered_validly() {
        let mut transcript = Transcript::default();
        assert!(transcript.apply(Event::Dialog(select("d1"))));
        assert_eq!(transcript.snapshot("x").dialogs.len(), 1);
        for wrong in [
            DialogAnswer::Value("Green".into()),
            DialogAnswer::Confirm(true),
        ] {
            assert!(transcript.take_dialog("d1", &wrong).is_err());
        }
        assert!(
            transcript
                .take_dialog("nope", &DialogAnswer::Cancel)
                .is_err()
        );
        assert_eq!(
            transcript.snapshot("x").dialogs.len(),
            1,
            "a refused answer keeps it open"
        );
        assert!(
            transcript
                .take_dialog("d1", &DialogAnswer::Value("Blue".into()))
                .is_ok()
        );
        assert!(transcript.snapshot("x").dialogs.is_empty());
        assert!(
            transcript.take_dialog("d1", &DialogAnswer::Cancel).is_err(),
            "an answered dialog cannot be answered again"
        );
    }

    #[test]
    fn the_agent_can_close_a_dialog_and_its_end_closes_them_all() {
        let mut transcript = Transcript::default();
        transcript.apply(Event::Dialog(select("a")));
        transcript.apply(Event::Dialog(select("b")));
        assert!(transcript.apply(Event::DialogClosed("a".into())));
        assert!(!transcript.apply(Event::DialogClosed("a".into())));
        assert_eq!(transcript.snapshot("x").dialogs[0].id, "b");
        transcript.end("gone".into());
        assert!(transcript.snapshot("x").dialogs.is_empty());
    }

    #[test]
    fn dialogs_are_bounded() {
        let mut transcript = Transcript::default();
        for n in 0..(MAX_DIALOGS + 3) {
            transcript.apply(Event::Dialog(select(&format!("d{n}"))));
        }
        assert_eq!(transcript.snapshot("x").dialogs.len(), MAX_DIALOGS);
        let mut big = select("big");
        big.options = (0..200)
            .map(|n| "o".repeat(1000) + &n.to_string())
            .collect();
        let mut transcript = Transcript::default();
        transcript.apply(Event::Dialog(big));
        let shown = &transcript.snapshot("x").dialogs[0];
        assert_eq!(shown.options.len(), MAX_OPTIONS);
        assert!(
            shown
                .options
                .iter()
                .all(|o| o.len() <= MAX_OPTION_BYTES + TRUNCATED.len())
        );
    }

    #[test]
    fn the_model_and_its_choices_are_kept_and_only_listed_ones_are_offered() {
        let mut transcript = Transcript::default();
        assert!(transcript.apply(Event::Model("p/a".into())));
        assert!(
            !transcript.apply(Event::Model("p/a".into())),
            "no change, no update"
        );
        transcript.apply(Event::Models(vec!["p/a".into(), "p/b".into()]));
        transcript.apply(Event::Levels(vec!["off".into(), "high".into()]));
        transcript.apply(Event::Thinking("high".into()));
        transcript.apply(Event::Context(42));
        assert!(transcript.offers_model("p/b") && !transcript.offers_model("p/c"));
        assert!(transcript.offers_level("off") && !transcript.offers_level("max"));
        let shown = transcript.snapshot("x");
        assert_eq!(
            (
                shown.model.as_deref(),
                shown.thinking.as_deref(),
                shown.context
            ),
            (Some("p/a"), Some("high"), Some(42))
        );
    }

    #[test]
    fn roles_are_bounded_and_untrusted_text_is_dropped() {
        let role = |name: &str, model: &str| ModelRole {
            role: name.into(),
            model: model.into(),
            thinking: None,
        };
        let mut transcript = Transcript::default();
        let mut roles = vec![
            role("default", "p/a"),
            role("", "p/b"),
            role("x\u{1b}[31m", "p/c"),
        ];
        roles.push(role("long", &"m".repeat(MAX_ROLE_BYTES + 1)));
        roles.extend((0..MAX_ROLES + 5).map(|n| role(&format!("r{n}"), "p/a")));
        transcript.set_roles(roles);
        let shown = transcript.snapshot("x").roles;
        assert_eq!(shown.len(), MAX_ROLES);
        assert_eq!(shown[0].role, "default");
        assert!(
            shown
                .iter()
                .all(|r| !r.role.is_empty() && !r.model.is_empty())
        );
    }

    #[test]
    fn a_usage_report_is_bounded_and_clean() {
        let window = |name: &str| spooky_core::UsageWindow {
            name: name.into(),
            account: "a\u{1b}[31m@b".into(),
            left: 250,
            resets: "é".repeat(MAX_USAGE_TEXT),
        };
        let provider = || UsageProvider {
            name: "p".into(),
            notes: vec!["n".into(); MAX_USAGE_ITEMS + 5],
            windows: (0..MAX_USAGE_ITEMS + 5)
                .map(|n| window(&n.to_string()))
                .collect(),
        };
        let mut transcript = Transcript::default();
        assert!(transcript.snapshot("x").usage.is_none());
        transcript.apply(Event::Usage(vec![provider(); MAX_USAGE_PROVIDERS + 3]));
        let shown = transcript.snapshot("x").usage.unwrap_or_default();
        assert_eq!(shown.len(), MAX_USAGE_PROVIDERS);
        assert_eq!(shown[0].windows.len(), MAX_USAGE_ITEMS);
        assert_eq!(shown[0].notes.len(), MAX_USAGE_ITEMS);
        assert_eq!(shown[0].windows[0].left, 100);
        assert_eq!(
            shown[0].windows[0].account, "a[31m@b",
            "control characters are dropped"
        );
        assert!(shown[0].windows[0].resets.len() <= MAX_USAGE_TEXT);
    }

    #[test]
    fn the_conversation_stays_within_its_bounds() {
        let mut transcript = Transcript::default();
        for n in 0..(MAX_MESSAGES + 30) {
            transcript.push(AgentRole::Notice, format!("entry {n}"));
        }
        let shown = transcript.snapshot("x");
        assert_eq!(shown.messages.len(), MAX_MESSAGES);
        assert_eq!(shown.dropped, 30);
        assert_eq!(shown.messages[0].text, "entry 30");

        let mut transcript = Transcript::default();
        transcript.apply(Event::BlockStarted(AgentRole::Assistant));
        let chunk = "é".repeat(4096);
        for _ in 0..20 {
            transcript.apply(Event::BlockDelta(chunk.clone()));
        }
        let text = &transcript.snapshot("x").messages[0].text;
        assert!(text.len() <= MAX_ENTRY_BYTES + TRUNCATED.len());
        assert!(
            text.ends_with(TRUNCATED),
            "an overlong entry is marked as cut"
        );
        assert!(text.is_char_boundary(text.len() - TRUNCATED.len()));

        let mut transcript = Transcript::default();
        for _ in 0..10 {
            transcript.push(AgentRole::Assistant, "x".repeat(MAX_ENTRY_BYTES));
        }
        let shown = transcript.snapshot("x");
        let total: usize = shown.messages.iter().map(|m| m.text.len()).sum();
        assert!(total <= MAX_TOTAL_BYTES + MAX_ENTRY_BYTES + TRUNCATED.len());
        assert!(shown.dropped > 0);

        let mut transcript = Transcript::default();
        transcript.apply(Event::ToolStarted {
            call: "c".into(),
            name: "bash".into(),
            intent: String::new(),
            detail: String::new(),
        });
        transcript.apply(Event::ToolFinished {
            call: "c".into(),
            output: "y".repeat(100_000),
            diff: String::new(),
            failed: false,
        });
        let shown = transcript.snapshot("x");
        let output = &shown.messages[0].tool.as_ref().expect("tool").output;
        assert!(output.len() <= MAX_TOOL_OUTPUT_BYTES + TRUNCATED.len());
    }

    #[test]
    fn an_edit_keeps_its_diff_instead_of_the_whole_file_and_a_huge_diff_is_cut() {
        let mut transcript = Transcript::default();
        for call in ["e1", "e2"] {
            transcript.apply(Event::ToolStarted {
                call: call.into(),
                name: "edit".into(),
                intent: "Editing".into(),
                detail: "/f".into(),
            });
        }
        transcript.apply(Event::ToolFinished {
            call: "e1".into(),
            output: "the whole file".into(),
            diff: " 1|a\n-2|b\n+2|B".into(),
            failed: false,
        });
        transcript.apply(Event::ToolFinished {
            call: "e2".into(),
            output: "x".into(),
            diff: "+1|y".repeat(100_000),
            failed: false,
        });
        let shown = transcript.snapshot("x");
        let first = shown.messages[0].tool.as_ref().expect("tool");
        assert_eq!(
            (first.output.as_str(), first.diff.as_str()),
            ("", " 1|a\n-2|b\n+2|B")
        );
        let second = shown.messages[1].tool.as_ref().expect("tool");
        assert!(second.diff.len() <= MAX_TOOL_DIFF_BYTES + TRUNCATED.len());
    }

    #[test]
    fn ending_the_agent_says_why_and_stops_the_turn() {
        let mut transcript = Transcript::default();
        transcript.apply(Event::TurnStarted);
        transcript.end("omp stopped (exit status: 1)".into());
        let shown = transcript.snapshot("x");
        assert!(!shown.busy);
        assert_eq!(shown.ended.as_deref(), Some("omp stopped (exit status: 1)"));
    }
}
