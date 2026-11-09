# Spooky product contract

Spooky is a native, GPU-rendered dropdown terminal with persistent project workspaces. It uses Rust crates rather than reimplementing a terminal parser, pseudoterminal layer or graphics stack.

## Platforms

- macOS is the initial development and runtime validation platform.
- Linux is a target for the same server and frontend. X11 and Wayland integration must be described separately; a successful build is not desktop verification.
- Windows is deferred. Its eventual implementation must be native, without requiring WSL. Do not claim current Windows support.
- tmux is not required. Users may run it inside a shell, but Spooky manages its own workspaces.

## Session model

A workspace has a display name, a server-side working directory and ordered tabs. Each tab has a binary split layout whose leaves identify terminal panes. A pane owns a shell, a PTY and terminal state.

The server is the authority. Closing/hiding the GUI disconnects or hides the client without terminating the shells. Reconnecting restores the existing terminal state and workspace tree. Server termination or operating-system restart does not preserve running processes.

Creating workspaces, tabs and splits starts real shells. Explicit pane/workspace closure is destructive and requires frontend confirmation. Process exit remains visible rather than fabricating a live terminal.

## Frontend

- Polished dark interface inspired by Ghostty, without a WebView.
- Workspace sidebar, tabs, visible split panes and unambiguous keyboard focus.
- Configurable global dropdown shortcut where supported. Unsupported compositor features must be surfaced.
- GPU-rendered terminal text, ANSI colors, cursor, wide/combining Unicode and terminal application input modes.
- Grid resizing updates both terminal state and the PTY. Clipboard paste respects bracketed-paste mode. Text fields must never leak input into shells.
- Copy selection and scrollback navigation.
- Network/process operations do not block the GUI thread.

## Remote module

Remote workspaces use SSH to a host with a compatible spooky-server installed. The server runs under the remote user, without privileged installation requirements. Connection loss must not terminate its shells.

Use the user's standard SSH authentication and host verification. No disabled host checks, automatic acceptance of changed keys, implicit agent forwarding, automatic remote installation or password storage. Local and remote target states must not mix, and queued input must not be replayed against a different target.

## Captures and attachments

Capture the focused terminal viewport as a real PNG. A text dump is not a screenshot. Accept files explicitly picked or dropped by the user, and transfer remote files using SFTP. Insert a shell-quoted destination path without executing it.

This does not mean every terminal program accepts image attachments. Agent-specific image delivery and desktop screen capture are separate capabilities and must not be claimed without their own implementation and permission flow.

## Acceptance evidence

Launch the real server with a PTY; observe command output, ANSI/Unicode state, resize, multiple panes/tabs, process exit and reconnect preserving environment/cwd. Exercise SSH and transfer with real SSH/SFTP, not a mock command. Launch the native frontend and observe rendered terminal output and interaction. Document each platform and scenario actually exercised, with unavailable checks explicit.
