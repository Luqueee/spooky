//! Real PTY protocol regressions for detach, emulation and pane-tree lifetime.
#![cfg(unix)]

use spooky_core::{Axis, Request, Response, Snapshot};
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
