//! KDE Plasma 6 on Wayland. Wayland lets a client neither register a global key nor place,
//! hide or raise its own window, so both go through KDE: the shortcut through the
//! `org.freedesktop.portal.GlobalShortcuts` portal, and the window through a one-shot
//! KWin QML script loaded over D-Bus. A leaf module: it depends on no feature.

use std::{
    io::Write,
    os::unix::fs::OpenOptionsExt,
    sync::{OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};
use zbus::block_on;

/// The application id: the window class winit gives the window (which the KWin script
/// matches), the portal's application id, and the name of the desktop entry.
pub(crate) const APP_ID: &str = "dev.spooky.terminal";
const SHORTCUT: &str = "toggle";
/// The first bind can show a confirmation dialog; the wait covers the person answering.
/// How long the window takes to fade in or out; the application's slide lasts as long.
const SLIDE_MS: u64 = 200;
/// Where a hidden window waits: beyond any display.
const PARK: u32 = 30000;
/// Matches `window::slide::FIRST_DELAY`, which holds the application's own slide back.
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
    /// Remembers the size this request asks for, if any, in case a later one replaces it.
    fn noting_size(self, asked: &mut Option<(Dim, Dim)>) -> Self {
        if let Self::Reveal { width, height, .. } = &self
            && !(matches!(width, Dim::Keep) && matches!(height, Dim::Keep))
        {
            *asked = Some((*width, *height));
        }
        self
    }

    /// Keeps a size across requests that were dropped in a burst: the application asks
    /// for the configured size once, and a later reveal only places the window. A
    /// reveal that carries a size takes it from `asked`, or leaves it there if it has
    /// none.
    fn with_size(self, asked: &mut Option<(Dim, Dim)>) -> Self {
        let Self::Reveal {
            width,
            height,
            above,
        } = self
        else {
            return self;
        };
        match (width, height, asked.take()) {
            (Dim::Keep, Dim::Keep, Some((width, height))) => Self::Reveal {
                width,
                height,
                above,
            },
            _ => Self::Reveal {
                width,
                height,
                above,
            },
        }
    }

    /// A KWin QML script that acts on this process's window only (another Spooky
    /// instance keeps its own), placing it and fading it in or out. The slide itself is
    /// drawn by the application inside the window: KWin cannot clip a moving window to
    /// its own display. Only numbers and booleans are interpolated, so it cannot be
    /// altered.
    ///
    /// `fresh` marks the first reveal after launch, whose window has not been hidden but
    /// still fades in like a later one.
    fn script(&self, fresh: bool) -> String {
        let pid = std::process::id();
        let body = match self {
            Self::Hide => format!(
                r#"
        if (w.frameGeometry.x <= -{PARK}) return;
        fade.from = w.opacity; fade.to = 0; fade.hideAtEnd = true;
        anim.start();"#
            ),
            Self::Reveal {
                width,
                height,
                above,
            } => {
                let (width, height) = (
                    width.expr("a.width", "w.frameGeometry.width"),
                    height.expr("a.height", "w.frameGeometry.height"),
                );
                // Only a window that was off screen fades in: a second request for a
                // window already shown just places it.
                format!(
                    r#"
        const wasHidden = w.frameGeometry.x <= -{PARK} || {fresh};
        // Like Yakuake, out of the task bar, pager and switcher.
        w.skipTaskbar = true;
        w.skipPager = true;
        w.skipSwitcher = true;
        if (w.minimized) w.minimized = false;
        const a = Workspace.clientArea(KWin.PlacementArea, Workspace.screenAt(Workspace.cursorPos), Workspace.currentDesktop);
        const width = Math.min({width}, a.width);
        const height = Math.min({height}, a.height);
        w.keepAbove = {above};
        try {{ if (!w.onAllDesktops) w.desktops = [Workspace.currentDesktop]; }} catch (e) {{}}
        if (wasHidden) w.opacity = 0;
        w.frameGeometry = Qt.rect(a.x + (a.width - width) / 2, a.y, width, height);
        // From wherever a fade that was cut short had got to.
        fade.from = w.opacity; fade.to = 1; fade.hideAtEnd = false;
        anim.start();
        Workspace.activeWindow = w;"#
                )
            }
        };
        format!(
            r#"import QtQuick
import org.kde.kwin

Item {{
    id: root
    property var win: null
    // Off screen, not minimized: KWin's minimize effect would fly the window to the task
    // bar, at the bottom, and back. Focus goes to the window that was below.
    function park() {{
        const w = root.win;
        w.frameGeometry = Qt.rect(-{PARK}, -{PARK}, w.frameGeometry.width, w.frameGeometry.height);
        w.opacity = 1;
        const order = Workspace.stackingOrder;
        for (let i = order.length - 1; i >= 0; i--) {{
            const c = order[i];
            if (c !== w && c.normalWindow && !c.minimized
                && (c.onAllDesktops || c.desktops.indexOf(Workspace.currentDesktop) >= 0)) {{
                Workspace.activeWindow = c;
                return;
            }}
        }}
    }}
    QtObject {{
        id: fade
        property real p: 0
        property real from: 1
        property real to: 1
        property bool hideAtEnd: false
        onPChanged: if (root.win) root.win.opacity = from + (to - from) * p
    }}
    NumberAnimation {{
        id: anim
        target: fade
        property: "p"
        from: 0
        to: 1
        duration: {SLIDE_MS}
        easing.type: Easing.Linear
        onFinished: if (fade.hideAtEnd) root.park()
    }}
    Component.onCompleted: {{
        for (const c of Workspace.windows) {{
            if (c.pid === {pid} && String(c.resourceClass).toLowerCase() === "{APP_ID}") root.win = c;
        }}
        const w = root.win;
        if (!w) return;{body}
    }}
}}
"#
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
    use mpsc::RecvTimeoutError::{Disconnected, Timeout};
    let mut reported = false;
    let mut report = |error: anyhow::Error| {
        if !std::mem::replace(&mut reported, true) {
            eprintln!("spooky: KWin window control failed: {error:#}");
        }
    };
    let mut first = true;
    // A size asked for by a request that a later one replaced.
    let mut asked: Option<(Dim, Dim)> = None;
    let mut loaded: Option<Loaded> = None;
    loop {
        // The script stays loaded while its animation runs, then is unloaded on time.
        let received = match &loaded {
            Some(script) => {
                actions.recv_timeout(script.until.saturating_duration_since(Instant::now()))
            }
            None => actions.recv().map_err(|_| Disconnected),
        };
        let mut action = match received {
            Ok(action) => action.noting_size(&mut asked),
            Err(Timeout) => {
                if let Some(script) = loaded.take()
                    && let Err(error) = block_on(script.unload())
                {
                    report(error);
                }
                continue;
            }
            Err(Disconnected) => break,
        };
        // Only the newest request matters: a press that came after another supersedes it,
        // so a burst of presses ends where the last one says instead of replaying.
        while let Ok(newer) = actions.try_recv() {
            action = newer.noting_size(&mut asked);
        }
        // The window is not mapped, so not known to KWin, until just after the first
        // reveal is requested; later ones find it.
        let mut fresh = first && matches!(action, Action::Reveal { .. });
        if fresh {
            first = false;
            thread::sleep(FIRST_REVEAL_DELAY);
            while let Ok(newer) = actions.try_recv() {
                action = newer.noting_size(&mut asked);
            }
            fresh = matches!(action, Action::Reveal { .. });
        }
        // The new script takes over from the one still animating.
        if let Some(script) = loaded.take()
            && let Err(error) = block_on(script.unload())
        {
            report(error);
        }
        match block_on(Loaded::run(&action.with_size(&mut asked).script(fresh))) {
            Ok(script) => loaded = Some(script),
            Err(error) => report(error),
        }
    }
}

/// A QML script loaded in KWin whose animation may still be running.
struct Loaded {
    scripting: zbus::Proxy<'static>,
    name: String,
    path: std::path::PathBuf,
    /// When its animation has ended and it can be unloaded.
    until: Instant,
}

impl Loaded {
    /// Loads `source` into KWin as a QML script and runs it.
    async fn run(source: &str) -> anyhow::Result<Self> {
        use anyhow::Context;
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is not set")?;
        // KWin's QML engine caches a script by path, so each run has a file of its own.
        let path = std::path::Path::new(&dir).join(format!(
            "spooky-kwin-{}-{}.qml",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
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
        let loaded = Self {
            scripting,
            name,
            path,
            until: Instant::now() + Duration::from_millis(SLIDE_MS + 100),
        };
        // A script left by a crashed run would make the load fail.
        let _: bool = loaded
            .scripting
            .call("unloadScript", &(loaded.name.as_str(),))
            .await?;
        let started = async {
            let path = loaded
                .path
                .to_str()
                .context("the runtime directory is not UTF-8")?;
            let id: i32 = loaded
                .scripting
                .call("loadDeclarativeScript", &(path, loaded.name.as_str()))
                .await?;
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
        match started {
            Ok(()) => Ok(loaded),
            Err(error) => {
                let _ = loaded.unload().await;
                Err(error)
            }
        }
    }

    /// Unloads the script, which ends its animation, and removes its file.
    async fn unload(self) -> anyhow::Result<()> {
        let _ = std::fs::remove_file(&self.path);
        let _: bool = self
            .scripting
            .call("unloadScript", &(self.name.as_str(),))
            .await?;
        Ok(())
    }
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
    fn a_burst_of_requests_keeps_the_size_that_was_asked_for() {
        let sized = || Action::Reveal {
            width: Dim::Px(1920),
            height: Dim::Px(540),
            above: false,
        };
        let placed = || Action::Reveal {
            width: Dim::Keep,
            height: Dim::Keep,
            above: true,
        };
        // A sized reveal, a hide and a plain reveal arrive together; only the last runs,
        // and the size is not lost with the first.
        let mut asked = None;
        sized().noting_size(&mut asked);
        Action::Hide.noting_size(&mut asked);
        let Action::Reveal { width, above, .. } =
            placed().noting_size(&mut asked).with_size(&mut asked)
        else {
            panic!("a reveal stays a reveal");
        };
        assert!(matches!(width, Dim::Px(1920)) && above);
        assert!(asked.is_none(), "the size is used once");
        let Action::Reveal { width, .. } = placed().with_size(&mut asked) else {
            panic!("a reveal stays a reveal");
        };
        assert!(matches!(width, Dim::Keep), "nothing is left to apply");
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
        .script(false);
        assert!(reveal.contains(&format!("c.pid === {}", std::process::id())));
        assert!(
            reveal.contains("Math.min(Math.max(720, Math.round(a.width * 100 / 100)), a.width)")
        );
        assert!(reveal.contains("Math.min(540, a.height)") && reveal.contains("keepAbove = true"));
        assert!(
            Action::Hide
                .script(false)
                .contains("fade.hideAtEnd = true;")
        );
        assert!(
            !Action::Hide.script(false).contains("w.minimized = true"),
            "KWin's minimize effect flies the window to the task bar"
        );
    }
}
