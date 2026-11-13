use super::{App, Message};
use crate::features::window::{FIRST_DELAY, Slide, dropdown_origin, hides_on_blur, usable_area};
use crate::platform::{fade, kde};
use iced::{Task, window};
use std::time::{Duration, Instant};

impl App {
    pub(super) fn window_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Hotkey(key) => {
                if self.hotkeys.as_ref().is_some_and(|keys| keys.id() == key)
                    && let Some(id) = self.window
                {
                    // The shortcut hides a window that is on screen and focused. A
                    // window that is hidden, minimized or behind another app is
                    // brought forward instead, so its real state decides, not a toggle.
                    return if !self.hidden && self.focused {
                        self.hide()
                    } else {
                        self.reveal(id)
                    };
                }
            }
            Message::WindowFocus(focused) => {
                self.focused = focused;
                if !focused
                    && hides_on_blur(
                        self.config.on_blur,
                        self.hidden,
                        self.attachments.dialog_open(),
                        self.hotkeys.is_some(),
                    )
                {
                    return self.hide();
                }
            }
            Message::SlideFrame(now) => {
                let Some(slide) = &mut self.slide else {
                    return Task::none();
                };
                slide.advance(now);
                fade::set(slide.opacity());
                if slide.finished_open() {
                    self.slide = None;
                    fade::set(1.0);
                } else if slide.closed()
                    && !kde::active()
                    && let Some(id) = self.window
                {
                    // Where the window can be hidden, it is once the content is out of sight.
                    return window::set_mode(id, window::Mode::Hidden);
                }
            }
            Message::Present(id, monitor, size) => {
                // A window that was out of sight slides in, and so does the first one (on
                // KDE, which only knows it a moment after it is shown, after a delay); one
                // already shown is only placed.
                let first = !std::mem::replace(&mut self.presented, true);
                if (self.hidden || first) && self.hotkeys.is_some() {
                    let delay = if first && kde::active() {
                        FIRST_DELAY
                    } else {
                        Duration::ZERO
                    };
                    self.slide = Some(Slide::open(Instant::now() + delay, self.slide_share(1.0)));
                }
                self.hidden = false;
                // Without waiting for the focus event, so a quick second press hides it.
                if kde::active() {
                    self.focused = true;
                }
                if !self.settings.is_open() {
                    self.terminal_focused = true;
                }
                // The configured size is applied the first time the display is known
                // and after a setting changes it; a later reveal only re-centres, so a
                // window the user resized keeps its size.
                // On KDE the display comes from KWin, which resolves the configured size
                // itself; elsewhere the measured display does.
                #[cfg(target_os = "linux")]
                let kde_size = kde::active() && std::mem::take(&mut self.size_pending);
                let target = match monitor {
                    Some(monitor) if self.size_pending => {
                        self.size_pending = false;
                        self.config.window_size(usable_area(monitor))
                    }
                    _ => size,
                };
                let mut tasks = vec![
                    window::set_mode(id, window::Mode::Windowed),
                    window::minimize(id, false),
                    window::gain_focus(id),
                ];
                if target != size {
                    tasks.insert(0, window::resize(id, target));
                }
                if let Some(at) = dropdown_origin(monitor, target) {
                    tasks.insert(0, window::move_to(id, at));
                }
                // Wayland ignores the placement above; KWin does it there.
                #[cfg(target_os = "linux")]
                if kde::active() {
                    let (width, height) = if kde_size {
                        self.config.kde_dims()
                    } else {
                        (kde::Dim::Keep, kde::Dim::Keep)
                    };
                    kde::send(kde::Action::Reveal {
                        width,
                        height,
                        above: self.config.on_blur == crate::config::OnBlur::Top,
                    });
                }
                return Task::batch(tasks);
            }
            Message::Opened(id) => {
                self.window = Some(id);
                if self.hotkeys.is_some() {
                    #[cfg(target_os = "macos")]
                    {
                        crate::platform::macos::app::show_windows_instantly();
                        crate::platform::macos::app::set_dock_icon(self.config.dock_icon);
                    }
                    return Task::batch([
                        // The fade needs the native window.
                        window::run(id, fade::attach).map(|()| Message::Tick),
                        self.reveal(id),
                    ]);
                }
            }
            // The close button hides the window while the shortcut can bring it back;
            // without one, hiding would strand the app, so closing quits. Sessions
            // run on the server either way.
            Message::Close(id) => {
                return if self.hotkeys.is_some() {
                    self.hide()
                } else {
                    window::close(id)
                };
            }
            Message::DragWindow => {
                if let Some(id) = self.window {
                    return window::drag(id);
                }
            }
            _ => {}
        }
        Task::none()
    }
    /// Where the content is now, or `idle` when no slide is in progress.
    fn slide_share(&self, idle: f32) -> f32 {
        self.slide.map_or(idle, |slide| slide.hidden_share())
    }
    /// Whether the slide needs frames.
    pub(super) fn slide_running(&self) -> bool {
        self.slide.is_some_and(|slide| slide.running())
    }
    /// Takes the window off screen. Only offered while the global shortcut can bring it back.
    pub(super) fn hide(&mut self) -> Task<Message> {
        self.hidden = true;
        self.focused = false;
        // The content slides out. Wayland cannot hide a window, so KWin parks it; elsewhere
        // it is hidden when the slide ends.
        self.slide = Some(Slide::close(Instant::now(), self.slide_share(0.0)));
        #[cfg(target_os = "linux")]
        if kde::active() {
            kde::send(kde::Action::Hide);
        }
        Task::none()
    }
    /// Measures the display and window, then shows the window via [`Message::Present`].
    pub(super) fn reveal(&self, id: window::Id) -> Task<Message> {
        window::monitor_size(id).then(move |monitor| {
            window::size(id).map(move |size| Message::Present(id, monitor, size))
        })
    }
}
