//! Compact native workspace chrome; terminal state remains server-owned.
use super::{App, Confirm, ContextTarget, Message};
use crate::commands::Command;

/// Height of the invisible strip at the window's bottom edge that drags its height.
const RESIZE_GRIP: f32 = 5.0;
use crate::{
    features::{
        activity::Indicator,
        agent,
        splits::{Divider, THICKNESS},
    },
    terminal,
    ui::*,
};
use iced::{
    Alignment, Border, Color, Element, Length, Padding, Point, Rectangle, Vector, mouse,
    widget::{
        Space, button, canvas, column, container, float, image, mouse_area, opaque, row,
        scrollable, stack, text, text_input, tooltip,
    },
};
use spooky_core::{Axis, Layout};
use std::{sync::LazyLock, time::Duration};

/// Padding around each terminal pane. The divider geometry accounts for it.
pub(crate) const PANE_PADDING_X: f32 = 9.0;
pub(crate) const PANE_PADDING_Y: f32 = 7.0;
const SIDEBAR_WIDTH: f32 = 232.0;
const HEADER_HEIGHT: f32 = 36.0;
/// Width of a tab's close button plus the gap before it: a tab that is not selected
/// keeps this much room so selecting tabs never moves or resizes the others.
const TAB_CLOSE_SLOT: f32 = 23.0;

fn brand_logo() -> Element<'static, Message> {
    static LOGO: LazyLock<image::Handle> = LazyLock::new(|| {
        image::Handle::from_bytes(include_bytes!("../../../../assets/logo-256.png").as_slice())
    });
    image(LOGO.clone())
        .width(18)
        .height(18)
        .border_radius(3)
        .into()
}

fn activity_marker(indicator: Option<Indicator>) -> Element<'static, Message> {
    let Some(indicator) = indicator else {
        return Space::new().width(16).into();
    };
    let (symbol, tone, label) = match indicator {
        Indicator::Output => ("●", TextTone::Accent, "New terminal output"),
        Indicator::AgentDone => ("✓", TextTone::Success, "Agent response finished"),
        Indicator::Attention => ("!", TextTone::Warning, "Agent needs your answer"),
    };
    tooltip(
        container(text(symbol).size(16).style(text_tone(tone)))
            .width(16)
            .center_x(16),
        text(label).size(10),
        tooltip::Position::Bottom,
    )
    .delay(Duration::from_millis(450))
    .into()
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

/// A transparent canvas that turns a right-click into a message carrying the
/// click position. Mouse motion publishes nothing, so it costs no updates.
struct RightClick<F>(F);

impl<F: Fn(Point) -> Message> canvas::Program<Message> for RightClick<F> {
    type State = ();

    fn update(
        &self,
        _: &mut (),
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if matches!(
            event,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))
        ) && cursor.is_over(bounds)
        {
            Some(canvas::Action::publish((self.0)(cursor.position()?)).and_capture())
        } else {
            None
        }
    }

    fn draw(
        &self,
        _: &(),
        _: &iced::Renderer,
        _: &iced::Theme,
        _: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        Vec::new()
    }
}

fn menu_style(theme: &iced::Theme) -> container::Style {
    let colors = color_palette_for(theme);
    container::Style {
        border: Border {
            color: colors.border,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..surface(colors.sidebar)
    }
}

fn menu_item(
    kind: Icon,
    label: &'static str,
    message: Message,
) -> button::Button<'static, Message> {
    button(
        row![themed_icon(kind, TextTone::Muted), text(label).size(11)]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .padding([6, 8])
    .width(Length::Fill)
    .style(quiet)
    .on_press(message)
}

fn modal<'a>(
    body: Element<'a, Message>,
    content: Element<'a, Message>,
    dismiss: Option<Message>,
) -> Element<'a, Message> {
    let panel = container(content).padding(20).width(380).style(|theme| {
        let colors = color_palette_for(theme);
        container::Style {
            border: Border {
                color: colors.border,
                width: 1.0,
                radius: 7.0.into(),
            },
            ..surface(colors.sidebar)
        }
    });
    let area = mouse_area(
        container(opaque(panel))
            .center(Length::Fill)
            .style(|_| surface(Color::BLACK.scale_alpha(0.45))),
    );
    let area = if let Some(message) = dismiss {
        area.on_press(message)
    } else {
        area
    };
    stack![body, opaque(area)].into()
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
    fn header(&self) -> Element<'_, Message> {
        // The native macOS traffic lights occupy this part of the unified titlebar.
        let inset = if cfg!(target_os = "macos") { 72 } else { 0 };
        let mut brand = row![Space::new().width(inset)]
            .align_y(Alignment::Center)
            .spacing(6);
        if self.sidebar_open {
            brand = brand
                .push(brand_logo())
                .push(text("Spooky").size(11).style(text_tone(TextTone::Text)))
                .push(Space::new().width(Length::Fill));
        }
        brand = brand.push(action(
            Icon::Sidebar,
            "Toggle sidebar",
            Message::Run(Command::ToggleSidebar),
        ));
        let brand = container(brand)
            .padding([0, 7])
            .height(HEADER_HEIGHT)
            .align_y(Alignment::Center)
            .width(if self.sidebar_open {
                SIDEBAR_WIDTH
            } else {
                inset as f32 + 42.0
            });
        let mut tabs = row![].spacing(3).align_y(Alignment::Center);
        if let Some((workspace, _)) = self.active() {
            for tab in &workspace.tabs {
                let selected = self.tab == Some(tab.id) && !self.settings.is_shown();
                let id = tab.id;
                let label = button(
                    row![
                        themed_icon(
                            if self.tab_is_agent(tab) {
                                Icon::Agent
                            } else {
                                Icon::Terminal
                            },
                            if selected {
                                TextTone::Text
                            } else {
                                TextTone::Muted
                            },
                        ),
                        text(self.tab_label(tab)).size(11),
                        activity_marker(self.activity.indicator(&tab.layout)),
                        if tab.locked || !tab.locked_panes.is_empty() {
                            themed_icon(Icon::Lock, TextTone::Muted)
                        } else {
                            Space::new().width(0).into()
                        }
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding(Padding {
                    top: 5.0,
                    right: if selected { 9.0 } else { 9.0 + TAB_CLOSE_SLOT },
                    bottom: 5.0,
                    left: 9.0,
                })
                .style(move |t, s| selectable(selected, t, s))
                .on_press(Message::Tab(id));
                // Middle-click closes any tab and right-click opens its menu.
                let entry = if selected {
                    row![
                        label,
                        action(Icon::Close, "Close tab", Message::AskCloseTab(id))
                    ]
                    .spacing(1)
                    .align_y(Alignment::Center)
                } else {
                    row![label].align_y(Alignment::Center)
                };
                tabs = tabs.push(
                    mouse_area(stack![
                        entry,
                        canvas(RightClick(move |at| Message::ContextMenu(
                            ContextTarget::Tab(id),
                            at
                        )))
                        .width(Length::Fill)
                        .height(Length::Fill),
                    ])
                    .on_middle_press(Message::AskCloseTab(id)),
                );
            }
        }
        // Settings is a tab of its own: it stays after the terminals' tabs until closed,
        // and choosing another tab leaves it without closing it.
        if self.settings.is_open() {
            let selected = self.settings.is_shown();
            let label = button(
                row![
                    themed_icon(
                        Icon::Settings,
                        if selected {
                            TextTone::Text
                        } else {
                            TextTone::Muted
                        },
                    ),
                    text("Settings").size(11)
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding(Padding {
                top: 5.0,
                right: if selected { 9.0 } else { 9.0 + TAB_CLOSE_SLOT },
                bottom: 5.0,
                left: 9.0,
            })
            .style(move |t, s| selectable(selected, t, s))
            .on_press(Message::Settings(crate::features::settings::Message::Show));
            let close = Message::Settings(crate::features::settings::Message::Close);
            let entry = if selected {
                row![label, action(Icon::Close, "Close settings", close.clone())]
                    .spacing(1)
                    .align_y(Alignment::Center)
            } else {
                row![label].align_y(Alignment::Center)
            };
            tabs = tabs.push(mouse_area(entry).on_middle_press(close));
        }
        let directory = tooltip(
            button(themed_icon(Icon::Folder, TextTone::Muted))
                .padding(5)
                .style(quiet)
                .on_press_maybe(
                    self.directory
                        .current()
                        .map(|_| Message::ToggleDirectoryMenu),
                ),
            container(text(self.directory.text(self.connection.is_remote())).size(11))
                .padding([5, 8])
                .style(menu_style),
            tooltip::Position::Bottom,
        )
        .delay(Duration::from_millis(450));
        let status = self.header_status();
        let controls = row![
            action(
                Icon::SplitColumns,
                SPLIT_COLUMNS,
                Message::Run(Command::Split(Axis::Vertical))
            ),
            action(
                Icon::SplitRows,
                SPLIT_ROWS,
                Message::Run(Command::Split(Axis::Horizontal))
            ),
            action(Icon::Plus, NEW_TAB, Message::Run(Command::NewTab)),
            directory,
            action(Icon::More, "Workspace actions", Message::ToggleMenu),
        ]
        .spacing(3)
        .align_y(Alignment::Center);
        let chrome =
            self.appearance_colors()
                .sidebar
                .scale_alpha(if self.background_opacity() < 1.0 {
                    0.55
                } else {
                    1.0
                });
        let header = container(
            row![
                brand,
                container(Space::new().width(1))
                    .height(Length::Fill)
                    .style(|theme| surface(color_palette_for(theme).border)),
                container(
                    scrollable(tabs)
                        .direction(scrollable::Direction::Horizontal(Default::default()))
                )
                .padding([0, 7])
                .width(Length::Fill)
                .align_y(Alignment::Center),
                container(
                    text(status)
                        .size(10)
                        .style(text_tone(TextTone::Muted))
                        .wrapping(text::Wrapping::None),
                )
                .max_width(220)
                .clip(true),
                controls,
                Space::new().width(5),
            ]
            .align_y(Alignment::Center),
        )
        .height(HEADER_HEIGHT)
        .style(move |_| surface(chrome));
        mouse_area(header).on_press(Message::DragWindow).into()
    }

    fn sidebar(&self) -> Element<'_, Message> {
        let connection_name = if !self.connection.is_remote() {
            "This computer"
        } else {
            self.connection.connected_target()
        };
        let palette = self.appearance_colors();
        let section = container(text("Spaces").size(10).style(text_tone(TextTone::Muted)))
            .height(22)
            .align_y(Alignment::Center);
        let machine = row![
            button(
                row![
                    themed_icon(Icon::Server, TextTone::Muted),
                    text(connection_name).size(10).width(Length::Fill),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([5, 6])
            .style(quiet)
            .on_press(Message::ToggleWorkspaces),
            action(
                Icon::Plus,
                "New workspace",
                Message::Run(Command::NewWorkspace)
            ),
            button(
                row![
                    dot(if self.online() {
                        palette.success
                    } else {
                        palette.muted
                    }),
                    themed_icon(
                        if self.workspaces_open {
                            Icon::Chevron
                        } else {
                            Icon::ChevronRight
                        },
                        TextTone::Muted,
                    ),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([5, 6])
            .style(quiet)
            .on_press(Message::ToggleWorkspaces),
        ]
        .align_y(Alignment::Center);
        let mut spaces = column![].spacing(2);
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
                let badge = container(icon(Icon::Folder, color))
                    .padding(2)
                    .style(move |_| container::Style {
                        border: Border {
                            radius: 3.0.into(),
                            ..Default::default()
                        },
                        ..surface(color.scale_alpha(0.14))
                    });
                let item = button(
                    row![
                        badge,
                        text(&workspace.name).size(11).style(text_tone(if selected {
                            TextTone::Text
                        } else {
                            TextTone::Muted
                        }))
                    ]
                    .spacing(7)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([5, 6])
                .style(move |t, s| selectable(selected, t, s))
                .on_press(Message::Workspace(workspace.id));
                spaces = spaces.push(
                    tooltip(
                        stack![
                            item,
                            canvas(RightClick(move |at| Message::ContextMenu(
                                ContextTarget::Workspace(workspace_id),
                                at
                            )))
                            .width(Length::Fill)
                            .height(Length::Fill),
                        ],
                        text(&workspace.cwd).size(10),
                        tooltip::Position::Right,
                    )
                    .delay(Duration::from_millis(500)),
                );
            }
        }
        let mut connection = column![].spacing(7);
        if self.connection_open {
            connection = connection
                .push(
                    text("Connect via SSH")
                        .size(10)
                        .style(text_tone(TextTone::Text)),
                )
                .push(
                    button(text("Browse Tailscale devices").size(10))
                        .padding([5, 8])
                        .style(quiet)
                        .on_press(Message::Tailscale(
                            crate::features::tailscale::Message::Open,
                        )),
                )
                .push(
                    text_input("user@host or SSH alias", self.connection.target())
                        .size(11)
                        .padding(6)
                        .on_input(Message::TargetValue)
                        .on_submit(Message::Connect),
                )
                .push(
                    row![
                        button(text("Connect").size(10))
                            .padding([5, 8])
                            .style(primary)
                            .on_press(Message::Connect),
                        button(text("Local").size(10))
                            .padding([5, 8])
                            .style(quiet)
                            .on_press(Message::Local),
                        Space::new().width(Length::Fill),
                        action(
                            Icon::Close,
                            "Close connection editor",
                            Message::ToggleConnection
                        ),
                    ]
                    .spacing(3)
                    .align_y(Alignment::Center),
                );
        } else {
            connection = connection.push(menu_item(
                Icon::Plus,
                "Connect machine",
                Message::ToggleConnection,
            ));
        }
        let chrome =
            self.appearance_colors()
                .sidebar
                .scale_alpha(if self.background_opacity() < 1.0 {
                    0.55
                } else {
                    1.0
                });
        container(column![
            container(column![section, machine].spacing(2)).padding([4, 6]),
            container(scrollable(spaces))
                .padding([0, 6])
                .height(Length::Fill),
            separator(),
            container(connection).padding(8),
        ])
        .width(SIDEBAR_WIDTH)
        .height(Length::Fill)
        .style(move |_| surface(chrome))
        .into()
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

    fn terminal_layout<'a>(&'a self, layout: &'a Layout) -> Element<'a, Message> {
        match layout {
            Layout::Pane(id) => {
                let Some(pane) = self
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.panes.iter().find(|p| p.id == *id))
                else {
                    return container(
                        text("Pane unavailable")
                            .size(11)
                            .style(text_tone(TextTone::Muted)),
                    )
                    .center(Length::Fill)
                    .into();
                };
                if let Some(conversation) = &pane.agent {
                    let id = *id;
                    if self.web_target() == Some(id) {
                        return super::web::area();
                    }
                    let can_zoom = self
                        .active()
                        .is_some_and(|(_, tab)| matches!(tab.layout, Layout::Split { .. }));
                    return mouse_area(agent::view(
                        &self.agents,
                        id,
                        conversation,
                        can_zoom,
                        self.theme(),
                    ))
                    .on_press(Message::FocusPane(id, Vec::new()))
                    .into();
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
                stack![
                    container(
                        canvas(terminal::Terminal {
                            pane,
                            font_size: self.config.font_size,
                            font: self.font,
                            focused: self.terminal_focused && selected,
                            selection,
                            cursor_on: !(selected && self.blink_wanted()) || self.blink_on,
                        })
                        .width(Length::Fill)
                        .height(Length::Fill),
                    )
                    .padding([PANE_PADDING_Y, PANE_PADDING_X])
                    .width(Length::Fill)
                    .height(Length::Fill),
                    if pane.scrolled > 0 && pane.history > 0 {
                        Element::from(
                            container(
                                tooltip(
                                    canvas(terminal::ScrollIndicator { pane })
                                        .width(4)
                                        .height(Length::Fill),
                                    text(format!("{} of {} lines", pane.scrolled, pane.history))
                                        .size(10),
                                    tooltip::Position::Left,
                                )
                                .delay(Duration::from_millis(450)),
                            )
                            .align_right(Length::Fill)
                            .height(Length::Fill),
                        )
                    } else {
                        Element::from(Space::new())
                    }
                ]
                .into()
            }
            Layout::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => {
                // A divider being dragged shows where it is now, not where the
                // server last saw it.
                let ratio = self.splits.dragged_ratio(*id).unwrap_or(*ratio);
                let side = |layout, share: u16| {
                    let (width, height) = match axis {
                        Axis::Horizontal => (Length::Fill, Length::FillPortion(share)),
                        Axis::Vertical => (Length::FillPortion(share), Length::Fill),
                    };
                    container(self.terminal_layout(layout))
                        .width(width)
                        .height(height)
                };
                let divider = canvas(Divider {
                    split: *id,
                    axis: *axis,
                });
                match axis {
                    Axis::Horizontal => column![
                        side(first, ratio),
                        divider.width(Length::Fill).height(THICKNESS),
                        side(second, 1000 - ratio)
                    ]
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                    Axis::Vertical => row![
                        side(first, ratio),
                        divider.width(THICKNESS).height(Length::Fill),
                        side(second, 1000 - ratio)
                    ]
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                }
            }
        }
    }

    /// Shown when no workspace is selectable: before the first snapshot, or after
    /// the last workspace was closed.
    fn empty_state(&self) -> Element<'_, Message> {
        if let Some((reason, retrying)) = self.connection.unreachable() {
            return self.unreachable_state(reason, retrying);
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|s| s.workspaces.is_empty())
        {
            container(
                column![
                    text("No workspaces")
                        .size(12)
                        .style(text_tone(TextTone::Muted)),
                    button(text("New workspace").size(11))
                        .padding([6, 10])
                        .style(primary)
                        .on_press(Message::Run(Command::NewWorkspace)),
                ]
                .spacing(10)
                .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else if self
            .active_workspace()
            .is_some_and(|workspace| workspace.tabs.is_empty())
        {
            container(
                column![
                    text("No tabs in this workspace")
                        .size(12)
                        .style(text_tone(TextTone::Muted)),
                    button(text("New tab").size(11))
                        .padding([6, 10])
                        .style(primary)
                        .on_press(Message::Run(Command::NewTab)),
                ]
                .spacing(10)
                .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else {
            container(
                text("Waiting for session…")
                    .size(12)
                    .style(text_tone(TextTone::Muted)),
            )
            .center(Length::Fill)
            .into()
        }
    }

    /// Shown while a target cannot be reached: what failed, and the way out. The
    /// way back to this computer is always offered for a remote target.
    fn unreachable_state(&self, reason: &str, retrying: bool) -> Element<'_, Message> {
        let remote = self.connection.is_remote();
        let title = if remote {
            format!("Cannot connect to {}", self.connection.connected_target())
        } else {
            "Cannot reach the local server".to_owned()
        };
        let mut actions = row![
            button(text("Retry").size(11))
                .padding([6, 10])
                .style(primary)
                .on_press(Message::Retry)
        ]
        .spacing(6);
        if remote {
            actions = actions.push(
                button(text("Use this computer").size(11))
                    .padding([6, 10])
                    .style(quiet)
                    .on_press(Message::Local),
            );
        }
        let mut card = column![
            text(title).size(13),
            text(reason.to_owned())
                .size(11)
                .style(text_tone(TextTone::Muted)),
        ]
        .spacing(8)
        .max_width(520)
        .align_x(Alignment::Center);
        if retrying {
            card = card.push(
                text("Retrying automatically…")
                    .size(10)
                    .style(text_tone(TextTone::Muted)),
            );
        }
        container(card.push(actions).spacing(10))
            .center(Length::Fill)
            .into()
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

    pub(crate) fn view(&self) -> Element<'_, Message> {
        let background = self
            .appearance_colors()
            .background
            .scale_alpha(self.background_opacity());
        let screen = container(self.screen())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| surface(background));
        match self.slide {
            // The content is moved inside the window, which clips it at its top edge.
            Some(slide) => float(screen)
                .translate(move |bounds, _| Vector::new(0.0, -slide.hidden_share() * bounds.height))
                .into(),
            None => screen.into(),
        }
    }

    fn screen(&self) -> Element<'_, Message> {
        let content = if self.settings.is_shown() {
            crate::features::settings::view(
                &self.settings,
                &self.config,
                &self.themes,
                self.hotkeys.is_some(),
                self.dropdown(),
                self.glass_available,
                self.reduce_transparency,
            )
        } else {
            self.active()
                .map(|(_, tab)| {
                    let pane = self
                        .zoomed
                        .and_then(|(id, pane)| (id == tab.id).then_some(pane));
                    self.terminal_layout(crate::layout::visible_layout(&tab.layout, pane))
                })
                .unwrap_or_else(|| self.empty_state())
        };

        let mut main = column![].width(Length::Fill).height(Length::Fill);
        if !self.settings.is_shown()
            && let Some(bar) = crate::features::search::view(&self.search)
        {
            main = main.push(bar).push(separator());
        }
        let main = main.push(container(content).width(Length::Fill).height(Length::Fill));
        let body: Element<'_, Message> = if self.sidebar_open {
            row![
                self.sidebar(),
                container(Space::new().width(1))
                    .height(Length::Fill)
                    .style(|theme| surface(color_palette_for(theme).border)),
                main
            ]
            .height(Length::Fill)
            .into()
        } else {
            main.into()
        };
        let mut body: Element<'_, Message> = column![self.header(), separator(), body]
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        if self.menu_open {
            let menu = container(
                column![
                    menu_item(
                        Icon::SplitColumns,
                        "Split columns",
                        Message::Run(Command::Split(Axis::Vertical))
                    ),
                    menu_item(
                        Icon::SplitRows,
                        "Split rows",
                        Message::Run(Command::Split(Axis::Horizontal))
                    ),
                    menu_item(
                        Icon::Zoom,
                        if self.zoomed.is_some() {
                            "Restore pane"
                        } else {
                            "Zoom pane"
                        },
                        Message::Run(Command::TogglePaneZoom)
                    ),
                    separator(),
                    menu_item(
                        Icon::Pencil,
                        "Rename workspace",
                        Message::Run(Command::RenameWorkspace)
                    ),
                    menu_item(Icon::Pencil, "Rename tab", Message::Run(Command::RenameTab)),
                    menu_item(
                        Icon::Lock,
                        if self.pane_locked(self.pane.unwrap_or_default()) {
                            "Unlock pane"
                        } else {
                            "Lock pane"
                        },
                        Message::TogglePaneLock(self.pane.unwrap_or_default())
                    ),
                    menu_item(
                        Icon::Lock,
                        if self.tab_locked(self.tab.unwrap_or_default()) {
                            "Unlock tab"
                        } else {
                            "Lock tab"
                        },
                        Message::ToggleTabLock(self.tab.unwrap_or_default())
                    ),
                    separator(),
                    menu_item(Icon::Close, "Close pane", Message::Run(Command::ClosePane))
                        .style(destructive),
                    menu_item(
                        Icon::Close,
                        "Close tab",
                        Message::AskCloseTab(self.tab.unwrap_or_default())
                    )
                    .style(destructive),
                    menu_item(
                        Icon::Close,
                        "Close workspace",
                        Message::Run(Command::CloseWorkspace)
                    )
                    .style(destructive),
                    separator(),
                    menu_item(
                        Icon::Search,
                        "Command palette…",
                        Message::Run(Command::Palette)
                    ),
                    menu_item(Icon::Settings, "Settings…", Message::Run(Command::Settings)),
                    menu_item(Icon::Close, "Quit Spooky", Message::Run(Command::Quit)),
                ]
                .spacing(2),
            )
            .padding(5)
            .width(200)
            .style(menu_style);
            body = stack![
                body,
                opaque(
                    mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
                        .on_press(Message::ToggleMenu)
                ),
                container(opaque(menu))
                    .align_right(Length::Fill)
                    .padding(Padding {
                        top: HEADER_HEIGHT + 4.0,
                        right: 6.0,
                        bottom: 0.0,
                        left: 0.0,
                    }),
            ]
            .into();
        }
        if let Some((selection, path)) = &self.directory_menu {
            let kind = match selection.kind {
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
            let mut items = column![
                text(kind).size(12).style(text_tone(TextTone::Text)),
                text(location).size(10).style(text_tone(TextTone::Muted)),
                container(
                    scrollable(
                        text(super::directory::full_path(path))
                            .size(11)
                            .style(text_tone(TextTone::Text))
                            .wrapping(text::Wrapping::WordOrGlyph),
                    )
                    .height(Length::Shrink),
                )
                .max_height(90),
                separator(),
                menu_item(Icon::Copy, "Copy path", Message::CopyDirectory),
            ]
            .spacing(6);
            if !self.connection.is_remote() && cfg!(any(target_os = "macos", target_os = "linux")) {
                items = items.push(
                    menu_item(
                        Icon::Folder,
                        if cfg!(target_os = "macos") {
                            "Open in Finder"
                        } else {
                            "Open folder"
                        },
                        Message::OpenDirectory,
                    )
                    .on_press_maybe((!self.directory_opening).then_some(Message::OpenDirectory)),
                );
            }
            let menu = container(items).padding(10).width(400).style(menu_style);
            body = stack![
                body,
                opaque(
                    mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
                        .on_press(Message::ToggleDirectoryMenu)
                        .on_right_press(Message::ToggleDirectoryMenu)
                ),
                container(opaque(menu))
                    .align_right(Length::Fill)
                    .align_top(Length::Fill)
                    .padding(Padding {
                        top: HEADER_HEIGHT + 4.0,
                        right: 6.0,
                        bottom: 0.0,
                        left: 6.0
                    }),
            ]
            .into();
        }
        if let Some((target, position)) = self.context_menu {
            let items = match target {
                ContextTarget::Workspace(id) => column![
                    menu_item(
                        Icon::Pencil,
                        "Rename workspace",
                        Message::RenameWorkspace(id)
                    ),
                    separator(),
                    menu_item(Icon::Close, "Close workspace", Message::CloseWorkspace(id))
                        .style(destructive),
                ],
                ContextTarget::Tab(id) => column![
                    menu_item(Icon::Pencil, "Rename tab", Message::RenameTab(id)),
                    menu_item(Icon::Plus, "New tab", Message::Run(Command::NewTab)),
                    menu_item(
                        Icon::Lock,
                        if self.tab_locked(id) {
                            "Unlock tab"
                        } else {
                            "Lock tab"
                        },
                        Message::ToggleTabLock(id)
                    ),
                    menu_item(Icon::Agent, "New agent", Message::Run(Command::NewAgent)),
                    separator(),
                    menu_item(Icon::Close, "Close tab", Message::AskCloseTab(id))
                        .style(destructive),
                ],
                ContextTarget::Pane(id) => {
                    let mut copy = menu_item(Icon::Copy, "Copy", Message::Run(Command::Copy));
                    if self.selected_text().is_none()
                        && !self
                            .selection
                            .is_some_and(|selection| selection.pane == id && selection.all)
                    {
                        copy = copy.on_press_maybe(None);
                    }
                    column![
                        copy,
                        menu_item(Icon::Paste, "Paste", Message::Run(Command::Paste)),
                        menu_item(Icon::Copy, "Select all", Message::Run(Command::SelectAll)),
                        menu_item(Icon::Copy, "Copy all", Message::Run(Command::CopyAll)),
                        menu_item(
                            Icon::Lock,
                            if self.pane_locked(id) {
                                "Unlock pane"
                            } else {
                                "Lock pane"
                            },
                            Message::TogglePaneLock(id)
                        ),
                        menu_item(
                            Icon::Close,
                            "Clear terminal",
                            Message::Run(Command::ClearViewport)
                        ),
                        menu_item(
                            Icon::Search,
                            "Find in scrollback",
                            Message::Run(Command::Find)
                        ),
                        menu_item(Icon::Plus, "New tab", Message::Run(Command::NewTab)),
                        menu_item(
                            Icon::Agent,
                            "New agent here",
                            Message::Run(Command::NewAgentHere)
                        ),
                        separator(),
                        menu_item(
                            Icon::SplitColumns,
                            "Split columns",
                            Message::SplitPane(id, Axis::Vertical)
                        ),
                        menu_item(
                            Icon::SplitRows,
                            "Split rows",
                            Message::SplitPane(id, Axis::Horizontal)
                        ),
                        menu_item(
                            Icon::Zoom,
                            if self.zoomed.is_some_and(|(_, pane)| pane == id) {
                                "Restore pane"
                            } else {
                                "Zoom pane"
                            },
                            Message::TogglePaneZoom(id)
                        ),
                        separator(),
                        menu_item(Icon::Close, "Close pane", Message::AskClosePaneOf(id))
                            .style(destructive),
                    ]
                }
            };
            let menu = container(items.spacing(2))
                .padding(5)
                .width(200)
                .style(menu_style);
            body = stack![
                body,
                opaque(
                    mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
                        .on_press(Message::DismissContextMenu)
                        .on_right_press(Message::DismissContextMenu)
                        .on_scroll(|_| Message::DismissContextMenu)
                ),
                float(opaque(menu)).translate(move |bounds, viewport| {
                    let left = viewport.x + 4.0;
                    let top = viewport.y + 4.0;
                    let right = (viewport.x + viewport.width - bounds.width - 4.0).max(left);
                    let bottom = (viewport.y + viewport.height - bounds.height - 4.0).max(top);
                    Vector::new(
                        position.x.clamp(left, right) - bounds.x,
                        position.y.clamp(top, bottom) - bounds.y,
                    )
                }),
            ]
            .into();
        }
        if self.tailscale.is_open() {
            body = modal(
                body,
                crate::features::tailscale::view(&self.tailscale, self.appearance_colors()),
                Some(Message::Tailscale(
                    crate::features::tailscale::Message::Close,
                )),
            );
        }
        if let Some(content) = crate::features::palette::view(&self.palette) {
            body = modal(
                body,
                content,
                Some(Message::Palette(crate::features::palette::Message::Close)),
            );
        }
        if self.renaming {
            body = modal(
                body,
                column![
                    text(if self.rename_tab.is_some() {
                        "Rename tab"
                    } else {
                        "Rename workspace"
                    })
                    .size(14)
                    .style(text_tone(TextTone::Text)),
                    text_input(
                        if self.rename_tab.is_some() {
                            "Tab name"
                        } else {
                            "Workspace name"
                        },
                        &self.rename
                    )
                    .id("workspace-name")
                    .size(12)
                    .padding(8)
                    .on_input(Message::RenameValue)
                    .on_submit(Message::RenameSubmit),
                    row![
                        Space::new().width(Length::Fill),
                        button(text("Cancel").size(11))
                            .padding([6, 10])
                            .style(quiet)
                            .on_press(Message::CancelRename),
                        button(text("Save").size(11))
                            .padding([6, 12])
                            .style(primary)
                            .on_press(Message::RenameSubmit),
                    ]
                    .spacing(6),
                ]
                .spacing(14)
                .into(),
                None,
            );
        }
        if let Some((request, id, error)) = &self.directory_fallback
            && let Some(workspace) = self.fallback_workspace(request, *id)
        {
            let detail: String = error.chars().take(400).collect();
            body = modal(
                body,
                column![
                    text("Could not create from this pane")
                        .size(14)
                        .style(text_tone(TextTone::Text)),
                    text(detail).size(11).style(text_tone(TextTone::Muted)),
                    text(format!(
                        "Try the starting directory of workspace \"{}\"?\n{}",
                        workspace.name, workspace.cwd
                    ))
                    .size(11)
                    .style(text_tone(TextTone::Muted)),
                    row![
                        Space::new().width(Length::Fill),
                        button(text("Cancel").size(11))
                            .padding([6, 10])
                            .style(quiet)
                            .on_press(Message::CancelWorkspaceDirectory),
                        button(text("Use workspace directory").size(11))
                            .padding([6, 10])
                            .style(primary)
                            .on_press(Message::UseWorkspaceDirectory),
                    ]
                    .spacing(6),
                ]
                .spacing(14)
                .into(),
                None,
            );
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
            let detail = "Every shell and running command in this workspace will be terminated.";
            body = modal(
                body,
                column![
                    text(title).size(14).style(text_tone(TextTone::Text)),
                    text(detail).size(11).style(text_tone(TextTone::Muted)),
                    text("Closing the window instead keeps your sessions running.")
                        .size(10)
                        .style(text_tone(TextTone::Muted)),
                    row![
                        Space::new().width(Length::Fill),
                        button(text("Cancel").size(11))
                            .padding([6, 10])
                            .style(quiet)
                            .on_press(Message::CancelClose),
                        button(text("Close permanently").size(11))
                            .padding([6, 10])
                            .style(destructive)
                            .on_press(Message::ConfirmClose),
                    ]
                    .spacing(6),
                ]
                .spacing(14)
                .into(),
                None,
            );
        }
        // The bottom edge drags the window's height, which a window without a title bar
        // has no other way to change.
        let grip = mouse_area(Space::new().width(Length::Fill).height(RESIZE_GRIP))
            .on_press(Message::ResizeBottom)
            .interaction(iced::mouse::Interaction::ResizingVertically);
        stack![
            body,
            column![Space::new().height(Length::Fill), grip].height(Length::Fill)
        ]
        .into()
    }
}
