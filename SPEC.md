# Spooky product contract

Spooky is a native, GPU-rendered dropdown terminal with persistent project workspaces. It uses Rust crates rather than reimplementing a terminal parser, pseudoterminal layer or graphics stack.

## Platforms

- macOS is the initial development and runtime validation platform.
- Linux is a target for the same server and frontend. X11 and Wayland integration must be described separately; a successful build is not desktop verification.
- Windows is deferred. Its eventual implementation must be native, without requiring WSL. Do not claim current Windows support.
- tmux is not required. Users may run it inside a shell, but Spooky manages its own workspaces.

## Session model

A workspace has a display name, a server-side working directory and ordered tabs. Each tab has a binary split layout whose leaves identify terminal panes. A pane owns a shell, a PTY and terminal state.

The server is the authority. Closing/hiding the GUI disconnects or hides the client without terminating the shells. Reconnecting restores the existing terminal state and workspace tree. Server termination or operating-system restart does not preserve running processes, but the server saves the workspace tree (names, directories, tabs, splits and divider positions) and the last verified directory of each shell, then rebuilds it with new shells at its next start. A missing or inaccessible pane directory falls back to the restored workspace directory and is logged; checkpoints after an abrupt failure may lag behind the last change. A damaged saved file is set aside, never half restored. Stopping a server that has running shells needs an explicit force. The server keeps an operational log and reports its status without a GUI.

Creating workspaces, tabs and splits starts real shells. When a shell exits normally, its pane closes; the last pane closes its tab, but the workspace remains even if it has no tabs. A new tab can be created in an empty workspace. An agent covering that shell remains open. A shell terminated by a signal or whose exit status cannot be read retains its viewport. Explicit closure of an unlocked pane or tab needs no confirmation and leaves an empty workspace when it removes the last tab. Explicit workspace closure remains destructive and requires frontend confirmation.

New tabs and splits use the selected pane's verified current directory on that pane's server; creating a tab in an empty workspace uses its starting directory. An agent pane supplies its starting directory for a new tab, but cannot be split. When creation from a pane fails, the GUI shows the error and may offer an explicit retry in that original workspace's starting directory; it must never guess or retry automatically. A disconnected or changed target must not receive a stale retry. Ordinary new-agent creation uses the workspace's directory; the explicit “new agent here” action uses the selected pane's directory and refuses when it cannot be determined.

The header's folder control identifies the selected shell's recently verified directory on its server, including SSH workspaces; foreground programs, nested SSH, unreadable paths and stale or failed checks show the directory as unknown. The agent pane instead identifies its starting directory, not a live directory. Changing panes, targets or connection state discards the previous observation. Directory checks and their failures must not block input, display updates or the GUI.

The directory menu displays the complete escaped path and copies the exact observed path. A local directory can be opened in the local file manager; an SSH directory identifies the connected target and cannot be opened locally. Actions are unavailable for unknown, stale or changed observations.

## Frontend

- Polished dark interface inspired by Ghostty, without a WebView.
- Workspace sidebar, tabs that can be renamed and closed, visible split panes and unambiguous keyboard focus. Application shortcuts drive workspaces, tabs and panes through the same actions as the buttons; plain Ctrl combinations remain terminal input.
- Every shortcut action is reachable with the mouse: split, new tab and close buttons, a close button on the selected tab, and right-click menus for workspaces, tabs and panes. A program that tracks the mouse keeps the right-click. Tabs and panes can be locked individually against closure; locks start off, persist with the workspace, and prevent indirect closure through their parent. Explicit shell exit is not intercepted.
- Split dividers are a single line that can be dragged and reset to even halves; the position belongs to the server and is shared by every client. Tabs show the title their terminal sets until renamed.
- Tabs show unseen shell output, completed agent turns and newly pending agent questions or approvals with distinct markers. Viewing the relevant panes acknowledges their markers; pending questions already seen do not re-alert merely because the user switches away. A resize or an initial snapshot is not new output. The server reports activity over both local and SSH transports; acknowledgment belongs to each client.
- A pane in a split can be temporarily enlarged and restored using a shortcut or mouse control, without changing the server's layout. Other panes keep running. Selecting another tab, workspace or pane, or removing the enlarged pane, restores the normal layout; the active pane's PTY follows the visible size.
- Scrollback search (highlighted matches, next/previous with wrap-around, closable with Escape), a scrollback position indicator, and a configurable scrollback size.
- Links: Cmd/Ctrl+click opens OSC 8 and plain `http(s)` URLs, only for `http`, `https` and `mailto`, with the real target visible before opening. OSC 52 clipboard writes are honoured and announced, can be turned off, and clipboard reads are never answered. Output never causes a command to run.
- The machine row owns workspace creation and disclosure; neither its name nor its arrow opens SSH. Each workspace exposes rename and confirmed closure through right-click and an explicit actions button.
- Configurable global dropdown shortcut where supported. Its effect follows the window's real state: it hides a focused window, or a visible unfocused window set to stay on top, and brings a hidden, minimized or covered one forward, anchored to the top of its display and focused. If minimized state is unavailable, focus decides instead. On macOS, the display containing the pointer is selected at reveal time. While the shortcut is registered, closing the window hides it and the macOS Dock icon is optional; without a shortcut, closing quits so the window cannot be stranded. Unsupported compositor features must be surfaced.
- The dropdown's size is configurable as a share of the usable display (default full width, half height) or in pixels. What the window does when another application takes the focus is configurable: nothing (default), hide, or stay on top. Hiding requires a working global shortcut and does not trigger while one of Spooky's own file dialogs has the focus.
- A settings page, reached from the keyboard and the mouse, exposes window size, focus-loss behaviour, Dock icon, global shortcut (recorded from a key press, never leaving the application without a working shortcut), font size, cursor blinking, OSC 52 writes and built-in and user-provided appearance palettes. Changes apply immediately, are saved to `config.toml`, and a refused or unsaved change is explained. Invalid theme files are skipped and reported in Settings. Appearance is desktop-local and does not change the session protocol or CLI.
- On macOS, background opacity is configurable from 20% to 100% and applies a behind-window blur to the app, without fading text or explicitly colored terminal cells. At 100%, when the system requests Reduce Transparency, or if the native material is unavailable, the background is solid. Other platforms retain their solid background.
- A command palette lists every application command by name, filters as the user types and runs the chosen one. Keyboard shortcuts and the palette run the same commands, each with a stable id, and a command does exactly what the matching button does.
- GPU-rendered terminal text, ANSI colors, cursor (shape and blinking), wide/combining Unicode and terminal application input modes, including mouse and focus reporting.
- Grid resizing updates both terminal state and the PTY. Clipboard paste respects bracketed-paste mode. Text fields must never leak input into shells.
- Copy selection and navigate scrollback. Clear the terminal viewport and its scrollback without executing input or disturbing alternate-screen applications, moving the current input line to the top while preserving its text and cursor position; select and copy all retained terminal text locally or over SSH, refusing oversized exports without truncation.
- Packaging produces a signed application bundle with an icon on macOS (ad-hoc by default, Developer ID and notarization when configured) and an archive on Linux.
- Network/process operations do not block the GUI thread.

## Agents

On macOS an agent pane that fills its tab is drawn by a bundled web page in a native child view (`docs/architecture.md`, *Agent web view*); acceptance: it shows only the pane on screen, hides while a native overlay is open, forwards only known shortcuts, posts only validated actions, loads and sends nothing over a network, and any other case uses the native view with the same behaviour.

The design is in `docs/agent.md`. A tab can hold an **agent pane** instead of a shell: a coding agent (oh-my-pi first, behind a backend seam meant for others such as Claude Code and Codex) run by the server in the workspace's directory, without a terminal. The server owns the process and a bounded conversation, so every client, local or over SSH, sees the same one and a closed or lost GUI does not stop it. The GUI draws the conversation and a composer; it holds no credentials and never talks to the agent directly. Text from an agent is untrusted data: shown, never executed, and a prompt is sent only by a user action. Closing the pane, tab or workspace ends the agent's process group; stopping the server ends it too and `stop` asks before doing so. Attach puts the chosen file on the pane's next message, shown as a chip with a picture of it when it is one, and names its path in that message. A message starting with `/` offers the commands the program lists, narrowed as it is typed; picking one only writes it, and sending it is still the user's action. A restarted server resumes saved agent tabs by their session (the conversation is reloaded; a running turn is lost) and never turns one into a shell. The conversation shows Markdown answers, folded tool calls and reasoning, and the agent's questions and approvals as dialogs that only a user action answers; the model and thinking level can be chosen from what the agent offers, with the model roles the agent's own configuration defines (default, plan...) shown and selectable, and the remaining usage of its providers (how much of each allowance is left and when it renews) shown on request. Typing `omp` in a terminal pane opens an agent tab instead of the terminal UI, through a script the server writes in its own private state directory (nothing is uploaded; it can be turned off in `server.toml`); with arguments the agent view supports (`--model`, `--thinking`, `--approval-mode`, `--resume`, `-c`, `--no-session`, `--auto-approve` and one sentence as the first prompt), they are passed on; with any other argument or subcommand, a non-terminal stdin or stdout, outside a Spooky pane or on any failure, the real `omp` runs unchanged. The shim covers bash, zsh and fish. Not yet specified or built: other backends, images and diffs beyond coloured edit output.

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
