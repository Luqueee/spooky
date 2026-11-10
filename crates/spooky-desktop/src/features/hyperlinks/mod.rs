//! Following a link the user clicked. Finding links is `terminal::links`; this decides
//! which may be opened and remembers the one under the pointer.
//!
//! What a program prints is untrusted. A link is only ever followed by an explicit
//! Cmd/Ctrl+click, only for the schemes in [`OPENABLE`], and its target is shown before
//! it is followed.

use std::{
    process::{Command, Stdio},
    thread,
};

/// Schemes a click may open. Everything else (`file:`, `ssh:`, `javascript:`,
/// application schemes) is refused: opening it would let output run things.
const OPENABLE: [&str; 3] = ["http", "https", "mailto"];

/// The scheme of `target`, lowercased, when it is a well-formed URL.
fn scheme(target: &str) -> Option<String> {
    let (scheme, rest) = target.split_once(':')?;
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c));
    (valid && !rest.is_empty() && !target.chars().any(|c| c.is_control() || c == ' '))
        .then(|| scheme.to_ascii_lowercase())
}

/// Whether a click may open `target`.
fn openable(target: &str) -> bool {
    scheme(target).is_some_and(|scheme| {
        OPENABLE.contains(&scheme.as_str())
            && target.split_once(':').is_some_and(|(_, rest)| {
                scheme == "mailto" || !rest.trim_start_matches('/').is_empty()
            })
    })
}

/// Opens `target` with the system's default handler.
fn open(target: &str) -> Result<(), String> {
    if !openable(target) {
        return Err(match scheme(target) {
            Some(scheme) => format!(
                "Not opening a {scheme}: link; only {} links are opened",
                OPENABLE.join(", ")
            ),
            None => "Not opening a malformed link".to_owned(),
        });
    }
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = Command::new(program)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Cannot run {program}: {error}"))?;
    // Reaped on its own thread so the handler never lingers as a zombie.
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// The link under the pointer, and what following one reports.
#[derive(Default)]
pub(crate) struct Hyperlinks {
    hover: Option<String>,
}

impl Hyperlinks {
    /// Target of the link under the pointer while the link modifier is held.
    pub(crate) fn hover(&self) -> Option<&str> {
        self.hover.as_deref()
    }

    /// Records the link under the pointer (or none).
    pub(crate) fn set_hover(&mut self, target: Option<String>) {
        self.hover = target;
    }

    /// Follows `target` if it may be opened; the text to show the user about it.
    pub(crate) fn follow(&mut self, target: &str) -> String {
        self.hover = None;
        match open(target) {
            Ok(()) => format!("Opening {target}"),
            Err(error) => error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::openable;

    #[test]
    fn only_web_and_mail_links_may_be_opened() {
        for ok in [
            "https://example.com",
            "HTTP://example.com/a?b=c#d",
            "mailto:me@example.com",
        ] {
            assert!(openable(ok), "{ok}");
        }
        for refused in [
            "file:///etc/passwd",
            "ssh://host",
            "javascript:alert(1)",
            "x-apple.systempreferences:com.apple.preference",
            "https://",
            "https:///",
            "https://exa mple.com",
            "https://example.com/\u{7}",
            " https://example.com",
            "example.com",
            "",
        ] {
            assert!(!openable(refused), "{refused:?}");
        }
    }
}
