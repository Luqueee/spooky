//! The system-wide shortcut that shows and hides the window.

use global_hotkey::{GlobalHotKeyManager, hotkey::HotKey};

/// Why the shortcut could not be set up at startup.
pub(crate) enum StartError {
    /// The text is not a shortcut; the configuration is wrong.
    Invalid(String),
    /// The system refused the registration (another application may hold it).
    Unavailable(String),
}

/// The registered global shortcut, with the manager that owns the registration.
pub(crate) struct Hotkeys {
    manager: GlobalHotKeyManager,
    registered: HotKey,
}

impl Hotkeys {
    /// Registers `text` (for example `Ctrl+Shift+Space`) as the global shortcut.
    pub(crate) fn start(text: &str) -> Result<Self, StartError> {
        let hotkey: HotKey = text
            .parse()
            .map_err(|error| StartError::Invalid(format!("{text}: {error}")))?;
        let manager = GlobalHotKeyManager::new()
            .and_then(|manager| {
                manager.register(hotkey)?;
                Ok(manager)
            })
            .map_err(|error| StartError::Unavailable(error.to_string()))?;
        Ok(Self {
            manager,
            registered: hotkey,
        })
    }

    /// Identity carried by the events of this shortcut.
    pub(crate) fn id(&self) -> u32 {
        self.registered.id()
    }

    /// Moves the shortcut to `text`. The new one is registered before the old one is
    /// released, so a refusal leaves the working shortcut in place.
    pub(crate) fn rebind(&mut self, text: &str) -> Result<(), String> {
        let hotkey: HotKey = text.parse().map_err(|error| format!("{text}: {error}"))?;
        if self.registered != hotkey {
            self.manager
                .register(hotkey)
                .map_err(|error| format!("Cannot use {text}: {error}"))?;
            // Failing to release the old binding is not a reason to refuse the new one.
            let _ = self.manager.unregister(self.registered);
            self.registered = hotkey;
        }
        Ok(())
    }
}
