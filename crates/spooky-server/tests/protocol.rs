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
fn shell_output_sequence_changes_for_pty_output_but_not_a_resize() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "activity".into(),
        cwd: Some(server.dir.to_string_lossy().into_owned()),
    }));
    let pane = client.snapshot().panes[0].id;
    ok(client.request(Request::Input {
        pane,
        bytes: b"stty -echo; printf 'SEQ_%s\\n' READY; exec cat\n".to_vec(),
    }));
    wait_for(|| {
        client.snapshot().panes.iter().any(|pane| {
            screen_rows(pane)
                .iter()
                .any(|row| row.contains("SEQ_READY"))
        })
    });
    let before = client.snapshot().panes[0].output_seq;
    ok(client.request(Request::Resize {
        pane,
        cols: 110,
        rows: 30,
    }));
    assert_eq!(client.snapshot().panes[0].output_seq, before);
    ok(client.request(Request::Input {
        pane,
        bytes: b"ACTIVITY_MARKER\n".to_vec(),
    }));
    wait_for(|| {
        client.snapshot().panes.iter().any(|pane| {
            pane.output_seq != before
                && screen_rows(pane)
                    .iter()
                    .any(|row| row.contains("ACTIVITY_MARKER"))
        })
    });
}

fn saved_pane_directory(server: &Server, pane: u64) -> Option<String> {
    let text = fs::read_to_string(server.dir.join("workspaces.json")).ok()?;
    let saved: serde_json::Value = serde_json::from_str(&text).ok()?;
    saved
        .get("pane_directories")?
        .get(pane.to_string())?
        .as_str()
        .map(str::to_owned)
}

#[test]
fn each_shell_recovers_its_own_directory_after_a_server_restart() {
    let server = Server::start();
    let first_dir = server.dir.join("first project");
    let second_dir = server.dir.join("second project");
    fs::create_dir(&first_dir).expect("first directory");
    fs::create_dir(&second_dir).expect("second directory");
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "work".into(),
        cwd: Some(server.dir.to_string_lossy().into_owned()),
    }));
    let first = client.snapshot().panes[0].id;
    ok(client.request(Request::Split {
        pane: first,
        axis: Axis::Vertical,
        here: false,
    }));
    let second = client
        .snapshot()
        .panes
        .into_iter()
        .find(|pane| pane.id != first)
        .expect("second shell")
        .id;
    for (pane, directory, marker) in [(first, &first_dir, "ONE"), (second, &second_dir, "TWO")] {
        ok(client.request(Request::Input {
            pane,
            bytes: format!(
                "cd '{}' && printf 'READY-%s\\n' {marker}\n",
                directory.display()
            )
            .into_bytes(),
        }));
    }
    wait_for(|| {
        let snapshot = client.snapshot();
        [(first, "READY-ONE"), (second, "READY-TWO")]
            .iter()
            .all(|(id, marker)| {
                snapshot.panes.iter().any(|pane| {
                    pane.id == *id && screen_rows(pane).iter().any(|row| row.contains(marker))
                })
            })
    });
    ok(client.request(Request::Shutdown));
    drop(client);
    let dir = server.wait_for_exit();
    let restored = Server::start_in(dir, |_| {});
    let mut client = restored.connect();
    for (pane, directory, marker) in [(first, &first_dir, "ONE"), (second, &second_dir, "TWO")] {
        ok(client.request(Request::Input {
            pane,
            bytes: format!(
                "test \"$PWD\" -ef '{}' && printf 'RESTORED-%s\\n' {marker}\n",
                directory.display()
            )
            .into_bytes(),
        }));
    }
    wait_for(|| {
        let snapshot = client.snapshot();
        [(first, "RESTORED-ONE"), (second, "RESTORED-TWO")]
            .iter()
            .all(|(id, marker)| {
                snapshot.panes.iter().any(|pane| {
                    pane.id == *id && screen_rows(pane).iter().any(|row| row.contains(marker))
                })
            })
    });
}

#[test]
fn a_checkpoint_survives_an_abrupt_server_exit() {
    let mut server = Server::start();
    let directory = server.dir.join("checkpoint project");
    fs::create_dir(&directory).expect("project directory");
    let expected = directory.canonicalize().expect("canonical directory");
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "work".into(),
        cwd: Some(server.dir.to_string_lossy().into_owned()),
    }));
    let pane = client.snapshot().panes[0].id;
    wait_for(|| saved_pane_directory(&server, pane).is_some());
    ok(client.request(Request::Input {
        pane,
        bytes: format!(
            "cd '{}' && printf 'READY-%s\\n' CHECKPOINT\n",
            directory.display()
        )
        .into_bytes(),
    }));
    wait_for(|| {
        saved_pane_directory(&server, pane).is_some_and(|saved| saved == expected.to_string_lossy())
    });
    drop(client);
    server.child.kill().expect("terminate isolated server");
    let dir = server.wait_for_exit();
    let restored = Server::start_in(dir, |_| {});
    let mut client = restored.connect();
    ok(client.request(Request::Input {
        pane,
        bytes: format!(
            "test \"$PWD\" -ef '{}' && printf 'RESTORED-%s\\n' CHECKPOINT\n",
            directory.display()
        )
        .into_bytes(),
    }));
    wait_for(|| {
        client.snapshot().panes.iter().any(|p| {
            p.id == pane
                && screen_rows(p)
                    .iter()
                    .any(|row| row.contains("RESTORED-CHECKPOINT"))
        })
    });
}

#[test]
fn a_busy_shell_restores_its_own_directory_not_its_childs() {
    let server = Server::start();
    let directory = server.dir.join("busy project");
    fs::create_dir(&directory).expect("project directory");
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "work".into(),
        cwd: Some(server.dir.to_string_lossy().into_owned()),
    }));
    let initial = client.snapshot();
    let workspace = initial.workspaces[0].id;
    let pane = initial.panes[0].id;
    ok(client.request(Request::Input {
        pane,
        bytes: format!(
            "cd '{}' && (cd / && printf '\\033]2;busy-directory\\007' && exec sleep 5)\n",
            directory.display()
        )
        .into_bytes(),
    }));
    wait_for(|| {
        client
            .snapshot()
            .panes
            .iter()
            .any(|p| p.id == pane && p.title == "busy-directory")
    });
    let busy = client.request(Request::CreateTab {
        workspace,
        source: Some(pane),
        title: "busy".into(),
    });
    assert!(matches!(busy, Response::Error(message) if message.contains("foreground program")));
    ok(client.request(Request::Shutdown));
    drop(client);
    let dir = server.wait_for_exit();
    let restored = Server::start_in(dir, |_| {});
    let mut client = restored.connect();
    ok(client.request(Request::Input {
        pane,
        bytes: format!(
            "test \"$PWD\" -ef '{}' && printf 'RESTORED-%s\\n' BUSY\n",
            directory.display()
        )
        .into_bytes(),
    }));
    wait_for(|| {
        client.snapshot().panes.iter().any(|p| {
            p.id == pane
                && screen_rows(p)
                    .iter()
                    .any(|row| row.contains("RESTORED-BUSY"))
        })
    });
}

#[test]
fn a_missing_saved_directory_falls_back_to_the_workspace() {
    let server = Server::start();
    let workspace_dir = server.dir.clone();
    let directory = workspace_dir.join("deleted project");
    fs::create_dir(&directory).expect("project directory");
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "work".into(),
        cwd: Some(workspace_dir.to_string_lossy().into_owned()),
    }));
    let pane = client.snapshot().panes[0].id;
    ok(client.request(Request::Input {
        pane,
        bytes: format!(
            "cd '{}' && printf 'READY-%s\\n' FALLBACK\n",
            directory.display()
        )
        .into_bytes(),
    }));
    wait_for(|| {
        client.snapshot().panes.iter().any(|p| {
            p.id == pane
                && screen_rows(p)
                    .iter()
                    .any(|row| row.contains("READY-FALLBACK"))
        })
    });
    ok(client.request(Request::Shutdown));
    drop(client);
    let dir = server.wait_for_exit();
    fs::remove_dir(&directory).expect("remove saved directory");
    let restored = Server::start_in(dir, |_| {});
    let mut client = restored.connect();
    ok(client.request(Request::Input {
        pane,
        bytes: format!(
            "test \"$PWD\" -ef '{}' && printf 'RESTORED-%s\\n' FALLBACK\n",
            workspace_dir.display()
        )
        .into_bytes(),
    }));
    wait_for(|| {
        client.snapshot().panes.iter().any(|p| {
            p.id == pane
                && screen_rows(p)
                    .iter()
                    .any(|row| row.contains("RESTORED-FALLBACK"))
        })
    });
    let log = fs::read_to_string(restored.dir.join("server.log")).expect("server log");
    assert!(log.contains("cannot restore directory for pane"));
}

#[test]
fn exiting_a_shell_closes_its_tab_without_stopping_the_other_tab() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "work".into(),
        cwd: Some(server.dir.to_string_lossy().into_owned()),
    }));
    let initial = client.snapshot();
    let workspace = initial.workspaces[0].id;
    let first = initial.panes[0].id;
    ok(client.request(Request::CreateTab {
        workspace,
        source: None,
        title: "second".into(),
    }));
    let before = client.snapshot();
    let second = before
        .panes
        .iter()
        .find(|pane| pane.id != first)
        .expect("second shell")
        .id;
    let second_tab = before.workspaces[0]
        .tabs
        .iter()
        .find(|tab| tab.layout.contains(second))
        .expect("second tab")
        .id;
    ok(client.request(Request::Input {
        pane: second,
        bytes: b"exit\n".to_vec(),
    }));
    wait_for(|| {
        let snapshot = client.snapshot();
        snapshot.workspaces[0]
            .tabs
            .iter()
            .all(|tab| tab.id != second_tab)
            && snapshot
                .panes
                .iter()
                .any(|pane| pane.id == first && !pane.exited)
            && snapshot.panes.iter().all(|pane| pane.id != second)
    });
}

#[test]
fn exiting_split_and_last_shell_preserves_empty_workspace_across_restart() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "keep me".into(),
        cwd: Some(server.dir.to_string_lossy().into_owned()),
    }));
    let initial = client.snapshot();
    let workspace = initial.workspaces[0].id;
    let first = initial.panes[0].id;
    ok(client.request(Request::Split {
        pane: first,
        axis: Axis::Vertical,
        here: false,
    }));
    let second = client
        .snapshot()
        .panes
        .into_iter()
        .find(|pane| pane.id != first)
        .expect("split shell")
        .id;
    ok(client.request(Request::Input {
        pane: second,
        bytes: b"exit\n".to_vec(),
    }));
    wait_for(|| {
        let snapshot = client.snapshot();
        snapshot.panes.len() == 1
            && matches!(snapshot.workspaces[0].tabs[0].layout, Layout::Pane(id) if id == first)
    });
    ok(client.request(Request::Input {
        pane: first,
        bytes: b"exit 7\n".to_vec(),
    }));
    drop(client);
    let mut client = server.connect();
    wait_for(|| {
        let snapshot = client.snapshot();
        snapshot.workspaces.len() == 1
            && snapshot.workspaces[0].id == workspace
            && snapshot.workspaces[0].tabs.is_empty()
            && snapshot.panes.is_empty()
    });
    ok(client.request(Request::Shutdown));
    drop(client);
    let dir = server.wait_for_exit();
    let restored = Server::start_in(dir, |_| {});
    let mut client = restored.connect();
    let snapshot = client.snapshot();
    assert_eq!(snapshot.workspaces.len(), 1);
    assert_eq!(snapshot.workspaces[0].id, workspace);
    assert!(snapshot.workspaces[0].tabs.is_empty());
    ok(client.request(Request::CreateTab {
        workspace,
        source: None,
        title: "new".into(),
    }));
    assert_eq!(client.snapshot().panes.len(), 1);
}

#[test]
fn directory_query_tracks_the_shell_and_refuses_foreground_programs() {
    let server = Server::start();
    let directory = server.dir.join("project with spaces");
    fs::create_dir(&directory).expect("create project directory");
    let expected = directory.canonicalize().expect("canonical directory");
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "work".into(),
        cwd: Some(server.dir.to_string_lossy().into_owned()),
    }));
    let pane = client.snapshot().panes[0].id;
    ok(client.request(Request::Input {
        pane,
        bytes: format!("cd '{}' && printf 'READY-%s\\n' DIR\n", directory.display()).into_bytes(),
    }));
    wait_for(|| {
        client
            .snapshot()
            .panes
            .iter()
            .any(|p| p.id == pane && screen_rows(p).iter().any(|row| row.contains("READY-DIR")))
    });
    wait_for(|| {
        matches!(
            client.request(Request::Directory { pane }),
            Response::Directory { kind: spooky_core::DirectoryKind::Shell, ref path }
                if path == &expected.to_string_lossy()
        )
    });
    ok(client.request(Request::Input {
        pane,
        bytes: b"sh -c 'printf \"\\033]2;directory-busy\\007\"; exec sleep 30'\n".to_vec(),
    }));
    wait_for(|| {
        client
            .snapshot()
            .panes
            .iter()
            .any(|p| p.title == "directory-busy")
    });
    assert!(matches!(
        client.request(Request::Directory { pane }),
        Response::Error(_)
    ));
    ok(client.request(Request::Input {
        pane,
        bytes: vec![3],
    }));
    wait_for(|| {
        matches!(
            client.request(Request::Directory { pane }),
            Response::Directory { kind: spooky_core::DirectoryKind::Shell, ref path }
                if path == &expected.to_string_lossy()
        )
    });
    assert!(matches!(
        client.request(Request::Directory { pane: u64::MAX }),
        Response::Error(_)
    ));
}

#[test]
fn new_tabs_and_splits_start_in_the_selected_shell_directory() {
    let server = Server::start();
    let directory = server.dir.join("project with spaces");
    fs::create_dir(&directory).expect("create project directory");
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "work".into(),
        cwd: Some(server.dir.to_string_lossy().into_owned()),
    }));
    let snapshot = client.snapshot();
    let workspace = snapshot.workspaces[0].id;
    let source = snapshot.panes[0].id;
    ok(client.request(Request::Input {
        pane: source,
        bytes: format!("cd '{}' && printf 'READY-%s\\n' CWD\n", directory.display()).into_bytes(),
    }));
    wait_for(|| {
        client
            .snapshot()
            .panes
            .iter()
            .any(|p| p.id == source && screen_rows(p).iter().any(|row| row.contains("READY-CWD")))
    });

    ok(client.request(Request::CreateTab {
        workspace,
        source: Some(source),
        title: "new".into(),
    }));
    ok(client.request(Request::Split {
        pane: source,
        axis: Axis::Vertical,
        here: true,
    }));
    let panes: Vec<_> = client
        .snapshot()
        .panes
        .into_iter()
        .filter(|pane| pane.id != source)
        .map(|pane| pane.id)
        .collect();
    assert_eq!(panes.len(), 2);
    for pane in &panes {
        ok(client.request(Request::Input {
            pane: *pane,
            bytes: format!(
                "test \"$PWD\" -ef '{}' && printf 'FOUND-%s\\n' CWD\n",
                directory.display()
            )
            .into_bytes(),
        }));
    }
    wait_for(|| {
        let snapshot = client.snapshot();
        panes.iter().all(|id| {
            snapshot.panes.iter().any(|pane| {
                pane.id == *id
                    && screen_rows(pane)
                        .iter()
                        .any(|row| row.contains("FOUND-CWD"))
            })
        })
    });
}

#[test]
fn busy_shell_refuses_here_without_preventing_an_ordinary_split() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "work".into(),
        cwd: Some(server.dir.to_string_lossy().into_owned()),
    }));
    let source = client.snapshot().panes[0].id;
    ok(client.request(Request::Input {
        pane: source,
        bytes: b"sh -c 'printf \"\\033]2;busy-test\\007\"; exec sleep 30'\n".to_vec(),
    }));
    wait_for(|| {
        client
            .snapshot()
            .panes
            .iter()
            .any(|pane| pane.id == source && pane.title == "busy-test")
    });
    let result = client.request(Request::Split {
        pane: source,
        axis: Axis::Vertical,
        here: true,
    });
    assert!(
        matches!(&result, Response::Error(message) if message.contains("foreground program")),
        "{result:?}"
    );
    assert_eq!(client.snapshot().panes.len(), 1);
    ok(client.request(Request::Split {
        pane: source,
        axis: Axis::Vertical,
        here: false,
    }));
    assert_eq!(client.snapshot().panes.len(), 2);
}

#[test]
fn a_foreground_child_sharing_the_shell_group_does_not_supply_a_directory() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "work".into(),
        cwd: Some(server.dir.to_string_lossy().into_owned()),
    }));
    let source = client.snapshot().panes[0].id;
    ok(client.request(Request::Input {
        pane: source,
        bytes: b"set +m; sh -c 'printf \"\\033]2;no-job-control\\007\"; exec sleep 30'\n".to_vec(),
    }));
    wait_for(|| {
        client
            .snapshot()
            .panes
            .iter()
            .any(|pane| pane.id == source && pane.title == "no-job-control")
    });
    let result = client.request(Request::Split {
        pane: source,
        axis: Axis::Vertical,
        here: true,
    });
    assert!(
        matches!(&result, Response::Error(message) if message.contains("foreground program")),
        "{result:?}"
    );
    assert_eq!(client.snapshot().panes.len(), 1);
}

#[test]
fn a_shell_replaced_by_a_foreground_program_does_not_supply_a_directory() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "work".into(),
        cwd: Some(server.dir.to_string_lossy().into_owned()),
    }));
    let snapshot = client.snapshot();
    let source = snapshot.panes[0].id;
    let workspace = snapshot.workspaces[0].id;
    let directory = server.dir.join("last recognized shell directory");
    fs::create_dir(&directory).expect("project directory");
    wait_for(|| saved_pane_directory(&server, source).is_some());
    ok(client.request(Request::Input {
        pane: source,
        bytes: format!(
            "cd '{}' && printf 'READY-%s\\n' REPLACEMENT\n",
            directory.display()
        )
        .into_bytes(),
    }));
    let expected = directory.canonicalize().expect("canonical directory");
    wait_for(|| {
        saved_pane_directory(&server, source)
            .is_some_and(|saved| saved == expected.to_string_lossy())
    });
    let pid_file = server.dir.join("shell-pid");
    ok(client.request(Request::Input {
        pane: source,
        bytes: format!(
            "cd /; printf '%s' $$ > '{}'; exec /bin/sleep 30\n",
            pid_file.display()
        )
        .into_bytes(),
    }));
    let mut system = sysinfo::System::new();
    wait_for(|| {
        let Ok(text) = fs::read_to_string(&pid_file) else {
            return false;
        };
        let Ok(raw) = text.parse::<u32>() else {
            return false;
        };
        let pid = sysinfo::Pid::from_u32(raw);
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            true,
            sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always),
        );
        system
            .process(pid)
            .and_then(sysinfo::Process::exe)
            .and_then(|path| path.file_name())
            .is_some_and(|name| name == "sleep")
    });
    let result = client.request(Request::CreateTab {
        workspace,
        source: Some(source),
        title: "here".into(),
    });
    assert!(
        matches!(&result, Response::Error(message) if message.contains("not a recognized shell")),
        "{result:?}"
    );
    assert_eq!(client.snapshot().workspaces[0].tabs.len(), 1);
    ok(client.request(Request::Shutdown));
    drop(client);
    let dir = server.wait_for_exit();
    let restored = Server::start_in(dir, |_| {});
    let mut client = restored.connect();
    ok(client.request(Request::Input {
        pane: source,
        bytes: format!(
            "test \"$PWD\" -ef '{}' && printf 'RESTORED-%s\\n' REPLACEMENT\n",
            directory.display()
        )
        .into_bytes(),
    }));
    wait_for(|| {
        client.snapshot().panes.iter().any(|p| {
            p.id == source
                && screen_rows(p)
                    .iter()
                    .any(|row| row.contains("RESTORED-REPLACEMENT"))
        })
    });
}

#[test]
fn a_source_pane_cannot_be_used_from_another_workspace() {
    let server = Server::start();
    let mut client = server.connect();
    for name in ["first", "second"] {
        ok(client.request(Request::CreateWorkspace {
            name: name.into(),
            cwd: Some(server.dir.to_string_lossy().into_owned()),
        }));
    }
    let snapshot = client.snapshot();
    let source = snapshot.panes[0].id;
    let workspace = snapshot.workspaces[1].id;
    let result = client.request(Request::CreateTab {
        workspace,
        source: Some(source),
        title: "wrong workspace".into(),
    });
    assert!(
        matches!(&result, Response::Error(message) if message.contains("not in the workspace")),
        "{result:?}"
    );
    assert_eq!(client.snapshot().workspaces[1].tabs.len(), 1);
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
        here: false,
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
    assert!(snapshot.workspaces[0].tabs.is_empty());
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
        source: None,
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
        here: false,
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
    assert_eq!(snapshot.workspaces[0].id, workspace);
    assert!(snapshot.workspaces[0].tabs.is_empty());
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
#[test]
fn explicit_background_equal_to_default_survives_the_pty_protocol() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "background".into(),
        cwd: Some("/".into()),
    }));
    let pane = client.snapshot().panes[0].id;
    ok(client.request(Request::Input {
        pane,
        bytes: b"printf '\\033[H\\033[2J\\033[0mA\\033[48;2;34;37;46mB \\033[0mC\\033[7mD\\033[0m\\n'\n".to_vec(),
    }));
    wait_for(|| has_cell(&client.snapshot().panes[0], "D", |run| run.explicit_bg));
    let snapshot = client.snapshot();
    let pane = &snapshot.panes[0];
    assert!(has_cell(pane, "A", |run| !run.explicit_bg));
    assert!(has_cell(pane, "B", |run| run.explicit_bg
        && run.bg == spooky_core::DEFAULT_BACKGROUND));
    assert!(has_cell(pane, " ", |run| run.explicit_bg
        && run.bg == spooky_core::DEFAULT_BACKGROUND));
    assert!(has_cell(pane, "C", |run| !run.explicit_bg));
    assert!(has_cell(pane, "D", |run| run.explicit_bg));
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
        here: false,
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
        source: None,
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
        here: false,
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
fn locks_prevent_indirect_closure_and_survive_reconnect() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "protected".into(),
        cwd: Some("/".into()),
    }));
    let snapshot = client.snapshot();
    let workspace = snapshot.workspaces[0].id;
    let tab = snapshot.workspaces[0].tabs[0].id;
    let pane = snapshot.panes[0].id;
    assert!(!snapshot.workspaces[0].tabs[0].locked);
    assert!(snapshot.workspaces[0].tabs[0].locked_panes.is_empty());
    ok(client.request(Request::SetPaneLocked { pane, locked: true }));
    let mut other = server.connect();
    assert_eq!(other.snapshot().workspaces[0].tabs[0].locked_panes, [pane]);
    for request in [
        Request::ClosePane { pane },
        Request::CloseTab { tab },
        Request::CloseWorkspace { workspace },
    ] {
        assert!(matches!(other.request(request), Response::Error(_)));
    }
    ok(client.request(Request::SetPaneLocked {
        pane,
        locked: false,
    }));
    ok(client.request(Request::SetTabLocked { tab, locked: true }));
    assert!(other.snapshot().workspaces[0].tabs[0].locked);
    assert!(matches!(
        other.request(Request::ClosePane { pane }),
        Response::Error(_)
    ));
    assert!(matches!(
        other.request(Request::CloseTab { tab }),
        Response::Error(_)
    ));
    assert!(matches!(
        other.request(Request::CloseWorkspace { workspace }),
        Response::Error(_)
    ));
    ok(client.request(Request::SetTabLocked { tab, locked: false }));
    ok(client.request(Request::CloseTab { tab }));
    let snapshot = other.snapshot();
    assert_eq!(snapshot.workspaces[0].id, workspace);
    assert!(snapshot.workspaces[0].tabs.is_empty());
}

#[test]
fn locks_survive_server_restart() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "persistent locks".into(),
        cwd: Some("/".into()),
    }));
    let snapshot = client.snapshot();
    let tab = snapshot.workspaces[0].tabs[0].id;
    let pane = snapshot.panes[0].id;
    ok(client.request(Request::SetTabLocked { tab, locked: true }));
    ok(client.request(Request::SetPaneLocked { pane, locked: true }));
    ok(client.request(Request::Shutdown));
    drop(client);
    let directory = server.wait_for_exit();
    let server = Server::start_in(directory, |_| {});
    let mut client = server.connect();
    let snapshot = client.snapshot();
    assert!(snapshot.workspaces[0].tabs[0].locked);
    assert_eq!(snapshot.workspaces[0].tabs[0].locked_panes, [pane]);
    assert!(matches!(
        client.request(Request::CloseTab { tab }),
        Response::Error(_)
    ));
}

#[test]
fn normal_exit_removes_only_the_exited_panes_lock() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "exit with locks".into(),
        cwd: Some("/".into()),
    }));
    let snapshot = client.snapshot();
    let tab = snapshot.workspaces[0].tabs[0].id;
    let pane = snapshot.panes[0].id;
    ok(client.request(Request::Split {
        pane,
        axis: Axis::Vertical,
        here: false,
    }));
    ok(client.request(Request::SetTabLocked { tab, locked: true }));
    ok(client.request(Request::SetPaneLocked { pane, locked: true }));
    ok(client.request(Request::Input {
        pane,
        bytes: b"exit\n".to_vec(),
    }));
    wait_for(|| !client.snapshot().panes.iter().any(|p| p.id == pane));
    let snapshot = client.snapshot();
    assert!(snapshot.workspaces[0].tabs[0].locked);
    assert!(snapshot.workspaces[0].tabs[0].locked_panes.is_empty());
    assert_eq!(snapshot.panes.len(), 1);
}

#[test]
fn clear_removes_history_and_preserves_unsubmitted_input() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "copy".into(),
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
    ok(client.request(Request::Input {
        pane,
        bytes: b"echo unfinished".to_vec(),
    }));
    wait_for(|| {
        screen_rows(&client.snapshot().panes[0])
            .iter()
            .any(|s| s.contains("echo unfinished"))
    });
    assert!(client.snapshot().panes[0].cursor.row > 0);
    let Response::AllText { text, .. } = client.request(Request::CopyAll { pane }) else {
        panic!("copy-all did not return terminal text");
    };
    assert!(text.lines().any(|line| line == "1"), "{text:?}");
    assert!(text.lines().any(|line| line == "60"), "{text:?}");
    ok(client.request(Request::ClearViewport { pane }));
    let after = client.snapshot();
    let rows = screen_rows(&after.panes[0]);
    assert_eq!(
        after.panes[0].cursor.row, 0,
        "cursor should move to the top"
    );
    assert!(rows[0].contains("echo unfinished"), "{rows:?}");
    assert!(!rows.iter().any(|row| row.trim() == "60"), "{rows:?}");
    let Response::AllText { text, .. } = client.request(Request::CopyAll { pane }) else {
        panic!("copy-all did not return terminal text");
    };
    assert!(!text.lines().any(|line| line == "1"), "{text:?}");
    assert!(!text.lines().any(|line| line == "60"), "{text:?}");
    assert!(text.contains("echo unfinished"), "{text:?}");
    ok(client.request(Request::Scroll { pane, lines: 30 }));
    assert_eq!(screen_rows(&client.snapshot().panes[0]), rows);
    ok(client.request(Request::Input {
        pane,
        bytes: b"\n".to_vec(),
    }));
    wait_for(|| shows_line(&mut client, "unfinished"));
}

#[test]
fn clear_preserves_wrapped_text_after_the_cursor() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "wrapped-input".into(),
        cwd: Some("/".into()),
    }));
    let pane = client.snapshot().panes[0].id;
    ok(client.request(Request::Resize {
        pane,
        cols: 10,
        rows: 6,
    }));
    ok(client.request(Request::Input {
        pane,
        bytes: b"stty -echo; printf '\\033[2J\\033[H\\n\\nABCDEFGHIJKLMNO\\033[1A\\033[5G'; read value\n".to_vec(),
    }));
    wait_for(|| {
        let snapshot = client.snapshot();
        let screen = &snapshot.panes[0];
        screen.row_text(2).starts_with("ABCDEFGHIJ")
            && screen.row_text(3).starts_with("KLMNO")
            && screen.cursor.row == 2
            && screen.cursor.col == 4
    });
    ok(client.request(Request::ClearViewport { pane }));
    let snapshot = client.snapshot();
    let screen = &snapshot.panes[0];
    assert!(screen.row_text(0).starts_with("ABCDEFGHIJ"));
    assert!(screen.row_text(1).starts_with("KLMNO"));
    assert_eq!((screen.cursor.row, screen.cursor.col), (0, 4));
}

#[test]
fn clear_refuses_to_disturb_the_alternate_screen() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "alt".into(),
        cwd: Some("/".into()),
    }));
    let pane = client.snapshot().panes[0].id;
    emit(&mut client, pane, "\\033[?1049hALT-\\123CREEN-MARKER");
    wait_for(|| {
        screen_rows(&client.snapshot().panes[0])
            .iter()
            .any(|s| s.contains("ALT-SCREEN-MARKER"))
    });
    assert!(matches!(
        client.request(Request::ClearViewport { pane }),
        Response::Error(_)
    ));
    assert!(
        screen_rows(&client.snapshot().panes[0])
            .iter()
            .any(|s| s.contains("ALT-SCREEN-MARKER"))
    );
}

#[test]
fn copy_all_preserves_wrapped_unicode_and_masks_hidden_text() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "unicode".into(),
        cwd: Some("/".into()),
    }));
    let pane = client.snapshot().panes[0].id;
    ok(client.request(Request::Input {
        pane,
        bytes: b"stty -echo; echo ECHO-OFF-DONE\n".to_vec(),
    }));
    wait_for(|| shows_line(&mut client, "ECHO-OFF-DONE"));
    ok(client.request(Request::Resize {
        pane,
        cols: 10,
        rows: 10,
    }));
    ok(client.request(Request::Input {
        pane,
        bytes: "printf '\\033[2J\\033[HABCDEFGHIJKLMN界e\u{301}\\033[8mSECRET\\033[0mVISIBLE'\n"
            .as_bytes()
            .to_vec(),
    }));
    let mut copied = String::new();
    wait_for(|| {
        if let Response::AllText { text, .. } = client.request(Request::CopyAll { pane }) {
            copied = text;
            copied.contains("VISIBLE")
        } else {
            false
        }
    });
    assert!(copied.contains("ABCDEFGHIJKLMN界e\u{301}"), "{copied:?}");
    assert!(!copied.contains("SECRET"), "{copied:?}");
}

#[test]
fn copy_all_refuses_oversized_history_without_returning_partial_text() {
    let server = Server::start();
    let mut client = server.connect();
    ok(client.request(Request::CreateWorkspace {
        name: "limit".into(),
        cwd: Some("/".into()),
    }));
    let pane = client.snapshot().panes[0].id;
    ok(client.request(Request::Resize {
        pane,
        cols: 160,
        rows: 5,
    }));
    ok(client.request(Request::Input {
        pane,
        bytes:
            b"stty -echo; x=$(printf '%0120d' 0); yes \"$x\" | head -n 9000; echo COPY-LIMIT-DONE\n"
                .to_vec(),
    }));
    wait_for(|| shows_line(&mut client, "COPY-LIMIT-DONE"));
    let response = client.request(Request::CopyAll { pane });
    assert!(
        matches!(response, Response::Error(ref message) if message.contains("1 MiB")),
        "{response:?}"
    );
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
        source: None,
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
