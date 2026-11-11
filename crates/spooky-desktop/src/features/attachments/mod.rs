//! Getting things in and out of a pane: files attached through a native dialog or
//! dropped on the window, whose uploaded path is typed into the shell (quoted, never
//! run), and a PNG capture of a pane saved through a native dialog.
//!
//! The transfer itself runs on the connection's own thread; this feature chooses the
//! file, reports what to upload, and keeps track of whether a native dialog is open
//! (it takes the keyboard focus without the user having clicked away).

pub(crate) mod preview;

use iced::{Rectangle, Task, window};
use spooky_core::Request;
use std::path::PathBuf;

/// What the attach and capture controls, the file dialogs and file drops tell the
/// application.
#[derive(Clone, Debug)]
pub(crate) enum Message {
    /// Pick a file to attach to the selected pane.
    Attach,
    /// The file dialog closed; with the chosen file, if any.
    Picked(Option<PathBuf>),
    /// A file was dropped on the window.
    Dropped(PathBuf),
    /// Capture the selected pane as a PNG.
    Capture,
    /// The window's pixels, and the area of the pane within them.
    Captured(window::Screenshot, Rectangle),
    /// The capture was saved (the path), or why it was not.
    Saved(Result<String, String>),
}

/// What the application knows that this feature needs.
pub(crate) struct Context {
    /// The selected pane.
    pub pane: Option<u64>,
    /// The application window.
    pub window: Option<window::Id>,
    /// Where the selected pane is drawn, in logical pixels of the window.
    pub pane_bounds: Option<Rectangle>,
}

/// What a message asks the application to do.
pub(crate) struct Outcome {
    /// Follow-up work, such as opening a dialog.
    pub task: Task<Message>,
    /// A file to upload for this pane.
    pub upload: Option<(u64, PathBuf)>,
    /// A line for the status bar.
    pub status: Option<String>,
}

impl Outcome {
    fn nothing() -> Self {
        Self {
            task: Task::none(),
            upload: None,
            status: None,
        }
    }

    fn task(task: Task<Message>) -> Self {
        Self {
            task,
            ..Self::nothing()
        }
    }

    fn status(status: impl Into<String>) -> Self {
        Self {
            status: Some(status.into()),
            ..Self::nothing()
        }
    }
}

/// Whether a native dialog of ours is open.
#[derive(Default)]
pub(crate) struct Attachments {
    dialog_open: bool,
}

impl Attachments {
    /// Whether a native file dialog is open, so that losing the keyboard focus to it is
    /// not the user clicking away.
    pub(crate) fn dialog_open(&self) -> bool {
        self.dialog_open
    }

    /// Applies a message.
    pub(crate) fn update(&mut self, message: Message, context: Context) -> Outcome {
        match message {
            Message::Attach => {
                if context.pane.is_none() {
                    return Outcome::nothing();
                }
                self.dialog_open = true;
                Outcome::task(Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .pick_file()
                            .await
                            .map(|file| file.path().to_path_buf())
                    },
                    Message::Picked,
                ))
            }
            Message::Picked(None) => {
                self.dialog_open = false;
                Outcome::nothing()
            }
            Message::Picked(Some(path)) | Message::Dropped(path) => {
                self.dialog_open = false;
                Outcome {
                    upload: context.pane.map(|pane| (pane, path)),
                    ..Outcome::nothing()
                }
            }
            Message::Capture => match (context.window, context.pane_bounds) {
                (Some(window), Some(bounds)) => Outcome::task(
                    window::screenshot(window).map(move |shot| Message::Captured(shot, bounds)),
                ),
                _ => Outcome::status("No visible pane to capture"),
            },
            Message::Captured(shot, bounds) => {
                self.dialog_open = true;
                Outcome::task(Task::perform(save_capture(shot, bounds), Message::Saved))
            }
            Message::Saved(result) => {
                self.dialog_open = false;
                Outcome::status(match result {
                    Ok(path) => format!("Saved capture: {path}"),
                    Err(error) => format!("Capture: {error}"),
                })
            }
        }
    }
}

/// The area of a screenshot taken at `scale` that a pane at `bounds` (logical pixels)
/// covers, at least one pixel in each direction.
fn crop_area(bounds: Rectangle, scale: f32) -> Rectangle<u32> {
    Rectangle {
        x: (bounds.x * scale).round().max(0.0) as u32,
        y: (bounds.y * scale).round().max(0.0) as u32,
        width: (bounds.width * scale).floor().max(1.0) as u32,
        height: (bounds.height * scale).floor().max(1.0) as u32,
    }
}

/// Crops the pane out of the screenshot and saves it where the user chooses.
async fn save_capture(shot: window::Screenshot, bounds: Rectangle) -> Result<String, String> {
    let cropped = shot
        .crop(crop_area(bounds, shot.scale_factor))
        .map_err(|error| error.to_string())?;
    let file = rfd::AsyncFileDialog::new()
        .set_file_name("spooky-pane.png")
        .add_filter("PNG", &["png"])
        .save_file()
        .await
        .ok_or_else(|| "Capture cancelled".to_owned())?;
    image::save_buffer_with_format(
        file.path(),
        cropped.as_ref(),
        cropped.size.width,
        cropped.size.height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|error| error.to_string())?;
    Ok(file.path().display().to_string())
}

/// A path as a message names it: quoted when it has blanks, so where it ends is clear.
pub(crate) fn quoted_path(path: &str) -> String {
    if path.chars().any(char::is_whitespace) {
        shell_quote(path)
    } else {
        path.to_owned()
    }
}

/// The request that types an uploaded file's path into a pane: shell-quoted, without
/// Enter, so it is never run by itself.
pub(crate) fn insert_path(pane: u64, path: &str) -> Request {
    Request::Input {
        pane,
        bytes: shell_quote(path).into_bytes(),
    }
}

/// `text` as one single-quoted shell word.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::Point;

    fn context(pane: Option<u64>) -> Context {
        Context {
            pane,
            window: None,
            pane_bounds: None,
        }
    }

    #[test]
    fn an_uploaded_path_is_typed_quoted_and_never_followed_by_enter() {
        let typed = |path: &str| match insert_path(3, path) {
            Request::Input { pane: 3, bytes } => String::from_utf8(bytes).expect("utf-8"),
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(typed("/tmp/a b.png"), "'/tmp/a b.png'");
        assert_eq!(typed("it's a shot.png"), r"'it'\''s a shot.png'");
        assert_eq!(typed("$(rm -rf ~)"), "'$(rm -rf ~)'");
        assert!(!typed("x\n").ends_with("\r"));
    }

    #[test]
    fn attaching_needs_a_pane_and_marks_the_dialog_until_it_closes() {
        let mut attachments = Attachments::default();
        attachments.update(Message::Attach, context(None));
        assert!(!attachments.dialog_open());
        attachments.update(Message::Attach, context(Some(1)));
        assert!(attachments.dialog_open());
        let outcome = attachments.update(Message::Picked(None), context(Some(1)));
        assert!(!attachments.dialog_open());
        assert!(outcome.upload.is_none());
    }

    #[test]
    fn a_chosen_or_dropped_file_is_uploaded_for_the_selected_pane_only() {
        let mut attachments = Attachments::default();
        for message in [
            Message::Picked(Some("/tmp/a".into())),
            Message::Dropped("/tmp/a".into()),
        ] {
            let outcome = attachments.update(message.clone(), context(Some(4)));
            assert_eq!(outcome.upload, Some((4, PathBuf::from("/tmp/a"))));
            let outcome = attachments.update(message, context(None));
            assert!(
                outcome.upload.is_none(),
                "no pane, nowhere to type the path"
            );
        }
    }

    #[test]
    fn a_capture_needs_a_visible_pane_and_reports_how_it_ended() {
        let mut attachments = Attachments::default();
        let outcome = attachments.update(Message::Capture, context(Some(1)));
        assert_eq!(
            outcome.status.as_deref(),
            Some("No visible pane to capture")
        );
        attachments.dialog_open = true;
        let saved = attachments.update(Message::Saved(Ok("/tmp/x.png".into())), context(None));
        assert_eq!(saved.status.as_deref(), Some("Saved capture: /tmp/x.png"));
        assert!(!attachments.dialog_open());
        let failed = attachments.update(Message::Saved(Err("disk full".into())), context(None));
        assert_eq!(failed.status.as_deref(), Some("Capture: disk full"));
    }

    #[test]
    fn a_path_named_in_a_message_is_quoted_only_when_it_has_blanks() {
        assert_eq!(quoted_path("/tmp/a.png"), "/tmp/a.png");
        assert_eq!(quoted_path("/tmp/a b.png"), "'/tmp/a b.png'");
        assert_eq!(
            quoted_path("/tmp/it's here.png"),
            "'/tmp/it'\\''s here.png'"
        );
    }

    #[test]
    fn the_crop_follows_the_screen_scale_and_is_never_empty_or_negative() {
        let bounds = Rectangle::new(Point::new(10.4, 20.0), iced::Size::new(100.9, 50.0));
        assert_eq!(
            crop_area(bounds, 2.0),
            Rectangle {
                x: 21,
                y: 40,
                width: 201,
                height: 100
            }
        );
        let off = Rectangle::new(Point::new(-5.0, -5.0), iced::Size::new(0.0, 0.0));
        let area = crop_area(off, 2.0);
        assert_eq!((area.x, area.y), (0, 0));
        assert_eq!((area.width, area.height), (1, 1));
    }
}
