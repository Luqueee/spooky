# Spooky product contract

Spooky is a native, GPU-rendered dropdown terminal with persistent project workspaces. It uses Rust crates rather than reimplementing a terminal parser, pseudoterminal layer or graphics stack.

## Platforms

- macOS is the initial development and runtime validation platform.
- Linux is a target for the same server and frontend. X11 and Wayland integration must be described separately; a successful build is not desktop verification.
- Windows is deferred. Its eventual implementation must be native, without requiring WSL. Do not claim current Windows support.
- tmux is not required. Users may run it inside a shell, but Spooky manages its own workspaces.

## Session model

A workspace has a display name, a server-side working directory and ordered tabs. Each tab has a binary split layout whose leaves identify terminal panes. A pane owns a shell, a PTY and terminal state.

The server is the authority. Closing/hiding the GUI disconnects or hides the client without terminating the shells. Reconnecting restores the existing terminal state and workspace tree. Server termination or operating-system restart does not preserve running processes, but the server saves the workspace tree (names, directories, tabs, splits and divider positions) and rebuilds it with new shells at its next start; a damaged saved file is set aside, never half restored. Stopping a server that has running shells needs an explicit force. The server keeps an operational log and reports its status without a GUI.

Creating workspaces, tabs and splits starts real shells. Explicit pane, tab and workspace closure is destructive and requires frontend confirmation that states what will really close (the last pane closes its tab; the last tab closes its workspace). Process exit remains visible rather than fabricating a live terminal.

## Frontend

- Polished dark interface inspired by Ghostty, without a WebView.
- Workspace sidebar, tabs that can be renamed and closed, visible split panes and unambiguous keyboard focus. Application shortcuts drive workspaces, tabs and panes through the same actions as the buttons; plain Ctrl combinations remain terminal input.
- Every shortcut action is reachable with the mouse: split, new tab and close buttons, a close button on the selected tab, quick actions on the pane under the pointer, and right-click menus for workspaces, tabs and panes. A program that tracks the mouse keeps the right-click. Closing keeps its confirmation.
- Split dividers are a single line that can be dragged and reset to even halves; the position belongs to the server and is shared by every client. Tabs show the title their terminal sets until renamed.
- Scrollback search (highlighted matches, next/previous with wrap-around, closable with Escape), a scrollback position indicator, and a configurable scrollback size.
- Links: Cmd/Ctrl+click opens OSC 8 and plain `http(s)` URLs, only for `http`, `https` and `mailto`, with the real target visible before opening. OSC 52 clipboard writes are honoured and announced, can be turned off, and clipboard reads are never answered. Output never causes a command to run.
- The machine row owns workspace creation and disclosure; neither its name nor its arrow opens SSH. Each workspace exposes rename and confirmed closure through right-click and an explicit actions button.
- Configurable global dropdown shortcut where supported. Its effect follows the window's real state: it hides a focused window and brings a hidden, minimized or covered one forward, anchored to the top of its display and focused. While the shortcut is registered, closing the window hides it and the macOS Dock icon is optional; without a shortcut, closing quits so the window cannot be stranded. Unsupported compositor features must be surfaced.
- The dropdown's size is configurable as a share of the usable display (default full width, half height) or in pixels. What the window does when another application takes the focus is configurable: nothing (default), hide, or stay on top. Hiding requires a working global shortcut and does not trigger while one of Spooky's own file dialogs has the focus.
- A settings page, reached from the keyboard and the mouse, exposes window size, focus-loss behaviour, Dock icon, global shortcut (recorded from a key press, never leaving the application without a working shortcut), font size, cursor blinking and OSC 52 writes. Changes apply immediately, are saved to `config.toml`, and a refused or unsaved change is explained. Invalid files are errors rather than silent defaults.
- GPU-rendered terminal text, ANSI colors, cursor (shape and blinking), wide/combining Unicode and terminal application input modes, including mouse and focus reporting.
- Grid resizing updates both terminal state and the PTY. Clipboard paste respects bracketed-paste mode. Text fields must never leak input into shells.
- Copy selection and scrollback navigation.
- Packaging produces a signed application bundle with an icon on macOS (ad-hoc by default, Developer ID and notarization when configured) and an archive on Linux.
- Network/process operations do not block the GUI thread.

## Mods (planned)

Not implemented; the design is in `docs/mods.md`. Mods are user-installed extensions (commands, status items, settings sections, panels, session events) built on the same command and contribution registry as Spooky's own features. A mod never blocks the GUI thread or stops a terminal session, runs only after the user enables it, and can do only what its granted capabilities allow; output never causes a mod to write to a pane unless a separate capability was granted. There is no marketplace, automatic download or automatic update.

## Remote module

Remote workspaces use SSH to a host with a compatible spooky-server installed. The server runs under the remote user, without privileged installation requirements. Connection loss must not terminate its shells.

Use the user's standard SSH authentication and host verification. No disabled host checks, automatic acceptance of changed keys, implicit agent forwarding, automatic remote installation or password storage. Local and remote target states must not mix, and queued input must not be replayed against a different target.

The native connection UI discovers devices through an already-authenticated local Tailscale client, with search, refresh, online state and optional SSH username. Selecting a device prepares a validated IP destination but does not connect automatically. No VPN embedding, token storage, account changes or remote installation is implied.

## Captures and attachments

Capture the focused terminal viewport as a real PNG. A text dump is not a screenshot. Accept files explicitly picked or dropped by the user, and transfer remote files using SFTP. Insert a shell-quoted destination path without executing it.

This does not mean every terminal program accepts image attachments. Agent-specific image delivery and desktop screen capture are separate capabilities and must not be claimed without their own implementation and permission flow.

## Acceptance evidence

Launch the real server with a PTY; observe command output, ANSI/Unicode state, resize, multiple panes/tabs, process exit and reconnect preserving environment/cwd. Stop and restart the server and observe the saved workspace tree return with new shells. Exercise SSH and transfer with real SSH/SFTP, not a mock command. Launch the native frontend and observe rendered terminal output and interaction, including divider dragging, search, link handling and clipboard writes. Document each platform and scenario actually exercised, with unavailable checks explicit.
