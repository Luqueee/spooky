use crate::pane::render::{RowCache, Style, color, palette, push_cell};
use crate::pane::shell::shell_command;
use crate::search;
use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    term::{Config, Term, TermMode, cell::Flags, point_to_viewport},
    vte::ansi::{CursorShape, CursorStyle, Processor, Rgb},
};
use anyhow::{Context, Result, anyhow};
use portable_pty::{ChildKiller, MasterPty, PtySize, native_pty_system};
use spooky_core::{
    Cursor, CursorShape as WireCursorShape, FindDirection, Highlight, Line, MAX_CLIPBOARD_BYTES,
    MouseEncoding, MouseTracking, Pane as PaneSnapshot, Response,
};
use std::{
    io::{self, Read, Write},
    path::Path,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::Duration,
};

pub(crate) mod render;
pub(crate) mod shell;

const INPUT_QUEUE: usize = 8;

#[derive(Default)]
pub(crate) struct Changes {
    pub(crate) revision: Mutex<u64>,
    pub(crate) wake: Condvar,
}
impl Changes {
    pub(crate) fn advance(&self, dirty: &AtomicU64) {
        if let Ok(mut revision) = self.revision.lock() {
            *revision = revision.wrapping_add(1);
            dirty.store(*revision, Ordering::Release);
            self.wake.notify_all();
        }
    }
    pub(crate) fn wait(&self, since: Option<u64>) -> Result<u64> {
        let revision = self
            .revision
            .lock()
            .map_err(|_| anyhow!("revision state poisoned"))?;
        let (revision, _) = self
            .wake
            .wait_timeout_while(revision, Duration::from_secs(1), |revision| {
                since == Some(*revision)
            })
            .map_err(|_| anyhow!("revision state poisoned"))?;
        Ok(*revision)
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Size {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
}
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
}
impl Size {
    fn pty(self) -> PtySize {
        PtySize {
            cols: self.cols,
            rows: self.rows,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// The latest OSC 52 write by a pane's application and how many there have been.
#[derive(Default)]
pub(crate) struct Clipboard {
    pub(crate) text: Mutex<String>,
    pub(crate) seq: AtomicU64,
}

/// The search a pane is showing: what was asked for and where it stopped.
#[derive(Default)]
pub(crate) struct FindState {
    query: String,
    needle: Vec<char>,
    current: Option<search::Match>,
}

/// Longest title kept, in characters.
const MAX_TITLE_CHARS: usize = 120;
/// Search matches reported for one viewport.
const MAX_HIGHLIGHTS: usize = 1000;

/// A terminal-supplied title as safe display text. Titles come from whatever
/// runs in the pane, including programs on remote hosts.
fn clean_title(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TITLE_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        spooky_core::DEFAULT_TITLE.into()
    } else {
        cleaned.to_owned()
    }
}

#[derive(Clone)]
pub(crate) struct Listener {
    pending: Arc<Mutex<Vec<Event>>>,
    title: Arc<Mutex<String>>,
    clipboard: Arc<Clipboard>,
    revision: Arc<AtomicU64>,
    changes: Arc<Changes>,
}
impl Listener {
    fn set_title(&self, title: String) {
        if let Ok(mut value) = self.title.lock()
            && *value != title
        {
            *value = title;
            self.changes.advance(&self.revision);
        }
    }
    fn store_clipboard(&self, text: String) {
        if text.len() > MAX_CLIPBOARD_BYTES {
            info!("ignored an OSC 52 write of {} bytes", text.len());
            return;
        }
        if let Ok(mut held) = self.clipboard.text.lock() {
            *held = text;
            self.clipboard.seq.fetch_add(1, Ordering::AcqRel);
            self.changes.advance(&self.revision);
        }
    }
}
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(title) => self.set_title(clean_title(&title)),
            Event::ResetTitle => self.set_title(spooky_core::DEFAULT_TITLE.into()),
            Event::ClipboardStore(_, text) => self.store_clipboard(text),
            event @ (Event::PtyWrite(_)
            | Event::TextAreaSizeRequest(_)
            | Event::ColorRequest(_, _)
            | Event::ClipboardLoad(_, _)) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.push(event);
                }
            }
            _ => {}
        }
    }
}

pub(crate) struct Pane {
    pub(crate) id: u64,
    pub(crate) terminal: Arc<Mutex<Term<Listener>>>,
    pub(crate) title: Arc<Mutex<String>>,
    pub(crate) clipboard: Arc<Clipboard>,
    pub(crate) dimensions: Arc<AtomicU32>,
    pub(crate) exited: Arc<AtomicBool>,
    pub(crate) master: Mutex<Box<dyn MasterPty + Send>>,
    pub(crate) writer: SyncSender<Vec<u8>>,
    pub(crate) killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pub(crate) revision: Arc<AtomicU64>,
    pub(crate) viewport: Mutex<RowCache>,
    pub(crate) changes: Arc<Changes>,
    pub(crate) find: Mutex<FindState>,
}
impl Drop for Pane {
    fn drop(&mut self) {
        self.terminate();
    }
}
impl Pane {
    pub(crate) fn spawn(
        id: u64,
        cwd: &Path,
        size: Size,
        changes: Arc<Changes>,
        scrollback: usize,
        shim: Option<&crate::shim::Shim>,
    ) -> Result<Arc<Self>> {
        let revision = Arc::new(AtomicU64::new(0));
        let pair = native_pty_system().openpty(size.pty())?;
        let mut reader = pair.master.try_clone_reader()?;
        let mut output = pair.master.take_writer()?;
        let mut cmd = shell_command();
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        if let Some(shim) = shim {
            shim.configure(&mut cmd, id);
        }
        let mut child = pair.slave.spawn_command(cmd).context("starting shell")?;
        drop(pair.slave);
        let killer = child.clone_killer();
        let (writer, rx) = mpsc::sync_channel::<Vec<u8>>(INPUT_QUEUE);
        let title = Arc::new(Mutex::new(spooky_core::DEFAULT_TITLE.to_string()));
        let clipboard = Arc::new(Clipboard::default());
        let dimensions = Arc::new(AtomicU32::new(
            (u32::from(size.rows) << 16) | u32::from(size.cols),
        ));
        let pending = Arc::new(Mutex::new(Vec::new()));
        let listener = Listener {
            pending: pending.clone(),
            title: title.clone(),
            clipboard: clipboard.clone(),
            revision: revision.clone(),
            changes: changes.clone(),
        };
        let terminal = Arc::new(Mutex::new(Term::new(
            terminal_config(scrollback),
            &size,
            listener,
        )));
        let exited = Arc::new(AtomicBool::new(false));
        let pane = Arc::new(Self {
            id,
            terminal: terminal.clone(),
            title,
            clipboard,
            dimensions: dimensions.clone(),
            exited: exited.clone(),
            master: Mutex::new(pair.master),
            writer: writer.clone(),
            killer: Mutex::new(killer),
            revision: revision.clone(),
            changes: changes.clone(),
            viewport: Mutex::new(RowCache::default()),
            find: Mutex::new(FindState::default()),
        });
        thread::spawn(move || {
            while let Ok(bytes) = rx.recv() {
                if output.write_all(&bytes).is_err() {
                    break;
                }
            }
        });
        let output_revision = revision.clone();
        let output_changes = changes.clone();
        thread::spawn(move || {
            let mut parser: Processor = Processor::default();
            let mut buffer = [0; 16384];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let replies = if let Ok(mut term) = terminal.lock() {
                            parser.advance(&mut *term, &buffer[..n]);
                            output_changes.advance(&output_revision);
                            let events = match pending.lock() {
                                Ok(mut events) => std::mem::take(&mut *events),
                                Err(_) => break,
                            };
                            events
                                .into_iter()
                                .filter_map(|event| match event {
                                    Event::PtyWrite(text) => Some(text),
                                    Event::TextAreaSizeRequest(format) => {
                                        let packed = dimensions.load(Ordering::Relaxed);
                                        Some(format(WindowSize {
                                            num_cols: packed as u16,
                                            num_lines: (packed >> 16) as u16,
                                            cell_width: 0,
                                            cell_height: 0,
                                        }))
                                    }
                                    Event::ColorRequest(index, format) => {
                                        let rgb = if index < alacritty_terminal::term::color::COUNT
                                        {
                                            term.colors()[index]
                                                .map_or_else(|| palette(index), |c| [c.r, c.g, c.b])
                                        } else {
                                            palette(index)
                                        };
                                        Some(format(Rgb {
                                            r: rgb[0],
                                            g: rgb[1],
                                            b: rgb[2],
                                        }))
                                    }
                                    Event::ClipboardLoad(_, format) => Some(format("")),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                        } else {
                            break;
                        };
                        for reply in replies {
                            if writer.send(reply.into_bytes()).is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });
        thread::spawn(move || {
            let _ = child.wait();
            exited.store(true, Ordering::Release);
            changes.advance(&revision);
        });
        Ok(pane)
    }
    pub(crate) fn size(&self) -> Size {
        let size = self.dimensions.load(Ordering::Relaxed);
        Size {
            cols: size as u16,
            rows: (size >> 16) as u16,
        }
    }
    /// Renders the viewport. `since` limits `lines` to rows that changed after
    /// that revision; `None` returns every row.
    pub(crate) fn snapshot(&self, since: Option<u64>) -> Result<PaneSnapshot> {
        let term = self
            .terminal
            .lock()
            .map_err(|_| anyhow!("terminal state poisoned"))?;
        let content = term.renderable_content();
        let size = self.size();
        let mut rendered: Vec<Line> = (0..size.rows)
            .map(|row| Line {
                row,
                runs: Vec::new(),
            })
            .collect();
        for indexed in content.display_iter {
            let cell = indexed.cell;
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let Some(point) = point_to_viewport(content.display_offset, indexed.point) else {
                continue;
            };
            let Some(line) = rendered.get_mut(point.line) else {
                continue;
            };
            let mut fg = color(cell.fg, content.colors);
            let mut bg = color(cell.bg, content.colors);
            if cell.flags.contains(Flags::DIM) {
                fg = fg.map(|channel| (u16::from(channel) * 2 / 3) as u8);
            }
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let mut text = if cell.flags.contains(Flags::HIDDEN) {
                " ".to_string()
            } else {
                cell.c.to_string()
            };
            if !cell.flags.contains(Flags::HIDDEN)
                && let Some(chars) = cell.zerowidth()
            {
                text.extend(chars);
            }
            let link = cell.hyperlink();
            push_cell(
                line,
                point.column.0 as u16,
                text,
                Style {
                    fg,
                    bg,
                    bold: cell.flags.contains(Flags::BOLD),
                    italic: cell.flags.contains(Flags::ITALIC),
                    underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                    strikethrough: cell.flags.contains(Flags::STRIKEOUT),
                },
                cell.flags.contains(Flags::WIDE_CHAR),
                link.as_ref().map(|link| link.uri()),
            );
        }
        // Content and its revision bump are committed together under the
        // terminal lock, so this revision is never older than the render. The
        // cache is updated before the lock is released so concurrent clients
        // cannot record an older render over a newer one.
        let revision = self.revision.load(Ordering::Acquire);
        let lines = {
            let mut viewport = self
                .viewport
                .lock()
                .map_err(|_| anyhow!("viewport cache poisoned"))?;
            viewport.update(size.cols, rendered, revision);
            viewport.changed_since(since)
        };
        let cursor_point = point_to_viewport(content.display_offset, content.cursor.point);
        let cursor = Cursor {
            row: cursor_point.map_or(0, |p| p.line.min(usize::from(size.rows - 1)) as u16),
            col: content
                .cursor
                .point
                .column
                .0
                .min(usize::from(size.cols - 1)) as u16,
            visible: content.cursor.shape != CursorShape::Hidden
                && cursor_point.is_some_and(|p| p.line < usize::from(size.rows)),
            shape: match content.cursor.shape {
                CursorShape::Underline => WireCursorShape::Underline,
                CursorShape::Beam => WireCursorShape::Beam,
                _ => WireCursorShape::Block,
            },
            blinking: term.cursor_style().blinking,
        };
        let title = self
            .title
            .lock()
            .map_err(|_| anyhow!("title state poisoned"))?
            .clone();
        let highlights = self.highlights(&term, size)?;
        Ok(PaneSnapshot {
            id: self.id,
            title,
            cols: size.cols,
            rows: size.rows,
            lines,
            cursor,
            exited: self.exited.load(Ordering::Acquire),
            bracketed_paste: content.mode.contains(TermMode::BRACKETED_PASTE),
            application_cursor: content.mode.contains(TermMode::APP_CURSOR),
            mouse_tracking: mouse_tracking(content.mode),
            mouse_encoding: mouse_encoding(content.mode),
            alternate_scroll: content
                .mode
                .contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL),
            history: term.grid().history_size() as u32,
            scrolled: content.display_offset as u32,
            focus_reporting: content.mode.contains(TermMode::FOCUS_IN_OUT),
            clipboard_seq: self.clipboard.seq.load(Ordering::Acquire),
            highlights,
            agent: None,
        })
    }
    pub(crate) fn resize(&self, size: Size) -> Result<()> {
        let master = self
            .master
            .lock()
            .map_err(|_| anyhow!("pty state poisoned"))?;
        let mut term = self
            .terminal
            .lock()
            .map_err(|_| anyhow!("terminal state poisoned"))?;
        master.resize(size.pty())?;
        term.resize(size);
        self.dimensions.store(
            (u32::from(size.rows) << 16) | u32::from(size.cols),
            Ordering::Relaxed,
        );
        self.changes.advance(&self.revision);
        Ok(())
    }
    /// Returns the viewport to live output; typing while scrolled into history
    /// must show the typed text rather than leave the view in the past.
    pub(crate) fn scroll_to_bottom(&self) -> Result<()> {
        let mut term = self
            .terminal
            .lock()
            .map_err(|_| anyhow!("terminal state poisoned"))?;
        if term.grid().display_offset() != 0 {
            term.scroll_display(Scroll::Bottom);
            self.changes.advance(&self.revision);
        }
        Ok(())
    }
    /// Ends the shell if it is still running.
    pub(crate) fn terminate(&self) {
        if !self.exited.load(Ordering::Acquire)
            && let Ok(mut killer) = self.killer.lock()
        {
            let _ = killer.kill();
        }
    }
    /// Search matches inside the viewport, for the pane's current search.
    fn highlights(&self, term: &Term<Listener>, size: Size) -> Result<Vec<Highlight>> {
        let state = self
            .find
            .lock()
            .map_err(|_| anyhow!("search state poisoned"))?;
        if state.needle.is_empty() {
            return Ok(Vec::new());
        }
        let top = -(term.grid().display_offset() as i32);
        let bottom = top + i32::from(size.rows) - 1;
        Ok(search::matches_in(term, &state.needle, top, bottom)
            .into_iter()
            .take(MAX_HIGHLIGHTS)
            .map(|found| Highlight {
                row: (found.line - top) as u16,
                start: found.start,
                end: found.end,
                current: state.current == Some(found),
            })
            .collect())
    }
    /// Searches the whole scrollback and scrolls to the match. An empty
    /// `query` clears the search.
    pub(crate) fn find(&self, query: &str, direction: FindDirection) -> Result<Response> {
        let mut term = self
            .terminal
            .lock()
            .map_err(|_| anyhow!("terminal state poisoned"))?;
        let mut state = self
            .find
            .lock()
            .map_err(|_| anyhow!("search state poisoned"))?;
        if query.is_empty() {
            if !state.query.is_empty() {
                *state = FindState::default();
                self.changes.advance(&self.revision);
            }
            return Ok(Response::Ok);
        }
        let needle = search::needle(query);
        let matches = search::all_matches(&term, &needle);
        let rows = i32::from(self.size().rows);
        let top = -(term.grid().display_offset() as i32);
        let anchor = if state.query == query {
            state.current
        } else {
            None
        };
        state.query = query.to_owned();
        state.needle = needle;
        let chosen = search::choose(
            &matches,
            anchor,
            direction == FindDirection::Next,
            top,
            top + rows - 1,
        );
        state.current = chosen.map(|index| matches[index]);
        if let Some(found) = state.current
            && (found.line < top || found.line >= top + rows)
        {
            // Matches outside the viewport are brought to its middle.
            let wanted = (rows / 2 - found.line).max(0);
            term.scroll_display(Scroll::Delta(wanted - (-top)));
        }
        self.changes.advance(&self.revision);
        Ok(match chosen {
            Some(index) => Response::Found {
                current: index as u32 + 1,
                total: matches.len() as u32,
            },
            None => Response::NotFound,
        })
    }
}

/// Terminal settings. Shells that never request a cursor shape keep the
/// steady bar; applications may change it (DECSCUSR) and reset it to this.
fn terminal_config(scrollback: usize) -> Config {
    Config {
        scrolling_history: scrollback,
        default_cursor_style: CursorStyle {
            shape: CursorShape::Beam,
            blinking: false,
        },
        ..Config::default()
    }
}

/// The most detailed mouse reporting the application enabled.
fn mouse_tracking(mode: TermMode) -> MouseTracking {
    if mode.contains(TermMode::MOUSE_MOTION) {
        MouseTracking::Motion
    } else if mode.contains(TermMode::MOUSE_DRAG) {
        MouseTracking::Drag
    } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
        MouseTracking::Clicks
    } else {
        MouseTracking::Off
    }
}

fn mouse_encoding(mode: TermMode) -> MouseEncoding {
    if mode.contains(TermMode::SGR_MOUSE) {
        MouseEncoding::Sgr
    } else if mode.contains(TermMode::UTF8_MOUSE) {
        MouseEncoding::Utf8
    } else {
        MouseEncoding::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_TITLE_CHARS, clean_title};

    #[test]
    fn a_title_is_plain_bounded_text_or_the_default() {
        assert_eq!(
            clean_title("  vim notes.md \u{7}\u{1b}[31m"),
            "vim notes.md [31m"
        );
        assert_eq!(clean_title("\u{1b}\u{7}   "), "Terminal");
        assert_eq!(clean_title(""), "Terminal");
        let long = "é".repeat(MAX_TITLE_CHARS + 50);
        assert_eq!(clean_title(&long).chars().count(), MAX_TITLE_CHARS);
    }
}
