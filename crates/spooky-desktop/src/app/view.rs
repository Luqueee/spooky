//! Compact native workspace chrome; terminal state remains server-owned.
use super::{App, Confirm, ContextTarget, Message};
use crate::{
    commands::Command,
    features::{
        activity::Indicator,
        agent,
        machines::{Machine, Stage, State},
        splits,
    },
    gui::{Icon, Look, Tone, Ui, View, column, geometry_point, hsla, row},
    terminal::DEFAULT_FAMILY,
    ui::Color,
};
use gpui::{
    AnyElement, Div, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, img, px,
};
use spooky_core::{Axis, Layout, client::Problem};
use std::sync::{Arc, LazyLock};

/// Height of the invisible strip at the window's bottom edge that drags its height.
const RESIZE_GRIP: f32 = 5.0;

/// Padding around each terminal pane. The divider geometry accounts for it.
pub(crate) const PANE_PADDING_X: f32 = 9.0;
pub(crate) const PANE_PADDING_Y: f32 = 7.0;
const SIDEBAR_WIDTH: f32 = 232.0;
const HEADER_HEIGHT: f32 = 36.0;
/// Width of a tab's close button plus the gap before it: a tab that is not selected
/// keeps this much room so selecting tabs never moves or resizes the others.
const TAB_CLOSE_SLOT: f32 = 23.0;

fn brand_logo() -> impl IntoElement {
    static LOGO: LazyLock<Arc<gpui::Image>> = LazyLock::new(|| {
        Arc::new(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            include_bytes!("../../../../assets/logo-transparent-256.png").to_vec(),
        ))
    });
    img(LOGO.clone()).size(px(22.))
}

fn activity_marker(v: &View, id: u64, indicator: Option<Indicator>) -> AnyElement {
    let Some(indicator) = indicator else {
        return div().w(px(16.)).into_any_element();
    };
    let (symbol, tone, label) = match indicator {
        Indicator::Output => ("●", Tone::Accent, "New terminal output"),
        Indicator::AgentDone => ("✓", Tone::Success, "Agent response finished"),
        Indicator::Attention => ("!", Tone::Warning, "Agent needs your answer"),
    };
    v.tip(
        div()
            .id(("activity", id as usize))
            .w(px(16.))
            .flex()
            .justify_center()
            .child(v.text(symbol, 16.0, tone)),
        label,
    )
    .into_any_element()
}

/// What the split buttons say; the shortcut differs by platform.
const SPLIT_COLUMNS: &str = if cfg!(target_os = "macos") {
    "Split columns (⌘D)"
} else {
    "Split columns (Ctrl+Shift+D)"
};
const SPLIT_ROWS: &str = if cfg!(target_os = "macos") {
    "Split rows (⇧⌘D)"
} else {
    "Split rows (Ctrl+Shift+Alt+D)"
};
const NEW_TAB: &str = if cfg!(target_os = "macos") {
    "New tab (⌘T)"
} else {
    "New tab (Ctrl+Shift+T)"
};

/// Megabytes, one decimal, for a progress line.
fn megabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_048_576.0)
}

/// A flex child that takes `share` parts of the room along the main axis.
fn weighted(share: f32) -> Div {
    let mut side = div().min_w_0().min_h_0().overflow_hidden();
    side.style().flex_grow = Some(share);
    side.style().flex_shrink = Some(1.0);
    side.style().flex_basis = Some(px(0.).into());
    side
}

/// A message sent by a right-click on the element, with where it happened.
fn context(
    v: &View,
    target: ContextTarget,
) -> impl Fn(&gpui::MouseDownEvent, &mut Window, &mut gpui::App) + 'static {
    v.on_mouse_down(move |event| Message::ContextMenu(target, geometry_point(event.position)))
}

/// The buttons of a dialog, pushed to the right.
fn buttons(items: Vec<AnyElement>) -> Div {
    let mut line = row().w_full().justify_end().gap(px(6.));
    for item in items {
        line = line.child(item);
    }
    line
}

impl App {
    /// Whether a tab's only pane is an agent.
    fn tab_is_agent(&self, tab: &spooky_core::Tab) -> bool {
        let Layout::Pane(id) = tab.layout else {
            return false;
        };
        self.snapshot
            .as_ref()
            .and_then(|s| s.panes.iter().find(|p| p.id == id))
            .is_some_and(|p| p.agent.is_some())
    }

    /// The chrome's background: a little see-through when the window is.
    fn chrome(&self) -> Color {
        self.appearance_colors()
            .sidebar
            .scale_alpha(if self.background_opacity() < 1.0 {
                0.55
            } else {
                1.0
            })
    }

    /// A tab's button, with room for the close button that only the selected tab shows.
    fn tab_button(
        &self,
        v: &View,
        id: SharedString,
        content: impl IntoElement,
        selected: bool,
        message: Message,
    ) -> gpui::Stateful<Div> {
        v.button(
            id,
            content,
            if selected {
                Look::Selected
            } else {
                Look::Quiet
            },
            Some(message),
        )
        .pl(px(9.))
        .pr(px(if selected { 9.0 } else { 9.0 + TAB_CLOSE_SLOT }))
        .py(px(5.))
    }

    fn header(&self, v: &mut View) -> AnyElement {
        // The native macOS traffic lights occupy this part of the unified titlebar.
        let inset = if cfg!(target_os = "macos") { 72.0 } else { 0.0 };
        let mut brand = row()
            .w_full()
            .gap(px(6.))
            .child(div().w(px(inset)).flex_none());
        if self.sidebar_open {
            brand = brand
                .child(brand_logo())
                .child(v.text("Spooky", 11.0, Tone::Text))
                .child(div().flex_1());
        }
        brand = brand.child(v.action(
            "toggle-sidebar",
            Icon::Sidebar,
            "Toggle sidebar",
            Message::Run(Command::ToggleSidebar),
        ));
        let brand = div()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .px(px(7.))
            .flex()
            .items_center()
            .w(px(if self.sidebar_open {
                SIDEBAR_WIDTH
            } else {
                inset + 42.0
            }))
            .child(brand);
        let mut tabs = row().gap(px(3.));
        if let Some((workspace, _)) = self.active() {
            for tab in &workspace.tabs {
                let selected = self.tab == Some(tab.id) && !self.settings.is_shown();
                let id = tab.id;
                let label_tone = if selected { Tone::Text } else { Tone::Muted };
                let content = row()
                    .gap(px(6.))
                    .child(v.icon(
                        if self.tab_is_agent(tab) {
                            Icon::Agent
                        } else {
                            Icon::Terminal
                        },
                        label_tone,
                    ))
                    .child(v.text(self.tab_label(tab), 11.0, Tone::Text))
                    .child(activity_marker(v, id, self.activity.indicator(&tab.layout)))
                    .child(if tab.locked || !tab.locked_panes.is_empty() {
                        v.icon(Icon::Lock, Tone::Muted).into_any_element()
                    } else {
                        div().into_any_element()
                    });
                let label = self.tab_button(
                    v,
                    SharedString::from(format!("tab-{id}")),
                    content,
                    selected,
                    Message::Tab(id),
                );
                let mut entry = row().gap(px(1.)).child(label);
                if selected {
                    entry = entry.child(v.action(
                        SharedString::from(format!("tab-close-{id}")),
                        Icon::Close,
                        "Close tab",
                        Message::AskCloseTab(id),
                    ));
                }
                // Middle-click closes any tab and right-click opens its menu.
                tabs = tabs.child(
                    div()
                        .id(SharedString::from(format!("tab-slot-{id}")))
                        .on_mouse_down(MouseButton::Right, context(v, ContextTarget::Tab(id)))
                        .on_mouse_down(
                            MouseButton::Middle,
                            v.on_mouse_down(move |_| Message::AskCloseTab(id)),
                        )
                        .child(entry),
                );
            }
        }
        // Settings is a tab of its own: it stays after the terminals' tabs until closed,
        // and choosing another tab leaves it without closing it.
        if self.settings.is_open() {
            let selected = self.settings.is_shown();
            let content = row()
                .gap(px(6.))
                .child(v.icon(
                    Icon::Settings,
                    if selected { Tone::Text } else { Tone::Muted },
                ))
                .child(v.text("Settings", 11.0, Tone::Text));
            let label = self.tab_button(
                v,
                SharedString::from("tab-settings"),
                content,
                selected,
                Message::Settings(crate::features::settings::Message::Show),
            );
            let close = Message::Settings(crate::features::settings::Message::Close);
            let mut entry = row().gap(px(1.)).child(label);
            if selected {
                entry = entry.child(v.action(
                    "settings-close",
                    Icon::Close,
                    "Close settings",
                    close.clone(),
                ));
            }
            tabs = tabs.child(
                div()
                    .id("tab-slot-settings")
                    .on_mouse_down(MouseButton::Middle, v.on_mouse_down(move |_| close.clone()))
                    .child(entry),
            );
        }
        let directory = {
            let folder = v
                .button(
                    "directory",
                    v.icon(Icon::Folder, Tone::Muted),
                    Look::Quiet,
                    self.directory
                        .current()
                        .map(|_| Message::ToggleDirectoryMenu),
                )
                .p(px(5.));
            v.tip(folder, self.directory.text(self.connection.is_remote()))
        };
        let status = self.header_status();
        let controls = row()
            .gap(px(3.))
            .child(v.action(
                "split-columns",
                Icon::SplitColumns,
                SPLIT_COLUMNS,
                Message::Run(Command::Split(Axis::Vertical)),
            ))
            .child(v.action(
                "split-rows",
                Icon::SplitRows,
                SPLIT_ROWS,
                Message::Run(Command::Split(Axis::Horizontal)),
            ))
            .child(v.action(
                "new-tab",
                Icon::Plus,
                NEW_TAB,
                Message::Run(Command::NewTab),
            ))
            .child(directory)
            .child(v.action(
                "workspace-actions",
                Icon::More,
                "Workspace actions",
                Message::ToggleMenu,
            ));
        let header = row()
            .id("header")
            .w_full()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .bg(hsla(self.chrome()))
            .child(brand)
            .child(v.vseparator())
            .child(
                div()
                    .id("tabs")
                    .flex_1()
                    .min_w_0()
                    .px(px(7.))
                    .overflow_x_scroll()
                    .child(tabs),
            )
            .child(
                div()
                    .max_w(px(220.))
                    .overflow_hidden()
                    .child(v.line(status, 10.0, Tone::Muted)),
            )
            .child(controls)
            .child(div().w(px(5.)).flex_none())
            .on_mouse_down(MouseButton::Left, v.on_mouse_down(|_| Message::DragWindow));
        header.into_any_element()
    }

    fn sidebar(&self, v: &mut View) -> AnyElement {
        let connection_name = if !self.connection.is_remote() {
            "This computer"
        } else {
            self.connection.connected_target()
        };
        let palette = self.appearance_colors();
        let section =
            div()
                .h(px(22.))
                .flex()
                .items_center()
                .child(v.text("Spaces", 10.0, Tone::Muted));
        let machine = row()
            .w_full()
            .child(
                v.button(
                    "machine-name",
                    row()
                        .w_full()
                        .gap(px(6.))
                        .child(v.icon(Icon::Server, Tone::Muted))
                        .child(
                            div()
                                .flex_1()
                                .child(v.line(connection_name, 10.0, Tone::Muted)),
                        ),
                    Look::Quiet,
                    Some(Message::ToggleWorkspaces),
                )
                .flex_1()
                .px(px(6.))
                .py(px(5.)),
            )
            .child(v.action(
                "new-workspace",
                Icon::Plus,
                "New workspace",
                Message::Run(Command::NewWorkspace),
            ))
            .child(
                v.button(
                    "machine-toggle",
                    row()
                        .gap(px(6.))
                        .child(v.dot(if self.online() {
                            palette.success
                        } else {
                            palette.muted
                        }))
                        .child(v.icon(
                            if self.workspaces_open {
                                Icon::Chevron
                            } else {
                                Icon::ChevronRight
                            },
                            Tone::Muted,
                        )),
                    Look::Quiet,
                    Some(Message::ToggleWorkspaces),
                )
                .px(px(6.))
                .py(px(5.)),
            );
        let mut spaces = column().w_full().gap(px(2.));
        if self.workspaces_open
            && let Some(snapshot) = &self.snapshot
        {
            let colors = [
                palette.success,
                Color::from_rgb8(193, 148, 104),
                palette.accent,
                Color::from_rgb8(166, 145, 190),
            ];
            for workspace in &snapshot.workspaces {
                let selected = self.workspace == Some(workspace.id);
                let workspace_id = workspace.id;
                let color = colors[workspace.id as usize % colors.len()];
                let badge = div()
                    .p(px(2.))
                    .rounded(px(3.))
                    .bg(hsla(color.scale_alpha(0.14)))
                    .child(crate::gui::icon(Icon::Folder, color, 14.0));
                let item = v
                    .button(
                        SharedString::from(format!("workspace-{workspace_id}")),
                        row().gap(px(7.)).child(badge).child(v.text(
                            workspace.name.clone(),
                            11.0,
                            if selected { Tone::Text } else { Tone::Muted },
                        )),
                        if selected {
                            Look::Selected
                        } else {
                            Look::Quiet
                        },
                        Some(Message::Workspace(workspace_id)),
                    )
                    .w_full()
                    .px(px(6.))
                    .py(px(5.))
                    .on_mouse_down(
                        MouseButton::Right,
                        context(v, ContextTarget::Workspace(workspace_id)),
                    );
                spaces = spaces.child(v.tip(item, workspace.cwd.clone()));
            }
        }
        let active = self.connection.connected_target();
        let mut in_use = Some(column().w_full().gap(px(2.)).child(machine).child(spaces));
        let mut machines = column().w_full().gap(px(8.));
        for known in self.machines.list() {
            machines = match in_use.take_if(|_| known.destination == active) {
                Some(section) => machines.child(section),
                None => machines.child(self.other_machine(v, known)),
            };
        }
        let mut connection = column().w_full().gap(px(7.));
        if self.connection_open {
            let spec = v.line_spec(
                self.connection.target(),
                "user@host or SSH alias",
                11.0,
                Message::TargetValue,
                Some(Message::AddMachine),
            );
            let target = v.boxed_field("connection-target", spec, (6.0, 6.0));
            connection = connection
                .child(v.text("Add a machine via SSH", 10.0, Tone::Text))
                .child(
                    div().child(
                        v.button(
                            "browse-tailscale",
                            div().text_size(px(10.)).child("Browse Tailscale devices"),
                            Look::Quiet,
                            Some(Message::Tailscale(
                                crate::features::tailscale::Message::Open,
                            )),
                        )
                        .px(px(8.))
                        .py(px(5.)),
                    ),
                )
                .child(target)
                .child(
                    row()
                        .w_full()
                        .justify_between()
                        .gap(px(3.))
                        .child(
                            v.button(
                                "add-machine",
                                div().text_size(px(10.)).child("Add"),
                                Look::Primary,
                                Some(Message::AddMachine),
                            )
                            .px(px(8.))
                            .py(px(5.)),
                        )
                        .child(v.action(
                            "close-connection",
                            Icon::Close,
                            "Close connection editor",
                            Message::ToggleConnection,
                        )),
                );
        } else {
            connection = connection.child(v.menu_item(
                "add-machine-open",
                Icon::Plus,
                "Add machine",
                Some(Message::ToggleConnection),
                Look::Quiet,
            ));
        }
        let mut footer = column().w_full().gap(px(2.));
        if let Some(pill) = self.update_pill(v) {
            footer = footer.child(pill);
        }
        footer = footer.child(connection);
        column()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .bg(hsla(self.chrome()))
            .child(div().w_full().px(px(6.)).py(px(4.)).child(section))
            .child(
                div()
                    .id("machines")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .px(px(6.))
                    .overflow_y_scroll()
                    .child(machines),
            )
            .child(v.separator())
            .child(div().w_full().p(px(8.)).child(footer))
            .into_any_element()
    }

    /// A machine that is not the one in use: its state, its workspaces as the probe last
    /// saw them, and what can be done when it fails.
    fn other_machine(&self, v: &View, known: &Machine) -> AnyElement {
        let palette = self.appearance_colors();
        let id = known.id;
        let color = match &known.state {
            State::Ready(_) => palette.success,
            State::Connecting => palette.warning,
            State::Failed(_) => palette.danger,
        };
        let key = |name: &str| SharedString::from(format!("machine-{id}-{name}"));
        let mut header = row().w_full().child(
            v.button(
                key("activate"),
                row()
                    .w_full()
                    .gap(px(6.))
                    .child(v.icon(Icon::Server, Tone::Muted))
                    .child(div().flex_1().child(v.line(
                        known.label().to_owned(),
                        10.0,
                        Tone::Muted,
                    ))),
                Look::Quiet,
                Some(Message::ActivateMachine(id)),
            )
            .flex_1()
            .px(px(6.))
            .py(px(5.)),
        );
        if !known.is_local() {
            header = header.child(v.action(
                key("remove"),
                Icon::Close,
                "Remove machine",
                Message::RemoveMachine(id),
            ));
        }
        header = header.child(
            v.button(
                key("toggle"),
                row().gap(px(6.)).child(v.dot(color)).child(v.icon(
                    if known.open {
                        Icon::Chevron
                    } else {
                        Icon::ChevronRight
                    },
                    Tone::Muted,
                )),
                Look::Quiet,
                Some(Message::ToggleMachine(id)),
            )
            .px(px(6.))
            .py(px(5.)),
        );
        let mut body = column().w_full().gap(px(2.)).child(header);
        if !known.open {
            return body.into_any_element();
        }
        match &known.state {
            State::Connecting => {
                body = body.child(div().px(px(8.)).py(px(2.)).child(v.text(
                    "Connecting…",
                    10.0,
                    Tone::Muted,
                )));
            }
            State::Ready(summary) => {
                for (index, workspace) in summary.workspaces.iter().enumerate() {
                    body = body.child(
                        v.button(
                            key(&format!("workspace-{index}")),
                            row()
                                .gap(px(7.))
                                .child(v.icon(Icon::Folder, Tone::Muted))
                                .child(v.text(workspace.name.clone(), 11.0, Tone::Muted)),
                            Look::Quiet,
                            Some(Message::ActivateMachine(id)),
                        )
                        .w_full()
                        .px(px(6.))
                        .py(px(5.)),
                    );
                }
            }
            State::Failed(failure) => {
                let line = match failure.problem {
                    Some(Problem::ServerMissing) => {
                        "Spooky is not installed on this machine.".to_owned()
                    }
                    Some(Problem::Incompatible { server, client }) => format!(
                        "Its Spooky server is out of date (protocol {server}; this app speaks {client})."
                    ),
                    None => failure.reason.clone(),
                };
                let mut actions = row().gap(px(4.)).child(
                    v.button(
                        key("retry"),
                        div().text_size(px(10.)).child("Retry"),
                        Look::Quiet,
                        Some(Message::RetryMachine(id)),
                    )
                    .px(px(8.))
                    .py(px(4.)),
                );
                if failure.problem.is_some() {
                    actions = actions.child(
                        v.button(
                            key("install"),
                            div().text_size(px(10.)).child("Install…"),
                            Look::Primary,
                            Some(Message::InstallAsk(id)),
                        )
                        .px(px(8.))
                        .py(px(4.)),
                    );
                }
                body = body.child(
                    column()
                        .w_full()
                        .gap(px(6.))
                        .px(px(8.))
                        .py(px(2.))
                        .child(v.text(line, 10.0, Tone::Danger))
                        .child(actions),
                );
            }
        }
        body.into_any_element()
    }

    /// The dialog that offers to install the server on a machine and shows how it goes.
    fn install_dialog(&self, v: &View) -> Option<AnyElement> {
        let dialog = self.machines.dialog()?;
        let label = self.machines.get(dialog.machine)?.label().to_owned();
        let muted = |line: String| v.text(line, 11.0, Tone::Muted);
        let title = |line: String| v.text(line, 14.0, Tone::Text);
        let plain = |id: &'static str, label: &'static str, message: Message| {
            v.label_button(id, label, Look::Quiet, Some(message))
                .into_any_element()
        };
        let main = |id: &'static str, label: &'static str, message: Message| {
            v.label_button(id, label, Look::Primary, Some(message))
                .into_any_element()
        };
        let content: AnyElement = match &dialog.stage {
            Stage::Ask(problem) => {
                let why = match problem {
                    Problem::ServerMissing => {
                        format!("{label} has no Spooky server, so there is nothing to connect to.")
                    }
                    Problem::Incompatible { server, client } => format!(
                        "The Spooky server on {label} is out of date (protocol {server}; this app speaks {client})."
                    ),
                };
                column()
                    .gap(px(12.))
                    .child(title(format!("Install Spooky on {label}?")))
                    .child(muted(why))
                    .child(muted(format!(
                        "Spooky downloads the server release on this computer, checks its checksum and copies it to ~/.local/bin on {label} over SSH. Nothing else is changed there."
                    )))
                    .child(buttons(vec![
                        plain("install-cancel", "Cancel", Message::InstallClose),
                        main("install-go", "Install", Message::InstallGo),
                    ]))
                    .into_any_element()
            }
            Stage::Running(run) => {
                let mut steps = column().gap(px(3.));
                let last = run.steps.len().saturating_sub(1);
                for (index, step) in run.steps.iter().enumerate() {
                    let done = index < last;
                    steps = steps.child(v.text(
                        format!("{} {step}", if done { "✓" } else { "…" }),
                        11.0,
                        if done { Tone::Muted } else { Tone::Text },
                    ));
                }
                let progress = match run.sent {
                    Some((sent, total)) => column()
                        .gap(px(4.))
                        .child(progress_bar(v, sent as f32 / total.max(1) as f32))
                        .child(muted(format!(
                            "{} of {} MB sent",
                            megabytes(sent),
                            megabytes(total)
                        )))
                        .into_any_element(),
                    None if run.downloaded > 0 => {
                        muted(format!("{} MB downloaded", megabytes(run.downloaded))).into_any_element()
                    }
                    None => div().into_any_element(),
                };
                column()
                    .gap(px(12.))
                    .child(title(format!("Installing on {label}")))
                    .child(steps)
                    .child(progress)
                    .child(buttons(vec![plain("install-abort", "Cancel", Message::InstallCancel)]))
                    .into_any_element()
            }
            Stage::Restart { running_protocol } => column()
                .gap(px(12.))
                .child(title(format!("Restart the server on {label}?")))
                .child(muted(format!(
                    "The new server is installed, but {label} is still running the old one (protocol {running_protocol}). Restarting it ends every shell and agent running there."
                )))
                .child(buttons(vec![
                    plain("install-later", "Later", Message::InstallClose),
                    v.label_button(
                        "install-restart",
                        "Restart and end its shells",
                        Look::Destructive,
                        Some(Message::RestartGo),
                    )
                    .into_any_element(),
                ]))
                .into_any_element(),
            Stage::Restarting => column()
                .gap(px(12.))
                .child(title(format!("Restarting the server on {label}")))
                .child(muted("Stopping the old server…".into()))
                .into_any_element(),
            Stage::Done => column()
                .gap(px(12.))
                .child(title(format!("Spooky is ready on {label}")))
                .child(buttons(vec![
                    plain("install-close", "Close", Message::InstallClose),
                    main("install-open", "Open", Message::ActivateMachine(dialog.machine)),
                ]))
                .into_any_element(),
            Stage::Failed(message) => column()
                .gap(px(12.))
                .child(title(format!("Could not install on {label}")))
                .child(v.text(message.clone(), 11.0, Tone::Danger))
                .child(buttons(vec![
                    plain("install-close", "Close", Message::InstallClose),
                    main("install-retry", "Try again", Message::InstallGo),
                ]))
                .into_any_element(),
        };
        Some(content)
    }

    /// The sidebar entry that says a newer release is known and opens the update dialog.
    fn update_pill(&self, v: &View) -> Option<AnyElement> {
        if self.updates.dialog().is_some() {
            return None;
        }
        let available = self.updates.available()?;
        Some(
            v.button(
                "update-pill",
                row()
                    .gap(px(8.))
                    .child(v.icon(Icon::ChevronUp, Tone::Accent))
                    .child(v.text(
                        format!("Update to {}", available.version),
                        11.0,
                        Tone::Accent,
                    )),
                Look::Quiet,
                Some(Message::UpdateAsk),
            )
            .w_full()
            .px(px(8.))
            .py(px(6.))
            .into_any_element(),
        )
    }

    /// The dialog that offers an update, shows how it goes and asks before restarting.
    fn update_dialog(&self, v: &View) -> Option<AnyElement> {
        use crate::features::updates::Stage as UpdateStage;
        use spooky_core::update::Support;
        let dialog = self.updates.dialog()?;
        let version = dialog.available.version.clone();
        let muted = |line: String| v.text(line, 11.0, Tone::Muted);
        let title = |line: String| v.text(line, 14.0, Tone::Text);
        let plain = |id: &'static str, label: &'static str, message: Message| {
            v.label_button(id, label, Look::Quiet, Some(message))
                .into_any_element()
        };
        let main = |id: &'static str, label: &'static str, message: Message| {
            v.label_button(id, label, Look::Primary, Some(message))
                .into_any_element()
        };
        let content: AnyElement = match &dialog.stage {
            UpdateStage::Ask => match self.updates.support() {
                Support::Update(_) => column()
                    .gap(px(12.))
                    .child(title(format!("Update Spooky to {version}?")))
                    .child(muted(format!(
                        "Spooky downloads {version}, checks its checksum, replaces this application and restarts. Your shells keep running; if the new version needs a new server, you are told before anything is stopped."
                    )))
                    .child(buttons(vec![
                        plain("update-cancel", "Cancel", Message::UpdateClose),
                        main("update-go", "Download and update", Message::UpdateGo),
                    ]))
                    .into_any_element(),
                Support::Manual(reason) => column()
                    .gap(px(12.))
                    .child(title(format!("Spooky {version} is available")))
                    .child(muted(reason.clone()))
                    .child(buttons(vec![
                        plain("update-close", "Close", Message::UpdateClose),
                        main(
                            "update-page",
                            "Open the release page",
                            Message::OpenLink(spooky_core::update::RELEASES_PAGE.into()),
                        ),
                    ]))
                    .into_any_element(),
            },
            UpdateStage::Running(run) => {
                let mut steps = column().gap(px(3.));
                let last = run.steps.len().saturating_sub(1);
                for (index, step) in run.steps.iter().enumerate() {
                    let done = index < last;
                    steps = steps.child(v.text(
                        format!("{} {step}", if done { "✓" } else { "…" }),
                        11.0,
                        if done { Tone::Muted } else { Tone::Text },
                    ));
                }
                let progress = if run.downloaded > 0 {
                    muted(format!("{} MB downloaded", megabytes(run.downloaded))).into_any_element()
                } else {
                    div().into_any_element()
                };
                column()
                    .gap(px(12.))
                    .child(title(format!("Downloading Spooky {version}")))
                    .child(steps)
                    .child(progress)
                    .child(buttons(vec![plain("update-abort", "Cancel", Message::UpdateClose)]))
                    .into_any_element()
            }
            UpdateStage::Ready(ready) => {
                let (line, restart): (String, AnyElement) = if ready.restart_server {
                    (
                        if ready.sessions == 0 {
                            format!(
                                "Spooky {version} is ready. Its terminal server is a newer version, so the one running now restarts; no shell or agent is running in it."
                            )
                        } else {
                            format!(
                                "Spooky {version} is ready. Its terminal server is a newer version, so the one running now must stop. That ends {} running shell(s) and agent(s); your workspaces come back with new shells.",
                                ready.sessions
                            )
                        },
                        v.label_button(
                            "update-restart",
                            if ready.sessions == 0 {
                                "Restart"
                            } else {
                                "Restart and end its shells"
                            },
                            if ready.sessions == 0 { Look::Primary } else { Look::Destructive },
                            Some(Message::UpdateRestart),
                        )
                        .into_any_element(),
                    )
                } else {
                    (
                        format!(
                            "Spooky {version} is ready. Spooky restarts to finish; your shells keep running."
                        ),
                        main("update-restart", "Restart", Message::UpdateRestart),
                    )
                };
                column()
                    .gap(px(12.))
                    .child(title("Restart to finish updating".into()))
                    .child(muted(line))
                    .child(buttons(vec![
                        plain("update-later", "Not now", Message::UpdateClose),
                        restart,
                    ]))
                    .into_any_element()
            }
            UpdateStage::Applying => column()
                .gap(px(12.))
                .child(title(format!("Updating to {version}")))
                .child(muted("Replacing Spooky and restarting…".into()))
                .into_any_element(),
            UpdateStage::Failed(message) => column()
                .gap(px(12.))
                .child(title("Could not update".into()))
                .child(v.text(message.clone(), 11.0, Tone::Danger))
                .child(buttons(vec![
                    plain("update-close", "Close", Message::UpdateClose),
                    main("update-retry", "Try again", Message::UpdateGo),
                ]))
                .into_any_element(),
        };
        Some(content)
    }

    fn pane_locked(&self, pane: u64) -> bool {
        self.snapshot.as_ref().is_some_and(|s| {
            s.workspaces
                .iter()
                .flat_map(|w| &w.tabs)
                .any(|t| t.locked_panes.contains(&pane))
        })
    }

    fn tab_locked(&self, tab: u64) -> bool {
        self.snapshot.as_ref().is_some_and(|s| {
            s.workspaces
                .iter()
                .flat_map(|w| &w.tabs)
                .any(|t| t.id == tab && t.locked)
        })
    }

    fn terminal_layout(&self, v: &mut View, layout: &Layout) -> AnyElement {
        match layout {
            Layout::Pane(id) => {
                let Some(snapshot) = self.snapshot.as_ref() else {
                    return unavailable(v);
                };
                let Some(pane) = snapshot.panes.iter().find(|p| p.id == *id) else {
                    return unavailable(v);
                };
                if let Some(conversation) = &pane.agent {
                    let id = *id;
                    if self.web_target() == Some(id) {
                        return super::web::area(v);
                    }
                    let can_zoom = self
                        .active()
                        .is_some_and(|(_, tab)| matches!(tab.layout, Layout::Split { .. }));
                    return div()
                        .id(("agent", id as usize))
                        .size_full()
                        .on_mouse_down(
                            MouseButton::Left,
                            v.on_mouse_down(move |_| Message::FocusPane(id, Vec::new())),
                        )
                        .child(agent::view(v, &self.agents, id, conversation, can_zoom))
                        .into_any_element();
                }
                let selected = self.pane == Some(*id);
                let selection = self.selection.and_then(|s| {
                    (s.pane == *id).then_some(if s.all {
                        (
                            (0, 0),
                            (pane.rows.saturating_sub(1), pane.cols.saturating_sub(1)),
                        )
                    } else {
                        (s.anchor, s.head)
                    })
                });
                let typing = if selected && self.terminal_has_keyboard() {
                    v.ui.root().map(|root| crate::gui::Typing {
                        root,
                        focus: v.ui.terminal_focus.clone(),
                        cursor_bounds: v.ui.cursor_bounds.clone(),
                    })
                } else {
                    None
                };
                let view = crate::gui::TerminalView {
                    snapshot: snapshot.clone(),
                    pane: *id,
                    font: self.font.clone(),
                    font_size: self.config.font_size,
                    focused: self.terminal_focused && selected,
                    selection,
                    cursor_on: !(selected && self.blink_wanted()) || self.blink_on,
                    colors: v.p,
                    pointer: v.ui.pointer(*id),
                    send: v.ui.sender(),
                    composing: if typing.is_some() {
                        v.ui.composing.clone()
                    } else {
                        None
                    },
                    typing,
                };
                let mut area = div().size_full().relative().child(
                    div()
                        .size_full()
                        .px(px(PANE_PADDING_X))
                        .py(px(PANE_PADDING_Y))
                        .child(view),
                );
                if pane.scrolled > 0 && pane.history > 0 {
                    let thumb = v.tip(
                        div()
                            .id(("scroll-thumb", *id as usize))
                            .w(px(4.))
                            .h_full()
                            .child(crate::gui::scroll_thumb(snapshot.clone(), *id, v.p.muted)),
                        format!("{} of {} lines", pane.scrolled, pane.history),
                    );
                    area = area.child(div().absolute().top_0().right_0().h_full().child(thumb));
                }
                area.into_any_element()
            }
            Layout::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => {
                // A divider being dragged shows where it is now, not where the server
                // last saw it.
                let ratio = self.splits.dragged_ratio(*id).unwrap_or(*ratio);
                let first = weighted(f32::from(ratio)).child(self.terminal_layout(v, first));
                let second =
                    weighted(f32::from(1000 - ratio)).child(self.terminal_layout(v, second));
                let divider = splits::divider_view(v, *id, *axis);
                let container = div().size_full().flex();
                match axis {
                    Axis::Horizontal => container.flex_col(),
                    Axis::Vertical => container.flex_row(),
                }
                .child(first)
                .child(divider)
                .child(second)
                .into_any_element()
            }
        }
    }

    /// Shown when no workspace is selectable: before the first snapshot, or after
    /// the last workspace was closed.
    fn empty_state(&self, v: &View) -> AnyElement {
        let centred = || {
            column()
                .size_full()
                .items_center()
                .justify_center()
                .gap(px(10.))
        };
        if let Some((reason, retrying)) = self.connection.unreachable() {
            return self.unreachable_state(v, reason, retrying);
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|s| s.workspaces.is_empty())
        {
            centred()
                .child(v.text("No workspaces", 12.0, Tone::Muted))
                .child(v.label_button(
                    "empty-new-workspace",
                    "New workspace",
                    Look::Primary,
                    Some(Message::Run(Command::NewWorkspace)),
                ))
                .into_any_element()
        } else if self
            .active_workspace()
            .is_some_and(|workspace| workspace.tabs.is_empty())
        {
            centred()
                .child(v.text("No tabs in this workspace", 12.0, Tone::Muted))
                .child(v.label_button(
                    "empty-new-tab",
                    "New tab",
                    Look::Primary,
                    Some(Message::Run(Command::NewTab)),
                ))
                .into_any_element()
        } else {
            centred()
                .child(v.text("Waiting for session…", 12.0, Tone::Muted))
                .into_any_element()
        }
    }

    /// Shown while a target cannot be reached: what failed, and the way out. The
    /// way back to this computer is always offered for a remote target.
    fn unreachable_state(&self, v: &View, reason: &str, retrying: bool) -> AnyElement {
        let remote = self.connection.is_remote();
        let title = if remote {
            format!("Cannot connect to {}", self.connection.connected_target())
        } else {
            "Cannot reach the local server".to_owned()
        };
        let mut actions = row().gap(px(6.)).child(v.label_button(
            "unreachable-retry",
            "Retry",
            Look::Primary,
            Some(Message::Retry),
        ));
        if remote {
            actions = actions.child(v.label_button(
                "unreachable-local",
                "Use this computer",
                Look::Quiet,
                Some(Message::Local),
            ));
        }
        let mut card = column()
            .max_w(px(520.))
            .items_center()
            .gap(px(10.))
            .child(v.text(title, 13.0, Tone::Text))
            .child(v.text(reason.to_owned(), 11.0, Tone::Muted));
        if retrying {
            card = card.child(v.text("Retrying automatically…", 10.0, Tone::Muted));
        }
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(card.child(actions))
            .into_any_element()
    }

    /// The link destination takes priority over the latest operational status.
    fn header_status(&self) -> String {
        if let Some(target) = self.hyperlinks.hover() {
            format!("↗ {target}")
        } else if self.current_pane().is_some_and(|pane| pane.exited) {
            "Terminal exited".into()
        } else if self.status == "Connected" {
            String::new()
        } else {
            self.status.clone()
        }
    }

    pub(crate) fn view(
        &self,
        ui: &mut Ui,
        window: &mut Window,
        cx: &mut gpui::Context<crate::gui::Root>,
    ) -> AnyElement {
        let mut v = View {
            cx,
            ui,
            p: self.appearance_colors(),
        };
        let background = self
            .appearance_colors()
            .background
            .scale_alpha(self.background_opacity());
        let screen = self.screen(&mut v);
        let mut root = div()
            .size_full()
            .relative()
            .font_family(DEFAULT_FAMILY)
            .text_color(hsla(v.p.text))
            .text_size(px(12.))
            .bg(hsla(background));
        // The content is moved inside the window, which clips it at its top edge.
        let share = self.slide_hidden_share();
        if share > 0.0 {
            let height = f32::from(window.viewport_size().height);
            root = root.child(
                div()
                    .size_full()
                    .absolute()
                    .left_0()
                    .top(px(-share * height))
                    .child(screen),
            );
        } else {
            root = root.child(screen);
        }
        root.into_any_element()
    }

    fn screen(&self, v: &mut View) -> AnyElement {
        let content = if self.settings.is_shown() {
            crate::features::settings::view(
                v,
                &self.settings,
                &self.config,
                &self.themes,
                &self.updates,
                crate::features::settings::Session {
                    shortcuts_available: self.hotkeys.is_some(),
                    can_hide: self.dropdown(),
                    reduce_transparency: self.reduce_transparency,
                },
            )
        } else {
            match self.active() {
                Some((_, tab)) => {
                    let pane = self
                        .zoomed
                        .and_then(|(id, pane)| (id == tab.id).then_some(pane));
                    self.terminal_layout(v, crate::layout::visible_layout(&tab.layout, pane))
                }
                None => self.empty_state(v),
            }
        };

        let mut main = column().flex_1().h_full().min_w_0();
        if !self.settings.is_shown()
            && let Some(bar) = crate::features::search::view(&self.search, v)
        {
            main = main.child(bar).child(v.separator());
        }
        main = main.child(div().w_full().flex_1().min_h_0().child(content));
        let body = if self.sidebar_open {
            row()
                .w_full()
                .flex_1()
                .min_h_0()
                .items_start()
                .child(self.sidebar(v))
                .child(v.vseparator())
                .child(main)
        } else {
            row().w_full().flex_1().min_h_0().items_start().child(main)
        };
        let mut page = column()
            .size_full()
            .child(self.header(v))
            .child(v.separator())
            .child(body);
        // The bottom edge drags the window's height, which a window without a title bar
        // has no other way to change.
        page = page.child(
            div()
                .id("resize-grip")
                .absolute()
                .bottom_0()
                .left_0()
                .w_full()
                .h(px(RESIZE_GRIP))
                .cursor(gpui::CursorStyle::ResizeUpDown)
                .on_mouse_down(
                    MouseButton::Left,
                    v.on_mouse_down(|_| Message::ResizeBottom),
                ),
        );
        for overlay in self.overlays(v) {
            page = page.child(overlay);
        }
        page.into_any_element()
    }

    /// Menus and dialogs, drawn over everything else, the topmost last.
    fn overlays(&self, v: &mut View) -> Vec<AnyElement> {
        let mut layers = Vec::new();
        if self.menu_open {
            layers.push(v.dismiss_layer("menu-dismiss", Message::ToggleMenu));
            layers.push(
                div()
                    .absolute()
                    .top(px(HEADER_HEIGHT + 4.0))
                    .right(px(6.))
                    .child(self.workspace_menu(v))
                    .into_any_element(),
            );
        }
        if let Some((selection, path)) = &self.directory_menu {
            layers.push(v.dismiss_layer("directory-dismiss", Message::ToggleDirectoryMenu));
            layers.push(
                div()
                    .absolute()
                    .top(px(HEADER_HEIGHT + 4.0))
                    .right(px(6.))
                    .child(self.directory_popover(v, selection.kind, path))
                    .into_any_element(),
            );
        }
        if let Some((target, position)) = self.context_menu {
            layers.push(v.dismiss_layer("context-dismiss", Message::DismissContextMenu));
            layers.push(v.popup(position, self.context_items(v, target)));
        }
        if self.tailscale.is_open() {
            let content = crate::features::tailscale::view(&self.tailscale, v);
            layers.push(v.modal(
                "tailscale-modal",
                content,
                Some(Message::Tailscale(
                    crate::features::tailscale::Message::Close,
                )),
            ));
        }
        if let Some(content) = crate::features::palette::view(&self.palette, v) {
            layers.push(v.modal(
                "palette-modal",
                content,
                Some(Message::Palette(crate::features::palette::Message::Close)),
            ));
        }
        if self.renaming {
            let renaming_tab = self.rename_tab.is_some();
            let spec = v.line_spec(
                &self.rename,
                if renaming_tab {
                    "Tab name"
                } else {
                    "Workspace name"
                },
                12.0,
                Message::RenameValue,
                Some(Message::RenameSubmit),
            );
            let field = v.boxed_field("workspace-name", spec, (8.0, 8.0));
            let content = column()
                .gap(px(14.))
                .child(v.text(
                    if renaming_tab {
                        "Rename tab"
                    } else {
                        "Rename workspace"
                    },
                    14.0,
                    Tone::Text,
                ))
                .child(field)
                .child(buttons(vec![
                    v.label_button(
                        "rename-cancel",
                        "Cancel",
                        Look::Quiet,
                        Some(Message::CancelRename),
                    )
                    .into_any_element(),
                    v.label_button(
                        "rename-save",
                        "Save",
                        Look::Primary,
                        Some(Message::RenameSubmit),
                    )
                    .into_any_element(),
                ]));
            layers.push(v.modal("rename-modal", content, None));
        }
        if let Some((request, id, error)) = &self.directory_fallback
            && let Some(workspace) = self.fallback_workspace(request, *id)
        {
            let detail: String = error.chars().take(400).collect();
            let content = column()
                .gap(px(14.))
                .child(v.text("Could not create from this pane", 14.0, Tone::Text))
                .child(v.text(detail, 11.0, Tone::Muted))
                .child(v.text(
                    format!(
                        "Try the starting directory of workspace \"{}\"?\n{}",
                        workspace.name, workspace.cwd
                    ),
                    11.0,
                    Tone::Muted,
                ))
                .child(buttons(vec![
                    v.label_button(
                        "fallback-cancel",
                        "Cancel",
                        Look::Quiet,
                        Some(Message::CancelWorkspaceDirectory),
                    )
                    .into_any_element(),
                    v.label_button(
                        "fallback-use",
                        "Use workspace directory",
                        Look::Primary,
                        Some(Message::UseWorkspaceDirectory),
                    )
                    .into_any_element(),
                ]));
            layers.push(v.modal("fallback-modal", content, None));
        }
        if let Some(confirm) = self.confirm {
            let Confirm::Workspace(id) = confirm;
            let title = format!(
                "Close \"{}\"?",
                self.snapshot
                    .as_ref()
                    .and_then(|s| s.workspaces.iter().find(|w| w.id == id))
                    .map_or("workspace", |w| w.name.as_str())
            );
            let content = column()
                .gap(px(14.))
                .child(v.text(title, 14.0, Tone::Text))
                .child(v.text(
                    "Every shell and running command in this workspace will be terminated.",
                    11.0,
                    Tone::Muted,
                ))
                .child(v.text(
                    "Closing the window instead keeps your sessions running.",
                    10.0,
                    Tone::Muted,
                ))
                .child(buttons(vec![
                    v.label_button(
                        "confirm-cancel",
                        "Cancel",
                        Look::Quiet,
                        Some(Message::CancelClose),
                    )
                    .into_any_element(),
                    v.label_button(
                        "confirm-close",
                        "Close permanently",
                        Look::Destructive,
                        Some(Message::ConfirmClose),
                    )
                    .into_any_element(),
                ]));
            layers.push(v.modal("confirm-modal", content, None));
        }
        if let Some(content) = self.install_dialog(v) {
            layers.push(v.modal("install-modal", content, None));
        }
        if let Some(content) = self.update_dialog(v) {
            layers.push(v.modal("update-modal", content, None));
        }
        layers
    }

    /// The menu of the header's "more" button.
    fn workspace_menu(&self, v: &View) -> impl IntoElement {
        let item = |id: &'static str, icon: Icon, label: &'static str, message: Message| {
            v.menu_item(id, icon, label, Some(message), Look::Quiet)
        };
        let danger = |id: &'static str, icon: Icon, label: &'static str, message: Message| {
            v.menu_item(id, icon, label, Some(message), Look::Destructive)
        };
        v.panel()
            .id("workspace-menu")
            .occlude()
            .w(px(200.))
            .p(px(5.))
            .child(
                column()
                    .gap(px(2.))
                    .child(item(
                        "menu-split-columns",
                        Icon::SplitColumns,
                        "Split columns",
                        Message::Run(Command::Split(Axis::Vertical)),
                    ))
                    .child(item(
                        "menu-split-rows",
                        Icon::SplitRows,
                        "Split rows",
                        Message::Run(Command::Split(Axis::Horizontal)),
                    ))
                    .child(item(
                        "menu-zoom",
                        Icon::Zoom,
                        if self.zoomed.is_some() {
                            "Restore pane"
                        } else {
                            "Zoom pane"
                        },
                        Message::Run(Command::TogglePaneZoom),
                    ))
                    .child(v.separator())
                    .child(item(
                        "menu-rename-workspace",
                        Icon::Pencil,
                        "Rename workspace",
                        Message::Run(Command::RenameWorkspace),
                    ))
                    .child(item(
                        "menu-rename-tab",
                        Icon::Pencil,
                        "Rename tab",
                        Message::Run(Command::RenameTab),
                    ))
                    .child(item(
                        "menu-lock-pane",
                        Icon::Lock,
                        if self.pane_locked(self.pane.unwrap_or_default()) {
                            "Unlock pane"
                        } else {
                            "Lock pane"
                        },
                        Message::TogglePaneLock(self.pane.unwrap_or_default()),
                    ))
                    .child(item(
                        "menu-lock-tab",
                        Icon::Lock,
                        if self.tab_locked(self.tab.unwrap_or_default()) {
                            "Unlock tab"
                        } else {
                            "Lock tab"
                        },
                        Message::ToggleTabLock(self.tab.unwrap_or_default()),
                    ))
                    .child(v.separator())
                    .child(danger(
                        "menu-close-pane",
                        Icon::Close,
                        "Close pane",
                        Message::Run(Command::ClosePane),
                    ))
                    .child(danger(
                        "menu-close-tab",
                        Icon::Close,
                        "Close tab",
                        Message::AskCloseTab(self.tab.unwrap_or_default()),
                    ))
                    .child(danger(
                        "menu-close-workspace",
                        Icon::Close,
                        "Close workspace",
                        Message::Run(Command::CloseWorkspace),
                    ))
                    .child(v.separator())
                    .child(item(
                        "menu-palette",
                        Icon::Search,
                        "Command palette…",
                        Message::Run(Command::Palette),
                    ))
                    .child(item(
                        "menu-settings",
                        Icon::Settings,
                        "Settings…",
                        Message::Run(Command::Settings),
                    ))
                    .child(item(
                        "menu-quit",
                        Icon::Close,
                        "Quit Spooky",
                        Message::Run(Command::Quit),
                    )),
            )
    }

    /// The popover that shows the full directory of the selected pane.
    fn directory_popover(
        &self,
        v: &View,
        kind: spooky_core::DirectoryKind,
        path: &str,
    ) -> impl IntoElement {
        let kind = match kind {
            spooky_core::DirectoryKind::Shell => "Recent directory",
            spooky_core::DirectoryKind::AgentStart => "Agent starting directory",
        };
        let location = if self.connection.is_remote() {
            format!(
                "SSH: {}",
                super::directory::full_path(self.connection.connected_target())
            )
        } else {
            "This computer".into()
        };
        let mut items = column()
            .gap(px(6.))
            .child(v.text(kind, 12.0, Tone::Text))
            .child(v.text(location, 10.0, Tone::Muted))
            .child(
                div()
                    .id("directory-path")
                    .max_h(px(90.))
                    .overflow_y_scroll()
                    .child(v.text(super::directory::full_path(path), 11.0, Tone::Text)),
            )
            .child(v.separator())
            .child(v.menu_item(
                "directory-copy",
                Icon::Copy,
                "Copy path",
                Some(Message::CopyDirectory),
                Look::Quiet,
            ));
        if !self.connection.is_remote() && cfg!(any(target_os = "macos", target_os = "linux")) {
            items = items.child(v.menu_item(
                "directory-open",
                Icon::Folder,
                if cfg!(target_os = "macos") {
                    "Open in Finder"
                } else {
                    "Open folder"
                },
                (!self.directory_opening).then_some(Message::OpenDirectory),
                Look::Quiet,
            ));
        }
        v.panel()
            .id("directory-popover")
            .occlude()
            .w(px(400.))
            .p(px(10.))
            .child(items)
    }

    /// What a context menu offers for what it was opened on.
    fn context_items(&self, v: &View, target: ContextTarget) -> AnyElement {
        let item = |id: &'static str, icon: Icon, label: &'static str, message: Message| {
            v.menu_item(id, icon, label, Some(message), Look::Quiet)
        };
        let danger = |id: &'static str, icon: Icon, label: &'static str, message: Message| {
            v.menu_item(id, icon, label, Some(message), Look::Destructive)
        };
        let items = match target {
            ContextTarget::Workspace(id) => column()
                .child(item(
                    "ctx-rename-workspace",
                    Icon::Pencil,
                    "Rename workspace",
                    Message::RenameWorkspace(id),
                ))
                .child(v.separator())
                .child(danger(
                    "ctx-close-workspace",
                    Icon::Close,
                    "Close workspace",
                    Message::CloseWorkspace(id),
                )),
            ContextTarget::Tab(id) => column()
                .child(item(
                    "ctx-rename-tab",
                    Icon::Pencil,
                    "Rename tab",
                    Message::RenameTab(id),
                ))
                .child(item(
                    "ctx-new-tab",
                    Icon::Plus,
                    "New tab",
                    Message::Run(Command::NewTab),
                ))
                .child(item(
                    "ctx-lock-tab",
                    Icon::Lock,
                    if self.tab_locked(id) {
                        "Unlock tab"
                    } else {
                        "Lock tab"
                    },
                    Message::ToggleTabLock(id),
                ))
                .child(item(
                    "ctx-new-agent",
                    Icon::Agent,
                    "New agent",
                    Message::Run(Command::NewAgent),
                ))
                .child(v.separator())
                .child(danger(
                    "ctx-close-tab",
                    Icon::Close,
                    "Close tab",
                    Message::AskCloseTab(id),
                )),
            ContextTarget::Pane(id) => {
                let can_copy = self.selected_text().is_some()
                    || self
                        .selection
                        .is_some_and(|selection| selection.pane == id && selection.all);
                column()
                    .child(v.menu_item(
                        "ctx-copy",
                        Icon::Copy,
                        "Copy",
                        can_copy.then_some(Message::Run(Command::Copy)),
                        Look::Quiet,
                    ))
                    .child(item(
                        "ctx-paste",
                        Icon::Paste,
                        "Paste",
                        Message::Run(Command::Paste),
                    ))
                    .child(item(
                        "ctx-select-all",
                        Icon::Copy,
                        "Select all",
                        Message::Run(Command::SelectAll),
                    ))
                    .child(item(
                        "ctx-copy-all",
                        Icon::Copy,
                        "Copy all",
                        Message::Run(Command::CopyAll),
                    ))
                    .child(item(
                        "ctx-lock-pane",
                        Icon::Lock,
                        if self.pane_locked(id) {
                            "Unlock pane"
                        } else {
                            "Lock pane"
                        },
                        Message::TogglePaneLock(id),
                    ))
                    .child(item(
                        "ctx-clear",
                        Icon::Close,
                        "Clear terminal",
                        Message::Run(Command::ClearViewport),
                    ))
                    .child(item(
                        "ctx-find",
                        Icon::Search,
                        "Find in scrollback",
                        Message::Run(Command::Find),
                    ))
                    .child(item(
                        "ctx-new-tab",
                        Icon::Plus,
                        "New tab",
                        Message::Run(Command::NewTab),
                    ))
                    .child(item(
                        "ctx-new-agent-here",
                        Icon::Agent,
                        "New agent here",
                        Message::Run(Command::NewAgentHere),
                    ))
                    .child(v.separator())
                    .child(item(
                        "ctx-split-columns",
                        Icon::SplitColumns,
                        "Split columns",
                        Message::SplitPane(id, Axis::Vertical),
                    ))
                    .child(item(
                        "ctx-split-rows",
                        Icon::SplitRows,
                        "Split rows",
                        Message::SplitPane(id, Axis::Horizontal),
                    ))
                    .child(item(
                        "ctx-zoom",
                        Icon::Zoom,
                        if self.zoomed.is_some_and(|(_, pane)| pane == id) {
                            "Restore pane"
                        } else {
                            "Zoom pane"
                        },
                        Message::TogglePaneZoom(id),
                    ))
                    .child(v.separator())
                    .child(danger(
                        "ctx-close-pane",
                        Icon::Close,
                        "Close pane",
                        Message::AskClosePaneOf(id),
                    ))
            }
        };
        v.panel()
            .id("context-menu")
            .occlude()
            .w(px(200.))
            .p(px(5.))
            .child(items.gap(px(2.)))
            .into_any_element()
    }
}

/// A pane the snapshot no longer has.
fn unavailable(v: &View) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(v.text("Pane unavailable", 11.0, Tone::Muted))
        .into_any_element()
}

/// A bar filled `share` of the way (0 to 1).
fn progress_bar(v: &View, share: f32) -> Div {
    div()
        .w_full()
        .h(px(6.))
        .rounded(px(3.))
        .bg(hsla(v.p.border))
        .child(
            div()
                .h_full()
                .w(gpui::relative(share.clamp(0.0, 1.0)))
                .rounded(px(3.))
                .bg(hsla(v.p.accent)),
        )
}
