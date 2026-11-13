//! KDE Plasma on Wayland. Wayland lets a client neither register a global key nor place,
//! hide or raise its own window, so both go through KDE: the shortcut through the
//! `org.freedesktop.portal.GlobalShortcuts` portal, and the window through a one-shot
//! KWin script loaded over D-Bus. A leaf module: it depends on no feature.

use std::{
    io::Write,
    os::unix::fs::OpenOptionsExt,
    sync::{OnceLock, mpsc},
    thread,
    time::Duration,
};
use zbus::block_on;

/// The application id: the window class winit gives the window (which the KWin script
/// matches), the portal's application id, and the name of the desktop entry.
pub(crate) const APP_ID: &str = "dev.spooky.terminal";
const SHORTCUT: &str = "toggle";
/// The first bind can show a confirmation dialog; the wait covers the person answering.
const FIRST_REVEAL_DELAY: Duration = Duration::from_millis(300);
const BIND_TIMEOUT: Duration = Duration::from_secs(60);

/// Whether this is a KDE Wayland session, where this module can do what Wayland forbids.
pub(crate) fn active() -> bool {
    static ACTIVE: OnceLock<bool> = OnceLock::new();
    *ACTIVE.get_or_init(|| {
        std::env::var_os("WAYLAND_DISPLAY").is_some()
            && std::env::var("XDG_CURRENT_DESKTOP")
                .is_ok_and(|desktop| desktop.split(':').any(|d| d.eq_ignore_ascii_case("KDE")))
    })
}

/// One side of the window: Wayland does not tell a client its display until the window
/// has been shown, so a share of the display is resolved by KWin, which knows it.
#[derive(Clone, Copy)]
pub(crate) enum Dim {
    /// Leave this side as the window has it.
    Keep,
    Px(u32),
    /// `percent` of the usable area, but at least `min` pixels.
    Percent {
        percent: f32,
        min: u32,
    },
}

impl Dim {
    /// A script expression for this side, given the area's extent and the window's
    /// current size along it.
    fn expr(self, extent: &str, current: &str) -> String {
        match self {
            Self::Keep => current.to_owned(),
            Self::Px(px) => px.to_string(),
            Self::Percent { percent, min } => {
                format!("Math.max({min}, Math.round({extent} * {percent} / 100))")
            }
        }
    }
}

/// What KWin is asked to do with the window.
pub(crate) enum Action {
    /// Un-minimize, centre against the top of the usable area of the display under the
    /// pointer, size, optionally keep above other windows, and focus.
    Reveal {
        width: Dim,
        height: Dim,
        above: bool,
    },
    /// Minimize: the only way to take a Wayland window off screen.
    Hide,
}

impl Action {
    /// Only numbers and booleans are interpolated, so the script cannot be altered. It
    /// acts on this process's window only: another Spooky instance keeps its own.
    fn script(&self) -> String {
        let pid = std::process::id();
        let body = match self {
            Self::Hide => "w.minimized = true;".to_owned(),
            Self::Reveal {
                width,
                height,
                above,
            } => {
                let (width, height) = (
                    width.expr("a.width", "w.frameGeometry.width"),
                    height.expr("a.height", "w.frameGeometry.height"),
                );
                format!(
                    r#"
                w.minimized = false;
                const out = (workspace.screenAt && workspace.cursorPos)
                    ? workspace.screenAt(workspace.cursorPos) : workspace.activeScreen;
                const a = workspace.clientArea(KWin.PlacementArea, out, workspace.currentDesktop);
                const width = Math.min({width}, a.width);
                const height = Math.min({height}, a.height);
                w.frameGeometry = {{x: a.x + (a.width - width) / 2, y: a.y, width: width, height: height}};
                w.keepAbove = {above};
                try {{ if (!w.onAllDesktops) w.desktops = [workspace.currentDesktop]; }} catch (e) {{}}
                if ("activeWindow" in workspace) workspace.activeWindow = w;
                else workspace.activeClient = w;
                "#
                )
            }
        };
        format!(
            r#"(function () {{
                const list = workspace.windowList ? workspace.windowList() : workspace.clientList();
                for (const w of list) {{
                    if (w.pid !== {pid} || String(w.resourceClass).toLowerCase() !== "{APP_ID}") continue;
                    {body}
                }}
            }})();"#
        )
    }
}

/// Queues `action` for KWin. A failure is reported once on stderr: the window then stays
/// where it is, which is a degraded dropdown, not a lost session.
pub(crate) fn send(action: Action) {
    static WORKER: OnceLock<mpsc::Sender<Action>> = OnceLock::new();
    let tx = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || worker(rx));
        tx
    });
    let _ = tx.send(action);
}

fn worker(actions: mpsc::Receiver<Action>) {
    let mut reported = false;
    let mut first = true;
    for action in actions {
        // The window is not mapped, so not known to KWin, until just after the first
        // reveal is requested; later ones find it.
        if first && matches!(action, Action::Reveal { .. }) {
            first = false;
            thread::sleep(FIRST_REVEAL_DELAY);
        }
        if let Err(error) = block_on(run_script(&action.script()))
            && !std::mem::replace(&mut reported, true)
        {
            eprintln!("spooky: KWin window control failed: {error:#}");
        }
    }
}

/// Loads `source` into KWin, runs it once and unloads it.
async fn run_script(source: &str) -> anyhow::Result<()> {
    use anyhow::Context;
    let dir = std::env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is not set")?;
    let path = std::path::Path::new(&dir).join(format!("spooky-kwin-{}.js", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?
        .write_all(source.as_bytes())?;
    let name = format!("spooky-{}", std::process::id());
    let connection = zbus::Connection::session().await?;
    let scripting = zbus::Proxy::new(
        &connection,
        "org.kde.KWin",
        "/Scripting",
        "org.kde.kwin.Scripting",
    )
    .await?;
    // A script left by a crashed run would make the load fail.
    let _: bool = scripting.call("unloadScript", &(name.as_str(),)).await?;
    let result = async {
        let path = path
            .to_str()
            .context("the runtime directory is not UTF-8")?;
        let id: i32 = scripting.call("loadScript", &(path, name.as_str())).await?;
        anyhow::ensure!(id >= 0, "KWin refused to load the script");
        let script = zbus::Proxy::new(
            &connection,
            "org.kde.KWin",
            format!("/Scripting/Script{id}"),
            "org.kde.kwin.Script",
        )
        .await?;
        script.call::<_, _, ()>("run", &()).await?;
        anyhow::Ok(())
    }
    .await;
    let _: bool = scripting.call("unloadScript", &(name.as_str(),)).await?;
    let _ = std::fs::remove_file(&path);
    result
}

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
    portal
        .bind_shortcuts(
            &session,
            &[NewShortcut::new(SHORTCUT, "Show or hide Spooky").preferred_trigger(trigger)],
            None,
        )
        .await?
        .response()?;
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

    #[test]
    fn the_scripts_touch_only_this_applications_windows() {
        let reveal = Action::Reveal {
            width: Dim::Percent {
                percent: 100.0,
                min: 720,
            },
            height: Dim::Px(540),
            above: true,
        }
        .script();
        assert!(reveal.contains(&format!("w.pid !== {}", std::process::id())));
        assert!(
            reveal.contains("Math.min(Math.max(720, Math.round(a.width * 100 / 100)), a.width)")
        );
        assert!(reveal.contains("Math.min(540, a.height)") && reveal.contains("keepAbove = true"));
        assert!(Action::Hide.script().contains("w.minimized = true;"));
    }
}
