//! PTY exit preserves rendered output and navigable terminal history.
#![cfg(unix)]

use spooky_core::{Request, Response, Snapshot};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Fixture {
    child: Child,
    dir: std::path::PathBuf,
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}
impl Fixture {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|v| v.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("spooky-exit-{}-{unique}", std::process::id()));
        fs::create_dir(&dir).unwrap_or_else(|e| panic!("directory: {e}"));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|e| panic!("permissions: {e}"));
        let socket = dir.join("s");
        let child = Command::new(env!("CARGO_BIN_EXE_spooky-server"))
            .arg("serve")
            .env("SPOOKY_SOCKET", &socket)
            .env("SHELL", "/bin/sh")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("server: {e}"));
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match UnixStream::connect(&socket) {
                Ok(stream) => break stream,
                Err(e) => {
                    assert!(Instant::now() < deadline, "connection timeout: {e}");
                    thread::sleep(Duration::from_millis(10));
                }
            }
        };
        let reader = BufReader::new(stream.try_clone().unwrap_or_else(|e| panic!("clone: {e}")));
        Self {
            child,
            dir,
            stream,
            reader,
        }
    }
    fn request(&mut self, request: Request) -> Response {
        serde_json::to_writer(&mut self.stream, &request)
            .unwrap_or_else(|e| panic!("request: {e}"));
        self.stream
            .write_all(b"\n")
            .unwrap_or_else(|e| panic!("write: {e}"));
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .unwrap_or_else(|e| panic!("read: {e}"));
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("response: {e}"))
    }
    fn snapshot(&mut self) -> Snapshot {
        match self.request(Request::Snapshot { since: None }) {
            Response::Snapshot(s) => s,
            other => panic!("snapshot: {other:?}"),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn exited_shell_keeps_scrollback_and_invalidates_revision() {
    let mut server = Fixture::new();
    assert!(matches!(
        server.request(Request::CreateWorkspace {
            name: "exit".into(),
            cwd: Some("/".into())
        }),
        Response::Ok
    ));
    let before = server.snapshot();
    let pane = before.panes[0].id;
    assert!(matches!(
        server.request(Request::Resize {
            pane,
            cols: 80,
            rows: 4
        }),
        Response::Ok
    ));
    assert!(matches!(
        server.request(Request::Input {
            pane,
            bytes: b"printf 'FIRSTLINE\\nsecond\\nthird\\nfourth\\nfifth\\n'; exit\n".to_vec()
        }),
        Response::Ok
    ));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if server.snapshot().panes[0].exited {
            break;
        }
        assert!(Instant::now() < deadline, "shell did not exit");
        thread::sleep(Duration::from_millis(20));
    }
    loop {
        assert!(matches!(
            server.request(Request::Scroll { pane, lines: 100 }),
            Response::Ok
        ));
        let after = server.snapshot();
        assert!(after.revision > before.revision);
        let screen = &after.panes[0];
        let text = (0..screen.rows)
            .map(|row| screen.row_text(row))
            .collect::<Vec<_>>()
            .join("\n");
        if text.contains("FIRSTLINE") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "scrollback missing first line: {text:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
