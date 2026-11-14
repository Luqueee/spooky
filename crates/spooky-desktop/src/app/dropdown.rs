use super::{App, Message};
use crate::config::SizeMode;
use crate::features::settings::{Change, Effect};
use crate::features::window::{FIRST_DELAY, Slide, dropdown_origin, hides_on_blur, usable_area};
use crate::platform::{fade, kde};
use iced::{Size, Task, window};
use std::time::{Duration, Instant};

/// How long without a resize event ends a drag of the window's edge.
const RESIZE_SETTLE: Duration = Duration::from_millis(400);

/// The setting that holds a window height of `height`, in the unit the configuration
/// uses: pixels as they are, or a share of the display (`None` when it is not known).
fn height_change(mode: SizeMode, height: f32, monitor: Option<Size>) -> Option<Change> {
    match (mode, monitor) {
        (SizeMode::Pixels, _) => Some(Change::HeightPixels(height.round())),
        (SizeMode::Percent, Some(display)) if display.height > 0.0 => Some(Change::HeightPercent(
            (height / display.height * 1000.0).round() / 10.0,
        )),
        (SizeMode::Percent, _) => None,
    }
}

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
                        #[cfg(target_os = "macos")]
                        if self.hidden {
                            self.slide = Some(Slide::open(Instant::now(), self.slide_share(1.0)));
                            self.hidden = false;
                            return Task::batch([
                                window::set_mode(id, window::Mode::Windowed)
                                    .chain(window::minimize(id, false))
                                    .chain(window::gain_focus(id)),
                                self.reveal(id),
                            ]);
                        }
                        if self.hidden && self.slide_running() {
                            // Reverse immediately, without waiting for the window measurements.
                            self.slide = Some(Slide::open(Instant::now(), self.slide_share(1.0)));
                            self.hidden = false;
                        }
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
            Message::ResizeBottom => {
                if let Some(id) = self.window {
                    self.resizing = true;
                    self.resize_from = self.window_size.map_or(0.0, |size| size.height);
                    return window::drag_resize(id, window::Direction::South);
                }
            }
            Message::WindowResized(size) => {
                self.window_size = Some(size);
                if self.resizing {
                    self.resize_gen += 1;
                    let generation = self.resize_gen;
                    // The drag has ended when no resize follows for a moment.
                    return Task::perform(
                        async move {
                            tokio::time::sleep(RESIZE_SETTLE).await;
                            generation
                        },
                        Message::ResizeSettled,
                    );
                }
            }
            Message::ResizeSettled(generation) => {
                if generation == self.resize_gen
                    && self.resizing
                    && let Some(id) = self.window
                {
                    self.resizing = false;
                    let height = self.window_size.map_or(0.0, |size| size.height);
                    // A press that moved nothing leaves the height as it was.
                    if (height - self.resize_from).abs() >= 1.0 {
                        return window::monitor_size(id)
                            .map(move |monitor| Message::ResizedTo(height, monitor));
                    }
                }
            }
            Message::ResizedTo(height, monitor) => return self.keep_height(height, monitor),
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
                self.resizing = false;
                // Without waiting for the focus event, so a quick second press hides it.
                if kde::active() {
                    self.focused = true;
                }
                if !self.settings.is_shown() {
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
    /// Makes the height the window was dragged to the configured one, in the unit the
    /// configuration already uses. The window is already that size, so the change does
    /// not resize it again.
    fn keep_height(&mut self, height: f32, monitor: Option<Size>) -> Task<Message> {
        let Some(change) = height_change(self.config.size_mode(), height, monitor) else {
            self.status = "The height was not saved: the display's size is unknown".into();
            return Task::none();
        };
        let mut effects = self
            .settings
            .update(change, &mut self.config, &mut self.hotkeys);
        effects.retain(|effect| *effect != Effect::Resize);
        self.apply_effects(effects)
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
        self.resizing = false;
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

#[cfg(test)]
mod height_tests {
    use super::*;

    #[test]
    fn a_dragged_height_is_kept_in_the_unit_the_configuration_uses() {
        let display = Some(Size::new(1920.0, 1080.0));
        assert!(matches!(
            height_change(SizeMode::Pixels, 640.4, None),
            Some(Change::HeightPixels(640.0))
        ));
        let Some(Change::HeightPercent(percent)) = height_change(SizeMode::Percent, 540.0, display)
        else {
            panic!("a share of the display");
        };
        assert_eq!(percent, 50.0);
        assert!(
            height_change(SizeMode::Percent, 540.0, None).is_none(),
            "a share of an unknown display cannot be worked out"
        );
    }
}
