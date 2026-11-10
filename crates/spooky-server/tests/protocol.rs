//! Real PTY protocol regressions for detach, emulation and pane-tree lifetime.
#![cfg(unix)]

use spooky_core::{Axis, FindDirection, Layout, Request, Response, Snapshot};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Server {
    child: Child,
    dir: PathBuf,
    socket: PathBuf,
}
impl Server {
    fn start() -> Self {
        Self::start_with(|_| {})
    }
    /// Starts a server; `configure` runs before the standard socket and shell variables.
    fn start_with(configure: impl FnOnce(&mut Command)) -> Self {
        static NEXT_SERVER: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_SERVER.fetch_add(1, Ordering::Relaxed);
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir =
            std::env::temp_dir().join(format!("spooky-test-{}-{suffix}-{id}", std::process::id()));
        fs::create_dir(&dir).unwrap_or_else(|e| panic!("create test directory: {e}"));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|e| panic!("set permissions: {e}"));
        Self::start_in(dir, configure)
    }
    /// Starts a server over an existing private directory, which is also where
    /// the server keeps its log and saved workspaces.
    fn start_in(dir: PathBuf, configure: impl FnOnce(&mut Command)) -> Self {
        let socket = dir.join("s");
        let mut command = Command::new(env!("CARGO_BIN_EXE_spooky-server"));
        configure(&mut command);
        command
            .arg("serve")
            .env("SPOOKY_SOCKET", &socket)
            .env("SHELL", "/bin/sh")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = command
            .spawn()
            .unwrap_or_else(|e| panic!("start server: {e}"));
        let deadline = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(&socket).is_err() {
            assert!(Instant::now() < deadline, "server socket did not appear");
            thread::sleep(Duration::from_millis(10));
        }
        Self { child, dir, socket }
    }
    /// Runs a `spooky-server` subcommand against this server's socket.
    fn cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_spooky-server"))
            .args(args)
            .env("SPOOKY_SOCKET", &self.socket)
            .output()
            .unwrap_or_else(|e| panic!("run spooky-server {args:?}: {e}"))
    }
    /// Waits for the server to exit on its own and hands back its directory so a
    /// replacement can be started over it.
    fn wait_for_exit(mut self) -> PathBuf {
        wait_for(|| matches!(self.child.try_wait(), Ok(Some(_))));
        std::mem::take(&mut self.dir)
    }
    fn connect(&self) -> Connection {
        let stream = UnixStream::connect(&self.socket).unwrap_or_else(|e| panic!("connect: {e}"));
        let reader = BufReader::new(
            stream
                .try_clone()
                .unwrap_or_else(|e| panic!("clone socket: {e}")),
        );
        Connection { stream, reader }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}
struct Connection {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}
impl Connection {
    fn request(&mut self, request: Request) -> Response {
        serde_json::to_writer(&mut self.stream, &request)
            .unwrap_or_else(|e| panic!("encode request: {e}"));
        self.stream
            .write_all(b"\n")
            .unwrap_or_else(|e| panic!("write request: {e}"));
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .unwrap_or_else(|e| panic!("read response: {e}"));
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("decode response {line:?}: {e}"))
    }
    fn snapshot(&mut self) -> Snapshot {
        match self.request(Request::Snapshot { since: None }) {
            Response::Snapshot(s) => s,
            other => panic!("expected snapshot, got {other:?}"),
        }
    }
}
fn ok(response: Response) {
    assert!(matches!(response, Response::Ok), "{response:?}");
}
fn wait_for(mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "condition timed out");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn screen_survives_disconnect_and_interprets_terminal_sequences() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "work".into(),
        cwd: Some("/".into()),
    }));
    let initial = client.snapshot();
    let pane = initial.panes[0].id;
    ok(client.request(Request::Input { pane, bytes: b"printf '\\033[2J\\033[H\\033[38;2;1;2;3mX\\033[48;2;0;0;0mB\\033[40mI\\033[49mD\\033[0m\\033[?2004h\\033[?1h\\033]2;MyTitle\\007'\n".to_vec() }));
    drop(client);
    let mut client = server.connect();
    wait_for(|| {
        let snapshot = client.snapshot();
        let p = &snapshot.panes[0];
        p.title == "MyTitle"
            && p.bracketed_paste
            && p.application_cursor
            && has_cell(p, "X", |run| run.fg == [1, 2, 3])
            && has_cell(p, "B", |run| run.bg == [0, 0, 0])
            && has_cell(p, "I", |run| run.bg == [0, 0, 0])
            && has_cell(p, "D", |run| run.bg != [0, 0, 0])
    });
    let snapshot = client.snapshot();
    assert!(snapshot.revision > initial.revision);
    assert!(matches!(
        client.request(Request::Snapshot {
            since: Some(snapshot.revision)
        }),
        Response::Unchanged { .. }
    ));
    ok(client.request(Request::CloseWorkspace {
        workspace: snapshot.workspaces[0].id,
    }));
    assert!(client.snapshot().panes.is_empty());
}

#[test]
fn split_and_close_final_panes_keep_tree_consistent() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "work".into(),
        cwd: Some("/".into()),
    }));
    let original = client.snapshot().panes[0].id;
    ok(client.request(Request::Split {
        pane: original,
        axis: Axis::Horizontal,
    }));
    let snapshot = client.snapshot();
    assert_eq!(snapshot.panes.len(), 2);
    let other = snapshot
        .panes
        .iter()
        .find(|p| p.id != original)
        .map(|p| p.id)
        .unwrap_or_else(|| panic!("second pane missing"));
    ok(client.request(Request::ClosePane { pane: original }));
    let snapshot = client.snapshot();
    assert_eq!(snapshot.panes.len(), 1);
    assert_eq!(snapshot.panes[0].id, other);
    ok(client.request(Request::ClosePane { pane: other }));
    let snapshot = client.snapshot();
    assert!(snapshot.workspaces.is_empty());
    assert!(snapshot.panes.is_empty());
}

#[test]
fn a_tab_is_renamed_and_closed_with_every_shell_it_holds() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "work".into(),
        cwd: Some("/".into()),
    }));
    let workspace = client.snapshot().workspaces[0].id;
    ok(client.request(Request::CreateTab {
        workspace,
        title: "second".into(),
    }));
    let snapshot = client.snapshot();
    let (first, second) = (
        snapshot.workspaces[0].tabs[0].clone(),
        snapshot.workspaces[0].tabs[1].clone(),
    );
    let mut first_panes = Vec::new();
    first.layout.panes(&mut first_panes);
    ok(client.request(Request::Split {
        pane: first_panes[0],
        axis: Axis::Vertical,
    }));
    assert_eq!(client.snapshot().panes.len(), 3);

    ok(client.request(Request::RenameTab {
        tab: first.id,
        title: "build".into(),
    }));
    let titles: Vec<String> = client.snapshot().workspaces[0]
        .tabs
        .iter()
        .map(|tab| tab.title.clone())
        .collect();
    assert_eq!(titles, ["build", "second"]);
    for rejected in [
        Request::RenameTab {
            tab: first.id,
            title: "  ".into(),
        },
        Request::RenameTab {
            tab: u64::MAX,
            title: "x".into(),
        },
        Request::CloseTab { tab: u64::MAX },
    ] {
        let response = client.request(rejected);
        assert!(matches!(response, Response::Error(_)), "{response:?}");
    }

    ok(client.request(Request::CloseTab { tab: first.id }));
    let snapshot = client.snapshot();
    assert_eq!(snapshot.workspaces[0].tabs.len(), 1);
    let mut remaining = Vec::new();
    second.layout.panes(&mut remaining);
    assert_eq!(
        snapshot.panes.iter().map(|p| p.id).collect::<Vec<_>>(),
        remaining,
        "both panes of the closed tab are gone, the other tab's pane is kept"
    );

    ok(client.request(Request::CloseTab { tab: second.id }));
    let snapshot = client.snapshot();
    assert!(
        snapshot.workspaces.is_empty(),
        "the last tab takes its workspace with it"
    );
    assert!(snapshot.panes.is_empty());
}

#[test]
fn waiting_snapshot_returns_only_the_pane_changed_by_pty_output() {
    let server = Server::start();
    let mut commands = server.connect();
    for name in ["active", "background"] {
        ok(commands.request(Request::CreateWorkspace {
            name: name.into(),
            cwd: Some("/".into()),
        }));
    }
    let initial = commands.snapshot();
    for pane in &initial.panes {
        ok(commands.request(Request::Input {
            pane: pane.id,
            bytes: b"stty -echo; printf '\\033[2J\\033[HREADY\\n'; exec cat\n".to_vec(),
        }));
    }
    wait_for(|| {
        commands
            .snapshot()
            .panes
            .iter()
            .all(|pane| pane.row_text(0).starts_with("READY"))
    });
    let initial = commands.snapshot();
    let active = initial.panes[0].id;
    let mut updates = server.connect();
    updates
        .stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap_or_else(|error| panic!("set update deadline: {error}"));
    let revision = initial.revision;
    let waiting = thread::spawn(move || {
        updates.request(Request::Snapshot {
            since: Some(revision),
        })
    });
    ok(commands.request(Request::Input {
        pane: active,
        bytes: b"X\n".to_vec(),
    }));
    let Response::Snapshot(update) = waiting
        .join()
        .unwrap_or_else(|_| panic!("update reader panicked"))
    else {
        panic!("PTY output must wake the pending snapshot");
    };
    assert_eq!(
        update.panes.iter().map(|pane| pane.id).collect::<Vec<_>>(),
        [active],
        "unchanged terminal grids must not travel with each keystroke"
    );
    assert_eq!(
        update.panes[0]
            .lines
            .iter()
            .map(|line| line.row)
            .collect::<Vec<_>>(),
        [1],
        "only the changed row travels, not the whole viewport"
    );
    assert_eq!(update.panes[0].row_text(1), "X");
    let background = initial.panes[1].id;
    let mut current = initial;
    current.apply_update(update);
    assert!(
        current
            .panes
            .iter()
            .find(|pane| pane.id == background)
            .is_some_and(|pane| pane.row_text(0).starts_with('R'))
    );

    ok(commands.request(Request::Input {
        pane: active,
        bytes: b"\x1b[2J\x1b[H\n".to_vec(),
    }));
    let Response::Snapshot(erased) = commands.request(Request::Snapshot {
        since: Some(current.revision),
    }) else {
        panic!("erasing output must invalidate the viewport");
    };
    current.apply_update(erased);
    assert!(
        current
            .panes
            .iter()
            .find(|pane| pane.id == active)
            .is_some_and(|pane| (0..pane.rows).all(|row| pane.row_text(row).is_empty()))
    );
    ok(commands.request(Request::ClosePane { pane: background }));
    let Response::Snapshot(removed) = commands.request(Request::Snapshot {
        since: Some(current.revision),
    }) else {
        panic!("closing a pane must invalidate the layout");
    };
    current.apply_update(removed);
    assert_eq!(
        current.panes.iter().map(|pane| pane.id).collect::<Vec<_>>(),
        [active]
    );
}

/// Visible text of each viewport row.
fn screen_rows(pane: &spooky_core::Pane) -> Vec<String> {
    (0..pane.rows).map(|row| pane.row_text(row)).collect()
}

fn shows_line(client: &mut Connection, line: &str) -> bool {
    screen_rows(&client.snapshot().panes[0])
        .iter()
        .any(|row| row.trim_end() == line)
}

#[test]
fn input_returns_a_scrolled_viewport_to_live_output() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "scroll".into(),
        cwd: Some("/".into()),
    }));
    let pane = client.snapshot().panes[0].id;
    ok(client.request(Request::Resize {
        pane,
        cols: 80,
        rows: 5,
    }));
    ok(client.request(Request::Input {
        pane,
        bytes: b"seq 1 60\n".to_vec(),
    }));
    wait_for(|| shows_line(&mut client, "60"));
    ok(client.request(Request::Scroll { pane, lines: 30 }));
    assert!(
        !shows_line(&mut client, "60"),
        "a scrolled viewport must show history"
    );
    ok(client.request(Request::Input {
        pane,
        bytes: b"x".to_vec(),
    }));
    assert!(
        shows_line(&mut client, "60"),
        "typing must return the viewport to live output"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_shells_are_login_shells_with_a_utf8_locale_without_a_gui_environment() {
    // Finder launches provide neither the user's profile nor a locale.
    let server = Server::start_with(|command| {
        command.env_clear();
        command.env("PATH", "/usr/bin:/bin");
        if let Some(home) = std::env::var_os("HOME") {
            command.env("HOME", home);
        }
    });
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "env".into(),
        cwd: Some("/".into()),
    }));
    let pane = client.snapshot().panes[0].id;
    ok(client.request(Request::Input {
        pane,
        bytes: b"printf 'login=%s lang=%s\\n' \"$0\" \"$LANG\"\n".to_vec(),
    }));
    wait_for(|| shows_line(&mut client, "login=-sh lang=en_US.UTF-8"));
}

/// Whether some cell of the pane shows `text` in a run accepted by `style`.
fn has_cell(
    pane: &spooky_core::Pane,
    text: &str,
    style: impl Fn(&spooky_core::Run) -> bool,
) -> bool {
    pane.lines
        .iter()
        .flat_map(|line| &line.runs)
        .any(|run| style(run) && run.cells().any(|(_, cell, _)| cell == text))
}

/// Applies updates until the server reports nothing further changed.
fn settle(client: &mut Connection, mirror: &mut Snapshot) {
    loop {
        match client.request(Request::Snapshot {
            since: Some(mirror.revision),
        }) {
            Response::Snapshot(update) => mirror.apply_update(update),
            Response::Unchanged { .. } => return,
            other => panic!("unexpected snapshot response {other:?}"),
        }
    }
}

#[test]
fn a_mostly_blank_screen_and_a_keystroke_cost_little_on_the_wire() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "wire".into(),
        cwd: Some("/".into()),
    }));
    let pane = client.snapshot().panes[0].id;
    ok(client.request(Request::Resize {
        pane,
        cols: 300,
        rows: 80,
    }));
    wait_for(|| {
        client.snapshot().panes[0]
            .lines
            .iter()
            .any(|line| !line.runs.is_empty())
    });
    let mut mirror = client.snapshot();
    settle(&mut client, &mut mirror);
    let full = serde_json::to_vec(&mirror).unwrap_or_else(|e| panic!("encode snapshot: {e}"));
    assert!(
        full.len() < 16 * 1024,
        "a 300x80 screen with one prompt line took {} bytes",
        full.len()
    );

    ok(client.request(Request::Input {
        pane,
        bytes: b"a".to_vec(),
    }));
    let Response::Snapshot(update) = client.request(Request::Snapshot {
        since: Some(mirror.revision),
    }) else {
        panic!("echoed input must produce an update");
    };
    assert_eq!(
        update.panes[0].lines.len(),
        1,
        "only the prompt row changed"
    );
    let update = serde_json::to_vec(&update).unwrap_or_else(|e| panic!("encode update: {e}"));
    assert!(
        update.len() < 2 * 1024,
        "one keystroke took {} bytes",
        update.len()
    );
}

#[test]
fn incremental_updates_converge_to_the_full_screen() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "converge".into(),
        cwd: Some("/".into()),
    }));
    let pane = client.snapshot().panes[0].id;
    ok(client.request(Request::Resize {
        pane,
        cols: 40,
        rows: 8,
    }));
    let mut mirror = client.snapshot();
    // Scrolling output, an erase, styled text with a wide character, more scrolling.
    for command in [
        &b"seq 1 30\n"[..],
        b"printf '\\033[2J\\033[Hcleared\\n'\n",
        b"printf '\\033[1;31mred\\033[0m \\347\\225\\214\\n'\n",
        b"seq 1 5\n",
    ] {
        ok(client.request(Request::Input {
            pane,
            bytes: command.to_vec(),
        }));
    }
    settle(&mut client, &mut mirror);
    let full = client.snapshot();
    assert_eq!(
        mirror.panes[0].lines, full.panes[0].lines,
        "a client that applied every update must show what a fresh client shows"
    );
}

/// Runs `printf '<escapes>'` in the pane so the terminal receives `escapes`.
fn emit(client: &mut Connection, pane: u64, escapes: &str) {
    ok(client.request(Request::Input {
        pane,
        bytes: format!("printf '{escapes}'\n").into_bytes(),
    }));
}

#[test]
fn mouse_modes_cursor_shape_and_text_styles_reach_the_client() {
    use spooky_core::{CursorShape, MouseEncoding, MouseTracking};
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "modes".into(),
        cwd: Some("/".into()),
    }));
    let initial = client.snapshot();
    let pane = initial.panes[0].id;
    let first = &initial.panes[0];
    assert_eq!(first.mouse_tracking, MouseTracking::Off);
    assert_eq!(first.mouse_encoding, MouseEncoding::Normal);
    assert!(!first.alternate_scroll);
    assert_eq!(first.cursor.shape, CursorShape::Beam);

    // Drag tracking with SGR reports; the most detailed enabled mode wins.
    emit(&mut client, pane, "\\033[?1002h\\033[?1006h");
    wait_for(|| {
        let p = &client.snapshot().panes[0];
        p.mouse_tracking == MouseTracking::Drag && p.mouse_encoding == MouseEncoding::Sgr
    });
    emit(&mut client, pane, "\\033[?1003h");
    wait_for(|| client.snapshot().panes[0].mouse_tracking == MouseTracking::Motion);
    emit(
        &mut client,
        pane,
        "\\033[?1003l\\033[?1002l\\033[?1006l\\033[?1000h\\033[?1005h",
    );
    wait_for(|| {
        let p = &client.snapshot().panes[0];
        p.mouse_tracking == MouseTracking::Clicks && p.mouse_encoding == MouseEncoding::Utf8
    });

    // The wheel becomes arrow keys only on the alternate screen, unless disabled.
    emit(&mut client, pane, "\\033[?1049h");
    wait_for(|| client.snapshot().panes[0].alternate_scroll);
    emit(&mut client, pane, "\\033[?1007l");
    wait_for(|| !client.snapshot().panes[0].alternate_scroll);

    // Applications choose a cursor shape; resetting returns to the steady bar.
    emit(&mut client, pane, "\\033[4 q");
    wait_for(|| client.snapshot().panes[0].cursor.shape == CursorShape::Underline);
    emit(&mut client, pane, "\\033[2 q");
    wait_for(|| client.snapshot().panes[0].cursor.shape == CursorShape::Block);
    emit(&mut client, pane, "\\033[0 q");
    wait_for(|| client.snapshot().panes[0].cursor.shape == CursorShape::Beam);

    // Strikethrough travels as a style; dim darkens the foreground to two thirds.
    emit(
        &mut client,
        pane,
        "\\033[2J\\033[H\\033[9mS\\033[0m \\033[2;38;2;100;200;50mD\\033[0m",
    );
    wait_for(|| {
        let p = &client.snapshot().panes[0];
        has_cell(p, "S", |run| run.strikethrough) && has_cell(p, "D", |run| run.fg == [66, 133, 33])
    });
}

/// Identity of the first split divider of the first workspace's first tab.
fn split_id(client: &mut Connection) -> u64 {
    match client.snapshot().workspaces[0].tabs[0].layout {
        Layout::Split { id, .. } => id,
        ref other => panic!("expected a split, got {other:?}"),
    }
}

fn split_ratio(client: &mut Connection) -> u16 {
    match client.snapshot().workspaces[0].tabs[0].layout {
        Layout::Split { ratio, .. } => ratio,
        ref other => panic!("expected a split, got {other:?}"),
    }
}

#[test]
fn workspaces_survive_a_server_stop_and_stopping_asks_before_ending_shells() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "alpha".into(),
        cwd: Some("/".into()),
    }));
    let first = client.snapshot();
    let (workspace, tab, pane) = (
        first.workspaces[0].id,
        first.workspaces[0].tabs[0].id,
        first.panes[0].id,
    );
    ok(client.request(Request::Split {
        pane,
        axis: Axis::Vertical,
    }));
    let split = split_id(&mut client);
    ok(client.request(Request::SetSplitRatio { split, ratio: 700 }));
    ok(client.request(Request::RenameTab {
        tab,
        title: "build".into(),
    }));

    // A status query reports what stopping would end.
    let status = server.cli(&["status"]);
    assert!(status.status.success());
    let text = String::from_utf8_lossy(&status.stdout);
    assert!(
        text.contains("1 workspace(s)") && text.contains("2 running"),
        "{text}"
    );

    // Stopping while shells run is refused unless forced, and leaves them alone.
    let refused = server.cli(&["stop"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--force"));
    assert_eq!(client.snapshot().panes.len(), 2);

    // The layout reaches disk by itself shortly after it changes.
    let saved = server.dir.join("workspaces.json");
    wait_for(|| {
        fs::read_to_string(&saved).is_ok_and(|text| text.contains("build") && text.contains("700"))
    });

    let stopped = server.cli(&["stop", "--force"]);
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    let dir = server.wait_for_exit();

    let server = Server::start_in(dir, |_| {});
    let mut client = server.connect();
    let restored = client.snapshot();
    let saved_workspace = &restored.workspaces[0];
    assert_eq!(
        (
            saved_workspace.id,
            saved_workspace.name.as_str(),
            saved_workspace.tabs[0].id
        ),
        (workspace, "alpha", tab)
    );
    assert!(saved_workspace.tabs[0].renamed);
    assert_eq!(saved_workspace.tabs[0].title, "build");
    assert_eq!(split_ratio(&mut client), 700);
    assert_eq!(restored.panes.len(), 2, "every saved pane gets a new shell");
    assert!(restored.panes.iter().all(|pane| !pane.exited));

    // Identities handed out before the stop are not issued again.
    let used = restored
        .panes
        .iter()
        .map(|pane| pane.id)
        .chain([workspace, tab, split_id(&mut client)])
        .max()
        .expect("identities");
    ok(client.request(Request::CreateTab {
        workspace,
        title: "later".into(),
    }));
    let later = client.snapshot().workspaces[0]
        .tabs
        .last()
        .expect("a tab")
        .id;
    assert!(later > used, "tab {later} reuses an identity up to {used}");

    let log = fs::read_to_string(server.dir.join("server.log")).expect("server log");
    assert!(
        log.contains("shutdown requested") && log.contains("restored 1 saved workspace(s)"),
        "{log}"
    );
}

#[test]
fn a_divider_moves_only_within_its_bounds() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "divide".into(),
        cwd: Some("/".into()),
    }));
    let pane = client.snapshot().panes[0].id;
    ok(client.request(Request::Split {
        pane,
        axis: Axis::Horizontal,
    }));
    let split = split_id(&mut client);
    assert_eq!(split_ratio(&mut client), spooky_core::EVEN_SPLIT);

    ok(client.request(Request::SetSplitRatio { split, ratio: 250 }));
    assert_eq!(split_ratio(&mut client), 250);
    for ratio in [0, 99, 901, 1000] {
        let response = client.request(Request::SetSplitRatio { split, ratio });
        assert!(
            matches!(response, Response::Error(_)),
            "{ratio}: {response:?}"
        );
    }
    let unknown = client.request(Request::SetSplitRatio {
        split: 9999,
        ratio: 500,
    });
    assert!(matches!(unknown, Response::Error(_)), "{unknown:?}");
    assert_eq!(split_ratio(&mut client), 250);
}

#[test]
fn osc_52_writes_wait_for_the_client_to_fetch_them() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "clip".into(),
        cwd: Some("/".into()),
    }));
    let pane = client.snapshot().panes[0].id;
    assert_eq!(client.snapshot().panes[0].clipboard_seq, 0);

    // "hello" in base64, then a second write that replaces it.
    emit(&mut client, pane, "\\033]52;c;aGVsbG8=\\007");
    wait_for(|| client.snapshot().panes[0].clipboard_seq == 1);
    match client.request(Request::Clipboard { pane }) {
        Response::Clipboard { seq, text } => assert_eq!((seq, text.as_str()), (1, "hello")),
        other => panic!("expected clipboard text, got {other:?}"),
    }
    emit(&mut client, pane, "\\033]52;c;d29ybGQ=\\007");
    wait_for(|| client.snapshot().panes[0].clipboard_seq == 2);
    match client.request(Request::Clipboard { pane }) {
        Response::Clipboard { seq, text } => assert_eq!((seq, text.as_str()), (2, "world")),
        other => panic!("expected clipboard text, got {other:?}"),
    }
}

#[test]
fn titles_links_blinking_and_focus_reporting_reach_the_client() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "modes".into(),
        cwd: Some("/".into()),
    }));
    let pane = client.snapshot().panes[0].id;
    assert_eq!(client.snapshot().panes[0].title, "Terminal");

    emit(&mut client, pane, "\\033]0;  my build \\007");
    wait_for(|| client.snapshot().panes[0].title == "my build");

    // A hyperlink applies to the text between its two OSC 8 sequences only.
    emit(
        &mut client,
        pane,
        "\\033[2J\\033[H\\033]8;;https://example.com/a\\033\\\\link\\033]8;;\\033\\\\ plain",
    );
    wait_for(|| {
        let p = &client.snapshot().panes[0];
        has_cell(p, "l", |run| {
            run.link.as_deref() == Some("https://example.com/a")
        }) && has_cell(p, "p", |run| run.link.is_none())
    });

    // DECSCUSR 1 asks for a blinking block, 2 for a steady one.
    assert!(!client.snapshot().panes[0].cursor.blinking);
    emit(&mut client, pane, "\\033[1 q");
    wait_for(|| client.snapshot().panes[0].cursor.blinking);
    emit(&mut client, pane, "\\033[2 q");
    wait_for(|| !client.snapshot().panes[0].cursor.blinking);

    assert!(!client.snapshot().panes[0].focus_reporting);
    emit(&mut client, pane, "\\033[?1004h");
    wait_for(|| client.snapshot().panes[0].focus_reporting);
    emit(&mut client, pane, "\\033[?1004l");
    wait_for(|| !client.snapshot().panes[0].focus_reporting);
}

#[test]
fn search_walks_matches_through_the_scrollback_and_scrolls_to_them() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "search".into(),
        cwd: Some("/".into()),
    }));
    let pane = client.snapshot().panes[0].id;
    // The command line is echoed once before echo is switched off, so it is
    // written such that only the program's output contains "hit-".
    ok(client.request(Request::Input {
        pane,
        bytes: b"stty -echo; p=hi; for i in 1 2 3; do echo ${p}t-$i; seq 1 40; done; echo done-marker\n"
            .to_vec(),
    }));
    wait_for(|| shows_line(&mut client, "done-marker"));
    assert_eq!(client.snapshot().panes[0].scrolled, 0);

    let find = |client: &mut Connection, query: &str, direction| {
        client.request(Request::Find {
            pane,
            query: query.into(),
            direction,
        })
    };
    // A fresh backward search starts at the bottom of the screen: hit-3 is the
    // nearest match above it, out of view, so the viewport moves to it.
    match find(&mut client, "HIT-", FindDirection::Previous) {
        Response::Found { current, total } => assert_eq!((current, total), (3, 3)),
        other => panic!("expected a match, got {other:?}"),
    }
    let shown = client.snapshot();
    let view = &shown.panes[0];
    assert!(view.scrolled > 0 && view.history >= view.scrolled);
    let current: Vec<_> = view.highlights.iter().filter(|h| h.current).collect();
    assert_eq!(current.len(), 1, "{:?}", view.highlights);
    assert_eq!(view.row_text(current[0].row).trim_end(), "hit-3");

    // Repeating continues from that match and wraps around in both directions.
    match find(&mut client, "HIT-", FindDirection::Next) {
        Response::Found { current, total } => assert_eq!((current, total), (1, 3)),
        other => panic!("expected a match, got {other:?}"),
    }
    let shown = client.snapshot();
    let current: Vec<_> = shown.panes[0]
        .highlights
        .iter()
        .filter(|h| h.current)
        .collect();
    assert_eq!(shown.panes[0].row_text(current[0].row).trim_end(), "hit-1");
    match find(&mut client, "HIT-", FindDirection::Previous) {
        Response::Found { current, .. } => assert_eq!(current, 3),
        other => panic!("expected a match, got {other:?}"),
    }

    assert!(matches!(
        find(&mut client, "absent text", FindDirection::Next),
        Response::NotFound
    ));
    // An empty query clears the search and its highlights.
    ok(find(&mut client, "", FindDirection::Next));
    assert!(client.snapshot().panes[0].highlights.is_empty());
}

#[test]
fn new_panes_start_at_the_size_of_the_window_sized_pane() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "size".into(),
        cwd: Some("/".into()),
    }));
    let first = client.snapshot();
    assert_eq!((first.panes[0].cols, first.panes[0].rows), (80, 24));
    ok(client.request(Request::Resize {
        pane: first.panes[0].id,
        cols: 132,
        rows: 41,
    }));
    let workspace = first.workspaces[0].id;
    ok(client.request(Request::CreateTab {
        workspace,
        title: "next".into(),
    }));
    ok(client.request(Request::CreateWorkspace {
        name: "other".into(),
        cwd: Some("/".into()),
    }));
    let after = client.snapshot();
    assert!(
        after
            .panes
            .iter()
            .all(|pane| (pane.cols, pane.rows) == (132, 41)),
        "{:?}",
        after
            .panes
            .iter()
            .map(|p| (p.cols, p.rows))
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_scrollback_limit_comes_from_the_server_settings() {
    let config = std::env::temp_dir().join(format!("spooky-config-{}", std::process::id()));
    fs::create_dir_all(config.join("spooky")).expect("config directory");
    fs::write(config.join("spooky/server.toml"), "scrollback_lines = 50\n").expect("server.toml");
    let server = Server::start_with(|command| {
        command.env("XDG_CONFIG_HOME", &config);
    });
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "history".into(),
        cwd: Some("/".into()),
    }));
    let pane = client.snapshot().panes[0].id;
    ok(client.request(Request::Input {
        pane,
        bytes: b"seq 1 500; echo end-marker\n".to_vec(),
    }));
    wait_for(|| shows_line(&mut client, "end-marker"));
    assert_eq!(client.snapshot().panes[0].history, 50);
    fs::remove_dir_all(config).expect("cleanup");
}
