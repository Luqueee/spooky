# Architecture

## Modules and dependency direction

- **spooky-core** defines the bounded versioned wire protocol, own-user local IPC paths, and local/SSH client transport. It has no GUI or terminal-renderer dependency.
- **spooky-server** owns workspace layout, shell processes, PTYs and terminal emulation. It depends on spooky-core, portable-pty and alacritty_terminal.
- **spooky-desktop** builds the spooky executable. It owns presentation, keyboard focus, clipboard, screenshots and window integration. It depends on spooky-core and iced with its wgpu renderer.

The GUI never owns the lifetime of a shell. The server never needs a display server or GPU.

## Transport

Local requests use a private Unix-domain socket in an owner-only directory. A client can start the sibling spooky-server executable, or an explicit SPOOKY_SERVER path. SPOOKY_SOCKET supports isolated environments and tests.

Remote requests use a persistent SSH child invoking spooky-server bridge. Bridge relays one bounded JSON-line request and response at a time to the remote user's local session server. SSH stderr is diagnostic; stdout is protocol only. The bridge/client may exit while the actual session server keeps running.

The two transports implement the same client interface. This is the seam for the optional remote module, not a general-purpose plugin marketplace. Uploads use SFTP separately from terminal input. Standard OpenSSH configuration and known-host verification remain authoritative.

## State and rendering

The server parses PTY output through alacritty_terminal and produces viewport cells with resolved colors, cursor and input modes. This permits a reconnecting client to recover state without replaying an unbounded output log. Workspace layout and terminal grids remain server-owned.

Snapshots carry a server-scoped revision. Protocol v2 returns a complete workspace tree but only pane grids changed since the requested revision. `since: None` requests a full snapshot; clients reset the revision on reconnect or target change. The desktop merges pane replacements and removes panes absent from the authoritative tree. Layout changes send every remaining pane, including newly inserted panes whose initial output preceded insertion.

An up-to-date snapshot request waits on a condition variable until PTY output, exit, resize, scroll or layout changes advance the revision. A one-second idle heartbeat bounds the wait; it adds no delay to active updates. No terminal or workspace lock is held while waiting. The desktop uses separate command and snapshot connections (and separate SSH bridges remotely), so a waiting snapshot or its serialization cannot hold up keyboard input. Ordered deltas reach iced through a bounded asynchronous channel with backpressure; native global-shortcut callbacks share that wake-up path.

Both endpoints must negotiate protocol v2. There is no v1 compatibility mode or automatic server restart: replacing an existing server terminates its shells, so the user must finish those sessions before upgrading the running daemon.

The frontend draws cells through iced's GPU-backed canvas/text infrastructure. iced already incorporates text layout/rendering crates; adding a second independent renderer is not required merely to name cosmic-text or glyphon as direct dependencies.

`spooky-desktop::view` composes a compact unified header, flat workspace navigation, floating action menu and confirmation overlays; `chrome` holds shared visual tokens and vector icons. Pane titles are omitted to preserve terminal space; pane borders remain neutral when focused and exited state is shown in the footer. The default terminal background is shared through spooky-core so server-resolved cells and frontend padding match. Indexed and truecolor backgrounds remain distinct from that default.

A single square-cornered container paints each pane's background and neutral border. The terminal area sits flush against the surrounding chrome, with no outer margin; only the terminal content has inner padding.

Keyboard events retain iced's captured/ignored status and physical key identity. Only ignored input reaches the terminal; Escape can dismiss an overlay even when a text field captured it, without forwarding that event to the shell. Space preserves its OS-provided text, including a composed character, while Control+Space emits NUL. On macOS, `macos_keyboard` resolves Option dead keys through AppKit's layout-aware `charactersByApplyingModifiers` API using safe `objc2` crate interfaces. It reuses winit's physical-key mapping, commits the spacing glyph immediately and discards that native composition session so the next key is not duplicated or accented. Other text keeps OS composition; no Spanish-specific character table is maintained. Option-generated text has no Meta prefix; Control+Option chords and printable Alt input on other platforms retain the terminal escape prefix.

## Process and failure behavior

PTY output, writes and child reaping must not block the shared workspace state. Input queues and wire frames are bounded. Closing the GUI is a detach. Explicit close requests remove the process and corresponding layout. Exited shells retain a readable viewport until explicitly closed.

The desktop explicitly requests the local user's home directory when creating local workspaces, independently of Finder's or the server's working directory. Remote requests leave the directory unset so the remote server resolves its own context. Tabs and splits reuse the workspace's stored starting directory; existing shells are not relocated.

A dead remote SSH connection does not imply a dead shell. Failed commands and uploads are reported; input and destructive actions must not be retried blindly across disconnection or target changes.

## Platform decisions

macOS and Linux use Unix sockets and native PTYs. Window shortcuts and placement are frontend concerns, separate from sessions. Wayland compositors may restrict global shortcuts and window placement.

Windows is deferred rather than hidden behind WSL. Future Windows work includes local IPC, process-lifetime verification with ConPTY and native window integration. portable-pty is retained to avoid coupling the session model to Unix PTY syscalls.

## Security

Use safe crate interfaces; first-party unsafe is forbidden. Reject foreign/symlinked IPC paths. SSH must verify host identity. Remote output and titles are untrusted display data. File transfer and shell input are separate operations; a path is quoted and never automatically executed. No global network listener is required for local sessions.
