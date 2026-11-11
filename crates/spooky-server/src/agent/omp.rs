//! oh-my-pi: `omp --mode rpc-ui`, JSON lines on stdio (`--mode rpc-ui` is `rpc` plus the
//! tool dialogs, so `ask` and approvals reach the user).

use super::{Backend, Event, Reaction};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};
use spooky_core::{
    AgentCommand, AgentDialog, AgentRole, DialogAnswer, DialogKind, MAX_AGENT_PROMPT_BYTES,
    ModelRole, UsageProvider, UsageWindow,
};
use std::{env, ffi::OsString, fs, io::Read, path::PathBuf, process::Command};

/// Largest configuration file read for its model roles.
const MAX_CONFIG_BYTES: u64 = 256 * 1024;
/// The thinking levels a role can end with (`provider/id:high`).
const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// The model roles of `omp`'s configuration, which its protocol does not report: the
/// `modelRoles` block of `config.yml` in its agent directory (`~/.omp/agent`, or the
/// directory in `PI_CODING_AGENT_DIR`). Only that one block of plain `role: model`
/// lines is understood; anything else, or no file, means no roles.
fn configured_roles() -> Vec<ModelRole> {
    let dir = env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".omp/agent")));
    let Some(file) = dir.map(|dir| dir.join("config.yml")) else {
        return Vec::new();
    };
    let mut text = String::new();
    let read =
        fs::File::open(file).and_then(|f| f.take(MAX_CONFIG_BYTES).read_to_string(&mut text));
    if read.is_err() {
        return Vec::new();
    }
    parse_roles(&text)
}

/// The allowances in the text of `omp`'s `/usage`: a provider per unindented line, then
/// `- <allowance>` entries each followed by `<account>: N% used (M% left)` and
/// `resets in <time>`; `- <account>: ... saved rate-limit resets` lines are notes. Models
/// listed under a provider are skipped. Text that fits none of that is ignored.
fn parse_usage(text: &str) -> Vec<UsageProvider> {
    let mut providers: Vec<UsageProvider> = Vec::new();
    let mut window: Option<String> = None;
    let mut last: Option<usize> = None;
    for line in text.lines() {
        if line.trim().is_empty() || line.starts_with("```") || line.starts_with("Usage (") {
            continue;
        }
        let Some(provider) = providers.last_mut() else {
            if !line.starts_with([' ', '-']) {
                providers.push(UsageProvider {
                    name: line.trim().to_owned(),
                    notes: Vec::new(),
                    windows: Vec::new(),
                });
            }
            continue;
        };
        if !line.starts_with([' ', '-']) {
            providers.push(UsageProvider {
                name: line.trim().to_owned(),
                notes: Vec::new(),
                windows: Vec::new(),
            });
            window = None;
            last = None;
            continue;
        }
        let trimmed = line.trim();
        if let Some(entry) = trimmed.strip_prefix("- ") {
            if let Some(at) = entry.find("saved rate-limit reset") {
                // Without the account in front or the hint to spend one.
                let from = entry[..at].rfind(": ").map_or(0, |i| i + 2);
                let note = entry[from..].replace(" — /usage reset to spend", "");
                provider.notes.push(note);
                window = None;
            } else {
                window = Some(entry.trim().to_owned());
            }
            last = None;
        } else if let (Some(name), Some(used)) = (&window, trimmed.find("% used")) {
            // `account: 84.00% used (16.0% left)`: the account is what precedes the figure.
            let head = &trimmed[..used];
            let start = head
                .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
                .map_or(0, |i| i + 1);
            let account = head[..start]
                .trim_end()
                .trim_end_matches(':')
                .trim()
                .to_owned();
            let used: f32 = head[start..].parse().unwrap_or(100.0);
            provider.windows.push(UsageWindow {
                name: name.clone(),
                account,
                left: (100.0 - used).clamp(0.0, 100.0).round() as u8,
                resets: String::new(),
            });
            last = Some(provider.windows.len() - 1);
        } else if let (Some(index), Some(resets)) = (last, trimmed.strip_prefix("resets in ")) {
            provider.windows[index].resets = resets.to_owned();
        } else if trimmed.starts_with("soonest expires")
            && let Some(note) = provider.notes.last_mut()
        {
            note.push_str(" · ");
            note.push_str(trimmed);
        }
    }
    providers.retain(|p| !p.windows.is_empty() || !p.notes.is_empty());
    providers
}

/// The `modelRoles` block of a YAML document: indented `role: provider/id[:level]` lines.
fn parse_roles(text: &str) -> Vec<ModelRole> {
    let mut roles = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            inside = line.trim_end() == "modelRoles:";
            continue;
        }
        if !inside {
            continue;
        }
        let Some((role, value)) = line.trim().split_once(':') else {
            continue;
        };
        let value = value
            .split(" #")
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches(['"', '\'']);
        if role.is_empty() || value.is_empty() {
            continue;
        }
        let (model, thinking) = match value.rsplit_once(':') {
            Some((model, level)) if LEVELS.contains(&level) => (model, Some(level.to_owned())),
            _ => (value, None),
        };
        roles.push(ModelRole {
            role: role.trim().trim_matches(['"', '\'']).to_owned(),
            model: model.to_owned(),
            thinking,
        });
    }
    roles
}

/// Events the client needs; omp sends nothing else.
const EVENTS: [&str; 9] = [
    "agent_start",
    "agent_end",
    "message_start",
    "message_update",
    "message_end",
    "tool_execution_start",
    "tool_execution_end",
    "extension_ui_request",
    "available_commands_update",
];
/// Thinking levels `--thinking` accepts.
const THINKING: [&str; 8] = [
    "off", "minimal", "low", "medium", "high", "xhigh", "max", "auto",
];
/// Values of `--approval-mode`.
const APPROVAL: [&str; 4] = ["always-ask", "always-ask-cached", "write", "yolo"];
/// Longest value of an option.
const MAX_VALUE_BYTES: usize = 256;
/// Pages of history loaded when resuming.
const MAX_HISTORY_PAGES: u32 = 30;
/// Arguments of a tool that name what it works on, most telling first.
const DETAIL_KEYS: [&str; 7] = [
    "command",
    "path",
    "file_path",
    "file",
    "url",
    "pattern",
    "query",
];

pub(super) struct Omp {
    /// Options passed to `omp`, normalized to one argument each.
    forwarded: Vec<String>,
    /// A first prompt the user gave on the command line.
    initial: Option<String>,
    /// Load the conversation of the session being resumed.
    loads_history: bool,
    /// The session file in use, once `omp` has said.
    session: Option<String>,
    pages: u32,
    /// A `/usage` we asked for is outstanding: its output is a report, not a notice.
    awaiting_usage: bool,
}

impl Omp {
    /// `args` are what the user typed after `omp`; only a closed set is accepted.
    /// `resume` is a saved session to continue instead.
    pub(super) fn new(args: &[String], resume: Option<&str>) -> Result<Self> {
        let mut omp = parse_args(args)?;
        if let Some(session) = resume {
            ensure!(
                valid_value(session),
                "the saved session is not a usable value"
            );
            omp.forwarded.retain(|a| {
                !matches!(a.as_str(), "--continue" | "--no-session") && !a.starts_with("--resume=")
            });
            omp.forwarded.push(format!("--resume={session}"));
            omp.initial = None;
            omp.loads_history = true;
        }
        Ok(omp)
    }
}

fn valid_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_VALUE_BYTES
        && !value.starts_with('-')
        && !value.chars().any(char::is_control)
}

/// Accepts `--model`, `--thinking`, `--approval-mode`, `--resume` with a value,
/// `-c/--continue`, `--no-session`, `--auto-approve`, and at most one prompt. Anything
/// else is refused so the real `omp` can take it.
fn parse_args(args: &[String]) -> Result<Omp> {
    let mut omp = Omp {
        forwarded: Vec::new(),
        initial: None,
        loads_history: false,
        session: None,
        pages: 0,
        awaiting_usage: false,
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        match name {
            "--model" | "--thinking" | "--approval-mode" | "--resume" => {
                let value = match inline {
                    Some(value) => value,
                    None => match iter.next() {
                        Some(value) => value.clone(),
                        None => bail!("{name} needs a value"),
                    },
                };
                ensure!(valid_value(&value), "unusable value for {name}");
                if name == "--thinking" {
                    ensure!(THINKING.contains(&value.as_str()), "unknown thinking level");
                }
                if name == "--approval-mode" {
                    ensure!(APPROVAL.contains(&value.as_str()), "unknown approval mode");
                }
                if name == "--resume" {
                    omp.loads_history = true;
                }
                omp.forwarded.push(format!("{name}={value}"));
            }
            "-c" | "--continue" if inline.is_none() => {
                omp.loads_history = true;
                omp.forwarded.push("--continue".into());
            }
            "--no-session" | "--auto-approve" if inline.is_none() => {
                omp.forwarded.push(name.to_owned());
            }
            _ if arg.starts_with('-') => bail!("option {arg} is not supported in the agent view"),
            _ if arg.starts_with('@') => {
                bail!("attached files are not supported in the agent view")
            }
            _ => {
                ensure!(omp.initial.is_none(), "only one prompt can be given");
                ensure!(
                    arg.len() <= MAX_AGENT_PROMPT_BYTES && !arg.trim().is_empty(),
                    "the prompt is empty or too long"
                );
                // `omp commit` and the like are subcommands of the real program, and
                // their names never contain a space; a sentence is a prompt.
                ensure!(
                    arg.trim().contains(char::is_whitespace),
                    "a single word is taken as a subcommand"
                );
                omp.initial = Some(arg.clone());
            }
        }
    }
    Ok(omp)
}

/// A request of ours, whose answer the backend interprets by `id`.
fn query(id: &str, kind: &str, extra: Value) -> String {
    let mut command = json!({ "id": id, "type": kind });
    if let (Some(command), Some(extra)) = (command.as_object_mut(), extra.as_object()) {
        command.extend(extra.clone());
    }
    command.to_string()
}

impl Backend for Omp {
    fn name(&self) -> &'static str {
        "omp"
    }

    /// `SPOOKY_OMP` names another executable.
    fn command(&self) -> Command {
        let program = env::var_os("SPOOKY_OMP").unwrap_or_else(|| OsString::from("omp"));
        // A server started over SSH has a minimal PATH; look where user programs live too.
        let mut command = Command::new(super::path::locate(&program));
        super::path::extend(&mut command);
        command.args(["--mode", "rpc-ui"]).args(&self.forwarded);
        command
    }

    fn install_hint(&self) -> &'static str {
        "is oh-my-pi (omp) installed and on this machine's PATH?"
    }

    fn start(&mut self) -> Vec<String> {
        let mut lines = vec![
            query(
                "q:filter",
                "set_event_filter",
                json!({ "events": EVENTS, "messageUpdates": "delta" }),
            ),
            query("q:state", "get_state", json!({})),
            query("q:models", "get_available_models", json!({})),
            query("q:levels", "get_available_thinking_levels", json!({})),
        ];
        if self.loads_history {
            lines.push(query(
                "q:history",
                "get_messages_page",
                json!({ "limit": 100 }),
            ));
        }
        if let Some(prompt) = self.initial.take() {
            lines.push(self.prompt(&prompt, false));
        }
        lines
    }

    fn frame(&mut self, frame: &Value) -> Reaction {
        let kind = frame
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind == "response" {
            return self.response(frame);
        }
        if kind == "command_output" && !self.awaiting_usage {
            // A command the user ran reports in the conversation, as its own entry.
            let text = frame
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            return Reaction {
                events: if text.is_empty() {
                    Vec::new()
                } else {
                    vec![Event::Notice(text.to_owned())]
                },
                send: Vec::new(),
            };
        }
        if kind == "command_output" && self.awaiting_usage {
            self.awaiting_usage = false;
            let text = frame
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return Reaction {
                events: vec![Event::Usage(parse_usage(text))],
                send: Vec::new(),
            };
        }
        let events = parse_events(frame);
        let mut send = Vec::new();
        if events.contains(&Event::TurnEnded) {
            // The context window use and the model may have changed.
            send.push(query("q:state", "get_state", json!({})));
        }
        Reaction { events, send }
    }

    fn prompt(&mut self, text: &str, busy: bool) -> String {
        let mut command = json!({ "type": "prompt", "message": text });
        if busy {
            command["streamingBehavior"] = json!("followUp");
        }
        command.to_string()
    }

    fn abort(&mut self) -> String {
        json!({ "type": "abort" }).to_string()
    }

    fn set_model(&mut self, model: &str) -> Option<String> {
        let (provider, id) = model.split_once('/')?;
        Some(query(
            "q:set",
            "set_model",
            json!({ "provider": provider, "modelId": id }),
        ))
    }

    fn set_thinking(&mut self, level: &str) -> Option<String> {
        Some(query(
            "q:set",
            "set_thinking_level",
            json!({ "level": level }),
        ))
    }

    fn answer(&mut self, dialog: &str, answer: &DialogAnswer) -> Option<String> {
        let mut line = json!({ "type": "extension_ui_response", "id": dialog });
        match answer {
            DialogAnswer::Confirm(confirmed) => line["confirmed"] = json!(confirmed),
            DialogAnswer::Value(value) => line["value"] = json!(value),
            DialogAnswer::Cancel => line["cancelled"] = json!(true),
        }
        Some(line.to_string())
    }

    fn session(&self) -> Option<String> {
        self.session.clone()
    }

    fn roles(&self) -> Vec<ModelRole> {
        configured_roles()
    }

    fn usage(&mut self) -> Option<String> {
        self.awaiting_usage = true;
        Some(json!({ "type": "prompt", "message": "/usage" }).to_string())
    }
}

impl Omp {
    /// Interprets an answer to one of our own requests, or reports a failed command.
    fn response(&mut self, frame: &Value) -> Reaction {
        let id = frame.get("id").and_then(Value::as_str).unwrap_or_default();
        let ok = frame.get("success").and_then(Value::as_bool) == Some(true);
        let data = frame.get("data").unwrap_or(&Value::Null);
        let mut reaction = Reaction::default();
        match (id, ok) {
            ("q:state", true) => {
                if let Some(model) = data.get("model").and_then(model_name) {
                    reaction.events.push(Event::Model(model));
                }
                if let Some(level) = data.get("thinkingLevel").and_then(Value::as_str) {
                    reaction.events.push(Event::Thinking(level.to_owned()));
                }
                if let Some(percent) = data
                    .get("contextUsage")
                    .and_then(|usage| usage.get("percent"))
                    .and_then(Value::as_f64)
                {
                    reaction
                        .events
                        .push(Event::Context(percent.round().clamp(0.0, 100.0) as u8));
                }
                if let Some(file) = data.get("sessionFile").and_then(Value::as_str) {
                    self.session = Some(file.to_owned());
                }
            }
            ("q:models", true) => {
                let models = data
                    .get("models")
                    .and_then(Value::as_array)
                    .map(|models| models.iter().filter_map(model_name).collect())
                    .unwrap_or_default();
                reaction.events.push(Event::Models(models));
            }
            ("q:levels", true) => {
                let levels = data
                    .get("levels")
                    .and_then(Value::as_array)
                    .map(|levels| {
                        levels
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                reaction.events.push(Event::Levels(levels));
            }
            ("q:history", true) => {
                if let Some(messages) = data.get("messages").and_then(Value::as_array) {
                    reaction.events.extend(history_events(messages));
                }
                self.pages += 1;
                if let Some(cursor) = data.get("nextCursor").and_then(Value::as_str)
                    && self.pages < MAX_HISTORY_PAGES
                {
                    reaction.send.push(query(
                        "q:history",
                        "get_messages_page",
                        json!({ "limit": 100, "cursor": cursor }),
                    ));
                }
            }
            ("q:set", true) => reaction.send.push(query("q:state", "get_state", json!({}))),
            // Our own lookups failing is not worth a notice: the control just stays away.
            (id, false) if id.starts_with("q:") && id != "q:set" => {}
            (_, false) => {
                let error = frame
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("request failed");
                reaction.events.push(Event::Failure(error.to_owned()));
            }
            _ => {}
        }
        reaction
    }
}

/// `provider/id` of a model object.
fn model_name(model: &Value) -> Option<String> {
    let provider = model.get("provider").and_then(Value::as_str)?;
    let id = model.get("id").and_then(Value::as_str)?;
    Some(format!("{provider}/{id}"))
}

/// The events one omp frame stands for.
fn parse_events(frame: &Value) -> Vec<Event> {
    let Some(kind) = frame.get("type").and_then(Value::as_str) else {
        return Vec::new();
    };
    match kind {
        "agent_start" => vec![Event::TurnStarted],
        // A non-terminal end means queued work resumes the turn.
        "agent_end" if frame.get("isTerminal").and_then(Value::as_bool) == Some(false) => {
            Vec::new()
        }
        "agent_end" => vec![Event::TurnEnded],
        "message_update" => block_event(frame).into_iter().collect(),
        "message_end" => match message_role(frame) {
            Some("user") if shown_to_user(frame.get("message")) => {
                let text = frame
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .map(text_of)
                    .unwrap_or_default();
                vec![Event::User(text)]
            }
            _ => Vec::new(),
        },
        "tool_execution_start" => {
            let call = frame.get("toolCallId").and_then(Value::as_str);
            let name = frame.get("toolName").and_then(Value::as_str);
            match (call, name) {
                (Some(call), Some(name)) => vec![Event::ToolStarted {
                    call: call.to_owned(),
                    name: name.to_owned(),
                    intent: frame
                        .get("intent")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    detail: frame.get("args").map(detail_of).unwrap_or_default(),
                }],
                _ => Vec::new(),
            }
        }
        "tool_execution_end" => match frame.get("toolCallId").and_then(Value::as_str) {
            Some(call) => vec![Event::ToolFinished {
                call: call.to_owned(),
                output: frame
                    .get("result")
                    .and_then(|r| r.get("content"))
                    .map(text_of)
                    .unwrap_or_default(),
                diff: diff_of(frame.get("result").and_then(|r| r.get("details"))),
                failed: frame.get("isError").and_then(Value::as_bool) == Some(true),
            }],
            None => Vec::new(),
        },
        "available_commands_update" => match frame.get("commands").and_then(Value::as_array) {
            Some(commands) => vec![Event::Commands(commands.iter().map(command_of).collect())],
            None => Vec::new(),
        },
        "extension_ui_request" => dialog_event(frame).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// One command of an `available_commands_update` frame, with its subcommands.
fn command_of(value: &Value) -> AgentCommand {
    let string = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    AgentCommand {
        name: string("name"),
        description: string("description"),
        hint: value
            .get("input")
            .and_then(|input| input.get("hint"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        subcommands: value
            .get("subcommands")
            .and_then(Value::as_array)
            .map(|list| list.iter().map(command_of).collect())
            .unwrap_or_default(),
    }
}

/// A streamed block of an assistant message.
fn block_event(frame: &Value) -> Option<Event> {
    let event = frame.get("assistantMessageEvent")?;
    let kind = event.get("type").and_then(Value::as_str)?;
    let (role, phase) = match kind.strip_prefix("text_") {
        Some(phase) => (AgentRole::Assistant, phase),
        None => (AgentRole::Thinking, kind.strip_prefix("thinking_")?),
    };
    match phase {
        "start" => Some(Event::BlockStarted(role)),
        "delta" => Some(Event::BlockDelta(
            event.get("delta").and_then(Value::as_str)?.to_owned(),
        )),
        "end" => Some(Event::BlockDone {
            role,
            text: event
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }),
        _ => None,
    }
}

/// A question the user must answer; other UI requests (widgets, notices) are ignored.
fn dialog_event(frame: &Value) -> Option<Event> {
    let id = frame.get("id").and_then(Value::as_str)?;
    let method = frame.get("method").and_then(Value::as_str)?;
    let text = |key: &str| {
        frame
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let (kind, message, options) = match method {
        "select" => (
            DialogKind::Select,
            text("message"),
            frame
                .get("options")
                .and_then(Value::as_array)
                .map(|o| {
                    o.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
        ),
        "confirm" => (DialogKind::Confirm, text("message"), Vec::new()),
        "input" | "editor" => (DialogKind::Input, text("placeholder"), Vec::new()),
        "cancel" => {
            return frame
                .get("targetId")
                .and_then(Value::as_str)
                .map(|target| Event::DialogClosed(target.to_owned()));
        }
        _ => return None,
    };
    Some(Event::Dialog(AgentDialog {
        id: id.to_owned(),
        kind,
        title: text("title"),
        message,
        options,
    }))
}

/// The conversation events of a page of saved messages, oldest first.
fn history_events(messages: &[Value]) -> Vec<Event> {
    let mut events = Vec::new();
    for message in messages {
        let content = message.get("content");
        match message.get("role").and_then(Value::as_str) {
            Some("user") if shown_to_user(Some(message)) => {
                events.push(Event::User(content.map(text_of).unwrap_or_default()));
            }
            Some("assistant") => {
                for block in content.and_then(Value::as_array).into_iter().flatten() {
                    let string = |key: &str| {
                        block
                            .get(key)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned()
                    };
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => events.push(Event::BlockDone {
                            role: AgentRole::Assistant,
                            text: string("text"),
                        }),
                        Some("thinking") => events.push(Event::BlockDone {
                            role: AgentRole::Thinking,
                            text: string("thinking"),
                        }),
                        Some("toolCall") => events.push(Event::ToolStarted {
                            call: string("id"),
                            name: string("name"),
                            intent: string("intent"),
                            detail: block.get("arguments").map(detail_of).unwrap_or_default(),
                        }),
                        _ => {}
                    }
                }
            }
            Some("toolResult") => {
                if let Some(call) = message.get("toolCallId").and_then(Value::as_str) {
                    events.push(Event::ToolFinished {
                        call: call.to_owned(),
                        output: content.map(text_of).unwrap_or_default(),
                        diff: diff_of(message.get("details")),
                        failed: message.get("isError").and_then(Value::as_bool) == Some(true),
                    });
                }
            }
            _ => {}
        }
    }
    events
}

/// The diff of a file edit, from a tool result's `details`; empty when there is none.
fn diff_of(details: Option<&Value>) -> String {
    details
        .and_then(|d| d.get("diff"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Messages the agent wrote to itself (directives, reminders) are not the user's.
fn shown_to_user(message: Option<&Value>) -> bool {
    message
        .and_then(|m| m.get("attribution"))
        .and_then(Value::as_str)
        .is_none_or(|attribution| attribution == "user")
}

/// The role of the message a frame carries.
fn message_role(frame: &Value) -> Option<&str> {
    frame
        .get("message")
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
}

/// What a tool works on: its first telling string argument.
fn detail_of(args: &Value) -> String {
    DETAIL_KEYS
        .iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned()
}

/// The text of message content: a string, or the text blocks of a list.
fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(line: &str) -> Value {
        serde_json::from_str(line).expect("valid frame")
    }

    fn events(line: &str) -> Vec<Event> {
        parse_events(&frame(line))
    }

    fn args(list: &[&str]) -> Result<Omp> {
        parse_args(&list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn a_turn_maps_to_events_and_unknown_frames_to_none() {
        assert_eq!(events(r#"{"type":"agent_start"}"#), [Event::TurnStarted]);
        assert_eq!(
            events(
                r#"{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#
            ),
            [Event::User("hi".into())]
        );
        assert!(
            events(r#"{"type":"message_end","message":{"role":"user","attribution":"agent","content":"hidden"}}"#)
                .is_empty(),
            "messages the agent wrote to itself are not the user's"
        );
        assert!(
            events(r#"{"type":"message_end","message":{"role":"assistant","content":"x"}}"#)
                .is_empty()
        );
        assert!(events(r#"{"type":"message_start","message":{"role":"assistant"}}"#).is_empty());
        assert!(events(r#"{"type":"notice","message":"x"}"#).is_empty());
        assert!(events(r#"{"no":"type"}"#).is_empty());
    }

    #[test]
    fn streamed_blocks_are_text_or_thinking() {
        let update = |kind: &str, extra: &str| {
            events(&format!(
                r#"{{"type":"message_update","assistantMessageEvent":{{"type":"{kind}"{extra}}}}}"#
            ))
        };
        assert_eq!(
            update("text_start", ""),
            [Event::BlockStarted(AgentRole::Assistant)]
        );
        assert_eq!(
            update("thinking_start", ""),
            [Event::BlockStarted(AgentRole::Thinking)]
        );
        assert_eq!(
            update("text_delta", r#","delta":"Hel""#),
            [Event::BlockDelta("Hel".into())]
        );
        assert_eq!(
            update("thinking_end", r#","content":"so""#),
            [Event::BlockDone {
                role: AgentRole::Thinking,
                text: "so".into()
            }]
        );
        assert!(update("toolcall_delta", r#","delta":"{""#).is_empty());
    }

    #[test]
    fn offered_commands_carry_their_hint_and_their_subcommands() {
        let offered = events(
            r#"{"type":"available_commands_update","commands":[{"name":"model","description":"Show current model"},{"name":"security","description":"Scans","input":{"hint":"<plan|scan>"},"subcommands":[{"name":"scan","description":"Start a scan"}]}]}"#,
        );
        let [Event::Commands(commands)] = offered.as_slice() else {
            panic!("the frame lists commands: {offered:?}");
        };
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].name, "model");
        assert_eq!(commands[0].hint, "", "a command may take nothing");
        assert_eq!(commands[1].hint, "<plan|scan>");
        assert_eq!(commands[1].subcommands[0].name, "scan");
        assert!(
            events(r#"{"type":"available_commands_update"}"#).is_empty(),
            "a frame with no list says nothing"
        );
    }

    #[test]
    fn a_non_terminal_end_is_not_the_end_of_the_turn() {
        assert!(events(r#"{"type":"agent_end","isTerminal":false}"#).is_empty());
        assert_eq!(events(r#"{"type":"agent_end"}"#), [Event::TurnEnded]);
    }

    #[test]
    fn tool_calls_carry_their_purpose_main_argument_and_result() {
        assert_eq!(
            events(
                r#"{"type":"tool_execution_start","toolCallId":"c1","toolName":"read","args":{"path":"/etc/hosts","sel":"x"},"intent":"Reading hosts"}"#
            ),
            [Event::ToolStarted {
                call: "c1".into(),
                name: "read".into(),
                intent: "Reading hosts".into(),
                detail: "/etc/hosts".into()
            }]
        );
        // An edit reports what it changed next to its output.
        assert_eq!(
            events(
                r#"{"type":"tool_execution_end","toolCallId":"e1","toolName":"edit","result":{"content":[{"type":"text","text":"[f#1]\n1:a"}],"details":{"diff":" 1|a\n-2|b\n+2|B","path":"/f"}},"isError":false}"#
            ),
            [Event::ToolFinished {
                call: "e1".into(),
                output: "[f#1]\n1:a".into(),
                diff: " 1|a\n-2|b\n+2|B".into(),
                failed: false
            }]
        );
        assert_eq!(
            events(
                r#"{"type":"tool_execution_end","toolCallId":"c1","toolName":"bash","result":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]},"isError":true}"#
            ),
            [Event::ToolFinished {
                call: "c1".into(),
                output: "a\nb".into(),
                diff: String::new(),
                failed: true
            }]
        );
    }

    #[test]
    fn questions_become_dialogs_and_other_ui_requests_are_ignored() {
        let Some(Event::Dialog(select)) = events(
            r#"{"type":"extension_ui_request","id":"u1","method":"select","title":"Color?","options":["Red","Blue"]}"#,
        )
        .pop() else {
            panic!("expected a dialog");
        };
        assert_eq!(
            (select.kind, select.title.as_str()),
            (DialogKind::Select, "Color?")
        );
        assert_eq!(select.options, ["Red", "Blue"]);
        let Some(Event::Dialog(confirm)) = events(
            r#"{"type":"extension_ui_request","id":"u2","method":"confirm","title":"Run?","message":"rm -rf x"}"#,
        )
        .pop() else {
            panic!("expected a dialog");
        };
        assert_eq!(
            (confirm.kind, confirm.message.as_str()),
            (DialogKind::Confirm, "rm -rf x")
        );
        assert_eq!(
            events(
                r#"{"type":"extension_ui_request","id":"u3","method":"cancel","targetId":"u1"}"#
            ),
            [Event::DialogClosed("u1".into())]
        );
        assert!(
            events(r#"{"type":"extension_ui_request","id":"u4","method":"setWidget"}"#).is_empty()
        );
        assert!(
            events(r#"{"type":"extension_ui_request","id":"u5","method":"notify","message":"x"}"#)
                .is_empty()
        );
    }

    #[test]
    fn answers_are_the_lines_omp_expects() {
        let mut omp = Omp::new(&[], None).expect("omp");
        let mut line =
            |a| serde_json::from_str::<Value>(&omp.answer("u1", &a).expect("line")).expect("json");
        assert_eq!(line(DialogAnswer::Confirm(false))["confirmed"], false);
        assert_eq!(line(DialogAnswer::Value("Blue".into()))["value"], "Blue");
        assert_eq!(line(DialogAnswer::Cancel)["cancelled"], true);
        assert_eq!(line(DialogAnswer::Cancel)["type"], "extension_ui_response");
    }

    #[test]
    fn state_models_and_levels_fill_in_the_controls_and_remember_the_session() {
        let mut omp = Omp::new(&[], None).expect("omp");
        let state = omp.frame(&frame(
            r#"{"id":"q:state","type":"response","command":"get_state","success":true,"data":{"model":{"provider":"p","id":"m"},"thinkingLevel":"high","contextUsage":{"percent":3.4},"sessionFile":"/s/a.jsonl"}}"#,
        ));
        assert_eq!(
            state.events,
            [
                Event::Model("p/m".into()),
                Event::Thinking("high".into()),
                Event::Context(3)
            ]
        );
        assert_eq!(omp.session().as_deref(), Some("/s/a.jsonl"));
        let models = omp.frame(&frame(
            r#"{"id":"q:models","type":"response","success":true,"data":{"models":[{"provider":"p","id":"a"},{"provider":"q","id":"b"},{"id":"broken"}]}}"#,
        ));
        assert_eq!(
            models.events,
            [Event::Models(vec!["p/a".into(), "q/b".into()])]
        );
        let levels = omp.frame(&frame(
            r#"{"id":"q:levels","type":"response","success":true,"data":{"levels":["off","high"]}}"#,
        ));
        assert_eq!(
            levels.events,
            [Event::Levels(vec!["off".into(), "high".into()])]
        );
    }

    #[test]
    fn a_failed_command_is_a_notice_but_a_failed_lookup_is_silent() {
        let mut omp = Omp::new(&[], None).expect("omp");
        let failed = omp.frame(&frame(
            r#"{"type":"response","success":false,"error":"no model"}"#,
        ));
        assert_eq!(failed.events, [Event::Failure("no model".into())]);
        let lookup = omp.frame(&frame(
            r#"{"id":"q:models","type":"response","success":false,"error":"x"}"#,
        ));
        assert!(lookup.events.is_empty());
        let set = omp.frame(&frame(
            r#"{"id":"q:set","type":"response","success":false,"error":"Model not found"}"#,
        ));
        assert_eq!(set.events, [Event::Failure("Model not found".into())]);
        let ok = omp.frame(&frame(r#"{"id":"q:set","type":"response","success":true}"#));
        assert!(
            ok.send.iter().any(|l| l.contains("get_state")),
            "a change re-reads the state"
        );
        assert!(
            omp.frame(&frame(r#"{"type":"response","success":true}"#))
                .events
                .is_empty()
        );
    }

    #[test]
    fn a_finished_turn_asks_for_the_new_context_use() {
        let mut omp = Omp::new(&[], None).expect("omp");
        let end = omp.frame(&frame(r#"{"type":"agent_end"}"#));
        assert!(end.send.iter().any(|l| l.contains("get_state")));
    }

    #[test]
    fn history_pages_become_the_conversation_and_ask_for_the_next_page() {
        let mut omp = Omp::new(&[], Some("/s/a.jsonl")).expect("omp");
        let page = omp.frame(&frame(
            r#"{"id":"q:history","type":"response","success":true,"data":{"messages":[
                {"role":"user","content":[{"type":"text","text":"hi"}]},
                {"role":"assistant","content":[{"type":"thinking","thinking":"hm"},{"type":"toolCall","id":"c","name":"read","intent":"Reading","arguments":{"path":"/x"}},{"type":"text","text":"done"}]},
                {"role":"toolResult","toolCallId":"c","content":[{"type":"text","text":"data"}],"isError":false}
            ],"nextCursor":"n2"}}"#,
        ));
        assert_eq!(
            page.events,
            [
                Event::User("hi".into()),
                Event::BlockDone {
                    role: AgentRole::Thinking,
                    text: "hm".into()
                },
                Event::ToolStarted {
                    call: "c".into(),
                    name: "read".into(),
                    intent: "Reading".into(),
                    detail: "/x".into()
                },
                Event::BlockDone {
                    role: AgentRole::Assistant,
                    text: "done".into()
                },
                Event::ToolFinished {
                    call: "c".into(),
                    output: "data".into(),
                    diff: String::new(),
                    failed: false
                },
            ]
        );
        assert!(page.send[0].contains("\"cursor\":\"n2\""));
        let last = omp.frame(&frame(
            r#"{"id":"q:history","type":"response","success":true,"data":{"messages":[]}}"#,
        ));
        assert!(last.send.is_empty(), "no cursor, no further page");
    }

    #[test]
    fn a_prompt_while_busy_is_queued_after_the_turn() {
        let mut omp = Omp::new(&[], None).expect("omp");
        let idle: Value = serde_json::from_str(&omp.prompt("go", false)).expect("json");
        assert!(idle.get("streamingBehavior").is_none());
        let busy: Value = serde_json::from_str(&omp.prompt("go", true)).expect("json");
        assert_eq!(busy["streamingBehavior"], "followUp");
    }

    #[test]
    fn only_a_closed_set_of_arguments_is_accepted_and_normalized() {
        let omp =
            args(&["--model", "opus", "--thinking=high", "-c", "fix the bug"]).expect("accepted");
        assert_eq!(
            omp.forwarded,
            ["--model=opus", "--thinking=high", "--continue"]
        );
        assert_eq!(omp.initial.as_deref(), Some("fix the bug"));
        assert!(omp.loads_history);
        assert_eq!(
            args(&["--resume=/s/x.jsonl", "--auto-approve", "--no-session"])
                .expect("accepted")
                .forwarded,
            ["--resume=/s/x.jsonl", "--auto-approve", "--no-session"]
        );
        for refused in [
            &["-p", "x"][..],
            &["--mode", "rpc"],
            &["--model"],
            &["--model", "--evil"],
            &["--thinking", "extreme"],
            &["--approval-mode", "none"],
            &["@file.md"],
            &["--cwd", "/"],
            &["one", "two"],
            &["commit"],
            &["--model=a\nb"],
            &["--continue=x"],
            &["   "],
        ] {
            assert!(args(refused).is_err(), "{refused:?} must be refused");
        }
    }

    #[test]
    fn resuming_a_saved_session_replaces_the_users_session_options() {
        let omp = Omp::new(
            &[
                "--continue".into(),
                "--no-session".into(),
                "--model=m".into(),
            ],
            Some("/s/a.jsonl"),
        )
        .expect("omp");
        assert_eq!(omp.forwarded, ["--model=m", "--resume=/s/a.jsonl"]);
        assert!(omp.loads_history);
        assert!(
            Omp::new(&[], Some("--evil")).is_err(),
            "a saved value cannot be an option"
        );
    }

    #[test]
    fn the_command_and_start_lines_cover_the_controls_and_the_first_prompt() {
        let mut omp = args(&["--model=m", "say hello"]).expect("omp");
        let command = omp.command();
        assert_eq!(
            command
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            ["--mode", "rpc-ui", "--model=m"]
        );
        let lines = omp.start();
        let kinds: Vec<_> = lines
            .iter()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|v| v["type"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(
            kinds,
            [
                "set_event_filter",
                "get_state",
                "get_available_models",
                "get_available_thinking_levels",
                "prompt"
            ]
        );
        assert!(
            omp.start().iter().all(|l| !l.contains("\"prompt\"")),
            "the first prompt is sent once"
        );
    }

    #[test]
    fn choosing_a_model_names_its_provider_and_id() {
        let mut omp = Omp::new(&[], None).expect("omp");
        let line: Value =
            serde_json::from_str(&omp.set_model("openai/gpt-x/y").expect("line")).expect("json");
        assert_eq!(
            (line["provider"].as_str(), line["modelId"].as_str()),
            (Some("openai"), Some("gpt-x/y"))
        );
        assert!(omp.set_model("no-slash").is_none());
    }

    #[test]
    fn the_roles_block_of_the_config_is_read_with_levels_and_comments() {
        let config = "setupVersion: 3\nmodelRoles:\n  default: openai-codex/gpt-6.1-terra:xhigh\n  # note\n  plan: \"anthropic/claude-opus-5:high\" # big\n  web: anthropic/claude-haiku-4-5\n  odd: amazon-bedrock/anthropic.claude-3-haiku-20240307-v1:0\nmemory:\n  backend: mnemopi\n";
        let roles: Vec<(String, String, Option<String>)> = parse_roles(config)
            .into_iter()
            .map(|r| (r.role, r.model, r.thinking))
            .collect();
        let want =
            |a: &str, b: &str, c: Option<&str>| (a.to_owned(), b.to_owned(), c.map(str::to_owned));
        assert_eq!(
            roles,
            [
                want("default", "openai-codex/gpt-6.1-terra", Some("xhigh")),
                want("plan", "anthropic/claude-opus-5", Some("high")),
                want("web", "anthropic/claude-haiku-4-5", None),
                want(
                    "odd",
                    "amazon-bedrock/anthropic.claude-3-haiku-20240307-v1:0",
                    None
                ),
            ],
            "a ':0' version suffix is part of the model, not a level; other blocks are ignored"
        );
        assert!(
            parse_roles("modelRoles: {default: a/b}\n").is_empty(),
            "flow style is not read"
        );
        assert!(parse_roles("memory:\n  default: a/b\n").is_empty());
    }

    #[test]
    fn the_usage_report_is_read_into_providers_allowances_and_notes() {
        let text = "```\nUsage (19s ago)\n\nAnthropic\n  Models with usage data\n    anthropic/claude-opus-5\n- a@b.c (a@b.c's Organization): 3 saved rate-limit resets — 1 usable now\n  soonest expires in 29d (2026-12-10)\n- Claude 7 Day\n  a@b.c (a@b.c's Organization): 84.00% used (16.0% left)\n  [████░░] 84%\n  resets in 13h\n\nOpenai Codex\n  Models with usage data\n- x@y.z · plan: plus: 0.00% used (100.0% left)  ← in use by this session\n- 5 hours\n  x@y.z · plan: plus: 0.00% used (100.0% left)  ← in use by this session\n  resets in 5h\n- 7 days\n  x@y.z · plan: plus: 100.00% used (0.0% left)\n  resets in 3d\n```";
        let providers = parse_usage(text);
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].name, "Anthropic");
        assert_eq!(providers[0].notes.len(), 1);
        assert_eq!(
            providers[0].notes[0],
            "3 saved rate-limit resets — 1 usable now · soonest expires in 29d (2026-12-10)"
        );
        let seven = &providers[0].windows[0];
        assert_eq!(
            (
                seven.name.as_str(),
                seven.account.as_str(),
                seven.left,
                seven.resets.as_str()
            ),
            ("Claude 7 Day", "a@b.c (a@b.c's Organization)", 16, "13h")
        );
        let codex: Vec<(&str, u8, &str)> = providers[1]
            .windows
            .iter()
            .map(|w| (w.name.as_str(), w.left, w.resets.as_str()))
            .collect();
        assert_eq!(codex, [("5 hours", 100, "5h"), ("7 days", 0, "3d")]);
        assert_eq!(providers[1].windows[0].account, "x@y.z · plan: plus");
        assert!(parse_usage("nothing useful here").is_empty());
    }

    #[test]
    fn a_usage_request_makes_the_next_command_output_a_report() {
        let mut omp = Omp::new(&[], None).expect("omp");
        let output = frame(
            r#"{"type":"command_output","text":"Anthropic\n- Day\n  a: 10.00% used\n  resets in 1h"}"#,
        );
        assert!(
            matches!(&omp.frame(&output).events[..], [Event::Notice(text)] if text.starts_with("Anthropic")),
            "unasked output is what a command the user ran reported"
        );
        let line = omp.usage().expect("line");
        assert!(line.contains("/usage"));
        let events = omp.frame(&output).events;
        assert!(matches!(&events[..], [Event::Usage(p)] if p[0].windows[0].left == 90));
        assert!(
            matches!(&omp.frame(&output).events[..], [Event::Notice(_)]),
            "only the asked-for one is a report"
        );
        assert!(
            omp.frame(&frame(r#"{"type":"command_output","text":"  "}"#))
                .events
                .is_empty(),
            "a command that said nothing adds nothing"
        );
    }
}
