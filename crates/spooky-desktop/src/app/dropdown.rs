use super::{App, Message};
use crate::config::{OnBlur, SizeMode};
use crate::features::settings::{Change, Effect};
use crate::features::window::{FIRST_DELAY, Slide, dropdown_origin, hides_on_blur, usable_area};
use crate::geometry::{Point, Rectangle, Size};
#[cfg(target_os = "linux")]
use crate::platform::kwin;
use crate::platform::{WindowControl, window, window_control};
use crate::runtime::{Task, WindowOp};
use std::time::{Duration, Instant};

/// Whether KWin places, hides and raises the window.
fn kwin_controls_window() -> bool {
    window_control() == WindowControl::Kwin
}

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

/// A topmost window can be visible without owning focus. If its minimized state
/// is unavailable, retain the conservative focus-based behavior.
fn hotkey_hides(hidden: bool, focused: bool, on_blur: OnBlur, minimized: Option<bool>) -> bool {
    !hidden && (focused || (on_blur == OnBlur::Top && minimized == Some(false)))
}

impl App {
    pub(super) fn window_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Hotkey(key) => {
                if self.hotkeys.as_ref().is_some_and(|keys| keys.id() == key) && self.opened {
                    if !self.dropdown() {
                        // The desktop does not let the window be hidden, so the shortcut
                        // only brings it forward.
                        return Task::batch([
                            Task::window(WindowOp::Minimize(false)),
                            Task::window(WindowOp::Raise),
                        ]);
                    }
                    if let Some(pending) = &mut self.hotkey_pending {
                        *pending = !*pending;
                        return Task::none();
                    }
                    if !self.hidden && !self.focused && self.config.on_blur == OnBlur::Top {
                        self.hotkey_generation = self.hotkey_generation.wrapping_add(1);
                        self.hotkey_pending = Some(true);
                        let generation = self.hotkey_generation;
                        return Task::minimized(move |minimized| {
                            Task::done(Message::HotkeyMinimized(generation, minimized))
                        });
                    }
                    return self.toggle_hotkey(None);
                }
            }
            Message::HotkeyMinimized(generation, minimized) => {
                if self.opened
                    && self.hotkey_generation == generation
                    && let Some(pending) = self.hotkey_pending.take()
                    && pending
                {
                    #[cfg(target_os = "macos")]
                    let minimized = match crate::platform::macos::app::dropdown_visible() {
                        Some(true) => minimized,
                        Some(false) => Some(true),
                        None => None,
                    };
                    return self.toggle_hotkey(minimized);
                }
            }
            Message::WindowFocus(focused) => {
                self.focused = focused;
                if !focused
                    && hides_on_blur(
                        self.config.on_blur,
                        self.hidden,
                        self.attachments.dialog_open(),
                        self.dropdown(),
                    )
                {
                    return self.hide();
                }
            }
            Message::ResizeBottom => {
                if self.opened {
                    self.resizing = true;
                    self.resize_from = self.window_size.map_or(0.0, |size| size.height);
                    return Task::window(WindowOp::DragResizeBottom);
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
                if generation == self.resize_gen && self.resizing && self.opened {
                    self.resizing = false;
                    let height = self.window_size.map_or(0.0, |size| size.height);
                    // A press that moved nothing leaves the height as it was.
                    if (height - self.resize_from).abs() >= 1.0 {
                        return Task::monitor_size(move |monitor| {
                            Task::done(Message::ResizedTo(height, monitor))
                        });
                    }
                }
            }
            Message::ResizedTo(height, monitor) => return self.keep_height(height, monitor),
            Message::SlideFrame(now) => {
                let Some(slide) = &mut self.slide else {
                    return Task::none();
                };
                slide.advance(now);
                // The view draws the content at the slide's offset; the window fades.
                window::set_opacity(slide.opacity());
                if slide.finished_open() {
                    self.slide = None;
                    window::set_opacity(1.0);
                } else if slide.closed() && !kwin_controls_window() && self.opened {
                    // Where the window can be hidden, it is once the content is out of sight.
                    return Task::window(WindowOp::Hide);
                }
            }
            Message::Present(generation, monitor, size, pointer_display) => {
                if generation != self.reveal_generation {
                    return Task::none();
                }
                // A window that was out of sight slides in, and so does the first one (on
                // KDE, which only knows it a moment after it is shown, after a delay); one
                // already shown is only placed.
                let first = !std::mem::replace(&mut self.presented, true);
                if (self.hidden || first) && self.dropdown() {
                    let delay = if first && kwin_controls_window() {
                        FIRST_DELAY
                    } else {
                        Duration::ZERO
                    };
                    self.slide = Some(Slide::open(Instant::now() + delay, self.slide_share(1.0)));
                }
                self.hidden = false;
                self.resizing = false;
                // Without waiting for the focus event, so a quick second press hides it.
                if kwin_controls_window() {
                    self.focused = true;
                }
                if !self.settings.is_shown() {
                    self.terminal_focused = true;
                }
                // Percentage dimensions follow the selected display. Pixel dimensions
                // remain fixed, and an edge drag has already updated the configuration.
                let bounds = pointer_display
                    .map(|(bounds, _)| bounds)
                    .or_else(|| monitor.map(|size| Rectangle::new(Point::ORIGIN, size)));
                let display_changed = pointer_display
                    .is_some_and(|(bounds, _)| self.dropdown_display != Some(bounds));
                if let Some((bounds, _)) = pointer_display {
                    self.dropdown_display = Some(bounds);
                }
                #[cfg(target_os = "linux")]
                let kwin_size = kwin_controls_window() && std::mem::take(&mut self.size_pending);
                let needs_size = self.size_pending
                    || (display_changed && self.config.size_mode() == SizeMode::Percent);
                let target = if needs_size {
                    let available = pointer_display
                        .map(|(_, usable)| usable)
                        .or_else(|| monitor.map(usable_area));
                    if let Some(available) = available {
                        self.size_pending = false;
                        self.config.window_size(available)
                    } else {
                        size
                    }
                } else {
                    size
                };
                #[cfg(target_os = "macos")]
                if pointer_display.is_some()
                    && let Some(at) = dropdown_origin(bounds, target)
                {
                    if crate::platform::macos::app::place_dropdown(at, target) {
                        return Task::window(WindowOp::Show)
                            .chain(Task::window(WindowOp::Minimize(false)))
                            .chain(Task::window(WindowOp::Raise));
                    }
                    self.status = "Could not position the window on the selected display".into();
                }
                let mut tasks = vec![
                    Task::window(WindowOp::Show),
                    Task::window(WindowOp::Minimize(false)),
                    Task::window(WindowOp::Raise),
                ];
                if target != size {
                    tasks.insert(0, Task::window(WindowOp::Resize(target)));
                }
                if let Some(at) = dropdown_origin(bounds, target) {
                    tasks.insert(0, Task::window(WindowOp::MoveTo(at)));
                }
                // Wayland ignores the placement above; KWin does it there.
                #[cfg(target_os = "linux")]
                if kwin_controls_window() {
                    let (width, height) = if kwin_size {
                        self.config.kwin_dims()
                    } else {
                        (kwin::Dim::Keep, kwin::Dim::Keep)
                    };
                    kwin::send(kwin::Action::Reveal {
                        width,
                        height,
                        above: self.config.on_blur == crate::config::OnBlur::Top,
                    });
                }
                return Task::batch(tasks);
            }
            Message::Opened => {
                if self.opened {
                    return Task::none();
                }
                self.opened = true;
                #[cfg(target_os = "macos")]
                {
                    self.sync_background();
                    if !crate::platform::macos::app::set_dock_artwork() {
                        self.status = "Dock icon unavailable".into();
                    }
                }
                if self.dropdown() {
                    #[cfg(target_os = "macos")]
                    {
                        crate::platform::macos::app::show_windows_instantly();
                        crate::platform::macos::app::set_dock_icon(self.config.dock_icon);
                    }
                    return Task::batch([
                        // The fade, the placement and the level need the native window.
                        Task::native(|window| {
                            window::attach(window);
                            Message::Tick
                        })
                        .chain(Task::window(WindowOp::SetLevel(
                            crate::features::window::level_for(self.config.on_blur),
                        ))),
                        self.reveal(),
                    ]);
                }
            }
            // The close button hides the window while the shortcut can bring it back;
            // without one, hiding would strand the app, so closing quits. Sessions
            // run on the server either way.
            Message::Close => {
                return if self.dropdown() {
                    self.hide()
                } else {
                    Task::window(WindowOp::Close)
                };
            }
            Message::DragWindow if self.opened => return Task::window(WindowOp::Drag),
            _ => {}
        }
        Task::none()
    }

    fn toggle_hotkey(&mut self, minimized: Option<bool>) -> Task<Message> {
        if hotkey_hides(self.hidden, self.focused, self.config.on_blur, minimized) {
            return self.hide();
        }
        #[cfg(target_os = "macos")]
        if self.hidden {
            self.slide = Some(Slide::open(Instant::now(), self.slide_share(1.0)));
            self.hidden = false;
            let changed_display = crate::platform::macos::app::pointer_display()
                .zip(self.dropdown_display)
                .is_some_and(|((bounds, _), previous)| bounds != previous);
            if changed_display {
                // Move before revealing, so the old display never flashes.
                return self.reveal();
            }
            return Task::batch([
                Task::window(WindowOp::Show)
                    .chain(Task::window(WindowOp::Minimize(false)))
                    .chain(Task::window(WindowOp::Raise)),
                self.reveal(),
            ]);
        }
        if self.hidden && self.slide_running() {
            // Reverse immediately, without waiting for the window measurements.
            self.slide = Some(Slide::open(Instant::now(), self.slide_share(1.0)));
            self.hidden = false;
        }
        self.reveal()
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
    /// Whether the window works as a dropdown: a global shortcut shows and hides it, and
    /// the desktop lets it be hidden and placed. Without that it is an ordinary window.
    pub(super) fn dropdown(&self) -> bool {
        self.hotkeys.is_some() && window_control() != WindowControl::Unavailable
    }
    /// How much of the content is out of sight: 0 when shown, 1 when hidden.
    pub(crate) fn slide_hidden_share(&self) -> f32 {
        self.slide_share(0.0)
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
        self.hotkey_pending = None;
        self.reveal_generation = self.reveal_generation.wrapping_add(1);
        self.hidden = true;
        self.resizing = false;
        // The content slides out. Wayland cannot hide a window, so KWin parks it; elsewhere
        // it is hidden when the slide ends.
        self.slide = Some(Slide::close(Instant::now(), self.slide_share(0.0)));
        #[cfg(target_os = "linux")]
        if kwin_controls_window() {
            kwin::send(kwin::Action::Hide);
        }
        Task::none()
    }
    /// Measures the display and window, then shows the window via [`Message::Present`].
    pub(super) fn reveal(&mut self) -> Task<Message> {
        self.hotkey_pending = None;
        self.reveal_generation = self.reveal_generation.wrapping_add(1);
        let generation = self.reveal_generation;
        #[cfg(target_os = "macos")]
        let pointer_display = crate::platform::macos::app::pointer_display();
        #[cfg(not(target_os = "macos"))]
        let pointer_display = None;
        Task::monitor_size(move |monitor| {
            Task::window_size(move |size| {
                Task::done(Message::Present(generation, monitor, size, pointer_display))
            })
        })
    }
}

#[cfg(test)]
mod height_tests {
    use super::*;

    #[test]
    fn shortcut_hides_visible_topmost_window_without_focus() {
        assert!(hotkey_hides(false, false, OnBlur::Top, Some(false)));
        assert!(!hotkey_hides(false, false, OnBlur::Top, Some(true)));
        assert!(!hotkey_hides(false, false, OnBlur::Top, None));
        assert!(!hotkey_hides(true, false, OnBlur::Top, Some(false)));
        assert!(!hotkey_hides(false, false, OnBlur::None, Some(false)));
        assert!(hotkey_hides(false, true, OnBlur::None, None));
    }

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
