//! Persistent Unix PTY sessions and the SSH stdio bridge.

#[cfg(unix)]
#[macro_use]
mod log;
#[cfg(unix)]
mod persist;
#[cfg(unix)]
mod search;

#[cfg(unix)]
mod unix {
    use crate::{
        log,
        persist::{self, Saved},
        search,
    };
    use alacritty_terminal::{
        event::{Event, EventListener, WindowSize},
        grid::{Dimensions, Scroll},
        term::{Config, Term, TermMode, cell::Flags, point_to_viewport},
        vte::ansi::{Color, CursorShape, CursorStyle, NamedColor, Processor, Rgb},
    };
    use anyhow::{Context, Result, anyhow, bail, ensure};
    use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
    use serde::Deserialize;
    use spooky_core::{
        Cursor, CursorShape as WireCursorShape, EVEN_SPLIT, FindDirection, Highlight, Layout, Line,
        MAX_CLIPBOARD_BYTES, MAX_MESSAGE_BYTES, MIN_SPLIT_SHARE, MouseEncoding, MouseTracking,
        PROTOCOL_VERSION, Pane as PaneSnapshot, Request, Response, Run, ServerStatus, Snapshot,
        Tab, Workspace,
    };
    use std::{
        collections::BTreeMap,
        env,
        fs::{self, OpenOptions},
        io::{self, BufRead, BufReader, Read, Write},
        os::unix::{
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
        path::{Path, PathBuf},
        sync::{
            Arc, Condvar, Mutex,
            atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
            mpsc::{self, SyncSender},
        },
        thread,
        time::{Duration, Instant},
    };

    const INPUT_QUEUE: usize = 8;

    #[derive(Default)]
    struct Changes {
        revision: Mutex<u64>,
        wake: Condvar,
    }
    impl Changes {
        fn advance(&self, dirty: &AtomicU64) {
            if let Ok(mut revision) = self.revision.lock() {
                *revision = revision.wrapping_add(1);
                dirty.store(*revision, Ordering::Release);
                self.wake.notify_all();
            }
        }
        fn wait(&self, since: Option<u64>) -> Result<u64> {
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
    struct Size {
        cols: u16,
        rows: u16,
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
    struct Clipboard {
        text: Mutex<String>,
        seq: AtomicU64,
    }

    /// The search a pane is showing: what was asked for and where it stopped.
    #[derive(Default)]
    struct FindState {
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
    struct Listener {
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

    struct Pane {
        id: u64,
        terminal: Arc<Mutex<Term<Listener>>>,
        title: Arc<Mutex<String>>,
        clipboard: Arc<Clipboard>,
        dimensions: Arc<AtomicU32>,
        exited: Arc<AtomicBool>,
        master: Mutex<Box<dyn MasterPty + Send>>,
        writer: SyncSender<Vec<u8>>,
        killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
        revision: Arc<AtomicU64>,
        viewport: Mutex<RowCache>,
        changes: Arc<Changes>,
        find: Mutex<FindState>,
    }
    impl Drop for Pane {
        fn drop(&mut self) {
            self.terminate();
        }
    }
    impl Pane {
        fn spawn(
            id: u64,
            cwd: &Path,
            size: Size,
            changes: Arc<Changes>,
            scrollback: usize,
        ) -> Result<Arc<Self>> {
            let revision = Arc::new(AtomicU64::new(0));
            let pair = native_pty_system().openpty(size.pty())?;
            let mut reader = pair.master.try_clone_reader()?;
            let mut output = pair.master.take_writer()?;
            let mut cmd = shell_command();
            cmd.cwd(cwd);
            cmd.env("TERM", "xterm-256color");
            cmd.env("COLORTERM", "truecolor");
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
                                            let rgb =
                                                if index < alacritty_terminal::term::color::COUNT {
                                                    term.colors()[index].map_or_else(
                                                        || palette(index),
                                                        |c| [c.r, c.g, c.b],
                                                    )
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
        fn size(&self) -> Size {
            let size = self.dimensions.load(Ordering::Relaxed);
            Size {
                cols: size as u16,
                rows: (size >> 16) as u16,
            }
        }
        /// Renders the viewport. `since` limits `lines` to rows that changed after
        /// that revision; `None` returns every row.
        fn snapshot(&self, since: Option<u64>) -> Result<PaneSnapshot> {
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
            })
        }
        fn resize(&self, size: Size) -> Result<()> {
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
        fn scroll_to_bottom(&self) -> Result<()> {
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
        fn terminate(&self) {
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
        fn find(&self, query: &str, direction: FindDirection) -> Result<Response> {
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

    /// The last rendered viewport of a pane and the revision at which each of its
    /// rows last changed, so any client can be sent exactly the rows it lacks.
    #[derive(Default)]
    struct RowCache {
        cols: u16,
        lines: Vec<Line>,
        revisions: Vec<u64>,
    }
    impl RowCache {
        /// Records a render taken at `revision`. Rows that differ from the cached
        /// ones take that revision; a different grid size replaces every row.
        fn update(&mut self, cols: u16, rendered: Vec<Line>, revision: u64) {
            if self.cols != cols || self.lines.len() != rendered.len() {
                self.cols = cols;
                self.revisions = vec![revision; rendered.len()];
                self.lines = rendered;
                return;
            }
            for ((cached, changed_at), line) in
                self.lines.iter_mut().zip(&mut self.revisions).zip(rendered)
            {
                if *cached != line {
                    *cached = line;
                    *changed_at = revision;
                }
            }
        }
        /// Rows that changed after `since`; every row when `since` is `None`.
        fn changed_since(&self, since: Option<u64>) -> Vec<Line> {
            self.lines
                .iter()
                .zip(&self.revisions)
                .filter(|(_, changed_at)| since.is_none_or(|since| **changed_at > since))
                .map(|(line, _)| line.clone())
                .collect()
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

    struct Style {
        fg: [u8; 3],
        bg: [u8; 3],
        bold: bool,
        italic: bool,
        underline: bool,
        strikethrough: bool,
    }

    /// Hyperlink targets longer than this are dropped.
    const MAX_LINK_BYTES: usize = 2048;

    /// An OSC 8 target the client may be shown: nonempty, bounded, and free of
    /// control characters. The application chose it, so it stays untrusted.
    fn link_target(uri: Option<&str>) -> Option<&str> {
        uri.filter(|uri| {
            !uri.is_empty() && uri.len() <= MAX_LINK_BYTES && !uri.chars().any(char::is_control)
        })
    }

    /// Appends one terminal cell to its row. Blank cells on the default background
    /// are left implicit; neighbouring plain cells with the same style and link
    /// share a run. A wide cell or one with combining characters is always a run
    /// of its own.
    fn push_cell(
        line: &mut Line,
        col: u16,
        text: String,
        style: Style,
        wide: bool,
        link: Option<&str>,
    ) {
        let link = link_target(link);
        if text == " "
            && style.bg == spooky_core::DEFAULT_BACKGROUND
            && !style.underline
            && !style.strikethrough
            && link.is_none()
        {
            return;
        }
        let single = wide || text.chars().nth(1).is_some();
        if !single
            && let Some(last) = line.runs.last_mut()
            && last.width == 0
            && last.fg == style.fg
            && last.bg == style.bg
            && last.bold == style.bold
            && last.italic == style.italic
            && last.underline == style.underline
            && last.strikethrough == style.strikethrough
            && last.link.as_deref() == link
            && last.col + last.columns() == col
        {
            last.text.push_str(&text);
            return;
        }
        line.runs.push(Run {
            col,
            text,
            fg: style.fg,
            bg: style.bg,
            bold: style.bold,
            italic: style.italic,
            underline: style.underline,
            strikethrough: style.strikethrough,
            width: if wide { 2 } else { u8::from(single) },
            link: link.map(str::to_owned),
        });
    }

    /// Locale supplied to shells when the server's environment has none.
    const DEFAULT_MACOS_LOCALE: &str = "en_US.UTF-8";

    fn env_is_unset(name: &str) -> bool {
        env::var_os(name).is_none_or(|value| value.is_empty())
    }

    /// Builds the shell command for a new pane.
    ///
    /// macOS runs a login shell so `~/.zprofile` (PATH, Homebrew) applies even
    /// when the GUI was launched from Finder, whose environment has neither it
    /// nor a locale; a UTF-8 locale is supplied only when `LC_ALL` and `LANG` are
    /// both unset. Other platforms keep the plain `$SHELL` invocation.
    fn shell_command() -> CommandBuilder {
        if cfg!(target_os = "macos") {
            let mut cmd = CommandBuilder::new_default_prog();
            if env_is_unset("LC_ALL") && env_is_unset("LANG") {
                cmd.env("LANG", DEFAULT_MACOS_LOCALE);
            }
            cmd
        } else {
            let shell = env::var_os("SHELL")
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "/bin/sh".into());
            CommandBuilder::new(shell)
        }
    }

    fn palette(index: usize) -> [u8; 3] {
        const BASE: [[u8; 3]; 16] = [
            [0, 0, 0],
            [205, 49, 49],
            [13, 188, 121],
            [229, 229, 16],
            [36, 114, 200],
            [188, 63, 188],
            [17, 168, 205],
            [229, 229, 229],
            [102, 102, 102],
            [241, 76, 76],
            [35, 209, 139],
            [245, 245, 67],
            [59, 142, 234],
            [214, 112, 214],
            [41, 184, 219],
            [255, 255, 255],
        ];
        match index {
            0..=15 => BASE[index],
            16..=231 => {
                let n = index - 16;
                let component = |v: usize| if v == 0 { 0 } else { (55 + 40 * v) as u8 };
                [component(n / 36), component((n / 6) % 6), component(n % 6)]
            }
            232..=255 => {
                let c = (8 + (index - 232) * 10) as u8;
                [c, c, c]
            }
            257 => spooky_core::DEFAULT_BACKGROUND,
            _ => [220, 220, 220],
        }
    }
    fn color(value: Color, colors: &alacritty_terminal::term::color::Colors) -> [u8; 3] {
        let index = match value {
            Color::Spec(rgb) => return [rgb.r, rgb.g, rgb.b],
            Color::Indexed(i) => usize::from(i),
            Color::Named(n) => n as usize,
        };
        if let Some(rgb) = colors[index] {
            [rgb.r, rgb.g, rgb.b]
        } else if index >= NamedColor::DimBlack as usize && index <= NamedColor::DimWhite as usize {
            let p = palette(index - NamedColor::DimBlack as usize);
            [p[0] / 2, p[1] / 2, p[2] / 2]
        } else {
            palette(index)
        }
    }

    struct State {
        next_id: u64,
        workspaces: Vec<Workspace>,
        panes: BTreeMap<u64, Arc<Pane>>,
    }

    /// Per-machine server settings, read from `spooky/server.toml` in the
    /// configuration directory.
    #[derive(Deserialize)]
    #[serde(default, deny_unknown_fields)]
    struct Settings {
        /// Scrollback lines kept per pane.
        scrollback_lines: usize,
    }
    impl Default for Settings {
        fn default() -> Self {
            Self {
                scrollback_lines: 10_000,
            }
        }
    }
    impl Settings {
        const MAX_SCROLLBACK: usize = 1_000_000;

        fn load() -> Result<Self> {
            let Some(base) = env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            else {
                return Ok(Self::default());
            };
            let path = base.join("spooky/server.toml");
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok(Self::default());
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("reading {}", path.display()));
                }
            };
            let settings: Self =
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
            ensure!(
                settings.scrollback_lines <= Self::MAX_SCROLLBACK,
                "{}: scrollback_lines must be at most {}",
                path.display(),
                Self::MAX_SCROLLBACK
            );
            Ok(settings)
        }
    }

    /// How long a burst of layout changes settles before it is written out.
    const SAVE_DELAY: Duration = Duration::from_millis(300);

    /// Wakes the thread that writes the saved workspaces.
    #[derive(Default)]
    struct Saver {
        dirty: Mutex<bool>,
        wake: Condvar,
    }
    impl Saver {
        fn mark(&self) {
            if let Ok(mut dirty) = self.dirty.lock() {
                *dirty = true;
                self.wake.notify_one();
            }
        }
        /// Blocks until something was marked, then takes the mark. False once
        /// the state is unusable.
        fn take(&self) -> bool {
            let Ok(mut dirty) = self.dirty.lock() else {
                return false;
            };
            while !*dirty {
                dirty = match self.wake.wait(dirty) {
                    Ok(dirty) => dirty,
                    Err(_) => return false,
                };
            }
            *dirty = false;
            true
        }
    }

    /// The size of the largest pane, which is the window-sized one when nothing
    /// is split, or `fallback` when no pane exists.
    fn largest_pane(state: &State, fallback: Size) -> Size {
        state
            .panes
            .values()
            .map(|pane| pane.size())
            .max_by_key(|size| u32::from(size.cols) * u32::from(size.rows))
            .unwrap_or(fallback)
    }

    struct Server {
        state: Mutex<State>,
        changes: Arc<Changes>,
        layout_revision: AtomicU64,
        settings: Settings,
        started: Instant,
        /// Size for new panes while no pane exists to copy one from.
        fallback_size: Size,
        state_dir: PathBuf,
        saver: Saver,
        /// Serializes writers of the saved workspaces.
        save_lock: Mutex<()>,
        /// Set by `Shutdown`; the connection that asked stops the server once it
        /// has been answered.
        stop_requested: AtomicBool,
    }
    impl Server {
        fn new(settings: Settings, state_dir: PathBuf, fallback_size: Size) -> Self {
            Self {
                state: Mutex::new(State {
                    next_id: 1,
                    workspaces: Vec::new(),
                    panes: BTreeMap::new(),
                }),
                changes: Arc::new(Changes::default()),
                layout_revision: AtomicU64::new(0),
                settings,
                started: Instant::now(),
                fallback_size,
                state_dir,
                saver: Saver::default(),
                save_lock: Mutex::new(()),
                stop_requested: AtomicBool::new(false),
            }
        }
        fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>> {
            self.state
                .lock()
                .map_err(|_| anyhow!("server state poisoned"))
        }
        fn changed(&self) {
            self.changes.advance(&self.layout_revision);
            self.saver.mark();
        }
        fn reserve_id(&self) -> Result<u64> {
            let mut state = self.lock()?;
            let id = state.next_id;
            state.next_id = id.checked_add(1).context("identifier space exhausted")?;
            Ok(id)
        }
        fn pane(&self, id: u64) -> Result<Arc<Pane>> {
            self.lock()?
                .panes
                .get(&id)
                .cloned()
                .context("pane not found")
        }
        /// Grid size for a pane that is not a split half.
        fn pane_size(&self) -> Result<Size> {
            Ok(largest_pane(&*self.lock()?, self.fallback_size))
        }
        fn spawn_pane(&self, id: u64, cwd: &Path, size: Size) -> Result<Arc<Pane>> {
            Pane::spawn(
                id,
                cwd,
                size,
                self.changes.clone(),
                self.settings.scrollback_lines,
            )
        }
        fn status(&self) -> Result<ServerStatus> {
            let state = self.lock()?;
            Ok(ServerStatus {
                pid: std::process::id(),
                version: PROTOCOL_VERSION,
                uptime_secs: self.started.elapsed().as_secs(),
                workspaces: state.workspaces.len() as u32,
                tabs: state.workspaces.iter().map(|w| w.tabs.len() as u32).sum(),
                panes: state.panes.len() as u32,
                running: state
                    .panes
                    .values()
                    .filter(|pane| !pane.exited.load(Ordering::Acquire))
                    .count() as u32,
            })
        }
        /// Writes the workspaces so the next server can restore them.
        fn save(&self) -> Result<()> {
            let _serial = self
                .save_lock
                .lock()
                .map_err(|_| anyhow!("save state poisoned"))?;
            let (next_id, workspaces, size) = {
                let state = self.lock()?;
                (
                    state.next_id,
                    state.workspaces.clone(),
                    largest_pane(&state, self.fallback_size),
                )
            };
            persist::store(
                &self.state_dir,
                &Saved::new(next_id, workspaces).sized(size.cols, size.rows),
            )
        }
        /// Rebuilds saved workspaces with fresh shells in their directories. A
        /// tab whose shells cannot all be started is left out.
        fn restore(&self, saved: Saved) -> Result<()> {
            let (cols, rows) = saved.size();
            let size = Size { cols, rows };
            let mut state = self.lock()?;
            state.next_id = saved.next_id;
            for mut workspace in saved.workspaces {
                let cwd = restore_cwd(&workspace.cwd);
                workspace.cwd = cwd.to_string_lossy().into_owned();
                let mut tabs = Vec::new();
                for tab in std::mem::take(&mut workspace.tabs) {
                    let mut ids = Vec::new();
                    tab.layout.panes(&mut ids);
                    let spawned = ids
                        .into_iter()
                        .map(|id| Ok((id, self.spawn_pane(id, &cwd, size)?)))
                        .collect::<Result<Vec<_>>>();
                    match spawned {
                        Ok(panes) => {
                            state.panes.extend(panes);
                            tabs.push(tab);
                        }
                        Err(error) => info!(
                            "left out saved tab {} of workspace {}: {error:#}",
                            tab.id, workspace.id
                        ),
                    }
                }
                if !tabs.is_empty() {
                    workspace.tabs = tabs;
                    state.workspaces.push(workspace);
                }
            }
            Ok(())
        }
        /// Saves the workspaces, ends every shell and exits.
        fn stop(&self) -> ! {
            if let Err(error) = self.save() {
                info!("final save failed: {error:#}");
            }
            let panes = self
                .lock()
                .map(|mut state| std::mem::take(&mut state.panes))
                .unwrap_or_default();
            for pane in panes.values() {
                pane.terminate();
            }
            if let Ok(path) = spooky_core::paths::socket_path() {
                let _ = fs::remove_file(path);
            }
            info!("stopped");
            std::process::exit(0)
        }
        fn dispatch(&self, request: Request) -> Response {
            match self.execute(request) {
                Ok(response) => response,
                Err(error) => Response::Error(format!("{error:#}")),
            }
        }
        fn execute(&self, request: Request) -> Result<Response> {
            match request {
                Request::Ping => {
                    return Ok(Response::Pong {
                        version: PROTOCOL_VERSION,
                    });
                }
                Request::Snapshot { since } => {
                    let revision = self.changes.wait(since)?;
                    if since == Some(revision) {
                        return Ok(Response::Unchanged { revision });
                    }
                    let (workspaces, panes) = {
                        let state = self.lock()?;
                        (
                            state.workspaces.clone(),
                            state.panes.values().cloned().collect::<Vec<_>>(),
                        )
                    };
                    // A topology change needs every viewport row, including newly
                    // inserted panes whose output may predate their insertion.
                    let full = since.is_none_or(|since| {
                        since > revision || self.layout_revision.load(Ordering::Acquire) > since
                    });
                    let delta = if full { None } else { since };
                    let panes = panes
                        .iter()
                        .filter(|pane| {
                            delta.is_none_or(|since| pane.revision.load(Ordering::Acquire) > since)
                        })
                        .map(|pane| pane.snapshot(delta))
                        .collect::<Result<Vec<_>>>()?;
                    return Ok(Response::Snapshot(Snapshot {
                        version: PROTOCOL_VERSION,
                        revision,
                        workspaces,
                        panes,
                    }));
                }
                Request::CreateWorkspace { name, cwd } => {
                    validate_name(&name)?;
                    let cwd = validated_cwd(cwd.as_deref())?;
                    let workspace = self.reserve_id()?;
                    let tab = self.reserve_id()?;
                    let id = self.reserve_id()?;
                    let size = self.pane_size()?;
                    let pane = self.spawn_pane(id, &cwd, size)?;
                    let mut state = self.lock()?;
                    state.panes.insert(id, pane);
                    state.workspaces.push(Workspace {
                        id: workspace,
                        name,
                        cwd: cwd.to_string_lossy().into_owned(),
                        tabs: vec![Tab {
                            id: tab,
                            title: spooky_core::DEFAULT_TITLE.into(),
                            renamed: false,
                            layout: Layout::Pane(id),
                        }],
                    });
                    self.changed();
                }
                Request::RenameWorkspace { workspace, name } => {
                    validate_name(&name)?;
                    let mut state = self.lock()?;
                    state
                        .workspaces
                        .iter_mut()
                        .find(|w| w.id == workspace)
                        .context("workspace not found")?
                        .name = name;
                    self.changed();
                }
                Request::CreateTab { workspace, title } => {
                    validate_name(&title)?;
                    let cwd = {
                        let state = self.lock()?;
                        PathBuf::from(
                            &state
                                .workspaces
                                .iter()
                                .find(|w| w.id == workspace)
                                .context("workspace not found")?
                                .cwd,
                        )
                    };
                    let id = self.reserve_id()?;
                    let tab = self.reserve_id()?;
                    let size = self.pane_size()?;
                    let pane = self.spawn_pane(id, &cwd, size)?;
                    let mut state = self.lock()?;
                    let ws = state
                        .workspaces
                        .iter_mut()
                        .find(|w| w.id == workspace)
                        .context("workspace closed during tab creation")?;
                    ws.tabs.push(Tab {
                        id: tab,
                        title,
                        renamed: false,
                        layout: Layout::Pane(id),
                    });
                    state.panes.insert(id, pane);
                    self.changed();
                }
                Request::RenameTab { tab, title } => {
                    validate_name(&title)?;
                    let mut state = self.lock()?;
                    let tab = state
                        .workspaces
                        .iter_mut()
                        .flat_map(|w| &mut w.tabs)
                        .find(|t| t.id == tab)
                        .context("tab not found")?;
                    tab.title = title;
                    tab.renamed = true;
                    self.changed();
                }
                Request::CloseTab { tab } => {
                    let removed = {
                        let mut state = self.lock()?;
                        let wi = state
                            .workspaces
                            .iter()
                            .position(|w| w.tabs.iter().any(|t| t.id == tab))
                            .context("tab not found")?;
                        let ws = &mut state.workspaces[wi];
                        let ti = ws
                            .tabs
                            .iter()
                            .position(|t| t.id == tab)
                            .context("tab not found")?;
                        let closed = ws.tabs.remove(ti);
                        if ws.tabs.is_empty() {
                            state.workspaces.remove(wi);
                        }
                        let mut ids = Vec::new();
                        closed.layout.panes(&mut ids);
                        let panes = ids
                            .into_iter()
                            .filter_map(|id| state.panes.remove(&id))
                            .collect::<Vec<_>>();
                        self.changed();
                        panes
                    };
                    // Shells are terminated outside the lock.
                    drop(removed);
                }
                Request::Split { pane, axis } => {
                    let (cwd, size) = {
                        let state = self.lock()?;
                        let p = state.panes.get(&pane).context("pane not found")?;
                        let ws = state
                            .workspaces
                            .iter()
                            .find(|w| w.tabs.iter().any(|t| t.layout.contains(pane)))
                            .context("pane is detached")?;
                        (PathBuf::from(&ws.cwd), p.size())
                    };
                    let id = self.reserve_id()?;
                    let split = self.reserve_id()?;
                    let new_pane = self.spawn_pane(id, &cwd, size)?;
                    let mut state = self.lock()?;
                    let tab = state
                        .workspaces
                        .iter_mut()
                        .flat_map(|w| &mut w.tabs)
                        .find(|t| t.layout.contains(pane))
                        .context("pane closed during split")?;
                    replace_leaf(
                        &mut tab.layout,
                        pane,
                        Layout::Split {
                            id: split,
                            axis,
                            ratio: EVEN_SPLIT,
                            first: Box::new(Layout::Pane(pane)),
                            second: Box::new(Layout::Pane(id)),
                        },
                    )?;
                    state.panes.insert(id, new_pane);
                    self.changed();
                }
                Request::Input { pane, bytes } => {
                    if bytes.len() > MAX_MESSAGE_BYTES {
                        bail!("input exceeds protocol size limit");
                    }
                    let pane = self.pane(pane)?;
                    if pane.exited.load(Ordering::Acquire) {
                        bail!("pane has exited");
                    }
                    pane.writer
                        .try_send(bytes)
                        .map_err(|e| anyhow!("terminal input unavailable: {e}"))?;
                    pane.scroll_to_bottom()?;
                }
                Request::Resize { pane, cols, rows } => {
                    validate_size(cols, rows)?;
                    self.pane(pane)?.resize(Size { cols, rows })?;
                }
                Request::Scroll { pane, lines } => {
                    let pane = self.pane(pane)?;
                    let mut term = pane
                        .terminal
                        .lock()
                        .map_err(|_| anyhow!("terminal state poisoned"))?;
                    term.scroll_display(Scroll::Delta(lines));
                    pane.changes.advance(&pane.revision);
                }
                Request::ClosePane { pane } => {
                    let removed = {
                        let mut state = self.lock()?;
                        let wi = state
                            .workspaces
                            .iter()
                            .position(|w| w.tabs.iter().any(|t| t.layout.contains(pane)))
                            .context("pane not found")?;
                        let ws = &mut state.workspaces[wi];
                        let ti = ws
                            .tabs
                            .iter()
                            .position(|t| t.layout.contains(pane))
                            .context("pane not found")?;
                        if matches!(ws.tabs[ti].layout, Layout::Pane(_)) {
                            ws.tabs.remove(ti);
                        } else {
                            remove_leaf(&mut ws.tabs[ti].layout, pane)?;
                        }
                        if ws.tabs.is_empty() {
                            state.workspaces.remove(wi);
                        }
                        let removed = state.panes.remove(&pane);
                        self.changed();
                        removed
                    };
                    drop(removed);
                }
                Request::CloseWorkspace { workspace } => {
                    let removed = {
                        let mut state = self.lock()?;
                        let index = state
                            .workspaces
                            .iter()
                            .position(|w| w.id == workspace)
                            .context("workspace not found")?;
                        let ws = state.workspaces.remove(index);
                        let mut ids = Vec::new();
                        for tab in ws.tabs {
                            tab.layout.panes(&mut ids);
                        }
                        let panes = ids
                            .into_iter()
                            .filter_map(|id| state.panes.remove(&id))
                            .collect::<Vec<_>>();
                        self.changed();
                        panes
                    };
                    drop(removed);
                }
                Request::SetSplitRatio { split, ratio } => {
                    if !(MIN_SPLIT_SHARE..=1000 - MIN_SPLIT_SHARE).contains(&ratio) {
                        bail!("split ratio out of range");
                    }
                    let mut state = self.lock()?;
                    let moved = state
                        .workspaces
                        .iter_mut()
                        .flat_map(|w| &mut w.tabs)
                        .any(|t| t.layout.set_ratio(split, ratio));
                    if !moved {
                        bail!("split not found");
                    }
                    self.changed();
                }
                Request::Find {
                    pane,
                    query,
                    direction,
                } => {
                    if query.len() > 1024 {
                        bail!("search text is too long");
                    }
                    return self.pane(pane)?.find(&query, direction);
                }
                Request::Clipboard { pane } => {
                    let pane = self.pane(pane)?;
                    let (seq, text) = {
                        let held = pane
                            .clipboard
                            .text
                            .lock()
                            .map_err(|_| anyhow!("clipboard state poisoned"))?;
                        (pane.clipboard.seq.load(Ordering::Acquire), held.clone())
                    };
                    return Ok(Response::Clipboard { seq, text });
                }
                Request::Status => return Ok(Response::Status(self.status()?)),
                Request::Shutdown => {
                    info!("shutdown requested");
                    self.stop_requested.store(true, Ordering::Release);
                }
            }
            Ok(Response::Ok)
        }
    }
    fn replace_leaf(layout: &mut Layout, pane: u64, replacement: Layout) -> Result<()> {
        match layout {
            Layout::Pane(id) if *id == pane => {
                *layout = replacement;
                Ok(())
            }
            Layout::Split { first, .. } if first.contains(pane) => {
                replace_leaf(first, pane, replacement)
            }
            Layout::Split { second, .. } => replace_leaf(second, pane, replacement),
            _ => bail!("pane not in layout"),
        }
    }
    fn remove_leaf(layout: &mut Layout, pane: u64) -> Result<()> {
        match layout {
            Layout::Split { first, second, .. } if matches!(**first, Layout::Pane(id) if id == pane) =>
            {
                *layout = (**second).clone();
                Ok(())
            }
            Layout::Split { first, second, .. } if matches!(**second, Layout::Pane(id) if id == pane) =>
            {
                *layout = (**first).clone();
                Ok(())
            }
            Layout::Split { first, .. } if first.contains(pane) => remove_leaf(first, pane),
            Layout::Split { second, .. } => remove_leaf(second, pane),
            _ => bail!("pane not in split"),
        }
    }
    fn validate_name(name: &str) -> Result<()> {
        if name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
            bail!("name must be nonempty, at most 256 bytes, and contain no control characters");
        }
        Ok(())
    }
    fn validate_size(cols: u16, rows: u16) -> Result<()> {
        if !(2..=500).contains(&cols) || !(1..=300).contains(&rows) {
            bail!("terminal size out of bounds");
        }
        Ok(())
    }
    fn validated_cwd(cwd: Option<&str>) -> Result<PathBuf> {
        let path = match cwd {
            Some(s) if !s.is_empty() => PathBuf::from(s),
            Some(_) => bail!("working directory is empty"),
            None => env::current_dir()?,
        };
        let path = path.canonicalize().context("invalid working directory")?;
        if !path.is_dir() {
            bail!("working directory is not a directory");
        }
        if path.to_str().is_none() {
            bail!("working directory is not valid UTF-8");
        }
        Ok(path)
    }

    fn serve() -> Result<()> {
        let path = spooky_core::paths::socket_path()?;
        let lock_path = path.with_extension("lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)
            .context("opening server lock")?;
        let metadata = lock.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
        {
            bail!("server lock is not a private file owned by this user");
        }
        lock.try_lock()
            .context("another server owns the socket lock")?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.file_type().is_socket()
                    || metadata.uid() != rustix::process::geteuid().as_raw()
                    || metadata.mode() & 0o777 != 0o600
                {
                    bail!("existing socket path is not a private socket owned by this user");
                }
                match UnixStream::connect(&path) {
                    Ok(_) => bail!("server already listening"),
                    Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                        fs::remove_file(&path).context("removing stale socket")?
                    }
                    Err(e) => return Err(e).context("cannot establish whether socket is stale"),
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("checking socket path"),
        }
        let state_dir = spooky_core::paths::state_dir().context("preparing the state directory")?;
        log::init(&state_dir).context("opening the server log")?;
        info!(
            "starting server (protocol {PROTOCOL_VERSION}, pid {})",
            std::process::id()
        );
        let settings = Settings::load().unwrap_or_else(|error| {
            info!("using default settings: {error:#}");
            Settings::default()
        });
        let saved = persist::load(&state_dir).unwrap_or_else(|error| {
            info!("{error:#}");
            None
        });
        let fallback = saved.as_ref().map_or(Size { cols: 80, rows: 24 }, |saved| {
            let (cols, rows) = saved.size();
            Size { cols, rows }
        });
        let server = Arc::new(Server::new(settings, state_dir, fallback));
        if let Some(saved) = saved {
            let count = saved.workspaces.len();
            match server.restore(saved) {
                Ok(()) => info!("restored {count} saved workspace(s)"),
                Err(error) => info!("restoring saved workspaces failed: {error:#}"),
            }
        }
        let listener = UnixListener::bind(&path).context("binding server socket")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let saver = server.clone();
        thread::spawn(move || save_loop(&saver));
        info!("listening on {}", path.display());
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let server = server.clone();
                    thread::spawn(move || {
                        if let Err(error) = handle_client(stream, server) {
                            info!("client connection ended: {error:#}");
                        }
                    });
                }
                Err(error) => info!("accept failed: {error}"),
            }
        }
        Ok(())
    }
    /// Writes the saved workspaces after each burst of layout changes.
    fn save_loop(server: &Server) {
        while server.saver.take() {
            thread::sleep(SAVE_DELAY);
            if let Err(error) = server.save() {
                info!("saving workspaces failed: {error:#}");
            }
        }
    }
    /// The directory a restored workspace's shells start in: the saved one if it
    /// still exists, otherwise the home directory.
    fn restore_cwd(saved: &str) -> PathBuf {
        validated_cwd(Some(saved))
            .or_else(|_| validated_cwd(env::var("HOME").ok().as_deref()))
            .or_else(|_| validated_cwd(None))
            .unwrap_or_else(|_| PathBuf::from("/"))
    }
    fn handle_client(mut stream: UnixStream, server: Arc<Server>) -> Result<()> {
        let mut reader = BufReader::new(stream.try_clone()?);
        loop {
            let mut line = Vec::new();
            let n = (&mut reader)
                .take((MAX_MESSAGE_BYTES + 1) as u64)
                .read_until(b'\n', &mut line)?;
            if n == 0 {
                return Ok(());
            }
            if n > MAX_MESSAGE_BYTES || !line.ends_with(b"\n") {
                write_response(
                    &mut stream,
                    &Response::Error("request too large or unterminated".into()),
                )?;
                return Ok(());
            }
            let response = match serde_json::from_slice(&line) {
                Ok(request) => server.dispatch(request),
                Err(e) => Response::Error(format!("invalid request: {e}")),
            };
            write_response(&mut stream, &response)?;
            if server.stop_requested.load(Ordering::Acquire) {
                server.stop();
            }
        }
    }
    fn write_response(output: &mut impl Write, response: &Response) -> Result<()> {
        let mut bytes = serde_json::to_vec(response)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_MESSAGE_BYTES {
            bytes = serde_json::to_vec(&Response::Error(
                "response exceeds protocol size limit".into(),
            ))?;
            bytes.push(b'\n');
        }
        output.write_all(&bytes)?;
        output.flush()?;
        Ok(())
    }
    fn bridge() -> Result<()> {
        use spooky_core::client::{Client, Target};
        let mut client = Client::connect(&Target::Local)?;
        let stdin = io::stdin();
        let mut input = stdin.lock();
        let stdout = io::stdout();
        let mut output = stdout.lock();
        loop {
            let mut line = Vec::new();
            let n = (&mut input)
                .take((MAX_MESSAGE_BYTES + 1) as u64)
                .read_until(b'\n', &mut line)?;
            if n == 0 {
                break;
            }
            if n > MAX_MESSAGE_BYTES || !line.ends_with(b"\n") {
                write_response(
                    &mut output,
                    &Response::Error("request too large or unterminated".into()),
                )?;
                break;
            }
            let response = match serde_json::from_slice::<Request>(&line) {
                Ok(request) => match client.request(&request) {
                    Ok(response) => response,
                    Err(error) => Response::Error(format!("server request failed: {error:#}")),
                },
                Err(error) => Response::Error(format!("invalid request: {error}")),
            };
            write_response(&mut output, &response)?;
        }
        Ok(())
    }
    /// The running local server, if any. Never starts one.
    fn running_server() -> Result<Option<spooky_core::client::Client>> {
        spooky_core::client::Client::attach_local().context(
            "talking to the running server; one from an older Spooky cannot be asked to stop, end its process instead",
        )
    }
    fn server_status(client: &mut spooky_core::client::Client) -> Result<ServerStatus> {
        match client.request(&Request::Status)? {
            Response::Status(status) => Ok(status),
            Response::Error(message) => bail!("server refused: {message}"),
            other => bail!("unexpected reply to status: {other:?}"),
        }
    }
    fn status() -> Result<()> {
        let Some(mut client) = running_server()? else {
            println!("no Spooky server is running");
            return Ok(());
        };
        let status = server_status(&mut client)?;
        println!(
            "Spooky server pid {}, protocol {}, up {}s: {} workspace(s), {} tab(s), {} shell(s) of which {} running",
            status.pid,
            status.version,
            status.uptime_secs,
            status.workspaces,
            status.tabs,
            status.panes,
            status.running
        );
        Ok(())
    }
    /// Stops the local server. Refuses while shells are running unless `force`,
    /// because stopping ends them.
    fn stop(force: bool) -> Result<()> {
        let Some(mut client) = running_server()? else {
            println!("no Spooky server is running");
            return Ok(());
        };
        let status = server_status(&mut client)?;
        if status.running > 0 && !force {
            bail!(
                "{} shell(s) are running in {} workspace(s) and stopping the server ends them. \
                 Run `spooky-server stop --force` to stop anyway; the workspaces come back, with \
                 new shells, the next time a server starts",
                status.running,
                status.workspaces
            );
        }
        match client.request(&Request::Shutdown)? {
            Response::Ok => {}
            Response::Error(message) => bail!("server refused to stop: {message}"),
            other => bail!("unexpected reply to shutdown: {other:?}"),
        }
        for _ in 0..200 {
            if matches!(spooky_core::client::Client::attach_local(), Ok(None)) {
                println!("stopped the Spooky server (pid {})", status.pid);
                return Ok(());
            }
            thread::sleep(Duration::from_millis(25));
        }
        bail!("the server acknowledged the request but is still running after 5 seconds")
    }
    pub fn main() -> Result<()> {
        let mut args = env::args().skip(1);
        match args.next().as_deref().unwrap_or("serve") {
            "serve" => serve(),
            "bridge" => bridge(),
            "status" => status(),
            "stop" => match args.next().as_deref() {
                None => stop(false),
                Some("--force") => stop(true),
                Some(other) => bail!("unknown option for stop: {other}"),
            },
            mode => bail!("unknown server mode: {mode} (expected serve, bridge, status or stop)"),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{MAX_LINK_BYTES, MAX_TITLE_CHARS, clean_title, link_target};

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

        #[test]
        fn a_link_target_must_be_bounded_and_free_of_control_characters() {
            assert_eq!(
                link_target(Some("https://example.com/a?b=c")),
                Some("https://example.com/a?b=c")
            );
            assert_eq!(link_target(None), None);
            assert_eq!(link_target(Some("")), None);
            assert_eq!(link_target(Some("https://exa\u{7}mple.com")), None);
            assert_eq!(link_target(Some("https://example.com/\n")), None);
            let long = format!("https://example.com/{}", "a".repeat(MAX_LINK_BYTES));
            assert_eq!(link_target(Some(&long)), None);
        }
    }
}

#[cfg(unix)]
fn main() -> anyhow::Result<()> {
    unix::main()
}
#[cfg(not(unix))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("spooky-server requires Unix; Windows is unsupported")
}
