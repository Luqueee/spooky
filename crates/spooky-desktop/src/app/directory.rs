//! Selection and freshness of the directory shown in the header.

use crate::link::DirectorySelection;
use spooky_core::DirectoryKind;
use std::time::{Duration, Instant};

/// A slow or lost reply must not leave a previously verified path on screen.
const MAX_AGE: Duration = Duration::from_secs(3);
/// Bound untrusted paths before displaying or placing them on the clipboard.
const MAX_PATH_BYTES: usize = 16 * 1024;

#[derive(Default)]
pub(super) struct Directory {
    selection: Option<DirectorySelection>,
    epoch: u64,
    value: Option<(String, Instant)>,
}

impl Directory {
    /// Returns a new subscription when the target, pane or pane kind changed.
    pub(super) fn select(
        &mut self,
        wanted: Option<(u64, u64, DirectoryKind)>,
    ) -> Option<Option<DirectorySelection>> {
        if self.selection.map(|s| (s.generation, s.pane, s.kind)) == wanted {
            return None;
        }
        self.epoch = self.epoch.wrapping_add(1);
        self.value = None;
        self.selection = wanted.map(|(generation, pane, kind)| DirectorySelection {
            generation,
            pane,
            kind,
            epoch: self.epoch,
        });
        Some(self.selection)
    }

    /// A command may change the foreground process or cwd before the next poll.
    pub(super) fn invalidate(&mut self, pane: u64) -> Option<DirectorySelection> {
        let mut selection = self.selection.filter(|selection| selection.pane == pane)?;
        self.epoch = self.epoch.wrapping_add(1);
        selection.epoch = self.epoch;
        self.selection = Some(selection);
        self.value = None;
        Some(selection)
    }

    pub(super) fn receive(
        &mut self,
        selection: DirectorySelection,
        result: Option<(DirectoryKind, String)>,
        started: Instant,
    ) {
        if self.selection != Some(selection) {
            return;
        }
        self.value = result
            .filter(|(kind, path)| {
                *kind == selection.kind
                    && path.starts_with('/')
                    && !path.contains('\0')
                    && path.len() <= MAX_PATH_BYTES
            })
            .map(|(_, path)| (path, started));
    }

    pub(super) fn current(&self) -> Option<(DirectorySelection, &str)> {
        self.current_at(Instant::now())
    }

    fn current_at(&self, now: Instant) -> Option<(DirectorySelection, &str)> {
        let selection = self.selection?;
        let (path, started) = self.value.as_ref()?;
        (now.saturating_duration_since(*started) <= MAX_AGE).then_some((selection, path.as_str()))
    }

    pub(super) fn matches(&self, selection: DirectorySelection, path: &str) -> bool {
        self.matches_at(selection, path, Instant::now())
    }

    fn matches_at(&self, selection: DirectorySelection, path: &str, now: Instant) -> bool {
        self.current_at(now) == Some((selection, path))
    }

    pub(super) fn text(&self, remote: bool) -> String {
        self.text_at(remote, Instant::now())
    }

    fn text_at(&self, remote: bool, now: Instant) -> String {
        let Some((selection, path)) = self.current_at(now) else {
            return "Directory unknown".into();
        };
        let label = match (remote, selection.kind) {
            (false, DirectoryKind::Shell) => "Directory (recent)",
            (true, DirectoryKind::Shell) => "SSH directory (recent)",
            (false, DirectoryKind::AgentStart) => "Agent start",
            (true, DirectoryKind::AgentStart) => "SSH agent start",
        };
        format!("{label}: {}", safe_path(path))
    }
}

impl super::App {
    pub(super) fn directory_message(
        &mut self,
        message: super::Message,
    ) -> crate::runtime::Task<super::Message> {
        use super::Message;
        match message {
            Message::ToggleDirectoryMenu => {
                if self.directory_menu.is_some() {
                    self.directory_menu = None;
                } else if let Some((selection, path)) = self.directory.current() {
                    self.directory_menu = Some((selection, path.to_owned()));
                    self.menu_open = false;
                    self.context_menu = None;
                }
            }
            Message::CopyDirectory | Message::OpenDirectory => {
                let Some((selection, path)) = self.directory_menu.take() else {
                    return crate::runtime::Task::none();
                };
                if !self.online() || self.hidden || !self.directory.matches(selection, &path) {
                    self.status = "Directory is no longer available".into();
                    return crate::runtime::Task::none();
                }
                if matches!(message, Message::CopyDirectory) {
                    self.status = "Directory copied".into();
                    return crate::runtime::Task::clipboard_write(path);
                }
                if self.connection.is_remote() || self.directory_opening {
                    return crate::runtime::Task::none();
                }
                self.directory_opening = true;
                self.status = "Opening folder…".into();
                return crate::runtime::Task::perform(open_folder(path), Message::DirectoryOpened);
            }
            Message::DirectoryOpened(result) => {
                self.directory_opening = false;
                self.status = match result {
                    Ok(()) => "Folder opened".into(),
                    Err(error) => format!("Could not open folder: {error}"),
                };
            }
            _ => {}
        }
        crate::runtime::Task::none()
    }
}

async fn open_folder(path: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
        if !metadata.is_dir() {
            return Err("the path is no longer a directory".into());
        }
        #[cfg(target_os = "macos")]
        let mut command = {
            let mut command = std::process::Command::new("/usr/bin/open");
            command.args(["-a", "Finder"]);
            command
        };
        #[cfg(target_os = "linux")]
        let mut command = std::process::Command::new("xdg-open");
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return Err("opening folders is not supported on this platform".into());
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let status = command
                .arg(path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map_err(|error| error.to_string())?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("folder opener exited with {status}"))
            }
        }
    })
    .await
    .map_err(|error| format!("folder opener failed: {error}"))?
}

/// A filename is untrusted: do not let control or bidi characters alter the UI.
fn safe_path(path: &str) -> String {
    sanitize_path(path, 512)
}

/// Display the complete observed path without letting it change the surrounding UI.
pub(super) fn full_path(path: &str) -> String {
    sanitize_path(path, usize::MAX)
}

fn sanitize_path(path: &str, limit: usize) -> String {
    use std::fmt::Write;
    let mut shown = String::new();
    let mut chars = path.chars();
    for ch in chars.by_ref().take(limit) {
        if ch.is_control()
            || matches!(ch, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            let _ = write!(shown, "\\u{{{:x}}}", u32::from(ch));
        } else {
            shown.push(ch);
        }
    }
    if chars.next().is_some() {
        shown.push('…');
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_changes_and_input_discard_old_replies() {
        let mut directory = Directory::default();
        let first = directory
            .select(Some((1, 7, DirectoryKind::Shell)))
            .expect("initial selection changed")
            .expect("shell selected");
        let now = Instant::now();
        directory.receive(first, Some((DirectoryKind::Shell, "/first".into())), now);
        assert!(directory.text_at(false, now).contains("/first"));
        let second = directory
            .select(Some((1, 8, DirectoryKind::Shell)))
            .expect("second selection changed")
            .expect("second shell selected");
        directory.receive(first, Some((DirectoryKind::Shell, "/first".into())), now);
        assert_eq!(directory.text_at(false, now), "Directory unknown");
        let again = directory
            .select(Some((1, 7, DirectoryKind::Shell)))
            .expect("return selection changed")
            .expect("original shell selected");
        directory.receive(first, Some((DirectoryKind::Shell, "/first".into())), now);
        directory.receive(second, Some((DirectoryKind::Shell, "/second".into())), now);
        assert_eq!(directory.text_at(false, now), "Directory unknown");
        let changed = directory.invalidate(7).expect("selected shell invalidated");
        directory.receive(again, Some((DirectoryKind::Shell, "/old".into())), now);
        assert_eq!(directory.text_at(false, now), "Directory unknown");
        directory.receive(changed, Some((DirectoryKind::Shell, "/new".into())), now);
        assert_eq!(directory.text_at(false, now), "Directory (recent): /new");
        directory.select(None);
        assert_eq!(directory.text_at(false, now), "Directory unknown");
    }

    #[test]
    fn results_expire_and_agent_start_is_not_a_live_directory() {
        let mut directory = Directory::default();
        let shell = directory
            .select(Some((1, 1, DirectoryKind::Shell)))
            .expect("shell selection changed")
            .expect("shell selected");
        let now = Instant::now();
        directory.receive(
            shell,
            Some((DirectoryKind::AgentStart, "/wrong".into())),
            now,
        );
        assert_eq!(directory.text_at(false, now), "Directory unknown");
        directory.receive(shell, Some((DirectoryKind::Shell, "/old".into())), now);
        assert_eq!(
            directory.text_at(false, now + MAX_AGE + Duration::from_millis(1)),
            "Directory unknown"
        );
        directory.receive(shell, None, now);
        assert_eq!(directory.text_at(false, now), "Directory unknown");
        let agent = directory
            .select(Some((1, 1, DirectoryKind::AgentStart)))
            .expect("agent selection changed")
            .expect("agent selected");
        directory.receive(
            agent,
            Some((DirectoryKind::AgentStart, "/start".into())),
            now,
        );
        assert_eq!(directory.text_at(true, now), "SSH agent start: /start");
    }

    #[test]
    fn directory_actions_require_the_exact_fresh_observation() {
        let mut directory = Directory::default();
        let first = directory
            .select(Some((1, 7, DirectoryKind::Shell)))
            .expect("selection changed")
            .expect("shell selected");
        let now = Instant::now();
        directory.receive(
            first,
            Some((DirectoryKind::Shell, "/one\nfolder".into())),
            now,
        );
        assert!(directory.matches_at(first, "/one\nfolder", now));
        assert!(!directory.matches_at(first, "/one", now));
        assert!(!directory.matches_at(
            first,
            "/one\nfolder",
            now + MAX_AGE + Duration::from_millis(1)
        ));
        let next = directory.invalidate(7).expect("selection invalidated");
        directory.receive(
            next,
            Some((DirectoryKind::Shell, "/one\nfolder".into())),
            now,
        );
        assert!(!directory.matches_at(first, "/one\nfolder", now));
        assert!(directory.matches_at(next, "/one\nfolder", now));
        directory.receive(next, Some((DirectoryKind::Shell, "relative".into())), now);
        assert!(!directory.matches_at(next, "relative", now));
        directory.receive(
            next,
            Some((DirectoryKind::Shell, format!("/{}", "x".repeat(16_384)))),
            now,
        );
        assert!(directory.current_at(now).is_none());
    }

    #[test]
    fn path_cannot_inject_display_controls_or_grow_without_bound() {
        assert_eq!(safe_path("/a\n\u{202e}b"), "/a\\u{a}\\u{202e}b");
        assert_eq!(safe_path(&"x".repeat(513)).chars().count(), 513);
        let long = format!("/{}\nend", "x".repeat(600));
        let full = full_path(&long);
        assert!(full.ends_with("\\u{a}end"));
        assert_eq!(full.chars().filter(|ch| *ch == 'x').count(), 600);
    }
}
