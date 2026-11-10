//! Compact native workspace chrome; terminal state remains server-owned.
use crate::{
    App, Confirm, ContextTarget, Message,
    chrome::*,
    divider::{Divider, THICKNESS},
    tailscale, terminal,
};
use iced::{
    Alignment, Border, Color, Element, Length, Padding, Point, Rectangle, Vector, mouse,
    widget::{
        Space, button, canvas, column, container, float, mouse_area, opaque, row, scrollable,
        stack, text, text_input, tooltip,
    },
};
use spooky_core::{Axis, FindDirection, Layout};
use std::time::Duration;

/// Padding around each terminal pane. The divider geometry accounts for it.
pub(crate) const PANE_PADDING_X: f32 = 9.0;
pub(crate) const PANE_PADDING_Y: f32 = 7.0;
const SIDEBAR_WIDTH: f32 = 232.0;
const HEADER_HEIGHT: f32 = 36.0;
/// Width of a tab's close button plus the gap before it: a tab that is not selected
/// keeps this much room so selecting tabs never moves or resizes the others.
const TAB_CLOSE_SLOT: f32 = 23.0;

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

fn menu_style(_: &iced::Theme) -> container::Style {
    container::Style {
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..surface(SIDEBAR)
    }
}

fn menu_item(
    kind: Icon,
    label: &'static str,
    message: Message,
) -> button::Button<'static, Message> {
    button(
        row![icon(kind, MUTED), text(label).size(11)]
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
    let panel = container(content)
        .padding(20)
        .width(380)
        .style(|_| container::Style {
            border: Border {
                color: BORDER,
                width: 1.0,
                radius: 7.0.into(),
            },
            ..surface(SIDEBAR)
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
    fn header(&self) -> Element<'_, Message> {
        // The native macOS traffic lights occupy this part of the unified titlebar.
        let inset = if cfg!(target_os = "macos") { 72 } else { 0 };
        let mut brand = row![Space::new().width(inset)]
            .align_y(Alignment::Center)
            .spacing(6);
        if self.sidebar_open {
            brand = brand
                .push(icon(Icon::Ghost, TEXT))
                .push(text("Spooky").size(11).color(TEXT))
                .push(Space::new().width(Length::Fill));
        }
        brand = brand.push(action(
            Icon::Sidebar,
            "Toggle sidebar",
            Message::ToggleSidebar,
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
                let selected = self.tab == Some(tab.id);
                let id = tab.id;
                let label = button(
                    row![
                        icon(Icon::Terminal, if selected { TEXT } else { MUTED }),
                        text(self.tab_label(tab)).size(11)
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
                // Only the selected tab shows its close button; a middle click
                // closes any tab and a right-click opens its menu. Closing always
                // confirms first.
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
        let controls = row![
            action(
                Icon::SplitColumns,
                SPLIT_COLUMNS,
                Message::Split(Axis::Vertical)
            ),
            action(
                Icon::SplitRows,
                SPLIT_ROWS,
                Message::Split(Axis::Horizontal)
            ),
            action(Icon::Plus, NEW_TAB, Message::NewTab),
            button(
                row![icon(Icon::Paperclip, MUTED), text("Attach").size(10)]
                    .spacing(5)
                    .align_y(Alignment::Center)
            )
            .padding([5, 7])
            .style(quiet)
            .on_press(Message::Upload),
            action(Icon::Camera, "Capture active pane", Message::Capture),
            action(Icon::More, "Workspace actions", Message::ToggleMenu),
        ]
        .spacing(3)
        .align_y(Alignment::Center);
        let header = container(
            row![
                brand,
                container(Space::new().width(1))
                    .height(Length::Fill)
                    .style(|_| surface(BORDER)),
                container(
                    scrollable(tabs)
                        .direction(scrollable::Direction::Horizontal(Default::default()))
                )
                .padding([0, 7])
                .width(Length::Fill)
                .align_y(Alignment::Center),
                controls,
                Space::new().width(5),
            ]
            .align_y(Alignment::Center),
        )
        .height(HEADER_HEIGHT)
        .style(|_| surface(SIDEBAR));
        mouse_area(header).on_press(Message::DragWindow).into()
    }

    fn sidebar(&self) -> Element<'_, Message> {
        let connection_name = if self.connected_target.is_empty() {
            "This computer"
        } else {
            &self.connected_target
        };
        let section = container(text("Spaces").size(10).color(MUTED))
            .height(22)
            .align_y(Alignment::Center);
        let machine = row![
            button(
                row![
                    icon(Icon::Server, MUTED),
                    text(connection_name).size(10).width(Length::Fill),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([5, 6])
            .style(quiet)
            .on_press(Message::ToggleWorkspaces),
            action(Icon::Plus, "New workspace", Message::NewWorkspace),
            button(
                row![
                    dot(if self.online() { SUCCESS } else { MUTED }),
                    icon(
                        if self.workspaces_open {
                            Icon::Chevron
                        } else {
                            Icon::ChevronRight
                        },
                        MUTED,
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
                SUCCESS,
                Color::from_rgb8(193, 148, 104),
                ACCENT,
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
                        text(&workspace.name)
                            .size(11)
                            .color(if selected { TEXT } else { MUTED })
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
                .push(text("Connect via SSH").size(10).color(TEXT))
                .push(
                    button(text("Browse Tailscale devices").size(10))
                        .padding([5, 8])
                        .style(quiet)
                        .on_press(Message::OpenTailscale),
                )
                .push(
                    text_input("user@host or SSH alias", &self.target)
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
        .style(|_| surface(SIDEBAR))
        .into()
    }

    fn terminal_layout<'a>(&'a self, layout: &'a Layout) -> Element<'a, Message> {
        match layout {
            Layout::Pane(id) => {
                let Some(pane) = self
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.panes.iter().find(|p| p.id == *id))
                else {
                    return container(text("Pane unavailable").size(11).color(MUTED))
                        .center(Length::Fill)
                        .into();
                };
                let selected = self.pane == Some(*id);
                let selection = self
                    .selection
                    .and_then(|s| (s.pane == *id).then_some((s.anchor, s.head)));
                let quick: Element<'_, Message> =
                    if self.hover_pane == Some(*id) && self.context_menu.is_none() {
                        self.pane_actions(*id, pane.scrolled > 0)
                    } else {
                        Space::new().into()
                    };
                // The overlay slot is always present so hovering never rebuilds the
                // terminal canvas beneath it.
                mouse_area(stack![
                    container(
                        canvas(terminal::Terminal {
                            pane,
                            font_size: self.config.font_size,
                            focused: self.terminal_focused && selected,
                            selection,
                            cursor_on: !(selected && self.blink_wanted()) || self.blink_on,
                        })
                        .width(Length::Fill)
                        .height(Length::Fill),
                    )
                    .padding([PANE_PADDING_Y, PANE_PADDING_X])
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|_| surface(BACKGROUND)),
                    quick
                ])
                .on_enter(Message::HoverPane(*id, true))
                .on_exit(Message::HoverPane(*id, false))
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
                let ratio = self
                    .split_drag
                    .filter(|(split, _)| split == id)
                    .map_or(*ratio, |(_, ratio)| ratio);
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

    /// The quick actions in the corner of the pane under the pointer, which act
    /// on that pane. `scrolled` moves them under the scroll position label.
    fn pane_actions(&self, id: u64, scrolled: bool) -> Element<'_, Message> {
        let cluster = container(
            row![
                action(
                    Icon::SplitColumns,
                    SPLIT_COLUMNS,
                    Message::SplitPane(id, Axis::Vertical)
                ),
                action(
                    Icon::SplitRows,
                    SPLIT_ROWS,
                    Message::SplitPane(id, Axis::Horizontal)
                ),
                action(Icon::Close, "Close pane", Message::AskClosePaneOf(id)),
            ]
            .spacing(1),
        )
        .padding(2)
        .style(|_| container::Style {
            border: Border {
                color: BORDER,
                width: 1.0,
                radius: 6.0.into(),
            },
            ..surface(SIDEBAR.scale_alpha(0.94))
        });
        container(cluster)
            .align_right(Length::Fill)
            .align_top(Length::Fill)
            .padding(Padding {
                top: if scrolled { 30.0 } else { 6.0 },
                right: 12.0,
                bottom: 0.0,
                left: 0.0,
            })
            .into()
    }

    /// Shown when no workspace is selectable: before the first snapshot, or after
    /// the last workspace was closed.
    fn empty_state(&self) -> Element<'_, Message> {
        if let Some((reason, retrying)) = &self.unreachable {
            return self.unreachable_state(reason, *retrying);
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|s| s.workspaces.is_empty())
        {
            container(
                column![
                    text("No workspaces").size(12).color(MUTED),
                    button(text("New workspace").size(11))
                        .padding([6, 10])
                        .style(primary)
                        .on_press(Message::NewWorkspace),
                ]
                .spacing(10)
                .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else {
            container(text("Waiting for session…").size(12).color(MUTED))
                .center(Length::Fill)
                .into()
        }
    }

    /// Shown while a target cannot be reached: what failed, and the way out. The
    /// way back to this computer is always offered for a remote target.
    fn unreachable_state(&self, reason: &str, retrying: bool) -> Element<'_, Message> {
        let remote = !self.connected_target.is_empty();
        let title = if remote {
            format!("Cannot connect to {}", self.connected_target)
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
            text(reason.to_owned()).size(11).color(MUTED),
        ]
        .spacing(8)
        .max_width(520)
        .align_x(Alignment::Center);
        if retrying {
            card = card.push(text("Retrying automatically…").size(10).color(MUTED));
        }
        container(card.push(actions).spacing(10))
            .center(Length::Fill)
            .into()
    }

    /// What the footer says: the target of a link under the pointer, else the status.
    fn footer_text(&self) -> String {
        match &self.hover_link {
            Some(target) => format!("↗ {target}"),
            None => self.status.clone(),
        }
    }

    /// The scrollback search strip above the terminal.
    fn search_bar(&self) -> Option<Element<'_, Message>> {
        let search = self.search.as_ref()?;
        let result = match search.result {
            None => String::new(),
            Some(None) => "No matches".to_owned(),
            Some(Some((current, total))) => format!("{current} of {total}"),
        };
        Some(
            container(
                row![
                    icon(Icon::Search, MUTED),
                    text_input("Find in scrollback", &search.query)
                        .id("search-input")
                        .size(11)
                        .padding([4, 6])
                        .on_input(Message::SearchInput)
                        .on_submit(Message::SearchStep(FindDirection::Next)),
                    text(result).size(10).color(MUTED),
                    action(
                        Icon::ChevronUp,
                        "Previous match",
                        Message::SearchStep(FindDirection::Previous)
                    ),
                    action(
                        Icon::Chevron,
                        "Next match",
                        Message::SearchStep(FindDirection::Next)
                    ),
                    action(Icon::Close, "Close search", Message::SearchClose),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding([4, 10])
            .style(|_| surface(SIDEBAR))
            .into(),
        )
    }

    pub(super) fn view(&self) -> Element<'_, Message> {
        let content = self
            .active()
            .map(|(_, tab)| self.terminal_layout(&tab.layout))
            .unwrap_or_else(|| self.empty_state());
        let grid = self
            .current_pane()
            .map(|p| {
                format!(
                    "{} × {}{}",
                    p.cols,
                    p.rows,
                    if p.exited { " · exited" } else { "" }
                )
            })
            .unwrap_or_default();
        let footer = container(
            row![
                dot(if self.online() { SUCCESS } else { MUTED }),
                container(
                    text(self.footer_text())
                        .size(9)
                        .color(MUTED)
                        .wrapping(text::Wrapping::None),
                )
                .width(Length::Fill)
                .clip(true),
                text(grid).size(9).color(MUTED),
                Space::new().width(10),
                text(&self.config.shortcut).size(9).color(MUTED),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([0, 10])
        .height(20)
        .align_y(Alignment::Center)
        .style(|_| surface(SIDEBAR));
        let mut main = column![].width(Length::Fill).height(Length::Fill);
        if let Some(bar) = self.search_bar() {
            main = main.push(bar).push(separator());
        }
        let main = main
            .push(
                container(content)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|_| surface(SIDEBAR)),
            )
            .push(separator())
            .push(footer);
        let body: Element<'_, Message> = if self.sidebar_open {
            row![
                self.sidebar(),
                container(Space::new().width(1))
                    .height(Length::Fill)
                    .style(|_| surface(BORDER)),
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
                        Message::Split(Axis::Vertical)
                    ),
                    menu_item(
                        Icon::SplitRows,
                        "Split rows",
                        Message::Split(Axis::Horizontal)
                    ),
                    separator(),
                    menu_item(Icon::Pencil, "Rename workspace", Message::Rename),
                    menu_item(
                        Icon::Pencil,
                        "Rename tab",
                        Message::RenameTab(self.tab.unwrap_or_default())
                    ),
                    separator(),
                    menu_item(Icon::Close, "Close pane", Message::AskClosePane).style(destructive),
                    menu_item(
                        Icon::Close,
                        "Close tab",
                        Message::AskCloseTab(self.tab.unwrap_or_default())
                    )
                    .style(destructive),
                    menu_item(Icon::Close, "Close workspace", Message::AskCloseWorkspace)
                        .style(destructive),
                    separator(),
                    menu_item(Icon::Close, "Quit Spooky", Message::Quit),
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
                    menu_item(Icon::Plus, "New tab", Message::NewTab),
                    separator(),
                    menu_item(Icon::Close, "Close tab", Message::AskCloseTab(id))
                        .style(destructive),
                ],
                ContextTarget::Pane(id) => {
                    let mut copy = menu_item(Icon::Copy, "Copy", Message::Copy);
                    if self.selected_text().is_none() {
                        copy = copy.on_press_maybe(None);
                    }
                    column![
                        copy,
                        menu_item(Icon::Paste, "Paste", Message::PasteClipboard),
                        menu_item(Icon::Search, "Find in scrollback", Message::SearchOpen),
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
        if self.tailscale_open {
            body = modal(body, self.tailscale_picker(), Some(Message::CloseTailscale));
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
                    .color(TEXT),
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
        if let Some(confirm) = self.confirm {
            let (title, detail) = match confirm {
                Confirm::Pane(pane) => {
                    // The server removes a tab with its last pane, and a workspace with its last tab.
                    let located = self.snapshot.as_ref().and_then(|s| {
                        s.workspaces.iter().find_map(|w| {
                            w.tabs
                                .iter()
                                .find(|t| t.layout.contains(pane))
                                .map(|t| (w, t))
                        })
                    });
                    match located {
                        Some((w, t))
                            if matches!(t.layout, Layout::Pane(_)) && w.tabs.len() == 1 =>
                        {
                            (
                                format!("Close \"{}\"?", w.name),
                                "This is its only pane, so the workspace closes and its shell is terminated.",
                            )
                        }
                        Some((_, t)) if matches!(t.layout, Layout::Pane(_)) => (
                            format!("Close tab \"{}\"?", t.title),
                            "This is its only pane, so the tab closes and its shell is terminated.",
                        ),
                        _ => (
                            "Close this pane?".to_owned(),
                            "Its shell and running commands will be terminated.",
                        ),
                    }
                }
                Confirm::Tab(id) => (
                    format!(
                        "Close tab \"{}\"?",
                        self.snapshot
                            .as_ref()
                            .and_then(|s| s
                                .workspaces
                                .iter()
                                .flat_map(|w| &w.tabs)
                                .find(|t| t.id == id))
                            .map_or("tab", |t| t.title.as_str())
                    ),
                    "Every shell and running command in this tab will be terminated. Closing a workspace's last tab closes the workspace.",
                ),
                Confirm::Workspace(id) => (
                    format!(
                        "Close \"{}\"?",
                        self.snapshot
                            .as_ref()
                            .and_then(|s| s.workspaces.iter().find(|w| w.id == id))
                            .map_or("workspace", |w| w.name.as_str())
                    ),
                    "Every shell and running command in this workspace will be terminated.",
                ),
            };
            body = modal(
                body,
                column![
                    text(title).size(14).color(TEXT),
                    text(detail).size(11).color(MUTED),
                    text("Closing the window instead keeps your sessions running.")
                        .size(10)
                        .color(MUTED),
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
        body
    }

    fn tailscale_picker(&self) -> Element<'_, Message> {
        let query = self.tailscale_query.to_lowercase();
        let user = self.tailscale_user.trim();
        let user_valid = user.is_empty()
            || (!user.starts_with('-')
                && user
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)));
        let mut devices = column![].spacing(3);
        match &self.tailscale {
            tailscale::Discovery::Idle | tailscale::Discovery::Loading => {
                devices = devices.push(
                    text("Reading local Tailscale client…")
                        .size(11)
                        .color(MUTED),
                );
            }
            tailscale::Discovery::Failed(error) => {
                devices = devices.push(text(error).size(11).color(TEXT));
            }
            tailscale::Discovery::Ready(network) => {
                let mut shown = 0;
                for peer in &network.peers {
                    if !peer.name.to_lowercase().contains(&query) && !peer.host.contains(&query) {
                        continue;
                    }
                    shown += 1;
                    let entry = button(
                        row![
                            dot(if peer.online { SUCCESS } else { MUTED }),
                            column![
                                text(&peer.name).size(11).color(if peer.online {
                                    TEXT
                                } else {
                                    MUTED
                                }),
                                text(format!(
                                    "{} · {} · {}",
                                    peer.host,
                                    peer.os,
                                    if peer.online { "online" } else { "offline" }
                                ))
                                .size(9)
                                .color(MUTED),
                            ]
                            .spacing(3)
                            .width(Length::Fill),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .padding(8)
                    .width(Length::Fill)
                    .style(quiet);
                    devices = devices.push(if peer.online && user_valid {
                        entry.on_press(Message::SelectTailscale(peer.host.clone()))
                    } else {
                        entry
                    });
                }
                if shown == 0 {
                    devices = devices.push(
                        text(if network.peers.is_empty() {
                            "No other devices in this tailnet."
                        } else {
                            "No matching devices."
                        })
                        .size(11)
                        .color(MUTED),
                    );
                }
            }
        }
        let refresh = button(text("Refresh").size(10))
            .padding([5, 8])
            .style(quiet);
        column![
            row![
                text("Tailscale devices").size(14).color(TEXT).width(Length::Fill),
                action(Icon::Close, "Close Tailscale devices", Message::CloseTailscale),
            ].align_y(Alignment::Center),
            text("Uses your installed Tailscale client. Selecting a device prepares SSH; Connect starts the session.").size(10).color(MUTED),
            text_input("Search devices or IP", &self.tailscale_query).size(11).padding(7).on_input(Message::TailscaleQuery),
            text_input("SSH user (optional)", &self.tailscale_user).size(11).padding(7).on_input(Message::TailscaleUser),
            text(if user_valid { "Remote device needs SSH and spooky-server." } else { "Invalid SSH user: use letters, digits, dot, underscore or hyphen." }).size(10).color(MUTED),
            container(scrollable(devices)).height(240),
            if matches!(self.tailscale, tailscale::Discovery::Loading) { refresh } else { refresh.on_press(Message::RefreshTailscale) },
        ].spacing(10).into()
    }
}
