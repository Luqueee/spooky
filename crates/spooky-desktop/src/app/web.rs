//! The agent pane shown by a web page instead of the native view (macOS).
//!
//! The page is a native child view of the window, so it sits over everything iced draws:
//! it is shown only while nothing of iced's own is open over the pane, and it takes the
//! place of the pane's area, which the native view measures and reports. Only a pane that
//! fills its tab is shown this way; a split keeps the native agent view.

use super::{App, Message};
use crate::{features::agent::web, platform::webview, ui::BACKGROUND};
use iced::{
    Element, Length, Rectangle, Task, mouse,
    widget::{canvas, canvas::Geometry},
    window,
};
use spooky_core::{Layout, MAX_AGENT_PROMPT_BYTES};

/// Where the page is in its life.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    /// Not asked for yet.
    #[default]
    Idle,
    /// Being created.
    Creating,
    /// Created: it can be shown and given state.
    Ready,
    /// Could not be created; the native view is used.
    Failed,
}

/// What the application knows of the page.
#[derive(Default)]
pub(super) struct WebHost {
    stage: Stage,
    /// The pane's area as last measured.
    area: Option<Rectangle>,
    /// Where the page was last placed.
    placed: Option<Rectangle>,
    /// The pane the page is showing, when it is on screen.
    pane: Option<u64>,
    /// The state last given to the page; cleared to give it again.
    pushed: String,
}

/// The measured stand-in for the page in the native layout: it fills the pane's area,
/// reports where that is, and paints the page's background until the page covers it.
struct Area;

impl canvas::Program<Message> for Area {
    type State = Option<Rectangle>;

    fn update(
        &self,
        state: &mut Self::State,
        _: &canvas::Event,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if *state == Some(bounds) {
            return None;
        }
        *state = Some(bounds);
        Some(canvas::Action::publish(Message::WebArea(bounds)))
    }

    fn draw(
        &self,
        _: &Self::State,
        renderer: &iced::Renderer,
        _: &iced::Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        frame.fill_rectangle(iced::Point::ORIGIN, bounds.size(), BACKGROUND);
        vec![frame.into_geometry()]
    }
}

pub(super) fn area<'a>() -> Element<'a, Message> {
    canvas(Area).width(Length::Fill).height(Length::Fill).into()
}

impl App {
    /// The agent pane the page should show: the selected pane when it is an agent that
    /// fills its tab, and the page works.
    pub(super) fn web_target(&self) -> Option<u64> {
        if !webview::supported() || self.web.stage == Stage::Failed || self.hidden {
            return None;
        }
        let pane = self.current_pane().filter(|pane| pane.agent.is_some())?;
        let (_, tab) = self.active()?;
        matches!(tab.layout, Layout::Pane(id) if id == pane.id).then_some(pane.id)
    }

    /// Something iced draws over the content area is open, and the page would cover it.
    fn overlay_open(&self) -> bool {
        self.menu_open
            || self.context_menu.is_some()
            || self.confirm.is_some()
            || self.pending.is_some()
            || self.settings.is_open()
            || self.palette.is_open()
            || self.tailscale.is_open()
            || self.renaming
            || self.attachments.dialog_open()
    }

    /// Brings the page in line with the application: creates it when first needed, shows
    /// it over the agent pane that is on screen, hides it otherwise, and gives it the
    /// agent's current state.
    pub(super) fn sync_web(&mut self) -> Task<Message> {
        let target = self.web_target();
        if self.web.stage == Stage::Idle && target.is_some() {
            let Some(id) = self.window else {
                return Task::none();
            };
            self.web.stage = Stage::Creating;
            return window::run(id, |handle| webview::create(handle)).map(Message::WebCreated);
        }
        if self.web.stage != Stage::Ready {
            return Task::none();
        }
        let shown = target.filter(|_| !self.overlay_open());
        let (Some(pane), Some(area)) = (shown, self.web.area) else {
            if self.web.pane.take().is_some() {
                webview::show(false);
            }
            return Task::none();
        };
        if self.web.placed != Some(area) {
            webview::place(area);
            self.web.placed = Some(area);
        }
        let Some(agent) = self.current_pane().and_then(|p| p.agent.as_ref()) else {
            return Task::none();
        };
        let state = web::state(pane, agent);
        if state != self.web.pushed {
            webview::push(&state);
            self.web.pushed = state;
        }
        if self.web.pane != Some(pane) {
            self.web.pane = Some(pane);
            webview::show(true);
            webview::focus();
            webview::focus_composer();
        }
        Task::none()
    }

    pub(super) fn web_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::WebCreated(Ok(())) => self.web.stage = Stage::Ready,
            Message::WebCreated(Err(error)) => {
                self.web.stage = Stage::Failed;
                self.status = error;
            }
            Message::WebArea(area) => self.web.area = Some(area),
            Message::WebEvent(text) => return self.web_event(&text),
            // Only a page that is on screen is given the clipboard.
            Message::WebPasted(Some(text))
                if self.web.pane.is_some() && text.len() <= MAX_AGENT_PROMPT_BYTES =>
            {
                webview::paste(&text);
            }
            _ => {}
        }
        Task::none()
    }

    /// Carries out what the page asked for, if it is allowed.
    fn web_event(&mut self, text: &str) -> Task<Message> {
        match web::event(text) {
            Err(error) => self.status = error,
            Ok(web::Event::Ready) => {
                // A page that has just loaded knows nothing yet.
                self.web.pushed.clear();
                if self.web.pane.is_some() {
                    webview::focus();
                    webview::focus_composer();
                }
            }
            Ok(web::Event::Agent { pane, message }) => {
                // Only the pane on screen can be acted on; a late message about another
                // one is ignored.
                if self.web.pane == Some(pane) {
                    return self.agent_message(message);
                }
            }
            Ok(web::Event::Paste) => {
                return iced::clipboard::read().map(Message::WebPasted);
            }
            Ok(web::Event::Copy(text)) => {
                self.status = "Copied".into();
                return iced::clipboard::write(text);
            }
            Ok(web::Event::Link { url, open }) => {
                // A link is the agent's text: it opens only on the open modifier.
                self.status = if open {
                    self.hyperlinks.follow(&url)
                } else {
                    format!("Link to {url}: hold the shortcut key and click to open it")
                };
            }
            Ok(web::Event::Command(command)) => return self.run_command(command),
            Ok(web::Event::Ignored) => {}
        }
        Task::none()
    }
}
