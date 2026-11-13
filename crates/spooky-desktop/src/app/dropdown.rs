use super::{App, Message};
use crate::{
    config::OnBlur,
    features::window::{dropdown_origin, hides_on_blur, usable_area},
    platform::kde,
};
use iced::{Task, window};

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
                        self.hide(id)
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
                    && let Some(id) = self.window
                {
                    return self.hide(id);
                }
            }
            Message::Present(id, monitor, size) => {
                self.hidden = false;
                if !self.settings.is_open() {
                    self.terminal_focused = true;
                }
                // The configured size is applied the first time the display is known
                // and after a setting changes it; a later reveal only re-centres, so a
                // window the user resized keeps its size.
                // On KDE the display comes from KWin, which resolves the configured size
                // itself; elsewhere the measured display does.
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
                if kde::active() {
                    let (width, height) = if kde_size {
                        self.config.kde_dims()
                    } else {
                        (kde::Dim::Keep, kde::Dim::Keep)
                    };
                    kde::send(kde::Action::Reveal {
                        width,
                        height,
                        above: self.config.on_blur == OnBlur::Top,
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
                    return self.reveal(id);
                }
            }
            // The close button hides the window while the shortcut can bring it back;
            // without one, hiding would strand the app, so closing quits. Sessions
            // run on the server either way.
            Message::Close(id) => {
                return if self.hotkeys.is_some() {
                    self.hide(id)
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
    /// Takes the window off screen. Only offered while the global shortcut can bring it back.
    pub(super) fn hide(&mut self, id: window::Id) -> Task<Message> {
        self.hidden = true;
        self.focused = false;
        if kde::active() {
            // Wayland cannot hide a window; KWin minimizes it.
            kde::send(kde::Action::Hide);
            return Task::none();
        }
        window::set_mode(id, window::Mode::Hidden)
    }
    /// Measures the display and window, then shows the window via [`Message::Present`].
    pub(super) fn reveal(&self, id: window::Id) -> Task<Message> {
        window::monitor_size(id).then(move |monitor| {
            window::size(id).map(move |size| Message::Present(id, monitor, size))
        })
    }
}
