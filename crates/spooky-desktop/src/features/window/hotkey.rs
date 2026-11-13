//! The system-wide shortcut that shows and hides the window.

use crate::{link, platform::kde};
use global_hotkey::{GlobalHotKeyManager, hotkey::HotKey};

/// Why the shortcut could not be set up at startup.
pub(crate) enum StartError {
    /// The text is not a shortcut; the configuration is wrong.
    Invalid(String),
    /// The system refused the registration (another application may hold it).
    Unavailable(String),
}

/// The identity of the portal's one shortcut; the native backend is never active with it.
const PORTAL_ID: u32 = 1;

/// The registered global shortcut.
pub(crate) enum Hotkeys {
    /// Registered by the application (macOS, X11), with the manager that owns it.
    Native {
        manager: GlobalHotKeyManager,
        registered: HotKey,
    },
    /// Registered through the desktop portal (KDE on Wayland); KDE owns the binding.
    Portal,
}

impl Hotkeys {
    /// Registers `text` (for example `Ctrl+Shift+Space`) as the global shortcut.
    pub(crate) fn start(text: &str) -> Result<Self, StartError> {
        let hotkey: HotKey = text
            .parse()
            .map_err(|error| StartError::Invalid(format!("{text}: {error}")))?;
        if kde::active() {
            kde::bind_shortcut(text, || {
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
            Self::Portal => PORTAL_ID,
        }
    }

    /// Moves the shortcut to `text`. The new one is registered before the old one is
    /// released, so a refusal leaves the working shortcut in place.
    pub(crate) fn rebind(&mut self, text: &str) -> Result<(), String> {
        let hotkey: HotKey = text.parse().map_err(|error| format!("{text}: {error}"))?;
        let Self::Native {
            manager,
            registered,
        } = self
        else {
            return Err("KDE owns this shortcut: change it in System Settings → Keyboard → Shortcuts → Spooky".into());
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
