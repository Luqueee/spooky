//! The system-wide shortcut that shows and hides the window.

#[cfg(target_os = "linux")]
use crate::link;
#[cfg(target_os = "linux")]
use crate::platform::{
    desktop::{Session, session},
    portal,
};
use global_hotkey::{GlobalHotKeyManager, hotkey::HotKey};

/// Why the shortcut could not be set up at startup.
pub(crate) enum StartError {
    /// The text is not a shortcut; the configuration is wrong.
    Invalid(String),
    /// The system refused the registration (another application may hold it).
    Unavailable(String),
}

/// The identity of the portal's one shortcut; the native backend is never active with it.
#[cfg(target_os = "linux")]
const PORTAL_ID: u32 = 1;

/// The registered global shortcut.
pub(crate) enum Hotkeys {
    /// Registered by the application (macOS, X11), with the manager that owns it.
    Native {
        manager: GlobalHotKeyManager,
        registered: HotKey,
    },
    /// Registered through the desktop portal (Wayland); the desktop owns the binding.
    #[cfg(target_os = "linux")]
    Portal,
}

impl Hotkeys {
    /// Registers `text` (for example `Ctrl+Shift+Space`) as the global shortcut.
    pub(crate) fn start(text: &str) -> Result<Self, StartError> {
        let hotkey: HotKey = text
            .parse()
            .map_err(|error| StartError::Invalid(format!("{text}: {error}")))?;
        #[cfg(target_os = "linux")]
        if matches!(session(), Session::KdeWayland | Session::Wayland) {
            portal::bind_shortcut(text, || {
                let _ = link::hotkey_sender().send(PORTAL_ID);
            })
            .map_err(StartError::Unavailable)?;
            return Ok(Self::Portal);
        }
        let manager = GlobalHotKeyManager::new()
            .and_then(|manager| {
                manager.register(hotkey)?;
                Ok(manager)
            })
            .map_err(|error| StartError::Unavailable(error.to_string()))?;
        Ok(Self::Native {
            manager,
            registered: hotkey,
        })
    }

    /// Identity carried by the events of this shortcut.
    pub(crate) fn id(&self) -> u32 {
        match self {
            Self::Native { registered, .. } => registered.id(),
            #[cfg(target_os = "linux")]
            Self::Portal => PORTAL_ID,
        }
    }

    /// Moves the shortcut to `text`. The new one is registered before the old one is
    /// released, so a refusal leaves the working shortcut in place.
    pub(crate) fn rebind(&mut self, text: &str) -> Result<(), String> {
        let hotkey: HotKey = text.parse().map_err(|error| format!("{text}: {error}"))?;
        #[cfg(not(target_os = "linux"))]
        let Self::Native {
            manager,
            registered,
        } = self;
        #[cfg(target_os = "linux")]
        let Self::Native {
            manager,
            registered,
        } = self
        else {
            return Err("The desktop owns this shortcut: change it in its keyboard shortcut settings (KDE: System Settings → Shortcuts → Spooky; GNOME: Settings → Keyboard)".into());
        };
        if *registered != hotkey {
            manager
                .register(hotkey)
                .map_err(|error| format!("Cannot use {text}: {error}"))?;
            // Failing to release the old binding is not a reason to refuse the new one.
            let _ = manager.unregister(*registered);
            *registered = hotkey;
        }
        Ok(())
    }
}
