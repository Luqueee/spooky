//! Agent panes: the conversation with a coding agent the server runs, and the composer
//! that talks to it. The server owns the agent and its conversation; this keeps only what
//! the user is typing for each agent pane, which entries are unfolded, and the parsed
//! Markdown of the answers.

mod view;
pub(crate) mod web;

use iced::widget::{markdown, text_editor};
use spooky_core::{Agent, AgentRole, DialogAnswer, Request};
use std::collections::{HashMap, HashSet};
pub(crate) use view::{FIELD, SEARCH, view};

/// The provider of a `provider/id` model name.
pub(crate) fn provider(model: &str) -> &str {
    model
        .split_once('/')
        .map_or("other", |(provider, _)| provider)
}

/// The model's own id, without its provider.
pub(crate) fn model_id(model: &str) -> &str {
    model.split_once('/').map_or(model, |(_, id)| id)
}

/// The agent program a new agent pane runs.
pub(crate) const BACKEND: &str = "omp";

/// The list of choices opened under the composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Menu {
    /// The models the agent offers.
    Model,
    /// The thinking levels the agent offers.
    Thinking,
    /// What the providers report about the remaining usage.
    Usage,
}

/// What the composer and its buttons tell the application.
#[derive(Clone, Debug)]
pub(crate) enum Message {
    /// The user edited, or moved in, a pane's composer.
    Edit(u64, text_editor::Action),
    /// Send a pane's draft.
    Send(u64),
    /// Send text to a pane's agent as a message (from the web view, which keeps its own
    /// draft).
    Prompt(u64, String),
    /// Interrupt a pane's current turn.
    Abort(u64),
    /// Copy text (an answer or a code block) to the clipboard.
    Copy(String),
    /// Open or close a list of choices of a pane.
    Menu(u64, Menu),
    /// Close the open list of choices.
    CloseMenu,
    /// The text typed in the model search box.
    MenuSearch(String),
    /// Show the models of one provider, or all of them.
    MenuProvider(Option<String>),
    /// Show the model roles.
    MenuRoles,
    /// Read the usage the providers report again.
    RefreshUsage(u64),
    /// Use a model role: its model, and its thinking level when it names one.
    SetRole(u64, String, Option<String>),
    /// Change a pane's model.
    SetModel(u64, String),
    /// Change a pane's thinking level.
    SetThinking(u64, String),
    /// Answer a dialog of a pane.
    Answer(u64, String, DialogAnswer),
    /// The text typed for an input dialog (pane, dialog).
    DialogText(u64, String, String),
    /// Fold or unfold a tool call or a reasoning entry (pane, entry).
    Toggle(u64, u64),
    /// A link in an answer was clicked.
    Link(String),
}

/// What a message asks the application to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// Requests for the server.
    pub requests: Vec<Request>,
    /// Put the keyboard focus back in the composer.
    pub focus_field: bool,
    /// Text to put on the clipboard.
    pub copy: Option<String>,
    /// Put the keyboard focus in the model search box.
    pub focus_search: bool,
    /// A link the user clicked, to be followed only under the hyperlink policy.
    pub link: Option<String>,
}

/// An entry's text and its parsed Markdown.
struct Rendered {
    text: String,
    content: markdown::Content,
}

/// The composers, folds and rendered answers of the agent panes.
#[derive(Default)]
pub(crate) struct Agents {
    composers: HashMap<u64, text_editor::Content>,
    rendered: HashMap<(u64, u64), Rendered>,
    /// Entries the user unfolded, by (pane, entry).
    unfolded: HashSet<(u64, u64)>,
    /// What is typed for input dialogs, by (pane, dialog).
    answers: HashMap<(u64, String), String>,
    /// The list of choices that is open, and in which pane.
    menu: Option<(u64, Menu)>,
    /// What is typed in the model search box.
    search: String,
    /// The provider whose models the open model list shows.
    provider: Option<String>,
    /// The open model list shows the model roles.
    roles: bool,
}

impl Agents {
    /// Carries out a message.
    pub(crate) fn update(&mut self, message: Message) -> Outcome {
        match message {
            Message::CloseMenu => {
                self.close_menu();
                Outcome::default()
            }
            Message::MenuSearch(text) => {
                // Searching looks through every model, not only the roles.
                self.roles = false;
                self.search = text;
                Outcome::default()
            }
            Message::MenuProvider(provider) => {
                self.provider = provider;
                self.roles = false;
                Outcome::default()
            }
            Message::RefreshUsage(pane) => Outcome {
                requests: vec![Request::AgentUsage { pane }],
                ..Outcome::default()
            },
            Message::MenuRoles => {
                self.provider = None;
                self.roles = true;
                Outcome::default()
            }
            Message::SetRole(pane, model, thinking) => {
                self.close_menu();
                let mut requests = vec![Request::AgentSetModel { pane, model }];
                if let Some(level) = thinking {
                    requests.push(Request::AgentSetThinking { pane, level });
                }
                Outcome {
                    requests,
                    focus_field: true,
                    ..Outcome::default()
                }
            }
            Message::Edit(pane, action) => {
                // Clicking in the composer is clicking outside an open list.
                if matches!(action, text_editor::Action::Click(_)) {
                    self.close_menu();
                }
                self.composers.entry(pane).or_default().perform(action);
                Outcome::default()
            }
            Message::Send(pane) => {
                let Some(composer) = self.composers.get_mut(&pane) else {
                    return Outcome::default();
                };
                let draft = composer.text();
                let text = draft.trim();
                // A draft of only blanks stays as it is.
                if text.is_empty() {
                    return Outcome::default();
                }
                let request = Request::AgentPrompt {
                    pane,
                    text: text.to_owned(),
                };
                *composer = text_editor::Content::new();
                Outcome {
                    requests: vec![request],
                    focus_field: true,
                    ..Outcome::default()
                }
            }
            Message::Prompt(pane, text) => {
                let text = text.trim();
                if text.is_empty() {
                    return Outcome::default();
                }
                Outcome {
                    requests: vec![Request::AgentPrompt {
                        pane,
                        text: text.to_owned(),
                    }],
                    ..Outcome::default()
                }
            }
            Message::Abort(pane) => Outcome {
                requests: vec![Request::AgentAbort { pane }],
                ..Outcome::default()
            },
            Message::Copy(text) => Outcome {
                copy: Some(text),
                ..Outcome::default()
            },
            Message::Menu(pane, kind) => {
                let closing = self.menu == Some((pane, kind));
                self.close_menu();
                if !closing {
                    self.menu = Some((pane, kind));
                }
                Outcome {
                    // The model list opens with its search box ready to type in, and the
                    // usage panel with a fresh reading.
                    focus_search: !closing && kind == Menu::Model,
                    requests: if !closing && kind == Menu::Usage {
                        vec![Request::AgentUsage { pane }]
                    } else {
                        Vec::new()
                    },
                    ..Outcome::default()
                }
            }
            Message::SetModel(pane, model) => {
                self.close_menu();
                Outcome {
                    requests: vec![Request::AgentSetModel { pane, model }],
                    focus_field: true,
                    ..Outcome::default()
                }
            }
            Message::SetThinking(pane, level) => {
                self.close_menu();
                Outcome {
                    requests: vec![Request::AgentSetThinking { pane, level }],
                    focus_field: true,
                    ..Outcome::default()
                }
            }
            Message::Answer(pane, dialog, answer) => {
                self.answers.remove(&(pane, dialog.clone()));
                Outcome {
                    requests: vec![Request::AgentAnswer {
                        pane,
                        dialog,
                        answer,
                    }],
                    focus_field: true,
                    ..Outcome::default()
                }
            }
            Message::DialogText(pane, dialog, text) => {
                self.answers.insert((pane, dialog), text);
                Outcome::default()
            }
            Message::Toggle(pane, entry) => {
                if !self.unfolded.remove(&(pane, entry)) {
                    self.unfolded.insert((pane, entry));
                }
                Outcome::default()
            }
            Message::Link(url) => Outcome {
                link: Some(url),
                ..Outcome::default()
            },
        }
    }

    /// Follows the server's snapshot: keeps a composer for each agent pane, parses the
    /// answers that changed, and forgets what belongs to panes and entries that are gone.
    pub(crate) fn sync<'a>(&mut self, agents: impl Iterator<Item = (u64, &'a Agent)>) {
        let mut panes = HashSet::new();
        let mut entries = HashSet::new();
        let mut dialogs = HashSet::new();
        for (pane, agent) in agents {
            panes.insert(pane);
            self.composers.entry(pane).or_default();
            for entry in &agent.messages {
                entries.insert((pane, entry.id));
                if matches!(entry.role, AgentRole::Assistant) {
                    self.parse(pane, entry.id, &entry.text);
                }
            }
            for dialog in &agent.dialogs {
                dialogs.insert((pane, dialog.id.clone()));
            }
        }
        self.composers.retain(|pane, _| panes.contains(pane));
        if self.menu.is_some_and(|(pane, _)| !panes.contains(&pane)) {
            self.close_menu();
        }
        self.rendered.retain(|key, _| entries.contains(key));
        self.unfolded.retain(|key| entries.contains(key));
        self.answers.retain(|key, _| dialogs.contains(key));
    }

    /// Brings the parsed Markdown of one answer up to date. A streaming answer only grows,
    /// so the new tail is appended; anything else is parsed again.
    fn parse(&mut self, pane: u64, entry: u64, text: &str) {
        match self.rendered.get_mut(&(pane, entry)) {
            Some(rendered) if rendered.text == text => {}
            Some(rendered) if text.starts_with(&rendered.text) => {
                rendered.content.push_str(&text[rendered.text.len()..]);
                text.clone_into(&mut rendered.text);
            }
            _ => {
                self.rendered.insert(
                    (pane, entry),
                    Rendered {
                        text: text.to_owned(),
                        content: markdown::Content::parse(text),
                    },
                );
            }
        }
    }

    /// The composer of a pane.
    pub(crate) fn composer(&self, pane: u64) -> Option<&text_editor::Content> {
        self.composers.get(&pane)
    }

    /// The text in a pane's composer.
    #[cfg(test)]
    pub(crate) fn draft(&self, pane: u64) -> String {
        self.composers
            .get(&pane)
            .map(text_editor::Content::text)
            .unwrap_or_default()
    }

    /// Closes the open list and forgets its search.
    fn close_menu(&mut self) {
        self.menu = None;
        self.search.clear();
        self.provider = None;
        self.roles = false;
    }

    /// Whether the open model list shows the model roles.
    pub(crate) fn shows_roles(&self) -> bool {
        self.roles
    }

    /// What is typed in the model search box.
    pub(crate) fn model_search(&self) -> &str {
        &self.search
    }

    /// The provider the model list is narrowed to.
    pub(crate) fn model_provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    /// The models that match the search, whatever their provider.
    pub(crate) fn matching<'a>(&self, models: &'a [String]) -> Vec<&'a String> {
        let needle = self.search.trim().to_lowercase();
        models
            .iter()
            .filter(|model| needle.is_empty() || model.to_lowercase().contains(&needle))
            .collect()
    }

    /// The models to list: those that match the search, of the chosen provider.
    pub(crate) fn shown<'a>(&self, models: &'a [String]) -> Vec<&'a String> {
        let mut shown = self.matching(models);
        if let Some(chosen) = &self.provider {
            shown.retain(|model| provider(model) == chosen);
        }
        shown
    }

    /// The list of choices open in a pane.
    pub(crate) fn menu(&self, pane: u64) -> Option<Menu> {
        self.menu
            .filter(|(open, _)| *open == pane)
            .map(|(_, kind)| kind)
    }

    /// The parsed Markdown of an answer.
    pub(crate) fn markdown(&self, pane: u64, entry: u64) -> Option<&markdown::Content> {
        self.rendered.get(&(pane, entry)).map(|r| &r.content)
    }

    /// Whether the user unfolded an entry.
    pub(crate) fn is_unfolded(&self, pane: u64, entry: u64) -> bool {
        self.unfolded.contains(&(pane, entry))
    }

    /// What is typed for an input dialog.
    pub(crate) fn answer_text(&self, pane: u64, dialog: &str) -> &str {
        self.answers
            .get(&(pane, dialog.to_owned()))
            .map_or("", String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spooky_core::{AgentDialog, AgentMessage, DialogKind};

    fn type_into(agents: &mut Agents, pane: u64, text: &str) {
        for c in text.chars() {
            let key = if c == '\n' {
                text_editor::Edit::Enter
            } else {
                text_editor::Edit::Insert(c)
            };
            agents.update(Message::Edit(pane, text_editor::Action::Edit(key)));
        }
    }

    fn answer(id: u64, text: &str) -> AgentMessage {
        AgentMessage {
            id,
            role: AgentRole::Assistant,
            text: text.into(),
            tool: None,
        }
    }

    fn agent(messages: Vec<AgentMessage>) -> Agent {
        Agent {
            messages,
            ..Agent::default()
        }
    }

    #[test]
    fn sending_takes_the_trimmed_multiline_draft_and_clears_it() {
        let mut agents = Agents::default();
        type_into(&mut agents, 7, "  fix the bug\nthen test it \n");
        let outcome = agents.update(Message::Send(7));
        assert_eq!(
            outcome.requests,
            vec![Request::AgentPrompt {
                pane: 7,
                text: "fix the bug\nthen test it".into()
            }]
        );
        assert!(outcome.focus_field);
        assert_eq!(agents.draft(7), "");
    }

    #[test]
    fn a_blank_draft_sends_nothing_and_each_pane_keeps_its_own() {
        let mut agents = Agents::default();
        type_into(&mut agents, 1, "   ");
        type_into(&mut agents, 2, "other");
        assert_eq!(agents.update(Message::Send(1)), Outcome::default());
        assert_eq!(agents.draft(1), "   ");
        assert_eq!(agents.draft(2), "other");
        assert_eq!(agents.update(Message::Send(9)), Outcome::default());
    }

    #[test]
    fn what_belongs_to_closed_panes_and_gone_entries_is_forgotten() {
        let mut agents = Agents::default();
        let both = agent(vec![answer(1, "a"), answer(2, "b")]);
        let one = agent(vec![answer(2, "b")]);
        type_into(&mut agents, 1, "draft");
        agents.sync([(1, &both), (2, &both)].into_iter());
        agents.update(Message::Toggle(1, 1));
        agents.update(Message::Toggle(2, 2));
        assert!(agents.markdown(1, 1).is_some() && agents.is_unfolded(1, 1));

        agents.sync([(2, &one)].into_iter());
        assert!(agents.composer(1).is_none() && agents.markdown(1, 1).is_none());
        assert!(agents.markdown(2, 1).is_none(), "entry 1 was dropped");
        assert!(agents.markdown(2, 2).is_some());
        assert!(!agents.is_unfolded(1, 1) && agents.is_unfolded(2, 2));
        assert_eq!(agents.draft(1), "");
    }

    #[test]
    fn a_growing_answer_is_parsed_incrementally_and_a_rewritten_one_again() {
        let mut agents = Agents::default();
        agents.sync([(1, &agent(vec![answer(1, "# Title\n\nfirst")]))].into_iter());
        let before = agents.markdown(1, 1).map_or(0, |c| c.items().len());
        agents.sync([(1, &agent(vec![answer(1, "# Title\n\nfirst\n\nsecond")]))].into_iter());
        let grown = agents.markdown(1, 1).map_or(0, |c| c.items().len());
        assert_eq!((before, grown), (2, 3));
        agents.sync([(1, &agent(vec![answer(1, "other")]))].into_iter());
        assert_eq!(agents.markdown(1, 1).map_or(0, |c| c.items().len()), 1);
    }

    #[test]
    fn a_dialog_answer_goes_to_the_server_and_drops_the_typed_text() {
        let mut agents = Agents::default();
        let asking = Agent {
            dialogs: vec![AgentDialog {
                id: "d1".into(),
                kind: DialogKind::Input,
                title: "Name?".into(),
                message: String::new(),
                options: Vec::new(),
            }],
            ..Agent::default()
        };
        agents.sync([(3, &asking)].into_iter());
        agents.update(Message::DialogText(3, "d1".into(), "Ada".into()));
        assert_eq!(agents.answer_text(3, "d1"), "Ada");
        let outcome = agents.update(Message::Answer(
            3,
            "d1".into(),
            DialogAnswer::Value("Ada".into()),
        ));
        assert_eq!(
            outcome.requests,
            vec![Request::AgentAnswer {
                pane: 3,
                dialog: "d1".into(),
                answer: DialogAnswer::Value("Ada".into())
            }]
        );
        assert_eq!(agents.answer_text(3, "d1"), "");
    }

    #[test]
    fn a_list_of_choices_opens_toggles_and_closes_on_a_choice() {
        let mut agents = Agents::default();
        agents.update(Message::Menu(1, Menu::Model));
        assert_eq!(agents.menu(1), Some(Menu::Model));
        assert_eq!(agents.menu(2), None);
        agents.update(Message::Menu(1, Menu::Thinking));
        assert_eq!(agents.menu(1), Some(Menu::Thinking));
        agents.update(Message::Menu(1, Menu::Thinking));
        assert_eq!(agents.menu(1), None);
        agents.update(Message::Menu(1, Menu::Model));
        agents.update(Message::CloseMenu);
        assert_eq!(agents.menu(1), None);
        agents.update(Message::Menu(1, Menu::Model));
        agents.update(Message::SetModel(1, "a/b".into()));
        assert_eq!(agents.menu(1), None);
        agents.update(Message::Menu(3, Menu::Model));
        agents.sync(std::iter::empty());
        assert_eq!(agents.menu(3), None, "its pane is gone");
    }

    #[test]
    fn a_model_name_splits_at_its_first_slash_only() {
        assert_eq!(
            provider("amazon-bedrock/anthropic.claude-3"),
            "amazon-bedrock"
        );
        assert_eq!(model_id("openai/gpt-5/mini"), "gpt-5/mini");
        assert_eq!((provider("local"), model_id("local")), ("other", "local"));
    }

    #[test]
    fn the_model_list_narrows_by_search_and_provider_and_resets_when_closed() {
        let models: Vec<String> = [
            "openai/gpt-5",
            "openai/o3",
            "anthropic/Claude-5",
            "other-co/gpt-mini",
        ]
        .map(String::from)
        .to_vec();
        let mut agents = Agents::default();
        let opened = agents.update(Message::Menu(1, Menu::Model));
        assert!(opened.focus_search, "the box is ready to type in");
        assert_eq!(agents.shown(&models).len(), 4);

        agents.update(Message::MenuSearch(" GPT ".into()));
        let found: Vec<&str> = agents
            .matching(&models)
            .iter()
            .map(|m| m.as_str())
            .collect();
        assert_eq!(
            found,
            ["openai/gpt-5", "other-co/gpt-mini"],
            "case-blind, whole name"
        );
        agents.update(Message::MenuProvider(Some("openai".into())));
        let shown: Vec<&str> = agents.shown(&models).iter().map(|m| m.as_str()).collect();
        assert_eq!(shown, ["openai/gpt-5"]);
        assert_eq!(
            agents.matching(&models).len(),
            2,
            "providers count what the search finds"
        );

        agents.update(Message::CloseMenu);
        assert_eq!((agents.model_search(), agents.model_provider()), ("", None));
        agents.update(Message::Menu(1, Menu::Model));
        agents.update(Message::MenuSearch("x".into()));
        let closed = agents.update(Message::Menu(1, Menu::Model));
        assert!(!closed.focus_search);
        assert_eq!(
            agents.model_search(),
            "",
            "closing by the button forgets it too"
        );
    }

    #[test]
    fn choosing_a_role_sets_its_model_then_its_level_and_closes_the_list() {
        let mut agents = Agents::default();
        agents.update(Message::Menu(1, Menu::Model));
        agents.update(Message::MenuRoles);
        assert!(agents.shows_roles());
        agents.update(Message::MenuProvider(Some("a".into())));
        assert!(!agents.shows_roles(), "a provider replaces the roles");
        agents.update(Message::MenuRoles);
        let outcome = agents.update(Message::SetRole(1, "p/m".into(), Some("high".into())));
        assert_eq!(
            outcome.requests,
            vec![
                Request::AgentSetModel {
                    pane: 1,
                    model: "p/m".into()
                },
                Request::AgentSetThinking {
                    pane: 1,
                    level: "high".into()
                },
            ]
        );
        assert_eq!(agents.menu(1), None);
        assert!(!agents.shows_roles());
        let plain = agents.update(Message::SetRole(1, "p/m".into(), None));
        assert_eq!(plain.requests.len(), 1, "no level asked, none set");
    }

    #[test]
    fn the_usage_panel_asks_for_a_reading_when_it_opens_and_on_refresh_only() {
        let mut agents = Agents::default();
        let opened = agents.update(Message::Menu(2, Menu::Usage));
        assert_eq!(opened.requests, vec![Request::AgentUsage { pane: 2 }]);
        assert!(!opened.focus_search);
        assert_eq!(agents.menu(2), Some(Menu::Usage));
        let again = agents.update(Message::RefreshUsage(2));
        assert_eq!(again.requests, vec![Request::AgentUsage { pane: 2 }]);
        assert_eq!(
            agents.menu(2),
            Some(Menu::Usage),
            "refreshing keeps it open"
        );
        let closed = agents.update(Message::Menu(2, Menu::Usage));
        assert!(closed.requests.is_empty());
        assert_eq!(agents.menu(2), None);
    }

    #[test]
    fn controls_ask_the_server_and_a_link_is_only_reported() {
        let mut agents = Agents::default();
        assert_eq!(
            agents.update(Message::Abort(4)).requests,
            vec![Request::AgentAbort { pane: 4 }]
        );
        assert_eq!(
            agents.update(Message::SetModel(4, "a/b".into())).requests,
            vec![Request::AgentSetModel {
                pane: 4,
                model: "a/b".into()
            }]
        );
        let outcome = agents.update(Message::Link("https://example.com".into()));
        assert!(outcome.requests.is_empty());
        assert_eq!(outcome.link.as_deref(), Some("https://example.com"));
    }
}
