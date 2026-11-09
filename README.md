# Spooky

A native Rust dropdown terminal with persistent workspaces, tabs and split panes. The interface uses iced's **wgpu** backend, not a WebView. Terminal emulation comes from **alacritty_terminal** and shell processes from **portable-pty**.

## Platforms

macOS is the initial runtime target. Linux is included in CI and packaging, but must be validated on real X11/Wayland desktops before claiming equivalent integration. Windows is deferred; future support must not require WSL.

## Run from source

Install Rust through rustup. The repository pins Rust 1.99.1 and declares Rust 1.98 as its minimum supported version.

```sh
cargo build --workspace
cargo run -p spooky-desktop --bin spooky
```

Build both binaries: the desktop client starts its sibling `spooky-server` when needed. `SPOOKY_SERVER` can override that executable; `SPOOKY_SOCKET` selects an isolated socket for development.

Linux build dependencies on Ubuntu:

```sh
sudo apt-get install libx11-dev libxi-dev libxrandr-dev libxcursor-dev libxkbcommon-dev libwayland-dev libgtk-3-dev pkg-config
```

## Sessions and controls

- **Ctrl+Shift+Space:** show/hide the dropdown window where global shortcuts are available.
- Use **Spaces +** to create a workspace and the sidebar rows to switch workspaces. The compact tab bar provides new tabs, **Attach** and pane capture; hover an icon for its label. The button beside the Spooky brand toggles the sidebar.
- Click a terminal pane to focus it; drag to select text and scroll to navigate history.
- **Ctrl+Shift+C** / **Cmd+Shift+C:** copy the selected terminal text.
- **Ctrl+Shift+V** / **Cmd+V:** paste. Applications requesting bracketed paste receive the appropriate delimiters.
- On macOS, **Option** follows the active keyboard layout without a terminal Meta prefix. Option dead keys insert their spacing character immediately (for example, **Option+ñ → `~`** on Spanish keyboards), without requiring Space or composing the next letter. Control+Option chords retain terminal encoding; ordinary accent composition remains unchanged.
- The **…** popover contains **Split columns / Split rows**, workspace renaming and **Close pane / Close workspace**. Closing asks for confirmation in a modal before terminating shells; Escape dismisses menus and modals without sending the key to the terminal.
- Closing the GUI is a detach, not session termination. Relaunch to reconnect to the running server.
- New local workspaces start in your home directory, including when launched from Finder. Tabs and splits inherit their workspace's starting directory. Existing sessions keep their directory; use `cd ~` to return home. SSH workspaces use the remote server's directory, never the local home path.

The server does not survive a system restart. It is an independent multiplexer, not a tmux wrapper; tmux is optional software that can be run inside a pane.

On Wayland, compositor policies restrict global shortcuts and programmatic positioning. The application reports this rather than claiming support it cannot provide. Native window decorations remain enabled.

The interface follows the compact Herdr reference: a unified titlebar on macOS, small monospaced tabs, flat workspace rows and thin pane outlines. Panes have no title strips; the focused pane's exited state appears in the footer. The default terminal background remains the blue-gray `#22252E` sampled from the Ghostty reference, with darker sidebar and toolbar surfaces. Explicit ANSI/truecolor black remains black. The server resolves default terminal colors, so an already-running older server retains its palette until explicitly restarted.

## Remote workspaces

Build/install `spooky-server` on the remote Unix machine and put it on the remote login shell's PATH. Use the same protocol version on both sides. Click **This computer** or **Connect machine** in the sidebar to expand the SSH editor, enter a destination or existing OpenSSH host alias, then select **Connect**. **Local** returns to the local server.

The remote bridge starts/reconnects to the remote session server. Disconnecting SSH or closing the GUI does not close those shells. Standard OpenSSH configuration, identities, agent and host-key verification are used. Connections are noninteractive: authenticate and establish the host key through normal SSH first. Spooky does not disable verification, store passwords or automatically install remote executables.

The remote module shares the session protocol with local transport. There is no general third-party plugin loader in this implementation.

## Captures and files

**Capture** saves a real PNG of the focused terminal viewport through a native file dialog. It does not capture unrelated desktop windows.

**Attach**, or dropping a file, resolves a local path or transfers the file over SFTP for an SSH target. The resulting shell-quoted path is inserted into the terminal **without pressing Enter**. This is not a claim that arbitrary terminal programs support image attachments; the receiving program decides how to consume the path.

## Configuration

Create `$XDG_CONFIG_HOME/spooky/config.toml`, or `~/.config/spooky/config.toml` when XDG_CONFIG_HOME is unset:

```toml
font_size = 12.0
width = 1440.0
height = 900.0
shortcut = "Ctrl+Shift+Space"
```

Unknown fields and invalid values produce an explicit startup error. There is no silent fallback to a broken configuration.

## Packages and checks

```sh
./scripts/package.sh
```

Produces `dist/Spooky.app` on macOS or a Linux archive containing both binaries and a desktop entry. The macOS bundle is a local unsigned development distribution, not a notarized release.

The incremental-update transport uses **protocol v2**. Update both client and server, including remote installations. A running v1 server must be stopped explicitly after finishing its sessions; restarting it terminates those shells and loses its in-memory workspaces. Closing only the GUI does not stop it. Spooky rejects incompatible endpoints rather than silently restarting a server.

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
cargo test --locked --workspace --doc
RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --no-deps
cargo +1.98.0 check --locked --workspace --all-targets
```

The Rust lint/toolchain policy and `AGENTS.md` were adapted from the authorized `luqueee/uislop` reference. Product-specific dependencies and instructions were not copied blindly.

## Structure

- `crates/spooky-core`: versioned protocol, secure local IPC and SSH/SFTP client.
- `crates/spooky-server`: persistent PTYs, terminal emulation and workspace tree.
- `crates/spooky-desktop`: GPU rendering, focus/input, clipboard and native window/file integration.

See [SPEC.md](SPEC.md) for the product contract, [architecture](docs/architecture.md) for module boundaries, and [AGENTS.md](AGENTS.md) for contributor/AI rules.

## Validation

Verified on macOS arm64:

- Workspace build and release packaging; the unsigned `Spooky.app` launches a native window.
- `cargo fmt`, strict clippy, 8 automated tests, doc tests and warning-free first-party rustdoc.
- The complete workspace checks with both pinned Rust 1.99.1 and MSRV 1.98.0.
- Real PTY output, ANSI truecolor, wide Unicode, resize, splits/tabs, explicit closure and reconnect retaining a shell environment variable.
- Real SSH/SFTP against an isolated loopback sshd with a pinned host key: session detach/reconnect and a filename containing a space/apostrophe. Transferred bytes matched by SHA-256.
- The native frontend starts/reconnects to its server and resizes the visible terminal grid. Closing its window leaves the session running.
- Direct native screenshots confirm the compact chrome, titleless panes and square panel corners with no outer margin against the header/sidebar. Real window interaction verified the split popover, sidebar hide/show, shell input/output, resize/reflow and explicit black against the themed background.
- New local workspace creation and a subsequent split started in the user's home directory, confirmed with `pwd` in the native terminal; reconnecting preserved the earlier sessions.
- Native key presses verified Space between shell arguments and inside quoted text, Enter, Backspace, arrow-key editing/history, Tab completion and Ctrl+C. A regression test covers named Space events with Shift, Control, Alt and Command modifiers, both with and without text payloads.
- Typing in the rename field stayed isolated from the shell. One Escape dismissed the focused dialog; subsequent shell input worked without another click.
- Native Option+ñ inserted `~` immediately; subsequent Space, `n` and repeated Option+ñ produced `~ ~n ~~` without duplicates. A shell command received `~/mañana @` unchanged. Conversion regressions cover uncommitted base keys, composed text carried by Space and Option-generated text.
- Event-driven incremental snapshots reduced median internal key-to-canvas-geometry latency from 49.9 ms to 5.3 ms (maximum 66.5 ms to 7.2 ms) in native 20-key runs with eight panes. The new run used an isolated server with comparable viewport sizes and the same shell configuration; existing user sessions were untouched. These measurements exclude compositor/display presentation. Native input also completed a `stty -echo`/`read` interaction while the snapshot connection waited.
- A real protocol-v2 bridge received an eight-pane full snapshot followed by a one-pane PTY update; its exit left the isolated session server alive. The regression suite also checks that incremental merging retains unchanged panes, clears erased cells and removes closed panes.

**Still unverified:** end-to-end clipboard, PNG-save and global-shortcut interactions, plus real Linux desktops. CI has not been run on GitHub.

The dependency graph currently reports a future-Rust incompatibility in upstream `block 0.1.6`. First-party clippy and rustdoc pass without suppressing that diagnostic.
