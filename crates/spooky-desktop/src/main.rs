//! Native GPU frontend for persistent Spooky sessions.

mod app;
mod commands;
mod config;
mod features;
mod layout;
mod link;
mod platform;
mod probe;
mod shortcuts;
mod tabs;
mod terminal;
#[cfg(test)]
mod test_support;
mod ui;

use app::{App, ContextTarget, Message, subscription};
use config::Config;
use features::window::{Hotkeys, StartError, initial_position, level_for};
use iced::window;

fn main() -> anyhow::Result<()> {
    let config = Config::load()?;
    let themes = ui::ThemeCatalog::load(&Config::themes_dir()?);
    let (hotkeys, status) = match Hotkeys::start(&config.shortcut) {
        Ok(hotkeys) => (Some(hotkeys), "Starting session…".to_owned()),
        Err(StartError::Invalid(error)) => {
            anyhow::bail!("Invalid global shortcut: {error}")
        }
        Err(StartError::Unavailable(error)) => {
            (None, format!("Global shortcut unavailable: {error}"))
        }
    };
    // A dropdown needs the shortcut and a desktop that lets the window be hidden.
    let dropdown =
        hotkeys.is_some() && platform::window_control() != platform::WindowControl::Unavailable;
    let size = config.initial_size();
    let level = level_for(config.on_blur);
    // The application state owns the shortcut registration; the boot function hands
    // it over once.
    let hotkeys = std::cell::Cell::new(hotkeys);
    let result = iced::application(
        move || {
            App::new(
                config.clone(),
                status.clone(),
                hotkeys.take(),
                themes.clone(),
            )
        },
        App::update,
        App::view,
    )
    .title("Spooky")
    .subscription(subscription)
    .theme(App::theme)
    .style(App::style)
    .default_font(iced::Font::MONOSPACE)
    .window(window::Settings {
        size,
        // The window is clear behind the content while it slides, so it can be see-through.
        // Clear behind the content, for the slide and for the background opacity setting.
        transparent: dropdown || cfg!(any(target_os = "macos", target_os = "linux")),
        // Blur what shows through where the compositor offers it (KDE's Blur effect, on
        // X11 and Wayland); others ignore the request. Fixed when the window is created.
        blur: cfg!(target_os = "linux"),
        min_size: Some(config::MIN_WINDOW),
        level,
        #[cfg(target_os = "linux")]
        icon: {
            let image = image::load_from_memory(include_bytes!("../../../assets/logo-256.png"))?
                .into_rgba8();
            let (width, height) = image.dimensions();
            Some(window::icon::from_rgba(image.into_raw(), width, height)?)
        },
        // The class KWin and the desktop entry know the window by.
        #[cfg(target_os = "linux")]
        platform_specific: window::settings::PlatformSpecific {
            application_id: platform::APP_ID.to_owned(),
            ..Default::default()
        },
        #[cfg(target_os = "macos")]
        platform_specific: window::settings::PlatformSpecific {
            title_hidden: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
        },
        // With the global shortcut the window starts hidden, and is shown (placed,
        // focused) by the same path as every later reveal.
        visible: !dropdown,
        position: window::Position::SpecificWith(initial_position),
        // On KDE the application's own header replaces KDE's title bar, which would
        // otherwise sit above it.
        decorations: platform::window_control() != platform::WindowControl::Kwin,
        exit_on_close_request: false,
        ..Default::default()
    })
    .run();
    result.map_err(|e| anyhow::anyhow!("{e}"))
}
