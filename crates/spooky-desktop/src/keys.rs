//! Keyboard vocabulary shared by the shortcut table, the terminal's key encoding and the
//! settings page: which key, which physical key, which modifiers. The GUI layer fills
//! these in from its own key events; nothing here depends on it.

/// A key as the layout reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    /// A key with a name of its own.
    Named(Named),
    /// A key that types a character (`"a"`, `"/"`).
    Character(String),
    /// A key this vocabulary has no name for.
    Unidentified,
}

/// Keys that have no character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Named {
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Backspace,
    Enter,
    Tab,
    Space,
    Escape,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

/// The key's position on the keyboard, independent of the layout. Only the keys the
/// application binds by position are named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Code {
    KeyA,
    KeyB,
    KeyC,
    KeyD,
    KeyE,
    KeyF,
    KeyG,
    KeyH,
    KeyI,
    KeyJ,
    KeyK,
    KeyL,
    KeyM,
    KeyN,
    KeyO,
    KeyP,
    KeyQ,
    KeyR,
    KeyS,
    KeyT,
    KeyU,
    KeyV,
    KeyW,
    KeyX,
    KeyY,
    KeyZ,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    BracketLeft,
    BracketRight,
    Comma,
    Space,
    Enter,
    Tab,
    Escape,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

/// Where a key is, or that it is not known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Physical {
    Code(Code),
    Unidentified,
}

/// Which modifier keys are held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    /// Cmd on macOS, the Windows or Super key elsewhere.
    pub logo: bool,
}

impl Modifiers {
    pub(crate) const NONE: Self = Self {
        shift: false,
        control: false,
        alt: false,
        logo: false,
    };
    #[cfg(test)]
    pub(crate) const CTRL: Self = Self {
        control: true,
        ..Self::NONE
    };
    #[cfg(test)]
    pub(crate) const SHIFT: Self = Self {
        shift: true,
        ..Self::NONE
    };
    #[cfg(test)]
    pub(crate) const ALT: Self = Self {
        alt: true,
        ..Self::NONE
    };
    #[cfg(test)]
    pub(crate) const LOGO: Self = Self {
        logo: true,
        ..Self::NONE
    };

    pub(crate) const fn empty() -> Self {
        Self::NONE
    }

    pub(crate) fn shift(self) -> bool {
        self.shift
    }

    pub(crate) fn control(self) -> bool {
        self.control
    }

    pub(crate) fn alt(self) -> bool {
        self.alt
    }

    pub(crate) fn logo(self) -> bool {
        self.logo
    }

    /// The modifier that makes a click open a link and a key a command: Cmd on macOS,
    /// Ctrl elsewhere.
    pub(crate) fn command(self) -> bool {
        if cfg!(target_os = "macos") {
            self.logo
        } else {
            self.control
        }
    }

    pub(crate) const fn union(self, other: Self) -> Self {
        Self {
            shift: self.shift || other.shift,
            control: self.control || other.control,
            alt: self.alt || other.alt,
            logo: self.logo || other.logo,
        }
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

/// A key press as the application receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Press {
    pub key: Key,
    pub physical: Physical,
    pub modifiers: Modifiers,
    /// The text the press types, when it types any.
    pub text: Option<String>,
}
