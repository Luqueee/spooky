//! Native GPU frontend for persistent Spooky sessions.

mod app;
mod commands;
mod config;
mod features;
mod geometry;
mod gui;
mod keys;
mod layout;
mod link;
mod platform;
mod probe;
mod runtime;
mod shortcuts;
mod tabs;
mod terminal;
#[cfg(test)]
mod test_support;
mod ui;
mod updater;

use app::{App, ContextTarget, Message};
use config::Config;
use features::window::{Hotkeys, StartError, initial_position};
use geometry::{Point, Size};
use std::{cell::RefCell, rc::Rc};

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
    let app = App::new(config, status, hotkeys, themes);
    let failure = Rc::new(RefCell::new(None));
    let reported = failure.clone();
    gpui_platform::application().run(move |cx| {
        let position = cx
            .primary_display()
            .map(|display| {
                let bounds = display.bounds();
                initial_position(
                    size,
                    Size::new(f32::from(bounds.size.width), f32::from(bounds.size.height)),
                )
            })
            .unwrap_or(Point::ORIGIN);
        let launch = gui::Launch {
            size,
            min_size: config::MIN_WINDOW,
            position,
            // With the global shortcut the window starts hidden, and is shown (placed,
            // focused) by the same path as every later reveal.
            hidden: dropdown,
            // On KDE the application's own header replaces KDE's title bar, which would
            // otherwise sit above it.
            frameless: platform::window_control() == platform::WindowControl::Kwin,
            app_id: platform::APP_ID,
        };
        if let Err(error) = gui::open(cx, app, &launch) {
            *reported.borrow_mut() = Some(error);
            cx.quit();
            return;
        }
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        if !dropdown {
            cx.activate(true);
        }
    });
    match failure.take() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
