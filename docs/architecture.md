# Architecture

## Modules and dependency direction

- **spooky-core** defines the bounded versioned wire protocol, own-user local IPC paths, and local/SSH client transport. It has no GUI or terminal-renderer dependency.
- **spooky-server** owns workspace layout, shell processes, PTYs, terminal emulation and agent processes (coding agents such as oh-my-pi, run without a terminal). It depends on spooky-core, portable-pty and alacritty_terminal.
- **spooky-desktop** builds the spooky executable. It owns presentation, keyboard focus, clipboard, screenshots and window integration. It depends on spooky-core and iced with its wgpu renderer.

The GUI never owns the lifetime of a shell. The server never needs a display server or GPU.

## Source layout

The layout of `spooky-desktop` (see [mods](mods.md), phase 0):

```
src/
  main.rs            startup: the window settings and the iced application
  app/               the application: App owns the shared state and composes the features
    mod.rs           App, Message, and the dispatch of every message to the file that handles it
    dropdown.rs      showing, hiding and placing the window; the global shortcut
    connection.rs    events from the server connection, connecting and retargeting
    workspaces.rs    workspaces, tabs and panes: creating, selecting, renaming, closing, reconciling with the snapshot
    input.rs         keyboard, clipboard, selection and shortcuts
    grid.rs          fitting panes to their area; divider geometry
    features.rs      adapters that apply a feature's messages and outcomes to App
    subscription.rs  the subscriptions: server events, keys, window events, blink
    view.rs          the interface
  features/          one folder per feature, each with its own state, message type, update logic and view
    agent/           mod.rs (drafts per agent pane, the composer's messages), view.rs (the conversation and composer)
    settings/        mod.rs (state, changes, effects), page.rs (the view)
    window/          dropdown.rs (placement, focus-loss rule), hotkey.rs (the global shortcut)
    tailscale/       mod.rs (picker state, messages, the choice it makes), discovery.rs (the client), view.rs
    hyperlinks/      mod.rs (which links may be opened, the link under the pointer)
    splits/          mod.rs (the dragged ratio, throttled sends), divider.rs (the divider widget)
    search/          mod.rs (the query, what was asked and answered), view.rs (the strip)
    attachments/     mod.rs (file dialog, dropped files, quoted path insertion, pane capture)
    palette/         mod.rs (the query, the highlight, the command to run), view.rs (the list)
    connection/      mod.rs (target, generation, channel state, why a target is unreachable; the link's threads stay in link.rs)
  terminal/          canvas.rs (draws a pane), pointer.rs, input.rs (keys, paste, focus reports), copy.rs (selection, OSC 52 writes), links.rs (finding links in output)
  ui/                theme.rs (colours, styles), icons.rs (vector icons and icon buttons)
  platform/          kde.rs (KDE Wayland: portal shortcut, KWin window control), macos/{app.rs, keyboard.rs}: code that exists because of one operating system
  config.rs          the typed config.toml model, shared by every feature
  commands.rs        the command registry: ids, titles, the palette's list and its matching
  link.rs            the server connection: its two threads and the events they report to App
  shortcuts.rs, tabs.rs, layout.rs   pure helpers with their own tests; test_support.rs  test fixtures
```

`spooky-server` and `spooky-core` follow the same rules:

```
spooky-server/src/
  main.rs      module list and the entry point (Unix only)
  cli.rs       serve, bridge, status and stop: argument dispatch and the control commands
  daemon.rs    the socket: single-instance lock, accept loop, one request per line, the save thread
  server.rs    Server: the shared state, request execution, save and restore, shutdown
  state.rs     State (workspaces, panes, identities), layout edits, and the validation of names, sizes and directories
  pane/        mod.rs (a pane: PTY, emulator, listener, snapshot), render.rs (rows and the row cache), shell.rs (the shell command)
  shim.rs      the omp shim: the script and zsh startup file written under the state directory, and the variables and PATH given to shells
  agent/       mod.rs (the Backend trait, normalized Event, the bounded Transcript, the child process and its reader), omp.rs (the oh-my-pi backend); see agent.md
  settings.rs  server.toml; persist.rs (saved workspaces); search.rs (scrollback matches); log.rs
spooky-core/src/
  lib.rs       the wire protocol
  paths.rs     the own-user socket and state directories
  client/      mod.rs (Client and Target), local.rs (Unix socket), ssh.rs (SSH child and its helpers),
               upload.rs (SFTP transfers), framing.rs (bounded JSON lines), errors.rs (Cancelled, Unrecoverable)
```

Rules: a feature is a folder; features do not import one another (they meet in `App`, which composes them, and talk through messages and small traits `App` implements, such as `settings::Shortcuts`); the shared leaf modules (`config`, `ui`, `platform`) depend on no feature; tests live beside the code they test. A feature reports what the rest of the application must do as data (`settings::Effect`) instead of reaching into it, which is also what lets its logic be tested without a display. The same rules will apply to mods.

## Agent web view

On macOS `app/web.rs` drives one `WKWebView` (`platform/macos/webview.rs`, through `wry`) that is a child of the main window and shows the agent pane that fills its tab. Responsibilities: the native layout reserves the pane's area with a measuring canvas that reports its rectangle; the application places, shows and hides the view, and pushes `features/agent/web::state` (the server's `Agent` as JSON) when it changed. The page posts messages through a bounded channel; `features/agent/web::event` parses them into a closed set, bounds them, and maps them to the messages the native view sends, so the server and `omp` are reached only through the usual requests. A forwarded chord becomes a command only if `shortcuts::shortcut` knows it. The view is hidden while a native overlay is open, since a child view draws above iced; splits, other platforms and a failed start keep the native `features/agent/view`. The page (`web/agent`) is built to one HTML file, committed, with a Content-Security-Policy that allows no network, and navigation and new windows are refused. `platform/webview.rs` is the only place that knows whether the view exists. Dependency direction: `app` → `features/agent/web` and `platform/webview`; the page depends on nothing in Rust but the JSON shape of `spooky_core::Agent` and the action set.

## Transport

Local requests use a private Unix-domain socket in an owner-only directory. A client can start the sibling spooky-server executable, or an explicit SPOOKY_SERVER path. SPOOKY_SOCKET supports isolated environments and tests.

Remote requests use a persistent SSH child invoking spooky-server bridge. Bridge relays one bounded JSON-line request and response at a time to the remote user's local session server. SSH stderr is diagnostic; stdout is protocol only. The bridge/client may exit while the actual session server keeps running.

The two transports implement the same client interface. This is the seam for the optional remote module. Extensibility is a separate, planned design: see [mods](mods.md). Uploads use SFTP separately from terminal input. Standard OpenSSH configuration and known-host verification remain authoritative.

`spooky-desktop::tailscale` performs read-only discovery with the installed CLI (`TAILSCALE_BE_CLI=1` for macOS app binaries). Tokio runs the child off the GUI thread, bounds stdout/stderr and applies a ten-second deadline with kill-on-drop. Discovery projects only typed IP addresses into SSH destinations; device labels are display-only, self is excluded, and offline peers remain visible but disabled. The native picker prepares a destination; only the existing Connect action changes transport. It does not install, authenticate, reconfigure Tailscale or bypass OpenSSH host checks.

## State and rendering

The server parses PTY output through alacritty_terminal and produces viewport rows of styled runs with resolved colors, cursor and input modes. This permits a reconnecting client to recover state without replaying an unbounded output log. Workspace layout and terminal grids remain server-owned.

Snapshots carry a server-scoped revision. Since protocol v3 a snapshot returns a complete workspace tree but only the panes changed since the requested revision, and within each pane only the rows that changed. `since: None` requests a full snapshot; clients reset the revision on reconnect or target change. The desktop merges updates with `Pane::apply_update` and removes panes absent from the authoritative tree. Layout changes send every remaining pane in full, including newly inserted panes whose initial output preceded insertion. A changed grid size also sends every row.

A viewport row is a `Line` of `Run`s: adjacent cells sharing one style, with blank cells on the default background left implicit. A run is one single-column cell per scalar value; a wide cell or one with combining characters is a run of its own (`Run::width`). A row that became blank is sent as an explicit empty line. Each server pane keeps a `RowCache` of its last rendered rows and the revision at which each changed. A snapshot renders under the terminal lock, reads the pane revision there (content and its revision bump are committed together under that lock), updates the cache, and returns the rows newer than the client's `since`. Clients at different revisions therefore each receive what they lack; a stale client can receive a row twice, never miss one. On the wire a mostly blank 300×80 screen is about 2 KB and one echoed keystroke about 0.5 KB, instead of the former 3 MB for either.

An up-to-date snapshot request waits on a condition variable until PTY output, exit, resize, scroll or layout changes advance the revision. A one-second idle heartbeat bounds the wait; it adds no delay to active updates. No terminal or workspace lock is held while waiting. The desktop uses separate command and snapshot connections (and separate SSH bridges remotely), so a waiting snapshot or its serialization cannot hold up keyboard input. Ordered deltas reach iced through a bounded asynchronous channel with backpressure; native global-shortcut callbacks share that wake-up path.

The link reports each connection's state to the frontend as `Event::Connection` (commands and display separately). These transitions use blocking sends so a full event queue cannot leave a stale "connected" indicator; the frontend shows connected only while both are up.

Connection attempts are cancellable. `Client::connect_cancellable` polls a caller predicate while waiting for the SSH handshake or local server start; on cancellation it kills the SSH child and returns `Cancelled`. `Link::retarget` queues the target change and then publishes the newest generation in an atomic, so an attempt for an older generation on either thread is abandoned instead of waited out, and the user can leave a hung host immediately. Failures are typed: `Unrecoverable` (rejected login, host-key failure, unknown host, missing `spooky-server`, protocol mismatch, missing `ssh`) stops automatic retries until the next target request; everything else retries with a doubling backoff from 2 s capped at 30 s. The command thread reports `Event::Unreachable` with a blocking send, since it is state, and the frontend shows the reason with Retry / Use this computer rather than an indefinite waiting pane. Dropping an SSH transport kills the child and waits at most 500 ms for its helper threads, so a pipe held open by a leftover process cannot stall the caller.

Both endpoints must negotiate protocol v11. There is no v1 to v10 compatibility mode and no automatic server restart: replacing an existing server terminates its shells, so the user must finish those sessions before upgrading the running daemon (`spooky-server stop` exists from v6 on; older servers are ended by process).

Protocol v4 adds each pane's mouse state, taken from the emulator's mode flags: tracking (`Off`, `Clicks`, `Drag`, `Motion`; the most detailed enabled mode wins), encoding (`Normal`, `Utf8`, `Sgr`) and `alternate_scroll` (alternate screen with DECSET 1007 on). It also adds `Cursor::shape` and `Run::strikethrough`. The server runs the emulator with a steady-bar default cursor so shells that never send DECSCUSR look as before, and applies the dim attribute by scaling the foreground to two thirds (the foreground is dimmed before reverse video swaps it).

Protocol v5 adds `RenameTab` and `CloseTab`. Tab titles and membership are server state like workspace names, so every client sees a rename and a closure; `CloseTab` removes the tab and all its panes in one step under the state lock, terminates the shells after the lock is released, and removes the workspace with its last tab (the same rule `ClosePane` already applied to a lone pane).

Protocol v6 moves more terminal state to the server and adds the operations that need it. Layouts carry a split `id` and a `ratio` (the first side's share in thousandths, kept within `MIN_SPLIT_SHARE`), and `SetSplitRatio` moves a divider, so divider positions are shared by every client and saved with the layout. Tabs carry `renamed`; an unnamed tab is labelled with its terminal's title (`Pane::title`, cleaned and bounded by the server). `Run::link` carries an OSC 8 target, bounded and free of control characters. `Cursor::blinking` and `Pane::focus_reporting` expose DECSCUSR blinking and DECSET 1004. `Pane::history` and `scrolled` describe the scrollback position. `Find` searches a pane's whole scrollback with the terminal lock held for at most `SEARCH_LINES` lines; the server remembers the query and the last match per pane, answers `Found` or `NotFound`, scrolls the viewport to the match, and reports the matches inside the viewport as `Pane::highlights`, recomputed from the visible rows on every snapshot so scrolling keeps them right. OSC 52 writes are kept per pane (latest only, at most `MAX_CLIPBOARD_BYTES`); the pane advertises a counter, and a client that sees it grow fetches the text with `Clipboard`, so each client acts on a write once and a large payload is never repeated in snapshots. Reading the clipboard stays refused by the emulator configuration. `Status` and `Shutdown` serve `spooky-server status` and `stop`.

Translating pointer events into bytes belongs to the frontend (`pointer`, used by the terminal canvas). While tracking is on, the pane is not exited and Shift is not held, presses, releases, motion and wheel notches become reports sent as ordinary pane input, and the pointer no longer selects text or scrolls history; Shift restores both. iced mouse events carry no modifiers, so the canvas follows them from keyboard events. Motion is reported only when the pointer enters another cell, only with a button held in `Drag` mode, and a drag that leaves the pane keeps reporting at the nearest edge cell until release. The release of a reported press is still reported if Shift is pressed in between. Legacy encodings drop positions they cannot express rather than clamping them. A wheel event forwards at most 30 notches, and with tracking off on the alternate screen the wheel becomes arrow keys honoring application cursor mode.

The frontend draws each row run by run through iced's GPU-backed canvas/text infrastructure: one background rectangle per non-default run, a selection rectangle per row, and one glyph per non-blank cell so the grid stays aligned when fonts fall back. iced already incorporates text layout/rendering crates; adding a second independent renderer is not required merely to name cosmic-text or glyphon as direct dependencies.

`spooky-desktop::view` composes a compact unified header, flat workspace navigation, floating action menu and confirmation overlays; `chrome` holds shared visual tokens and vector icons. Pane titles are omitted to preserve terminal space and panes have no border of their own; exited state is shown in the footer. The default terminal background is shared through spooky-core so server-resolved cells and frontend padding match. Indexed and truecolor backgrounds remain distinct from that default.

Machine disclosure, sidebar visibility and the SSH editor have independent UI state. The machine name and chevron toggle only its workspace list, preserving the active pane and transport; the chevron reflects that state. A separate + button on the same machine row creates a workspace on that target without toggling the disclosure. Creating a workspace or switching targets expands the list. Only Connect machine opens the SSH editor. Escape or hiding the sidebar dismisses that editor rather than leaving terminal input disabled by an invisible field.

Workspace context actions retain the clicked workspace ID independently of active selection. Rename and close dialogs use that ID, not whichever terminal is active; snapshot reconciliation and target changes clear stale action state. Destructive confirmation displays the target name. Modal input is isolated from the PTY, and action menus dismiss on Escape or outside clicks.

Workspace, tab and pane context menus share one overlay, `App::context_menu: Option<(ContextTarget, Point)>`, so Escape, outside clicks and the keyboard guards treat them alike, and `reconcile` closes the menu when its target leaves the snapshot (`context_target_exists`). Right-clicks reach it from two places: a transparent `RightClick` canvas stacked over a workspace row or tab, which publishes only on a right press, and the terminal canvas, which turns a right press into `ContextMenu` only when the program is not tracking the mouse (Shift takes it back). Opening a pane menu selects that pane, so the menu's items act on the pane that was clicked. A pane also has hover quick actions (`pane_actions`): `mouse_area` enter and exit events, which carry the pane id so that leaving one pane cannot clear the hover of the next, drive `hover_pane`, and the overlay occupies a permanent slot in the pane's `stack` so hovering never rebuilds the terminal canvas under it. Those buttons, the menus and the header buttons all send targeted messages (`SplitPane`, `AskClosePaneOf`, `AskCloseTab`) and closing stays behind the confirmation. A tab's double click is deliberately not bound: on macOS a double click inside the unified titlebar area zooms the window.

`spooky-desktop::shortcuts` is the table of application chords: a pure function from key, physical key and modifiers to a `Shortcut`, which `App::run_shortcut` turns into the message the corresponding button sends, so a shortcut can never do something the UI cannot. It is consulted only for ignored key events while the terminal owns the keyboard (no dialog, menu or text field), except Quit. macOS chords use Cmd; other platforms use Ctrl+Shift, so plain Ctrl stays terminal input. Digits and brackets are matched by physical key so they are layout-independent, and Cmd+Shift+digit is left to the system screenshot shortcuts. The close confirmation derives its wording from the layout: the server removes a tab with its last pane and a workspace with its last tab, so the modal names what will really close.

The dropdown window state is tracked from real events, not toggled blindly. `hidden` records that Spooky itself hid the window; `focused` follows the window's focus events. The global shortcut hides a window that is shown and focused and otherwise reveals it (un-hides, un-minimizes, focuses), so a window the user minimized or buried is brought forward by the next press. Revealing measures the display (`window::monitor_size`) and window, then `dropdown_origin` centres it against the top edge. The close button hides only while a global shortcut is registered; without one it quits, because a hidden window with no way back would strand the application. On macOS `macos_app::set_dock_icon` sets the process's activation policy at window open and when the setting changes (accessory unless `dock_icon = true`, and always regular without a global shortcut) so the shortcut is the way back. `on_blur` is applied from the same real focus events: `hide` takes the shortcut's `hide` path, but only while the window is shown, the shortcut is registered and no native dialog of Spooky's own is open (`hides_on_blur`, because such a dialog takes the focus without the user having clicked away); `top` is the window level (`level_for`). Following the display under the cursor is not implemented: iced exposes neither the cursor position nor the native window handle without unsafe code.

Placement is part of window creation, not a correction after it: the window is created with `Position::SpecificWith(initial_position)`, the same `dropdown_origin` function a later reveal uses, and starts hidden (`visible: false`) whenever the global shortcut is registered. The `Opened` handler then makes `macos_app::show_windows_instantly` set `NSWindowAnimationBehavior::None` on the application's windows (safe objc2 calls, no window handle needed) before the first `Present`. Without both, macOS shows a window created at the placeholder position, scales it up a couple of percent as it appears, and Spooky then moves it to the centre, which is visible as the layout shifting.

`spooky-desktop::config` owns `config.toml`: the typed `Config` (unknown keys and out-of-range values are errors), loading, and `save`, which writes the whole file through a temporary file and a rename. The settings page (`settings.rs`, drawn by `App::settings_page` in place of the terminal while `App::settings` is set) only emits `Change` messages. `App::change_setting` builds the next `Config`, and `apply_config` validates and saves it and then updates what each changed field drives: a font-size change refits every pane's grid (`refit_grids`, the same `fit` the bounds message uses, so the server learns the new size); a size change marks `size_pending` and reveals, so `Present` resizes the window against the display's usable area (`usable_area`: `NSScreen.visibleFrame` of the window's display on macOS, so full height does not hang below the menu bar; the whole display elsewhere); `on_blur` sets the window level; the Dock switch sets the activation policy. A failed save leaves the change applied and says so in the page header. The size is applied only when the display is first known and when a size setting changes, never on a plain reveal, so a window the user resized keeps its size. The shortcut recorder consumes key presses while `recording`: `shortcuts::hotkey_text` turns one into a chord the `global-hotkey` parser accepts (a function key alone, otherwise Ctrl, Alt or Cmd is required so no typing key is taken from other applications), and `register_shortcut` registers the new chord before unregistering the old one, so a refusal keeps the working shortcut. `App` owns the `GlobalHotKeyManager` for this reason.

Uploads (`spooky_core::client::upload`) never run on the command thread. The link starts a transfer on a dedicated thread (one at a time) whose cancellation predicate is the same newest-generation atomic as connections, and reports the result as an event. For SSH one round trip sweeps upload directories older than seven days, prints `$HOME` and creates a private `~/.spooky-upload.XXXXXX`; the reported directory is validated (absolute, inside the reported home, alphanumeric suffix) before any path derived from it is used, `sftp` copies the file in batch mode, and a failed or cancelled transfer removes the directory (a shell-quoted `rm -rf` of the validated path, with its own short timeout). The transfer timeout is 30 s plus the file size at 256 KiB/s, capped at 30 minutes, so a large file on a slow link is not cut off by the limit meant for a stalled one.

A single square-cornered container paints each pane's background. The terminal area sits flush against the surrounding chrome, with no outer margin; only the terminal content has inner padding. The gap between the two sides of a split is a `divider::Divider` canvas that fills it with the same background and draws one hairline, so the panes read as one surface.

Keyboard events retain iced's captured/ignored status and physical key identity. Only ignored input reaches the terminal; Escape can dismiss an overlay even when a text field captured it, without forwarding that event to the shell. Space preserves its OS-provided text, including a composed character, while Control+Space emits NUL. On macOS, `macos_keyboard` resolves Option dead keys through AppKit's layout-aware `charactersByApplyingModifiers` API using safe `objc2` crate interfaces. It reuses winit's physical-key mapping, commits the spacing glyph immediately and discards that native composition session so the next key is not duplicated or accented. Other text keeps OS composition; no Spanish-specific character table is maintained. Option-generated text has no Meta prefix; Control+Option chords and printable Alt input on other platforms retain the terminal escape prefix.

Clipboard chords are resolved by `clipboard_action`: Cmd+C/V and Ctrl+Shift+C/V, never plain Ctrl+C/V. Bracketed paste removes ESC and ETX from the pasted text (`paste_bytes`) so clipboard contents, which are untrusted, cannot terminate the bracket and run the remainder as typed input. Selection text drops the blank cells that pad each row.

A divider drag publishes the pointer's window position. `App::split_geometry` takes the split's area as the union of its panes' reported bounds grown by the pane padding, and `divider::ratio_at` turns the position into a ratio (the divider's own thickness is not shared). The dragged ratio is drawn immediately and sent to the server at most every 40 ms and once more at release; it is dropped when the server shows the same ratio or has had a second to. Sides are sized with `FillPortion` from the layout's ratio, so the server's value and the drawn one use the same arithmetic.

`links` finds the link under a cell: an OSC 8 target (adjacent runs with one target are one link) or a plain `http(s)://` URL (bounded by blank columns and quoting characters, without sentence punctuation or brackets the URL does not open). It is consulted only while the link modifier is held, and `links::open` runs the system opener for the `http`, `https` and `mailto` schemes only; the target is shown in the footer first because an OSC 8 target need not match its text. OSC 52 text arrives as an event and is written to the clipboard only when `osc52` is `write`; the per-pane write counter is baselined the first time a pane is seen so a reconnecting client does not replay old writes. A blinking cursor drives a 530 ms subscription only while the window and pane have the keyboard focus, and typing makes it solid. `App::sync_focus_report` compares the pane that should hold the keyboard focus (window focused, terminal focused, pane selected) with the last one told, and sends focus-out and focus-in only to panes whose program asked for them; it runs after every message.

Search keeps its state in the frontend (`Search`: query, the pane it ran in, the last answer); the server owns the match position. The search chords are accepted while the search field has the keyboard, but iced's text field inserts the letter of Cmd+F and Cmd+G, so an edit that adds exactly that letter while the chord modifier is held is discarded (`is_chord_echo`).

## Process and failure behavior

PTY output, writes and child reaping must not block the shared workspace state. Input queues and wire frames are bounded. Closing the GUI is a detach. Explicit close requests remove the process and corresponding layout. Exited shells retain a readable viewport until explicitly closed.

The desktop explicitly requests the local user's home directory when creating local workspaces, independently of Finder's or the server's working directory. Remote requests leave the directory unset so the remote server resolves its own context. Tabs and splits reuse the workspace's stored starting directory; existing shells are not relocated.

The server builds each shell with `shell_command`. macOS uses `portable-pty`'s default program, a login shell (`argv[0]` prefixed with `-`) resolved from `$SHELL` or the password database, because a Finder launch gives the server a minimal launchd environment without the user's profile or locale; it supplies `LANG=en_US.UTF-8` only when `LANG` and `LC_ALL` are both unset. Other platforms keep the plain `$SHELL` invocation until verified on real Linux desktops. `Request::Input` returns the pane's viewport to live output before the request completes, because the display offset belongs to the server-side terminal state shared by every client.

A dead remote SSH connection does not imply a dead shell. Failed commands and uploads are reported; input and destructive actions must not be retried blindly across disconnection or target changes.

The server writes `server.log` and `workspaces.json` in `spooky_core::paths::state_dir` (the socket's own directory under SPOOKY_SOCKET, so an isolated instance never touches real state). The log (`log` module) is private, capped at 1 MiB with one previous generation, and control characters are replaced so text from a client or terminal cannot forge lines. The layout is saved by a thread that waits for a change mark, lets a burst settle for 300 ms and writes atomically with mode 0600 (`persist`); `Shutdown` saves synchronously, terminates every shell, removes the socket and exits after answering. At start the file is read before the socket is bound: identities must be unique and issued, ratios are clamped, and a file that fails any check is moved aside as `.corrupt` and reported, never half restored. Restored panes are new shells in the workspace's directory (the saved one if it still exists, else home) at the saved window-sized grid, under the saved identities, and identity allocation continues past every saved identity. New panes outside a split start at the size of the largest existing pane. The scrollback limit comes from `spooky/server.toml` on the machine the server runs on. `spooky-server stop` refuses while shells run unless forced, because stopping ends them.

## Platform decisions

macOS and Linux use Unix sockets and native PTYs. Window shortcuts and placement are frontend concerns, separate from sessions. Wayland compositors may restrict global shortcuts and window placement. On KDE Wayland `platform/kde.rs` supplies both: the shortcut through the GlobalShortcuts portal (`Hotkeys::Portal`; KDE owns the binding, so `rebind` is refused) and window placement, size, hide (minimize), raise and focus through a one-shot KWin script loaded over D-Bus on a worker thread, matched by process id and `dev.spooky.terminal` window class. Sizes given as a share of the display are resolved by KWin, because winit knows no monitor before the window is mapped; the first reveal waits 300 ms for the window to exist.

Windows is deferred rather than hidden behind WSL. Future Windows work includes local IPC, process-lifetime verification with ConPTY and native window integration. portable-pty is retained to avoid coupling the session model to Unix PTY syscalls.

## Security

Use safe crate interfaces; first-party unsafe is forbidden. Reject foreign/symlinked IPC paths. SSH must verify host identity. Remote output and titles are untrusted display data. File transfer and shell input are separate operations; a path is quoted and never automatically executed. No global network listener is required for local sessions.
