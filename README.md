# Spooky

A native Rust dropdown terminal with persistent workspaces, tabs and split panes. The interface uses iced's **wgpu** backend, not a WebView. Terminal emulation comes from **alacritty_terminal** and shell processes from **portable-pty**.

## Platforms

macOS is the initial runtime target. Linux is supported on Debian (13, "trixie", is the tested release) with a `.deb` package; Windows is deferred and must not require WSL.

What works depends on the session, because a Wayland client can neither register a global key nor place, hide or raise its own window. Spooky detects the session once (`platform/desktop.rs`) and uses what it offers:

| Session | Global shortcut | Window placement, hide, animation | Verified |
|---|---|---|---|
| X11, any desktop | Spooky grabs the key itself | Spooky, through the window manager; slide and fade | Debian 13 under XWayland |
| KDE Plasma 6 on Wayland | Desktop portal; KDE owns the binding | KWin script; slide and fade | Debian 13, Plasma 6.3 |
| GNOME and other Wayland | Desktop portal where it exists (GNOME 48 and later) | None: an ordinary window; the shortcut only brings it forward | Simulated on KDE by naming the desktop `GNOME`; never run on GNOME |
| macOS | Spooky grabs the key itself | Spooky; slide and fade | CI compiles it; never run in this repository's Linux work |

The **Background opacity** setting (Settings → Window) works on Linux as well as macOS: the window is created transparent and the interface backgrounds are drawn with that opacity, so text stays solid. On Linux the window also asks for blur behind it through winit (the KDE blur protocol on Wayland, the KDE blur property on X11), which KDE's Blur effect honours; compositors that ignore the request show plain transparency. It is requested when the window is created, and a compositor with no alpha support shows a solid background. Checked on KDE Plasma 6.3 Wayland: text behind a 50 % window was readable without blur and not with it.

Where the shortcut or the window control is missing the settings page says so, closing the window quits instead of hiding it, and nothing is claimed that the desktop does not allow.

### Install on Debian

```sh
./scripts/package.sh                 # builds dist/spooky_<version>_<arch>.deb
sudo apt install ./dist/spooky_*.deb
```

The package installs `spooky` and `spooky-server` in `/usr/bin`, the desktop entry, the icon and AppStream metadata, and depends on the libraries the window system loads at run time (Vulkan, xkbcommon, Wayland and X11 client libraries) and on a monospace font. It requires glibc 2.39 or newer, the version of the machine it was built on (Debian 13 ships 2.41); build it on the oldest release it must run on.

Mods (extensions for commands, status items, settings and panels) are designed in `docs/mods.md` and not implemented yet.

A native interface for coding agents (a conversation view instead of their terminal UI, from the GUI, and on a remote machine's server) is designed in `docs/agent.md`. Phases 1 to 5 are implemented for oh-my-pi (`omp`) behind a backend trait meant for other agents: **New agent** (a tab's right-click menu or the command palette) opens a tab with a streamed Markdown conversation (code blocks with a copy button), folded tool cards and reasoning, question and approval dialogs, a model and thinking selector and a multiline composer, owned by the server like a shell. Typing `omp` in a pane (alone, or with `--model`, `--thinking`, `-c`, `--resume`, `--no-session`, `--auto-approve`, `--approval-mode` or one sentence as the first prompt) also shows the agent in that same pane, with the shell kept running underneath and shown again when the agent is closed (the shim; anything else, such as `omp commit`, runs the real `omp`). Agents are saved and resumed after a server restart. Other backends are not implemented.

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

- **Ctrl+Shift+Space** (configurable): show/hide the dropdown window where global shortcuts are available. The window opens as wide as the display and half as tall (configurable, see **Settings**), flush against the top edge, and takes keyboard focus. The shortcut hides a focused window; with **Stay on top**, it also hides the visible unfocused window. A hidden or minimized window, or a normal window behind another app, is brought forward instead. If the compositor cannot report whether the window is minimized, the shortcut uses focus instead. While the shortcut is registered the close button hides the window rather than quitting, and on macOS Spooky runs without a Dock icon (`dock_icon = true` keeps it). Without the shortcut (Wayland, or one that could not be registered) closing quits, so the window can never be stranded. **Cmd+Q** (macOS), **Ctrl+Shift+Q** or **Quit Spooky** in the **…** popover exits the GUI; sessions keep running.
- Opening and closing animate the same way on every platform where the dropdown works: the window stays where it is and its content slides down from, or up through, the top edge (`features/window/slide.rs`, drawn by the application, so nothing crosses onto a display above), while the window fades in or out (200 ms). The window is transparent for this, and is hidden once the slide out has ended. The fade is the window's own opacity, set per platform: `_NET_WM_WINDOW_OPACITY` on X11 (verified here under XWayland: the property ran from 0 to 1 and back around map and unmap, and a burst of presses ended consistent), KWin on KDE Wayland (see below) and `NSWindow.alphaValue` on macOS, which CI compiles but nobody has run. A press during a slide turns it around from where it was. Windows is deferred.
- On macOS, the dropdown opens on the display under the pointer. Moving between displays recalculates percentage dimensions using the destination's usable area; pixel dimensions stay fixed. The window is positioned before it is shown when changing displays, avoiding a flash on the previous display.
- **Settings** (**Cmd+,**, Ctrl+Shift+, on Linux, or **Settings…** in the **…** popover) opens a **Settings** tab after the terminals' tabs, whose page replaces the terminal while the tab is selected, with the options below, grouped as Window, Shortcut and Terminal. Every change applies immediately and is written to `config.toml`: window size (percent of the display or pixels, with *Full screen / Top half / Top third* presets), what the window does when another app is clicked (nothing, hide, or stay on top), the Dock icon, the global shortcut (press **Record**, then the new keys; the new shortcut is registered before the old one is released, so a refusal keeps the working one), the terminal font (any monospace family installed, with the width of its cells measured so the grid fits it) and its size, cursor blinking and OSC 52 writes. A change that cannot be applied or saved is explained in the page's header. Choosing a terminal tab or a workspace leaves the page without closing its tab; **Escape**, **Done** or the tab's **×** (or a middle click) closes the tab. Saving rewrites the whole file, so comments in it are not kept. The scrollback size belongs to the server and is not on the page.
- Drag the bottom edge of the window (a 5 px strip with a resize cursor) to change its height; the window stays anchored to the top of the display. The new height is saved in the unit `config.toml` already uses (a share of the display, or pixels), so the next launch opens that tall. Compositor behaviour can vary, particularly on KDE Wayland.
- Use the **+** beside **This computer** (or the current SSH host) to create a workspace on that machine and the sidebar rows to switch workspaces. The compact tab bar provides new tabs; hover an icon for its label. **Attach file** and **Capture pane as PNG** are in the command palette, and a dropped file is attached too. The button beside the Spooky brand toggles the sidebar.
- Click **This computer** (or the current SSH host) or its arrow to collapse/expand its workspace list without changing connections. Creating a workspace expands the list so the new workspace remains visible.
- Right-click a workspace or use its **…** button for **Rename workspace** and **Close workspace**, including inactive workspaces. Rename opens a focused field; closure confirms the workspace name before terminating its shells. Escape or clicking outside dismisses the action menu.
- Click a terminal pane to focus it; drag to select text and scroll to navigate history.
- **Cmd+C** (macOS) or **Ctrl+Shift+C:** copy the selected terminal text, without the blank padding that fills each row. Plain **Ctrl+C** stays the terminal interrupt.
- **Cmd+K** (Linux **Ctrl+Shift+K**) clears the viewport and its scrollback without running a shell command; cleared output cannot be recovered by scrolling or copying. The current input line moves to the top, retaining its text and cursor position. Clear is refused while an alternate-screen application is active.
- **Cmd+A**, then **Cmd+C**, selects and copies all retained terminal text; **Cmd+Shift+A** copies it directly. Linux: **Ctrl+Shift+A** and **Ctrl+Shift+Alt+A**. Copy runs on the session server, including over SSH, and writes to the local clipboard. Exports exceeding 1 MiB, 100,000 lines or 16 million cells fail visibly rather than truncating. Both endpoints must run the same protocol version.
- **Cmd+V** or **Ctrl+Shift+V:** paste. Applications requesting bracketed paste receive the appropriate delimiters; ESC and ETX characters in the pasted text are dropped so it cannot end the bracket early or interrupt the program.
- On macOS, **Option** follows the active keyboard layout without a terminal Meta prefix. Option dead keys insert their spacing character immediately (for example, **Option+ñ → `~`** on Spanish keyboards), without requiring Space or composing the next letter. Control+Option chords retain terminal encoding; ordinary accent composition remains unchanged.
- The **…** popover contains **Split columns / Split rows**, **Rename workspace / tab**, **Lock pane / tab**, **Close pane / tab / workspace** and **Quit Spooky**. Lock and unlock a tab or pane from its context menu. Locks are off for new tabs and panes, persist with the workspace and protect against closing them through a tab or workspace. A lock does not prevent a shell from exiting itself. Closing an unlocked pane or tab is immediate; closing the last one leaves an empty workspace. Closing a workspace still asks for confirmation and refuses while it contains a lock. Escape dismisses menus and modals without sending the key to the terminal. Rename fields open with the current name selected.
- Closing the GUI is a detach, not session termination. Relaunch to reconnect to the running server.
- `exit` closes its shell pane and, when that was the tab's last pane, the tab. Other panes and agents keep running. If it was the workspace's last tab, the workspace stays available with a **New tab** button. A shell terminated by a signal retains its output instead of closing automatically.
- New local workspaces start in your home directory, including when launched from Finder. New tabs and splits use the selected pane's current directory; the pane's server determines it, including over Spooky's SSH connection. An agent pane supplies its starting directory for a new tab, but cannot be split. If the shell's directory cannot be verified—for example while it runs a foreground program—the action explains the failure and offers an explicit retry in the workspace's starting directory. An empty workspace's new tab starts there directly. Ordinary **New agent** still uses the workspace directory; **New agent here** uses the selected pane's directory and reports an error if unavailable. Existing sessions keep their directory; use `cd ~` to return home.
- Hover the folder icon in the header for the active pane's recently verified directory, or **Directory unknown** when it cannot be checked. It refreshes about once per second and expires after three seconds. An agent shows its starting directory rather than a live one. SSH workspaces are labelled separately.
- Click the folder icon when a directory is known to see its complete path and copy it. Local directories can also be opened in Finder on macOS or the file manager on Linux. SSH paths show the connected host and can be copied, but are never opened on the local machine. Actions disappear when the observation becomes stale.
- On macOS new shells are login shells, so `~/.zprofile` (PATH, Homebrew) applies even when Spooky is launched from Finder. If neither `LANG` nor `LC_ALL` is set, shells get `LANG=en_US.UTF-8`; set your own in `~/.zprofile` to override. Other platforms run `$SHELL` directly.
- Typing, pasting or attaching while a pane is scrolled into history returns it to live output. The terminal has keyboard focus at launch. The status dot is green only while both the command and display connections are up; with no workspaces the pane offers **New workspace**.
- Applications that enable mouse tracking (vim with `mouse=a`, htop, tmux with `mouse on`) receive clicks, drags, motion and the wheel as xterm reports in the normal, UTF-8 or SGR encoding they requested; hold **Shift** to select text and scroll history instead. Reports stop at column 222 for the legacy normal encoding, which cannot express more. On the alternate screen (less, man) the wheel sends arrow keys, as xterm's alternate scroll does; on the normal screen it scrolls history.
- Applications can request a block, underline or bar cursor (DECSCUSR), steady or blinking. Spooky's default remains the steady bar, a block is translucent over its glyph, and an unfocused block is drawn as an outline that never blinks. A blinking cursor blinks (about every half second) only while the window and pane have the keyboard focus and stays solid while you type. Strikethrough and dim text are rendered.
- A tab shows the title its terminal sets (OSC 0 and 2, such as `vim notes.md`) until you rename it; a renamed tab keeps its name. Titles lose control characters and are cut to 120 characters by the server and to 28 in the tab.
- The divider between split panes is a single line. Drag it to resize (the position is stored on the server, so it survives reconnects and restarts) and double-click it to return to even halves; each side keeps at least 10 %.
- **Cmd+Shift+Enter** (Linux **Ctrl+Shift+Enter**) temporarily enlarges the selected pane to fill its tab; press it again to restore the split layout. The pane's context menu, the **…** menu and the command palette offer the same action. Switching tabs or workspaces, creating a new selected pane, or removing the pane restores the normal layout. Zoom is local to the window; the pane's PTY is resized to match.
- Everything the shortcuts do can be done with the mouse. The header has **split columns**, **split rows** and **new tab** buttons; the selected tab shows its **×** (the others keep its room, so selecting a tab moves nothing), and a middle click closes an unlocked tab immediately. Right-click opens a menu for a tab, pane or workspace; the pane menu includes split, zoom, lock and close actions for that pane. A program that tracks the mouse keeps the right-click; hold **Shift** for Spooky's pane menu. Double-clicking a tab is not bound: on macOS the unified titlebar turns it into the window's zoom gesture.
- Tabs mark unseen activity: a dot means shell output, a check means an agent finished a turn, and an exclamation mark means an agent has a new unanswered question or approval. Hover for an explanation. Viewing a tab acknowledges its visible panes; an already viewed question does not re-alert until a new one arrives. If the window is hidden or unfocused, activity is retained until it is viewed. Indicators are local to this window; reconnecting or switching servers establishes a new baseline for output and completed turns, while pending questions remain visible.
- The **Command palette** (**Cmd+Shift+P**, Ctrl+Shift+P on Linux, or **Command palette…** in the **…** popover) lists every command by name: type to filter, ↑/↓ to move, Enter or a click to run, Escape to close. Each command has a stable id (`core.tab.new`) that shortcuts and, later, extensions use.
- **Cmd+click** (Ctrl+click on Linux) opens an OSC 8 hyperlink or a plain `http(s)://` URL under the pointer. While the modifier is held the link is underlined and its target is shown in the header, which is not necessarily what the text says. Only `http`, `https` and `mailto` targets open; others (`file:`, `ssh:`, application schemes) are refused with a message. A URL wrapped onto a second row is not joined.
- A program can put text on the clipboard with OSC 52, which is how tmux and vim over SSH yank; the header reports each write. Reading the clipboard is never allowed, and writes made before the GUI connected are not replayed. `osc52 = "off"` ignores these requests.
- **Cmd+F** opens a search bar over the pane's scrollback (case-insensitive, the most recent 100,000 lines; a match cannot span a wrapped line break). Matches are highlighted, the current one more strongly, and the view moves to it. **Enter** or **Cmd+G** goes to the next match, **Cmd+Shift+G** to the previous one, wrapping around; **Escape** closes the bar and clears the highlights. A pane scrolled into history shows a scrollbar and how far back it is.
- Programs that ask for focus reports (DECSET 1004, as vim and tmux do) are told when their pane gains or loses the keyboard focus, including when the window is deactivated.

Application shortcuts:

| Action | macOS | Linux |
|---|---|---|
| New tab / new workspace | Cmd+T / Cmd+N | Ctrl+Shift+T / Ctrl+Shift+N |
| Split columns / rows | Cmd+D / Cmd+Shift+D | Ctrl+Shift+D / Ctrl+Shift+Alt+D |
| Zoom / restore pane | Cmd+Shift+Enter | Ctrl+Shift+Enter |
| Close pane / workspace (workspace confirms) | Cmd+W / Cmd+Shift+W | Ctrl+Shift+W / Ctrl+Shift+Alt+W |
| Rename tab / workspace | Cmd+R / Cmd+Shift+R | Ctrl+Shift+R / Ctrl+Shift+Alt+R |
| Select tab 1–9 (9 is the last tab) | Cmd+1…9 | Ctrl+Shift+1…9 |
| Next / previous tab | Cmd+Shift+] / [ , Ctrl+Tab / Ctrl+Shift+Tab | Ctrl+Shift+PageDown / PageUp, Ctrl+Tab / Ctrl+Shift+Tab |
| Select workspace 1–9, next, previous | Cmd+Option+1…9, ↓, ↑ | Ctrl+Shift+Alt+1…9, ↓, ↑ |
| Toggle sidebar / quit | Cmd+B / Cmd+Q | Ctrl+Shift+B / Ctrl+Shift+Q |
| Find in scrollback / next / previous | Cmd+F / Cmd+G / Cmd+Shift+G | Ctrl+Shift+F / Ctrl+Shift+G / Ctrl+Shift+Alt+G |
| Clear / select all / copy all | Cmd+K / Cmd+A / Cmd+Shift+A | Ctrl+Shift+K / Ctrl+Shift+A / Ctrl+Shift+Alt+A |
| Settings | Cmd+, | Ctrl+Shift+, |
| Command palette | Cmd+Shift+P | Ctrl+Shift+P |

Plain Ctrl combinations stay terminal input. Digits and brackets are matched by physical key, so they work on any layout. Shortcuts do not fire while a dialog or text field has the keyboard, except the search chords while the search field has it. The Linux column follows the same scheme but has not been run on Linux.

Shells run on the server, so they survive closing the GUI and dropped SSH connections. They do not survive a system restart or `spooky-server stop`, but the server saves the workspace layout and each shell pane's last verified directory. On restart it creates **new shells** in those directories; running programs and scrollback are not restored. If a saved directory is unavailable, that shell uses the workspace's starting directory and the server logs the fallback. Checkpoints are best effort and may lag after an abrupt failure. The server reads the original shell's directory on its own machine, not a nested SSH session's directory. `spooky-server status` reports what is running; `spooky-server stop` refuses while shells are running unless given `--force`, and a running GUI reconnects by starting a fresh server. The server keeps `server.log` (rotated at 1 MiB) and `workspaces.json` in `~/Library/Application Support/Spooky/` on macOS or `$XDG_STATE_HOME/spooky/` (default `~/.local/state/spooky/`). It is an independent multiplexer, not a tmux wrapper; tmux is optional software that can be run inside a pane.

For SSH workspaces, directory checkpoints are made by the server on the target machine; that server must also be updated to use this feature.

On Wayland other than KDE, Spooky registers the shortcut through the desktop portal when the desktop offers one, but cannot hide or place its window, so the shortcut only brings it forward and the window keeps its native decorations.

**KDE Plasma on Wayland** is supported through KDE itself, because a Wayland client can neither register a key nor place, hide or raise its own window. The shortcut is registered with the `org.freedesktop.portal.GlobalShortcuts` portal under the application id `dev.spooky.terminal` (install `packaging/linux/dev.spooky.terminal.desktop`, which `scripts/package.sh` ships, so KDE can name it); the configured `shortcut` is only the suggested trigger, and KDE keeps what you set in **System Settings → Keyboard → Shortcuts → Spooky**, so the Record button in Spooky's settings refuses to change it there. The window has no KDE title bar (Spooky draws its own header, which drags the window), and is revealed, sized (the configured share of the usable area of the display under the pointer, panels excluded), centred against its top, focused and, with `on_blur = "top"`, kept above other windows by a short KWin QML script loaded over D-Bus (Plasma 6); it slides in from the top edge fading in, and slides out and fades the same way, (where another display lies above the window the slide is only 80 px, since KWin cannot clip a window to its own display and a full one would show on that display), then waits off screen (it is not minimized: KWin's minimize effect flies a window to the task bar), outside the task bar, pager and switcher like Yakuake. Only this process's window is touched. If KWin refuses, the window simply stays where it is and one line goes to stderr. Verified on Plasma 6.3 (Wayland) with the real portal and KWin by triggering the shortcut through KGlobalAccel: bind, place and size, hide, reveal with focus. A physical key press and Plasma 5 were not exercised. Other Wayland compositors keep the behaviour above: no global shortcut, and closing quits.

The interface follows the compact Herdr reference: a unified titlebar on macOS, small monospaced tabs, flat workspace rows and a single hairline between split panes. Panes have no title strips; the focused pane's exited state appears in the header. The default terminal background remains the blue-gray `#22252E` sampled from the Ghostty reference, with darker sidebar and toolbar surfaces. Explicit ANSI/truecolor black remains black. The server resolves default terminal colors, so an already-running older server retains its palette until explicitly restarted.

## Remote workspaces

Build/install `spooky-server` on the remote Unix machine and put it on the remote login shell's PATH. Use the same protocol version on both sides. Click **Connect machine** in the sidebar to open the SSH editor, enter a destination or existing OpenSSH host alias, then select **Connect**. **Local** returns to the local server. Escape, the editor's close button, or hiding the sidebar dismisses the editor and returns keyboard input to the terminal.

If the target cannot be reached, the terminal area explains why and offers **Retry** and **Use this computer**. Failures that retrying cannot fix (rejected login, host-key failure, unknown host, `spooky-server` missing, protocol mismatch) stop automatic attempts until you act; other failures retry with a 2 s to 30 s backoff. Choosing another target, including **Local**, abandons an attempt still in progress and ends its SSH child.

**Browse Tailscale devices** opens a native searchable picker using the installed client's `tailscale status --json`. It shows device names, addresses, OS and online/offline state; offline devices cannot be selected. Enter an optional SSH username and select a device to populate the SSH destination, then explicitly press **Connect**. Refresh rereads the local client without blocking the terminal. This is SSH over the existing tailnet, not an embedded VPN or automatic Tailscale SSH setup: Tailscale must already be signed in and connected, and the destination still needs SSH access and a compatible `spooky-server`. Standard SSH identity and host-key rules remain unchanged.

The remote bridge starts/reconnects to the remote session server. Disconnecting SSH or closing the GUI does not close those shells. Standard OpenSSH configuration, identities, agent and host-key verification are used. Connections are noninteractive: authenticate and establish the host key through normal SSH first. Spooky does not disable verification, store passwords or automatically install remote executables.

The remote module shares the session protocol with local transport. There is no general third-party plugin loader in this implementation.

## Captures and files

**Capture** saves a real PNG of the focused terminal viewport through a native file dialog. It does not capture unrelated desktop windows.

**Attach**, or dropping a file, resolves a local path or transfers the file over SFTP for an SSH target. The resulting shell-quoted path is inserted into the terminal **without pressing Enter**. This is not a claim that arbitrary terminal programs support image attachments; the receiving program decides how to consume the path.

The transfer runs on its own thread, so typing and output continue; one upload runs at a time, and choosing another target abandons it. It may take 30 s plus the time the file needs at 256 KiB/s, at most 30 minutes. Each file goes into a private (mode 700) `~/.spooky-upload.XXXXXX` directory on the remote account. A failed or cancelled transfer removes its directory, and upload directories older than seven days are deleted when the next upload prepares its own; a successful upload otherwise stays for the program that reads it.

## Configuration

Open **Settings** (Cmd+,) to change these, or create `$XDG_CONFIG_HOME/spooky/config.toml` (`~/.config/spooky/config.toml` when XDG_CONFIG_HOME is unset) yourself. Every key is optional; an unknown key or an out-of-range value is an error at startup, not a silent default:

```toml
font_size = 12.0            # 8 to 48
# font_family = "JetBrains Mono"   # a monospace family; absent: Menlo on macOS, DejaVu Sans Mono elsewhere
size = "percent"            # "percent" or "pixels"
width_percent = 100.0       # 30 to 100, of the display (on macOS, of the area below the menu bar and Dock)
height_percent = 50.0       # 20 to 100
background_opacity = 90.0   # macOS only: 20 to 100; 100 is solid
# width = 1440.0            # pixels, 400 to 4096, used when size = "pixels"
# height = 900.0            # pixels, 200 to 2160
shortcut = "Ctrl+Shift+Space"
dock_icon = false
osc52 = "write"
cursor_blink = true
on_blur = "none"            # "none", "hide" or "top"
appearance = "spooky"     # "spooky", "nord" or "dracula"
```

A file that sets `width` or `height` and no `size` keeps meaning pixels, as before this key existed. A percent window is never smaller than 720 × 440 px. The size is applied when the window opens and when a size setting changes; a window you resized by hand keeps its size when the shortcut shows it again.

`dock_icon` only matters on macOS and only while the global shortcut is registered: by default Spooky then runs as an accessory app, with no Dock icon or menu bar, and the shortcut is the way back to a hidden window.

On macOS, **Background opacity** controls the window's blurred background without
fading text. Setting it to 100% or enabling macOS Reduce Transparency gives a solid
background. An older connected server cannot distinguish an explicitly selected
background that happens to equal the default color; update that server to preserve it.

`on_blur` is what happens when another app takes the keyboard focus: `"none"` leaves the window where it is, `"top"` keeps it above every other window, and `"hide"` hides it (the global shortcut brings it back, so `"hide"` is refused without one). A native file dialog opened by Spooky does not count as clicking away.

`osc52` is `"write"` (programs may set the clipboard) or `"off"`. `cursor_blink = false` keeps every cursor solid.

Settings offers Spooky, Nord, Dracula, Ember, Ocean and Midnight, plus user themes. Put a
`.toml` file in `$XDG_CONFIG_HOME/spooky/themes/` (normally
`~/.config/spooky/themes/`) and restart Spooky; it appears in **Settings →
Appearance**. For example:

```toml
id = "my-theme"
name = "My theme"

[colors]
background = "#19121A"
text = "#F3E8F5"
accent = "#F08755"
success = "#70C89A"
warning = "#F0C674"
danger = "#F06C75"
```

All colors are required `#RRGGBB` strings. Surface and muted colors are derived
from them. IDs must be unique lowercase letters, digits or hyphens, starting
with a letter; built-in IDs are reserved. Invalid files are skipped and reported
in Settings. The selected theme is local to this desktop; explicit ANSI colors
from terminal programs are unchanged.

The server reads its own file, `spooky/server.toml` in the same directory, on each machine it runs on (including a remote one):

```toml
scrollback_lines = 10000
agent_shim = true
```

`scrollback_lines` (0 to 1,000,000) is the history kept per pane; it applies to panes created after the server starts. `agent_shim` (default `true`) makes `omp` typed in a pane of this machine's server open the agent view (see `docs/agent.md`); `false` leaves `omp` alone, and it only takes effect for panes created after the server starts. An unreadable or invalid `server.toml` is reported in `server.log` and the defaults are used.

Unknown fields and invalid values produce an explicit startup error. There is no silent fallback to a broken configuration.

## Agent view (web)

On macOS an agent pane that fills its tab is drawn by a React page in a `WKWebView` (through `wry`) that is a child of the window; the terminal, tabs, sidebar and every other overlay stay native. The page is `web/agent` (see its README for the structure), built to one HTML file that is committed and embedded, so `cargo build` needs no Node. A split pane, any other platform, a page that fails to start, and `SPOOKY_AGENT_UI=native` use the native iced agent view. The page holds no state of its own beyond folds, drafts and open lists: the server's `Agent` is pushed to it as JSON, and it can only post a closed set of validated actions (`crates/spooky-desktop/src/features/agent/web.rs`). While one of iced's own overlays (palette, settings, menus, confirmations) is open the page is hidden, because a native child view always draws above iced. Rebuild the page after changing it:

```sh
cd web/agent && npm ci && npm run check && npm run build
```

## Packages and checks

```sh
./scripts/package.sh
```

Produces `dist/Spooky.app` and `dist/Spooky-macos-<arch>.zip` on macOS, or, on Linux, a tarball with both binaries, a desktop entry and its hicolor icon, plus a `.deb` where `dpkg-deb` is available (`scripts/package-deb.sh`). `assets/logo.png` is the supplied artwork for the macOS app icon; packaging brightens its dark tones and rounds the corners before generating the icon resources. `assets/logo-256.png` remains the copy used in the window header and Linux icon. After changing the artwork, regenerate the latter on macOS with `sips -z 256 256 assets/logo.png --out assets/logo-256.png`. The macOS bundle has an ad-hoc signature: it runs on the machine that built it, and Gatekeeper refuses it elsewhere. Set `SPOOKY_SIGN_IDENTITY` to a Developer ID Application identity to sign with the hardened runtime, and `SPOOKY_NOTARY_PROFILE` (a `notarytool store-credentials` profile) to notarize and staple; both need an Apple developer account. CI builds both packages and keeps them as artifacts.

The running macOS Dock tile also uses the rounded artwork generated in the bundle; the standard bundle icon is available when Spooky is not running.

The transport uses **protocol v18**, adding shell-output and agent-turn counters for tab activity. Update both client and server, including remote installations. Spooky rejects incompatible endpoints rather than silently restarting a server: finish your sessions before restarting the old one, since stopping it terminates its shells and agents. Closing only the GUI does not stop it.

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
- `web/agent`: the agent view's React page, built to one embedded HTML file.
- `crates/spooky-server`: persistent PTYs, terminal emulation and workspace tree.
- `crates/spooky-desktop`: GPU rendering, focus/input, clipboard and native window/file integration, organised as `features/` (one folder per feature) plus shared modules.

See [SPEC.md](SPEC.md) for the product contract, [architecture](docs/architecture.md) for module boundaries, and [AGENTS.md](AGENTS.md) for contributor/AI rules.

## Validation

Verified on macOS arm64:

- Workspace build and release packaging; the ad-hoc signed `Spooky.app` verifies with `codesign --verify --strict` and carries its icon.
- Viewport clear, select-all, copy-all and alternate-screen refusal were exercised in an isolated native macOS window with a real PTY; retained history copied correctly before and after clearing. A second isolated frontend exercised copy and clear through a loopback SSH server with a pinned host key. Protocol regressions cover unsubmitted input, wrapped Unicode, concealed text and oversized exports.
- `cargo fmt`, strict clippy, 247 automated Rust tests plus the agent view's own unit tests (`web/agent`), doc tests and warning-free first-party rustdoc.
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
- Protocol v3 row deltas (isolated server, real shell): a full 300×80 snapshot fell from 3,012,519 to 2,179 bytes and one echoed keystroke from 3,012,519 to 463 bytes, with only the changed row sent. Tests cover convergence of applied updates with a fresh full snapshot across scrolling, erase, styled and wide text, and fail when the server stops recording changed rows. The native window rendered bold, italic, underline, 16-colour and truecolor backgrounds, a wide character and a combining accent from runs; a drag selection was highlighted and Cmd+C copied exactly the selected text. `spooky-server bridge` relayed v3 ping, full snapshot and incremental update over stdio, and its exit left the server running.
- Native sidebar interaction: machine-name and chevron collapse/expand without opening SSH; the machine-row + creates and reveals a workspace; switching rows selects the correct terminal. Closing the SSH editor with Escape/X or hiding the sidebar restores terminal input.
- Native workspace context actions: right-click an inactive workspace, rename that exact workspace, cancel closure, then close it with confirmation. Protocol snapshots confirmed that only its workspace and pane IDs disappeared; outside-click and Escape dismissed the action menu.
- Mouse-only control (isolated instance): the header split button, the pane's hover buttons and the pane menu built a four-pane layout; **+** opened tabs, a tab's right-click menu renamed that exact tab (not the selected one), and the pane menu's **Paste** inserted the clipboard. Pane and tab closure now happens without confirmation when unlocked; locked items reject closure.
- Native Tailscale discovery against the installed, signed-in client: real devices rendered, searching filtered them, selecting a device with a username populated the SSH destination without connecting, invalid usernames disabled selection, and Escape restored terminal input without leaking field text.
- Connection failures (isolated instance, `ssh` stand-ins first on `PATH`): a rejecting login showed the failure card with the key-authorization advice, ran `ssh` twice (once per connection) in 22 s instead of retrying every 2 s, and **Use this computer** returned to a working local shell; a hung `ssh` was abandoned and killed when switching back to **Local**. Unit tests cover cancelling a hung handshake, rejected login versus refused connection, and the retry backoff. No real sshd rejection was exercised.
- Dropdown (isolated instance, own server and shortcut): the window opened centred against the top of the display with no Dock icon; the real global shortcut hid it while focused and showed and focused it again; the close button hid it with the process alive and the shortcut restored it; a minimized window was restored by the shortcut; **Cmd+Q** exited with the server still running.
- Shortcuts in the native window: new tab and split, tab selection by number and by Cmd+Shift+], rename (modal prefilled and fully selected), workspace creation and selection by Cmd+Option+digit/arrow, and workspace closure confirmation. Unit tests cover the chord table (any layout for digits, Ctrl combinations left to the terminal, macOS screenshot chords left alone) and stepping/wrapping; protocol tests cover tab and pane closure and locking.
- Real OpenSSH under protocol v5 on an isolated loopback sshd: a remote workspace connected through the GUI and showed its shell, a tab was renamed there, and **Attach** uploaded a 3 MB file named `it's a shot.png` through the native file dialog and inserted the quoted remote path without Enter. The same upload function run directly against that sshd delivered identical bytes (SHA-256) into a mode-700 directory; a failed transfer (unreadable source) left nothing behind; a stale upload directory (older than seven days) was swept while a fresh one was kept. Unit tests with stand-in `ssh`/`sftp` cover cleanup after failure and cancellation, the sweep leaving unrelated directories alone, size-based timeouts and shell quoting; removing the cleanup call makes two of them fail.
- Server lifecycle (isolated servers, real PTYs): `status` and `stop` against a live server (stop refused while shells ran, `--force` stopped it); a replacement server restored the workspace, renamed tab, divider position and fresh shells without reusing identities, and the saved file was written by itself shortly after each change. The native window relaunched onto a server that had been killed outright showed its workspace again (`restored 1 saved workspace(s)` in `server.log`). A damaged `workspaces.json` is set aside as `.corrupt` and never half restored; new panes start at the size of the window-sized pane; `scrollback_lines` in `server.toml` bounded a pane's history; a full log rotates and keeps one generation (unit and protocol tests).
- Native window (isolated instance, real shell, protocol v6): a tab took the title set by `printf '\033]0;…'`; Cmd+click on an OSC 8 link to a `file:` target was refused in the footer, and on a plain URL opened it without its trailing full stop; an OSC 52 write set the system clipboard and the footer said so (the clipboard was put back afterwards); a DECSCUSR 1 cursor alternated between two frames while a steady one did not; `cat -v` with DECSET 1004 printed `^[[O` then `^[[I` as the keyboard focus left and returned; Cmd+F found three matches in the scrollback and highlighted them, and Enter, Cmd+G and Cmd+Shift+G moved between them with wrap-around without typing a letter into the field; a divider was dragged (the second pane grew from 57 to 82 columns) and a double-click restored even halves. Unit and protocol tests cover URL detection (brackets, sentence punctuation, word boundaries), the openable-scheme table, search matching and walking, divider geometry, OSC 52 baseline handling and focus-report routing.
- Settings page (isolated instance with its own server, config directory and shortcut; background clicks and key presses): the **…** menu entry, **Cmd+,**, **Escape** and choosing a tab each opened or closed it. A font-size change refitted the pane (footer 174 × 25 to 149 × 22) and was written to the file together with a cursor-blink change. **Record** refused a bare letter with its reason and took `Ctrl+Alt+Shift+F8`, which was saved. *Top third* and *Full screen* resized the live window (Full screen came to exactly the display height below the menu bar, 1512 × 949 on a 1512 × 982 display; Top third came to the 440 px minimum), *Pixels* gave 1440 × 900 and the default opened as 1512 × 475. *Stay on top* moved the window from layer 0 to layer 3 and *Nothing* back; the Dock switch changed the activation policy from accessory to regular and back. With `on_blur = "hide"` the window hid when a focus-taking click handed focus back to the previous app. Unit tests cover the file format (round trip, old pixel-size files, mistakes are errors, percent sizing and the minimum), the recorder's key spelling against the global-hotkey parser, the **Cmd+,** chord, the presets and the focus-loss rule.
- Command palette (isolated instance, background key presses): **Cmd+Shift+P** opened it over the terminal, typing `split` narrowed the list to the two split commands, **↓** moved the highlight and **Enter** ran *Split rows* (the pane became two rows); **Cmd+Shift+P** then **Escape** returned the keyboard to the shell. Unit tests cover the matching and ordering, the highlight wrapping, an empty result, and that every command id is unique and namespaced.
- Agent panes (phase 1, isolated instance with its own server, real `omp` 18.18.6, background clicks and key presses): the header sparkle opened an *Agent* tab with a focused composer; a prompt streamed the answer (`pong`), then a longer prompt streamed an essay and **Stop** ended it with the status back to *Ready*. A GUI killed and relaunched against the same server showed the same conversation. `spooky-server status` counted the agents, `stop` refused while they ran and `stop --force` left no `omp --mode rpc` process behind. Server tests drive a scripted stand-in for the agent through the real protocol (streaming and a second client reading the same conversation, terminal and agent requests refused by name, closing the tab ends the process, an exiting agent reports why and refuses more messages); unit tests cover the omp frame translation, the bounds on the conversation and an oversize frame. One synthetic click on the header buttons created two tabs, for **New tab** as well as **New agent**: the same artifact either way, not specific to agents, and not investigated. The view follows a chat-app look (user bubble on the right, answers as plain proportional-font text in a centred column with a **Copy** action, a rounded composer, the backend and state underneath); a second pass checked it by screenshot and that Copy put the answer on the clipboard (restored afterwards). Those limits (raw text, one-line composer, no restore) are gone; see the *Agent interface* bullet below.
- `omp` shim (isolated instance, the real zsh with the real profile, which puts `~/.local/bin` first): `command -v omp` printed the shim in the server's state directory, a bare `omp` printed `Opened omp in this pane.`, returned the prompt and showed the agent with a focused composer (at that check it opened in a new tab, then beside the shell; it now takes the shell's own pane: the server tests show one tab and one pane whose terminal returns, with its screen, when the agent is closed and is saved and restored under it, but the GUI side of that last change was not looked at: the screen was locked), and a prompt typed there streamed an answer. Server tests run a profile that puts a directory with its own `omp` first, under bash and, where installed, zsh: the shim still wins, `omp --flag` ran the real one, and a bare `omp` created the agent tab. Not covered: shells other than bash, zsh and fish (the real `omp` runs), a profile that resets `PROMPT_COMMAND` in bash, a remote machine, and `omp` started from inside a program that is not a shell.
- Agent interface, items 1 to 5 (isolated instance, real `omp`, background clicks and key presses; window 1280 px wide only): a prompt asking for a heading, a list and a Python block rendered as Markdown with a code block carrying a **Copy** button; a `read` appeared as a folded tool card with its status dot that unfolded to the file's lines; the real `ask` tool showed a dialog above the composer (*Red*, *Blue*, *Other*, *Dismiss*) and clicking *Blue* produced "You chose Blue"; the footer showed the model, the thinking level, the context percentage and the state. After `stop --force` and a relaunch, the agent tab came back with its conversation (resumed with `--resume`, history loaded). Server tests drive a scripted agent for model and thinking changes (an unlisted choice never reaches the agent), a question that only a valid user answer reaches, reasoning as its own entry, restore after a server restart (saved session, `--resume`, history), and the shim with `--model x "two words"`, a bare `omp commit` and an unsupported option running the real `omp` under bash, zsh and fish. The model list (own drop-down, not a native widget) opened with a search box and a provider sidebar with counts: typing `opus` narrowed the 270 models to 52 and the providers to two, and picking *anthropic* showed only its seven; an earlier run chose a model and the real agent's model and context figure changed. Clicking the context meter under the composer opened a **Usage** panel with the real quotas (Anthropic: *Claude 7 Day* 15% left, *Claude 7 Day (Fable)* 96% left, saved resets; Openai Codex: *5 hours* 100% left, *7 days* 0% left, renewal times), read from `omp`'s own `/usage` command, whose text the server parses because the protocol has no structured report; the panel's bars, notes and refresh were seen, but not the account names shown when a provider has several accounts, nor a refresh while the agent is working (refused by the server). A restored agent whose conversation never began used to end at once on a restart (`omp` writes its session file with the first message); it now starts empty, with a server test. Typing `omp` in the first terminal replaced it with the agent in the same pane (one tab, no split). The model list also has a **Roles** entry (`default`, `smol`, `plan`, `web` from the real `~/.omp/agent/config.yml`, read by the server because `omp`'s protocol does not report them), a tag with the role on each model that has one, and choosing the `plan` role switched the real agent to `anthropic/claude-opus-5` with its `xhigh` level. Not covered: the multiline composer and the thinking list by hand in the GUI (only their messages are unit-tested), the approval *Confirm* and text *Input* dialogs with a real `omp`, a remote machine, Linux, other agents.
- Packaging: `scripts/package.sh` built the bundle and zip in 22 s from a warm cache. Developer ID signing and notarization are scripted but have not been run: there are no credentials here.

- Agent view in a WKWebView (isolated instance, real `omp`, background clicks and key presses): a new agent showed the page exactly over the content area with its state (model, context, *Ready*) and a focused composer; a prompt round-tripped through the server and `omp` and rendered with a heading, a list and a highlighted Rust block; the model list (270 models, providers, roles) and the usage panel opened over the page; **Cmd+Shift+P** pressed in the page opened the native palette, the page hid while it was open and came back on Escape; **Cmd+B** and **Cmd+T** pressed in the page ran the application's commands and the page followed the sidebar's width; switching to a terminal tab and back kept the typed draft. Unit tests cover the validation and bounds of every page message and the wire shape of the state. Pasting, select all, copy and cut work through the application's clipboard (the page has no Edit menu); an `ask` question from `omp` showed as a dialog above the composer and clicking *Blue* produced "You chose Blue"; with `--approval-mode always-ask` the approval for `bash` appeared drawn like oh-my-pi's own (framed, titled in its border, `❯` on the choice, ↑/↓ and ↵ to choose), choosing with the keyboard ran the command and typing went back to the composer; `omp` typed in a pane opened the page over that pane, a server restart brought it back with its conversation, and closing it returned the shell. Also checked: the thinking list, the model list with search and roles (a role sets its model and level), Stop, a typed answer to an `ask` question, the reasoning fold, a Cmd+click on a link ("Opening …"), a long streamed answer that follows its tail, typing after clicking a non-input reaching the composer, and `SPOOKY_AGENT_UI=native` showing the same conversation in the native view. Not exercised: window resizing by dragging (the sidebar toggle re-lays the area out through the same path), window resizing by dragging, Linux (which uses the native view).

- Agent over SSH (the user's real Linux x86_64 host, `omp` 18.x there; the server built there from the sources in a temporary directory and removed afterwards): the GUI reached the remote server through a forwarded socket (the GUI treats it as local, so it is not the `ssh host spooky-server bridge` path itself), showed the host's own fish shell, and an agent tab answered with the remote's model and `bash` output (`devlabs Linux x86_64`); closing the GUI left the remote agent running. This found that the non-interactive `PATH` of an SSH-started server lacks `~/.local/bin`, so `omp` was not found although the host's shells find it: the server now also looks in `~/.local/bin`, `~/.bun/bin`, `~/.cargo/bin`, the mise shims, `/opt/homebrew/bin` and `/usr/local/bin`, and gives the agent those directories on its `PATH` (unit tests; verified on that host).

- Agent over the real SSH bridge (same Linux host): with `spooky-server` on the remote's non-interactive `PATH`, **Connect machine** → `devlabs` started the remote server, showed its fish shell, and a new agent answered with the remote's model (`devlabs Linux x86_64`); closing the GUI left the remote server and `omp` running, and reconnecting showed the same conversation. `omp "…"` typed in a remote pane first ran the real `omp`: that host's bash `exec`s fish and its mise hook rebuilds `PATH` at every prompt, so the shim was neither loaded nor kept first. The shim now configures bash, zsh and fish whichever shell `$SHELL` names, and zsh and fish also re-check `PATH` before each command; after that the same line opened the agent view. Server tests cover bash starting fish and zsh and fish rebuilding `PATH` at every prompt (each fails on the previous shim).

- Edit diffs (isolated instance, real `omp`, haiku): a file edit showed `+1 −2` on its card and, unfolded, the file with the changed lines marked, numbered and tinted (web page) or coloured (native view); the card used to show the whole file again. The diff is the one `omp` reports in the tool result's `details`, bounded to 16 KiB, and the entry's own output is dropped when there is one. Tests cover reading it from the frame, the bound, and the diff parser.

- Attaching to an agent (isolated instance, real `omp`, haiku): **Attach** with an agent pane selected uploads the file to the machine the agent runs on and puts it on the next message, shown as a chip with a small picture of it (web page and native view alike) and a button to drop it; sending names each file on its own line before the message, and the agent read the image and described it. A file that is not a picture, or is over 32 MiB, is named without one.

- Commands of the agent's program (isolated instance, real `omp`): a message that starts with `/` offers what the program lists (160 commands with their subcommands), narrowed as it is typed; ↑/↓ move, Tab or Enter writes the highlighted one, Enter sends one already written whole, Escape puts the list away until the message changes. `/model` ran and its answer came back as its own entry. Checked in the web page and in the native view.

- Current-directory actions (isolated instance, zsh, real `omp`): the earlier explicit actions created a tab, split and agent in a directory with spaces; `pwd -P` and the agent's saved directory confirmed the result. A pinned-key loopback SSH bridge also used the remote pane's directory and preserved shells across reconnect. Tests cover mismatched workspaces, foreground programs, disabled job control and a shell replaced by another program.

- Directory inheritance by default (isolated macOS windows with zsh): Cmd+T, Cmd+D and Cmd+Shift+D created shells in the source pane's directory, confirmed with `pwd -P`. With a foreground process, cancelling created nothing; confirming used the named workspace directory for a tab and a split. Disconnecting dismissed a pending offer. Over a pinned-key loopback SSH connection, Cmd+T and Cmd+D also used the server's directory and an occupied shell showed the refusal. Tests cover a source moved to another workspace, a closed pane and an agent split. This does not establish Linux runtime behavior or an SSH connection to another machine.

- Shell exit (isolated macOS window with zsh): `exit` closed the selected tab and left the other shell running; exiting the last shell left the workspace empty, and its **New tab** button opened another shell. Real-PTY protocol tests cover a split, a nonzero normal exit, disconnect, empty-workspace restore, an agent covering a shell, and a signal-terminated shell retaining its scrollback.

- Directory restoration (isolated macOS window with zsh): two split shells in different directories both returned to their own directory after a server restart, verified with `pwd -P`. A pinned-key loopback SSH bridge also preserved both directories across a server restart and kept live shells across reconnect. Real-PTY tests cover abrupt server exit, a busy shell, a replaced shell, a missing saved directory and a shell covered by an agent; older saved layouts remain readable. This does not establish Linux runtime behavior or an SSH connection to another machine.

**Still unverified:** a remote session initiated from the Tailscale picker, SSH to a host other than loopback, mouse reporting, focus reporting and OSC 52 with real vim, htop or tmux (the checks used `cat -v` and `printf`), the Shift bypass with a physical keyboard, end-to-end PNG-save, dropping a file onto the window, the dropdown on a second display (it appears on the display the window is already on, not under the cursor) or in a fullscreen Space, Developer ID signing and notarization, plus real Linux desktops for the dropdown, shortcuts and everything else. From the settings page specifically: a press of a *newly recorded* global shortcut (the registration succeeded, but synthetic key presses cannot trigger a global hotkey), and `on_blur = "hide"` followed by a reveal with the real shortcut, where a focus event arriving right after the reveal could hide the window again. **Not implemented:** input-method composition (iced's canvas cannot request an input method, so it needs a custom widget, and testing it needs a CJK input source enabled); search across a wrapped line; URLs wrapped onto two rows; theme and colors on the settings page. GitHub CI passed for the previous commit (quality and package on macOS and Linux); the settings page has only been checked locally.

The dependency graph currently reports a future-Rust incompatibility in upstream `block 0.1.6`. First-party clippy and rustdoc pass without suppressing that diagnostic.
