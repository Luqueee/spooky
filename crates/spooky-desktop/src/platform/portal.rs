//! The global shortcut through the `org.freedesktop.portal.GlobalShortcuts` portal, which
//! is how a Wayland client registers a key (KDE Plasma, and GNOME from version 48). A
//! leaf module: it depends on no feature.

use super::APP_ID;
use std::{sync::mpsc, thread, time::Duration};
use zbus::block_on;

const SHORTCUT: &str = "toggle";
/// The first bind can show a confirmation dialog; the wait covers the person answering.
const BIND_TIMEOUT: Duration = Duration::from_secs(60);

/// Registers `text` as the preferred trigger of the application's one shortcut and calls
/// `on_press` for each activation. Blocks until the portal answers. The trigger is a
/// suggestion: KDE keeps whatever the person set in System Settings.
pub(crate) fn bind_shortcut(
    text: &str,
    on_press: impl Fn() + Send + 'static,
) -> Result<(), String> {
    let trigger = portal_trigger(text);
    let (ready, answer) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("spooky-portal".into())
        .spawn(move || {
            let error = |message: String| {
                let _ = ready.try_send(Err(message));
            };
            if let Err(e) = block_on(listen(&trigger, &ready, on_press)) {
                error(e.to_string());
            }
        })
        .map_err(|e| e.to_string())?;
    answer
        .recv_timeout(BIND_TIMEOUT)
        .map_err(|_| "the desktop portal did not answer".to_owned())?
}

async fn listen(
    trigger: &str,
    ready: &mpsc::SyncSender<Result<(), String>>,
    on_press: impl Fn(),
) -> Result<(), ashpd::Error> {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    use iced::futures::StreamExt;
    // A process outside a sandbox has no application id for the portal; an older portal
    // lacks the registry, and the bind below then says what is missing.
    if let Ok(id) = ashpd::AppID::try_from(APP_ID) {
        let _ = ashpd::register_host_app(id).await;
    }
    let portal = GlobalShortcuts::new().await?;
    // Listening starts before binding so no early press is lost.
    let mut activated = portal.receive_activated().await?;
    let session = portal.create_session().await?;
    // Binding asks the desktop to show its shortcut-configuration window, which KDE 6.3
    // does at every start. A shortcut the desktop already knows from an earlier run (it
    // keeps them per application id) needs no binding, so it is only bound the first
    // time; the person's choice in System Settings is kept either way.
    let known = portal
        .list_shortcuts(&session)
        .await?
        .response()?
        .shortcuts()
        .iter()
        .any(|shortcut| shortcut.id() == SHORTCUT);
    if !known {
        portal
            .bind_shortcuts(
                &session,
                &[NewShortcut::new(SHORTCUT, "Show or hide Spooky").preferred_trigger(trigger)],
                None,
            )
            .await?
            .response()?;
    }
    let _ = ready.try_send(Ok(()));
    while let Some(press) = activated.next().await {
        if press.shortcut_id() == SHORTCUT {
            on_press();
        }
    }
    drop(session);
    Ok(())
}

/// Spells a shortcut (`Ctrl+Shift+Space`) as the portal's XDG form (`CTRL+SHIFT+space`).
fn portal_trigger(text: &str) -> String {
    text.split('+')
        .map(|part| {
            let part = part.trim();
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => "CTRL".to_owned(),
                "shift" => "SHIFT".to_owned(),
                "alt" | "option" => "ALT".to_owned(),
                "cmd" | "command" | "super" | "win" | "meta" => "LOGO".to_owned(),
                "space" => "space".to_owned(),
                "enter" | "return" => "Return".to_owned(),
                lower => match lower
                    .strip_prefix("key")
                    .or_else(|| lower.strip_prefix("digit"))
                {
                    Some(key) if key.len() == 1 => key.to_owned(),
                    _ if part.len() == 1 => lower.to_owned(),
                    _ => part.to_owned(),
                },
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_are_spelled_the_way_the_portal_expects() {
        assert_eq!(portal_trigger("Ctrl+Shift+Space"), "CTRL+SHIFT+space");
        assert_eq!(portal_trigger("Ctrl+Alt+Shift+F8"), "CTRL+ALT+SHIFT+F8");
        assert_eq!(portal_trigger("Cmd+KeyK"), "LOGO+k");
        assert_eq!(portal_trigger("Ctrl+Enter"), "CTRL+Return");
        assert_eq!(portal_trigger("Alt+Up"), "ALT+Up");
    }
}
