//! Agent panes against a real server, with a scripted stand-in for the agent program.
#![cfg(unix)]

use spooky_core::{Agent, AgentRole, DialogAnswer, Pane, Request, Response, Snapshot, ToolState};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

/// A scripted stand-in for `omp --mode rpc-ui`: it answers the lookups the server makes,
/// streams a reply to a prompt, asks a question when told to ("ask me"), thinks when told to
/// ("think"), exits on "die", and records its pid, its arguments and every line it gets.
const FAKE_OMP: &str = r#"#!/bin/sh
echo $$ > "$FAKE_PIDFILE"
echo "$*" >> "$FAKE_ARGS"
MODEL=m1
LEVEL=low
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$FAKE_LOG"
  # Like omp, the session file exists once the conversation has begun.
  case "$line" in *'"type":"prompt"'*) : > "$FAKE_SESSION" ;; esac
  case "$line" in
    *'"message":"die"'*) exit 3 ;;
    *'"id":"q:state"'*)
      echo "{\"id\":\"q:state\",\"type\":\"response\",\"command\":\"get_state\",\"success\":true,\"data\":{\"model\":{\"provider\":\"fake\",\"id\":\"$MODEL\"},\"thinkingLevel\":\"$LEVEL\",\"contextUsage\":{\"percent\":7.2},\"sessionFile\":\"$FAKE_SESSION\"}}" ;;
    *'"id":"q:models"'*)
      echo '{"id":"q:models","type":"response","command":"get_available_models","success":true,"data":{"models":[{"provider":"fake","id":"m1"},{"provider":"fake","id":"m2"}]}}' ;;
    *'"id":"q:levels"'*)
      echo '{"id":"q:levels","type":"response","command":"get_available_thinking_levels","success":true,"data":{"levels":["off","low","high"]}}' ;;
    *'"id":"q:history"'*)
      echo '{"id":"q:history","type":"response","command":"get_messages_page","success":true,"data":{"messages":[{"role":"user","content":[{"type":"text","text":"earlier question"}]},{"role":"assistant","content":[{"type":"text","text":"earlier answer"}]}]}}' ;;
    *'"type":"set_model"'*)
      case "$line" in *'"modelId":"m2"'*) MODEL=m2 ;; esac
      echo '{"id":"q:set","type":"response","success":true}' ;;
    *'"type":"set_thinking_level"'*)
      case "$line" in *'"level":"high"'*) LEVEL=high ;; esac
      echo '{"id":"q:set","type":"response","success":true}' ;;
    *'"type":"extension_ui_response"'*)
      echo '{"type":"tool_execution_end","toolCallId":"c1","toolName":"ask","result":{"content":[{"type":"text","text":"answered"}]},"isError":false}'
      echo '{"type":"agent_end"}' ;;
    *'"message":"/usage"'*)
      printf '%s\n' '{"type":"command_output","text":"Fake\n- 1 hour\n  me@x: 25.00% used (75.0% left)\n  resets in 40m"}' ;;
    *'"message":"ask me"'*)
      echo '{"type":"agent_start"}'
      echo '{"type":"tool_execution_start","toolCallId":"c1","toolName":"ask","args":{"html":"<form/>"},"intent":"Asking a question"}'
      echo '{"type":"extension_ui_request","id":"ui1","method":"select","title":"Which color?","options":["Red","Blue"]}' ;;
    *'"message":"think"'*)
      echo '{"type":"agent_start"}'
      echo '{"type":"message_update","assistantMessageEvent":{"type":"thinking_start"}}'
      echo '{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":"mm"}}'
      echo '{"type":"message_update","assistantMessageEvent":{"type":"thinking_end","content":"mm, so"}}'
      echo '{"type":"message_update","assistantMessageEvent":{"type":"text_start"}}'
      echo '{"type":"message_update","assistantMessageEvent":{"type":"text_end","content":"ok"}}'
      echo '{"type":"agent_end"}' ;;
    *'"type":"prompt"'*)
      echo '{"type":"agent_start"}'
      echo '{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}'
      echo '{"type":"message_update","assistantMessageEvent":{"type":"text_start"}}'
      echo '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"po"}}'
      echo '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"ng"}}'
      echo '{"type":"message_update","assistantMessageEvent":{"type":"text_end","content":"pong"}}'
      echo '{"type":"agent_end"}' ;;
  esac
done
"#;

struct Fixture {
    child: Child,
    dir: PathBuf,
    socket: PathBuf,
}

impl Fixture {
    fn start() -> Self {
        Self::start_with(|_, _| {})
    }

    /// Starts a server; `configure` gets the command and the test directory and runs
    /// after the standard variables, so it can override them.
    fn start_with(configure: impl FnOnce(&mut Command, &Path)) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("spooky-agent-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap_or_else(|e| panic!("test directory: {e}"));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|e| panic!("permissions: {e}"));
        let fake = dir.join("omp");
        fs::write(&fake, FAKE_OMP).unwrap_or_else(|e| panic!("fake agent: {e}"));
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|e| panic!("permissions: {e}"));
        fs::write(
            dir.join("config.yml"),
            "modelRoles:\n  default: fake/m1:low\n  plan: fake/m2\n",
        )
        .unwrap_or_else(|e| panic!("agent configuration: {e}"));
        Self::launch(dir, configure)
    }

    /// Starts a server over the directory `dir`, which holds the stand-in agent and is
    /// where the server keeps its saved state.
    fn launch(dir: PathBuf, configure: impl FnOnce(&mut Command, &Path)) -> Self {
        let fake = dir.join("omp");
        let socket = dir.join("s");
        let mut command = Command::new(env!("CARGO_BIN_EXE_spooky-server"));
        command
            .arg("serve")
            .env("SPOOKY_SOCKET", &socket)
            .env("SHELL", "/bin/sh")
            .env("SPOOKY_OMP", &fake)
            .env("FAKE_PIDFILE", dir.join("pid"))
            .env("PI_CODING_AGENT_DIR", &dir)
            .env("FAKE_SESSION", dir.join("fake-session.jsonl"))
            .env("FAKE_ARGS", dir.join("args"))
            .env("FAKE_LOG", dir.join("log"))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure(&mut command, &dir);
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
        let reader = BufReader::new(stream.try_clone().unwrap_or_else(|e| panic!("clone: {e}")));
        Connection { stream, reader }
    }

    /// Everything the stand-in agent has been sent so far.
    fn agent_log(&self) -> String {
        fs::read_to_string(self.dir.join("log")).unwrap_or_default()
    }

    /// The arguments of each start of the stand-in agent.
    fn agent_starts(&self) -> Vec<String> {
        fs::read_to_string(self.dir.join("args"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Stops this server (saving its workspaces) and starts another over the same state.
    fn restart(mut self) -> Self {
        let status = Command::new(env!("CARGO_BIN_EXE_spooky-server"))
            .args(["stop", "--force"])
            .env("SPOOKY_SOCKET", &self.socket)
            .output()
            .unwrap_or_else(|e| panic!("stop: {e}"));
        assert!(status.status.success(), "stop failed: {status:?}");
        let _ = self.child.wait();
        let dir = std::mem::take(&mut self.dir);
        Self::launch(dir, |_, _| {})
    }

    fn agent_pid(&self) -> i32 {
        wait_for(|| {
            fs::read_to_string(self.dir.join("pid")).is_ok_and(|s| s.trim().parse::<i32>().is_ok())
        });
        fs::read_to_string(self.dir.join("pid"))
            .unwrap_or_else(|e| panic!("pid file: {e}"))
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("pid: {e}"))
    }
}

impl Drop for Fixture {
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
        serde_json::to_writer(&mut self.stream, &request).unwrap_or_else(|e| panic!("encode: {e}"));
        self.stream
            .write_all(b"\n")
            .unwrap_or_else(|e| panic!("write: {e}"));
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .unwrap_or_else(|e| panic!("read: {e}"));
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("decode {line:?}: {e}"))
    }

    fn snapshot(&mut self) -> Snapshot {
        match self.request(Request::Snapshot { since: None }) {
            Response::Snapshot(s) => s,
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    /// The first agent pane, if the workspace has one.
    fn agent_pane(&mut self) -> Option<Pane> {
        self.snapshot()
            .panes
            .into_iter()
            .find(|p| p.agent.is_some())
    }
}

fn ok(response: Response) {
    assert!(matches!(response, Response::Ok), "{response:?}");
}

fn error(response: Response) -> String {
    match response {
        Response::Error(message) => message,
        other => panic!("expected an error, got {other:?}"),
    }
}

fn wait_for(mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "condition timed out");
        thread::sleep(Duration::from_millis(20));
    }
}

fn alive(pid: i32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn start_workspace(client: &mut Connection, dir: &Path) {
    ok(client.request(Request::CreateWorkspace {
        name: "w".into(),
        cwd: Some(dir.to_string_lossy().into_owned()),
    }));
}

fn create_agent(client: &mut Connection) {
    let workspace = client.snapshot().workspaces[0].id;
    ok(client.request(Request::CreateAgent {
        workspace,
        backend: "omp".into(),
    }));
}

#[test]
fn a_conversation_streams_and_every_client_sees_the_same_one() {
    let fixture = Fixture::start();
    let mut client = fixture.connect();
    start_workspace(&mut client, &fixture.dir);
    create_agent(&mut client);
    let pane = client.agent_pane().unwrap_or_else(|| panic!("agent pane"));
    assert_eq!(pane.agent.as_ref().map(|a| a.backend.as_str()), Some("omp"));

    ok(client.request(Request::AgentPrompt {
        pane: pane.id,
        text: "hello".into(),
    }));
    wait_for(|| {
        client
            .agent_pane()
            .and_then(|p| p.agent)
            .is_some_and(|a| !a.busy && a.messages.len() == 2)
    });

    // A second connection, as after a reconnect, reads the same conversation.
    let mut other = fixture.connect();
    let agent = other
        .agent_pane()
        .and_then(|p| p.agent)
        .unwrap_or_else(|| panic!("agent"));
    assert_eq!(agent.messages[0].role, AgentRole::User);
    assert_eq!(agent.messages[0].text, "hello");
    assert_eq!(agent.messages[1].role, AgentRole::Assistant);
    assert_eq!(agent.messages[1].text, "pong");
}

#[test]
fn an_agent_pane_refuses_terminal_requests_and_a_terminal_refuses_agent_requests() {
    let fixture = Fixture::start();
    let mut client = fixture.connect();
    start_workspace(&mut client, &fixture.dir);
    create_agent(&mut client);
    let agent = client
        .agent_pane()
        .unwrap_or_else(|| panic!("agent pane"))
        .id;
    let shell = client
        .snapshot()
        .panes
        .iter()
        .find(|p| p.agent.is_none())
        .unwrap_or_else(|| panic!("shell pane"))
        .id;

    for request in [
        Request::Input {
            pane: agent,
            bytes: b"ls\n".to_vec(),
        },
        Request::Resize {
            pane: agent,
            cols: 80,
            rows: 24,
        },
        Request::Split {
            pane: agent,
            axis: spooky_core::Axis::Vertical,
        },
    ] {
        let message = error(client.request(request));
        assert!(message.contains("agent"), "{message}");
    }
    let message = error(client.request(Request::AgentPrompt {
        pane: shell,
        text: "hi".into(),
    }));
    assert!(message.contains("terminal"), "{message}");
    let message = error(client.request(Request::AgentPrompt {
        pane: agent,
        text: "   ".into(),
    }));
    assert!(message.contains("empty"), "{message}");
}

#[test]
fn closing_the_tab_ends_the_agent_process_and_an_unknown_backend_is_refused() {
    let fixture = Fixture::start();
    let mut client = fixture.connect();
    start_workspace(&mut client, &fixture.dir);
    let workspace = client.snapshot().workspaces[0].id;
    let message = error(client.request(Request::CreateAgent {
        workspace,
        backend: "nope".into(),
    }));
    assert!(message.contains("unknown agent backend"), "{message}");

    create_agent(&mut client);
    let pid = fixture.agent_pid();
    assert!(alive(pid));
    let snapshot = client.snapshot();
    let tab = snapshot.workspaces[0]
        .tabs
        .iter()
        .find(|tab| {
            let mut ids = Vec::new();
            tab.layout.panes(&mut ids);
            ids.iter().any(|id| {
                snapshot
                    .panes
                    .iter()
                    .any(|p| p.id == *id && p.agent.is_some())
            })
        })
        .unwrap_or_else(|| panic!("agent tab"))
        .id;
    ok(client.request(Request::CloseTab { tab }));
    wait_for(|| !alive(pid));
}

#[test]
fn an_agent_that_exits_says_why_and_refuses_more_messages() {
    let fixture = Fixture::start();
    let mut client = fixture.connect();
    start_workspace(&mut client, &fixture.dir);
    create_agent(&mut client);
    let pane = client
        .agent_pane()
        .unwrap_or_else(|| panic!("agent pane"))
        .id;
    ok(client.request(Request::AgentPrompt {
        pane,
        text: "die".into(),
    }));
    wait_for(|| {
        client.agent_pane().and_then(|p| p.agent).is_some_and(|a| {
            a.ended
                .as_deref()
                .is_some_and(|r| r.contains("exit status: 3"))
        })
    });
    let message = error(client.request(Request::AgentPrompt {
        pane,
        text: "again".into(),
    }));
    assert!(message.contains("ended"), "{message}");
}

/// Whether some row of the shell pane shows `text`.
fn screen_has(client: &mut Connection, text: &str) -> bool {
    client.snapshot().panes.iter().any(|pane| {
        pane.agent.is_none() && (0..pane.rows).any(|row| pane.row_text(row).contains(text))
    })
}

/// One startup file and what it contains, given the directory with a decoy `omp`.
type ProfileFile = (&'static str, fn(&Path) -> String);

/// How a shell's profile puts a directory first on `PATH`, and which files it lives in.
struct Profile {
    /// The shell the server is told is the user's (`$SHELL`).
    shell: &'static str,
    /// Another program the profile needs, when there is one.
    needs: Option<&'static str>,
    files: &'static [ProfileFile],
}

const FISH: &str = "/opt/homebrew/bin/fish";

fn sh_prepend(dir: &Path) -> String {
    format!("export PATH=\"{}:$PATH\"\n", dir.display())
}

/// What mise does: rebuild `PATH`, with its own directories first, at every prompt.
fn zsh_every_prompt(dir: &Path) -> String {
    format!(
        "{}_rebuild() {{ path=(\"{}\" $path) }}\nprecmd_functions+=(_rebuild)\n",
        sh_prepend(dir),
        dir.display()
    )
}

fn fish_every_prompt(dir: &Path) -> String {
    format!(
        "set -gx PATH {0} $PATH\nfunction _rebuild --on-event fish_prompt\n    set -gx PATH {0} $PATH\nend\n",
        dir.display()
    )
}

/// A bash profile that hands over to fish, as many setups do.
fn exec_fish(_: &Path) -> String {
    format!("exec {FISH}\n")
}

const PROFILES: [Profile; 4] = [
    Profile {
        shell: "/bin/bash",
        needs: None,
        files: &[(".bash_profile", sh_prepend), (".bashrc", sh_prepend)],
    },
    Profile {
        shell: "/bin/zsh",
        needs: None,
        files: &[(".zprofile", sh_prepend), (".zshrc", zsh_every_prompt)],
    },
    Profile {
        shell: FISH,
        needs: None,
        files: &[(".config/fish/config.fish", fish_every_prompt)],
    },
    // `$SHELL` is bash, as over SSH, but bash starts fish: the shim must reach fish too.
    Profile {
        shell: "/bin/bash",
        needs: Some(FISH),
        files: &[
            (".bash_profile", exec_fish),
            (".bashrc", exec_fish),
            (".config/fish/config.fish", fish_every_prompt),
        ],
    },
];

#[test]
fn omp_typed_in_a_pane_shows_the_agent_there_and_anything_unsupported_runs_the_real_one() {
    // Each shell is checked where it is installed (zsh and fish are not on every CI image).
    let mut checked = 0;
    for profile in &PROFILES {
        if Path::new(profile.shell).exists() && profile.needs.is_none_or(|n| Path::new(n).exists())
        {
            shim_wins_over_a_profile(profile);
            checked += 1;
        }
    }
    assert!(checked > 0, "no shell to check the shim with");
}

fn shim_wins_over_a_profile(profile: &Profile) {
    // A profile that puts a directory with its own `omp` first on PATH, as real ones do:
    // the shim must still win, and must hand over to that `omp` when it is not wanted.
    let fixture = Fixture::start_with(|command, dir| {
        let decoy = dir.join("decoy");
        fs::create_dir(&decoy).unwrap_or_else(|e| panic!("decoy directory: {e}"));
        let real = decoy.join("omp");
        fs::write(&real, "#!/bin/sh\necho \"REAL-OMP $#\"\n")
            .unwrap_or_else(|e| panic!("real omp: {e}"));
        fs::set_permissions(&real, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|e| panic!("permissions: {e}"));
        for (name, text) in profile.files {
            let file = dir.join(name);
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent).unwrap_or_else(|e| panic!("{name}: {e}"));
            }
            fs::write(&file, text(&decoy)).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        command.env("SHELL", profile.shell).env("HOME", dir);
    });
    let shell = profile.shell;
    let mut client = fixture.connect();
    start_workspace(&mut client, &fixture.dir);
    let pane = client.snapshot().panes[0].id;
    let type_line = |client: &mut Connection, line: &str| {
        ok(client.request(Request::Input {
            pane,
            bytes: format!("{line}\n").into_bytes(),
        }));
    };

    type_line(&mut client, "command -v omp");
    wait_for(|| screen_has(&mut client, "/shim/omp"));

    // Unsupported options and subcommands belong to the real program.
    type_line(&mut client, "omp --flag");
    wait_for(|| screen_has(&mut client, "REAL-OMP 1"));
    type_line(&mut client, "omp commit");
    wait_for(|| screen_has(&mut client, "REAL-OMP 1"));
    assert!(
        client.agent_pane().is_none(),
        "{shell}: an unsupported option or a subcommand means the real omp"
    );

    // A bare `omp` shows the agent in this very pane, with the shell kept underneath.
    type_line(&mut client, "omp");
    wait_for(|| client.agent_pane().is_some());
    let snapshot = client.snapshot();
    let tabs = &snapshot.workspaces[0].tabs;
    let mut panes = Vec::new();
    tabs[0].layout.panes(&mut panes);
    assert_eq!(
        (tabs.len(), panes.len()),
        (1, 1),
        "{shell}: no new tab and no split"
    );
    assert_eq!(panes[0], pane, "{shell}: the agent takes the shell's place");
    assert_eq!(
        snapshot.panes.len(),
        1,
        "{shell}: the covered shell is not shown"
    );
    // Closing the agent brings the shell back, with what it had on screen.
    wait_for(|| fixture.agent_starts().len() == 1);
    ok(client.request(Request::ClosePane { pane }));
    wait_for(|| client.agent_pane().is_none());
    wait_for(|| screen_has(&mut client, "Opened omp in this pane"));
    assert_eq!(client.snapshot().workspaces[0].tabs.len(), 1);
    // Supported options and a sentence open one too.
    type_line(&mut client, "omp --model fake/m2 'say hi now'");
    wait_for(|| fixture.agent_starts().len() == 2);
    let starts = fixture.agent_starts();
    for expected in ["--mode rpc-ui", "--mode rpc-ui --model=fake/m2"] {
        assert!(starts.iter().any(|s| s == expected), "{shell}: {starts:?}");
    }
    wait_for(|| fixture.agent_log().contains("say hi now"));
}

/// The first agent pane's conversation.
fn agent_state(client: &mut Connection) -> Option<Agent> {
    client.agent_pane().and_then(|pane| pane.agent)
}

#[test]
fn the_agent_reports_its_model_and_choices_and_only_listed_ones_can_be_chosen() {
    let fixture = Fixture::start();
    let mut client = fixture.connect();
    start_workspace(&mut client, &fixture.dir);
    create_agent(&mut client);
    wait_for(|| {
        agent_state(&mut client).is_some_and(|a| {
            a.model.as_deref() == Some("fake/m1") && !a.models.is_empty() && !a.levels.is_empty()
        })
    });
    let agent = agent_state(&mut client).unwrap_or_else(|| panic!("agent"));
    assert_eq!(agent.models, ["fake/m1", "fake/m2"]);
    assert_eq!(agent.levels, ["off", "low", "high"]);
    let roles: Vec<_> = agent
        .roles
        .iter()
        .map(|r| (r.role.as_str(), r.model.as_str(), r.thinking.as_deref()))
        .collect();
    assert_eq!(
        roles,
        [
            ("default", "fake/m1", Some("low")),
            ("plan", "fake/m2", None)
        ],
        "the roles come from the program's own configuration"
    );
    assert_eq!(
        (agent.thinking.as_deref(), agent.context),
        (Some("low"), Some(7))
    );
    let pane = client.agent_pane().unwrap_or_else(|| panic!("pane")).id;

    ok(client.request(Request::AgentSetModel {
        pane,
        model: "fake/m2".into(),
    }));
    wait_for(|| agent_state(&mut client).is_some_and(|a| a.model.as_deref() == Some("fake/m2")));
    ok(client.request(Request::AgentSetThinking {
        pane,
        level: "high".into(),
    }));
    wait_for(|| agent_state(&mut client).is_some_and(|a| a.thinking.as_deref() == Some("high")));

    let message = error(client.request(Request::AgentSetModel {
        pane,
        model: "fake/zzz".into(),
    }));
    assert!(message.contains("does not offer"), "{message}");
    let message = error(client.request(Request::AgentSetThinking {
        pane,
        level: "max".into(),
    }));
    assert!(message.contains("does not offer"), "{message}");
    assert!(
        !fixture.agent_log().contains("zzz") && !fixture.agent_log().contains("\"max\""),
        "a refused choice never reaches the agent"
    );
}

#[test]
fn a_question_waits_for_the_user_and_only_a_valid_answer_reaches_the_agent() {
    let fixture = Fixture::start();
    let mut client = fixture.connect();
    start_workspace(&mut client, &fixture.dir);
    create_agent(&mut client);
    let pane = client.agent_pane().unwrap_or_else(|| panic!("pane")).id;
    ok(client.request(Request::AgentPrompt {
        pane,
        text: "ask me".into(),
    }));
    wait_for(|| agent_state(&mut client).is_some_and(|a| !a.dialogs.is_empty()));
    let agent = agent_state(&mut client).unwrap_or_else(|| panic!("agent"));
    assert!(agent.busy, "the turn waits for the answer");
    assert_eq!(agent.dialogs[0].options, ["Red", "Blue"]);
    let tool = agent
        .messages
        .iter()
        .find_map(|m| m.tool.as_ref().map(|t| (m, t)))
        .unwrap_or_else(|| panic!("tool entry"));
    assert_eq!(
        (tool.0.text.as_str(), tool.1.name.as_str()),
        ("Asking a question", "ask")
    );
    assert_eq!(tool.1.state, ToolState::Running);

    thread::sleep(Duration::from_millis(300));
    assert!(
        !fixture.agent_log().contains("extension_ui_response"),
        "nothing answers the question but the user"
    );
    for wrong in [
        DialogAnswer::Value("Green".into()),
        DialogAnswer::Confirm(true),
    ] {
        let message = error(client.request(Request::AgentAnswer {
            pane,
            dialog: "ui1".into(),
            answer: wrong,
        }));
        assert!(
            message.contains("option") || message.contains("fit"),
            "{message}"
        );
    }
    let message = error(client.request(Request::AgentAnswer {
        pane,
        dialog: "nope".into(),
        answer: DialogAnswer::Cancel,
    }));
    assert!(message.contains("no longer open"), "{message}");
    assert!(!fixture.agent_log().contains("extension_ui_response"));

    ok(client.request(Request::AgentAnswer {
        pane,
        dialog: "ui1".into(),
        answer: DialogAnswer::Value("Blue".into()),
    }));
    wait_for(|| {
        agent_state(&mut client).is_some_and(|a| {
            !a.busy
                && a.dialogs.is_empty()
                && a.messages.iter().any(|m| {
                    m.tool
                        .as_ref()
                        .is_some_and(|t| t.state == ToolState::Done && t.output == "answered")
                })
        })
    });
    let log = fixture.agent_log();
    assert_eq!(log.matches("extension_ui_response").count(), 1);
    assert!(log.contains("\"value\":\"Blue\""), "{log}");
}

#[test]
fn reasoning_is_its_own_entry_before_the_answer() {
    let fixture = Fixture::start();
    let mut client = fixture.connect();
    start_workspace(&mut client, &fixture.dir);
    create_agent(&mut client);
    let pane = client.agent_pane().unwrap_or_else(|| panic!("pane")).id;
    ok(client.request(Request::AgentPrompt {
        pane,
        text: "think".into(),
    }));
    wait_for(|| agent_state(&mut client).is_some_and(|a| !a.busy && a.messages.len() == 2));
    let agent = agent_state(&mut client).unwrap_or_else(|| panic!("agent"));
    let shown: Vec<_> = agent
        .messages
        .iter()
        .map(|m| (m.role, m.text.as_str()))
        .collect();
    assert_eq!(
        shown,
        [
            (AgentRole::Thinking, "mm, so"),
            (AgentRole::Assistant, "ok")
        ]
    );
}

#[test]
fn an_agent_comes_back_after_a_server_restart_with_its_conversation() {
    let fixture = Fixture::start();
    let mut client = fixture.connect();
    start_workspace(&mut client, &fixture.dir);
    create_agent(&mut client);
    let pane = client.agent_pane().unwrap_or_else(|| panic!("pane")).id;
    ok(client.request(Request::AgentPrompt {
        pane,
        text: "hello".into(),
    }));
    wait_for(|| agent_state(&mut client).is_some_and(|a| !a.busy && a.messages.len() == 2));
    // The saved layout must hold the session the program reported.
    wait_for(|| {
        fs::read_to_string(fixture.dir.join("workspaces.json"))
            .is_ok_and(|saved| saved.contains("fake-session.jsonl"))
    });
    drop(client);

    let fixture = fixture.restart();
    let mut client = fixture.connect();
    wait_for(|| {
        agent_state(&mut client).is_some_and(|a| {
            a.messages.iter().any(|m| m.text == "earlier answer") && a.model.is_some()
        })
    });
    let snapshot = client.snapshot();
    assert_eq!(
        snapshot.workspaces[0].tabs.len(),
        2,
        "the shell tab and the agent tab"
    );
    assert!(
        snapshot.panes.iter().any(|p| p.agent.is_none()),
        "the shell came back too"
    );
    let starts = fixture.agent_starts();
    assert_eq!(starts.len(), 2, "the agent was started again: {starts:?}");
    assert!(
        starts[1].contains("--mode rpc-ui")
            && starts[1].contains("--resume=")
            && starts[1].contains("fake-session.jsonl"),
        "{starts:?}"
    );
    let agent = agent_state(&mut client).unwrap_or_else(|| panic!("agent"));
    assert_eq!(agent.messages[0].role, AgentRole::User);
    assert_eq!(agent.messages[0].text, "earlier question");
}

#[test]
fn an_agent_over_a_terminal_comes_back_over_it_and_closing_it_shows_the_shell() {
    let fixture = Fixture::start();
    let mut client = fixture.connect();
    start_workspace(&mut client, &fixture.dir);
    let shell = client.snapshot().panes[0].id;
    ok(client.request(Request::OpenAgent {
        pane: shell,
        backend: "omp".into(),
        cwd: fixture.dir.to_string_lossy().into_owned(),
        args: Vec::new(),
    }));
    wait_for(|| agent_state(&mut client).is_some_and(|a| a.model.is_some()));
    wait_for(|| {
        fs::read_to_string(fixture.dir.join("workspaces.json")).is_ok_and(|saved| {
            saved.contains("\"covers\": true") || saved.contains("\"covers\":true")
        })
    });
    assert_eq!(client.agent_pane().map(|p| p.id), Some(shell));
    drop(client);

    let fixture = fixture.restart();
    let mut client = fixture.connect();
    wait_for(|| agent_state(&mut client).is_some_and(|a| a.model.is_some()));
    let snapshot = client.snapshot();
    assert_eq!(snapshot.panes.len(), 1, "the agent hides the shell again");
    assert_eq!(snapshot.panes[0].id, shell);
    ok(client.request(Request::ClosePane { pane: shell }));
    wait_for(|| client.agent_pane().is_none());
    let snapshot = client.snapshot();
    assert_eq!((snapshot.panes.len(), snapshot.panes[0].id), (1, shell));
    assert_eq!(snapshot.workspaces[0].tabs.len(), 1, "the pane stays");
    // A second close is the terminal's own.
    ok(client.request(Request::ClosePane { pane: shell }));
}

#[test]
fn usage_is_read_when_the_agent_is_idle_and_refused_while_it_works() {
    let fixture = Fixture::start();
    let mut client = fixture.connect();
    start_workspace(&mut client, &fixture.dir);
    create_agent(&mut client);
    let pane = client.agent_pane().unwrap_or_else(|| panic!("pane")).id;
    assert!(agent_state(&mut client).is_some_and(|a| a.usage.is_none()));
    ok(client.request(Request::AgentUsage { pane }));
    wait_for(|| agent_state(&mut client).is_some_and(|a| a.usage.is_some()));
    let usage = agent_state(&mut client)
        .and_then(|a| a.usage)
        .unwrap_or_default();
    assert_eq!(usage[0].name, "Fake");
    let window = &usage[0].windows[0];
    assert_eq!(
        (window.name.as_str(), window.left, window.resets.as_str()),
        ("1 hour", 75, "40m")
    );
    assert!(
        !agent_state(&mut client)
            .is_some_and(|a| a.messages.iter().any(|m| m.text.contains("Fake"))),
        "the report is not a message of the conversation"
    );

    ok(client.request(Request::AgentPrompt {
        pane,
        text: "ask me".into(),
    }));
    wait_for(|| agent_state(&mut client).is_some_and(|a| a.busy));
    let message = error(client.request(Request::AgentUsage { pane }));
    assert!(message.contains("working"), "{message}");
}

#[test]
fn a_conversation_that_never_began_comes_back_empty_instead_of_failing_to_resume() {
    let fixture = Fixture::start();
    let mut client = fixture.connect();
    start_workspace(&mut client, &fixture.dir);
    create_agent(&mut client);
    // The session is known, but the program has not written its file: nothing was said.
    wait_for(|| agent_state(&mut client).is_some_and(|a| a.model.is_some()));
    wait_for(|| {
        fs::read_to_string(fixture.dir.join("workspaces.json"))
            .is_ok_and(|saved| saved.contains("fake-session.jsonl"))
    });
    drop(client);

    let fixture = fixture.restart();
    let mut client = fixture.connect();
    wait_for(|| agent_state(&mut client).is_some_and(|a| a.model.is_some()));
    let agent = agent_state(&mut client).unwrap_or_else(|| panic!("agent"));
    assert!(agent.ended.is_none(), "{:?}", agent.ended);
    assert!(
        fixture
            .agent_starts()
            .iter()
            .all(|s| !s.contains("--resume")),
        "{:?}",
        fixture.agent_starts()
    );
}
