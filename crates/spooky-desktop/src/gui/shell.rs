//! The window's root view. It owns the application, carries out the tasks the application
//! returns, feeds it the events of the window and of the background (the server link, the
//! global shortcut, probes, updates, the agent page) and draws it.

use super::terminal::SharedPointer;
use super::{
    keys,
    text_input::{Spec, TextInput},
};
use crate::features::splits::SharedDivider;
use crate::{
    Message,
    app::{App as Application, Wakeups},
    features::attachments,
    geometry::{Point, Rectangle, Size},
    link, platform,
    runtime::{Level, Op, Query, Task, WindowOp},
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture};
use gpui::{
    App, AppContext, Bounds, ClipboardItem, Context, Entity, EntityInputHandler, ExternalPaths,
    FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent, ModifiersChangedEvent,
    ParentElement, Pixels, Render, ResizeEdge, ScrollHandle, Styled, TitlebarOptions,
    UTF16Selection, WeakEntity, Window, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowKind, WindowOptions, div, point, px, size,
};
use std::{
    cell::Cell,
    collections::{HashMap, VecDeque},
    ops::Range,
    rc::Rc,
    time::Duration,
};

/// Sends a message to the application from an event handler.
pub(crate) type Sender = Rc<dyn Fn(Message, &mut Window, &mut App)>;

/// The window's text fields, keyboard focus and other state the views need but the
/// application does not own.
pub(crate) struct Ui {
    root: WeakEntity<Root>,
    fields: Vec<(&'static str, Entity<TextInput>)>,
    /// The fields drawn in the frame being built.
    rendered: Vec<&'static str>,
    /// The field that should get the keyboard once it has been drawn.
    focus_request: Option<String>,
    /// Whether that field's text should also be selected.
    select_request: bool,
    /// The keyboard focus of the terminal, and with it the input method's target.
    pub(crate) terminal_focus: FocusHandle,
    /// Text the input method is composing for the terminal, not yet sent.
    pub(crate) composing: Option<String>,
    /// Where the terminal's cursor is, for the input method's candidate window.
    pub(crate) cursor_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// What each terminal pane's pointer handling remembers between events.
    pub(crate) pointers: HashMap<u64, SharedPointer>,
    /// What each divider remembers between events.
    pub(crate) dividers: HashMap<u64, SharedDivider>,
    /// How far each agent conversation is scrolled.
    pub(crate) scrolls: HashMap<u64, ScrollHandle>,
    /// Where the agent page's area was last reported to be.
    pub(crate) web_area: Rc<Cell<Option<Rectangle>>>,
}

impl Ui {
    /// A function that sends messages to the application.
    pub(crate) fn sender(&self) -> Sender {
        let root = self.root.clone();
        Rc::new(move |message, window, cx| {
            root.update(cx, |root, cx| root.dispatch(message, window, cx))
                .ok();
        })
    }

    /// The root view, for things that need to be its input handler.
    pub(crate) fn root(&self) -> Option<Entity<Root>> {
        self.root.upgrade()
    }

    /// The text field `id`, made on first use and given what the application wants it to be.
    pub(crate) fn field(
        &mut self,
        id: &'static str,
        spec: Spec,
        cx: &mut Context<Root>,
    ) -> Entity<TextInput> {
        let existing = self
            .fields
            .iter()
            .find(|(name, _)| *name == id)
            .map(|(_, field)| field.clone());
        let field = existing.unwrap_or_else(|| {
            let root = self.root.clone();
            let made = cx.new(|cx| TextInput::new(root, spec.clone(), cx));
            self.fields.push((id, made.clone()));
            made
        });
        field.update(cx, |input, _| input.configure(spec));
        self.rendered.push(id);
        field
    }

    /// The pointer state of a terminal pane.
    pub(crate) fn pointer(&mut self, pane: u64) -> SharedPointer {
        self.pointers.entry(pane).or_default().clone()
    }

    /// What the divider of a split remembers between events.
    pub(crate) fn divider(&mut self, split: u64) -> SharedDivider {
        self.dividers.entry(split).or_default().clone()
    }

    /// The scroll position of an agent pane's conversation.
    pub(crate) fn scroll(&mut self, pane: u64) -> ScrollHandle {
        self.scrolls.entry(pane).or_default().clone()
    }

    /// Gives the keyboard to the field a task asked for, once it exists, and to the
    /// terminal when it is the application's turn.
    fn settle_focus(&mut self, app: &Application, window: &mut Window, cx: &mut Context<Root>) {
        if let Some(id) = self.focus_request.take() {
            let field = self
                .fields
                .iter()
                .find(|(name, _)| *name == id && self.rendered.contains(name))
                .map(|(_, field)| field.clone());
            match field {
                Some(field) => {
                    let handle = field.read(cx).focus_handle().clone();
                    let select = std::mem::take(&mut self.select_request);
                    window.defer(cx, move |window, cx| {
                        window.focus(&handle, cx);
                        if select {
                            field.update(cx, |input, cx| input.select_all(cx));
                        }
                    });
                    return;
                }
                // Not drawn yet: ask again after the next frame.
                None => self.focus_request = Some(id),
            }
        }
        if self.focus_request.is_none()
            && app.terminal_wants_focus()
            && !self.terminal_focus.is_focused(window)
        {
            let handle = self.terminal_focus.clone();
            window.defer(cx, move |window, cx| window.focus(&handle, cx));
        }
    }
}

/// How the window is first opened.
pub(crate) struct Launch {
    pub size: Size,
    pub min_size: Size,
    pub position: Point,
    /// Starts hidden: a global shortcut shows it.
    pub hidden: bool,
    /// KDE draws no title bar of its own.
    pub frameless: bool,
    pub app_id: &'static str,
}

/// Opens the window and runs the application in it.
pub(crate) fn open(cx: &mut App, app: Application, launch: &Launch) -> anyhow::Result<()> {
    let bounds = Bounds::new(
        point(px(launch.position.x), px(launch.position.y)),
        size(px(launch.size.width), px(launch.size.height)),
    );
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Spooky".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(13.), px(13.))),
        }),
        focus: !launch.hidden,
        show: !launch.hidden,
        kind: WindowKind::Normal,
        window_background: WindowBackgroundAppearance::Opaque,
        app_id: Some(launch.app_id.to_owned()),
        window_min_size: Some(size(px(launch.min_size.width), px(launch.min_size.height))),
        window_decorations: Some(if launch.frameless {
            WindowDecorations::Client
        } else {
            WindowDecorations::Server
        }),
        ..Default::default()
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("spooky-work")
        .enable_all()
        .build()
        .map_err(|error| anyhow::anyhow!("cannot start the background runtime: {error}"))?;
    cx.open_window(options, move |window, cx| {
        cx.new(|cx| Root::new(app, runtime, window, cx))
    })?;
    Ok(())
}

/// The window's view.
pub(crate) struct Root {
    pub(crate) app: Application,
    pub(crate) ui: Ui,
    /// Runs the work tasks ask for off the interface thread.
    runtime: tokio::runtime::Runtime,
    background: Option<WindowBackgroundAppearance>,
    last_size: Option<Size>,
}

impl Root {
    fn new(
        app: Application,
        runtime: tokio::runtime::Runtime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let ui = Ui {
            root: cx.weak_entity(),
            fields: Vec::new(),
            rendered: Vec::new(),
            focus_request: None,
            select_request: false,
            terminal_focus: cx.focus_handle(),
            composing: None,
            cursor_bounds: Rc::default(),
            pointers: HashMap::new(),
            dividers: HashMap::new(),
            scrolls: HashMap::new(),
            web_area: Rc::default(),
        };
        let root = Self {
            app,
            ui,
            runtime,
            background: None,
            last_size: None,
        };
        root.observe_window(window, cx);
        root.start_events(window, cx);
        cx.defer_in(window, |root, window, cx| {
            root.dispatch(Message::Opened, window, cx);
        });
        root
    }

    fn observe_window(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.observe_window_activation(window, |root, window, cx| {
            let active = window.is_window_active();
            root.dispatch(Message::WindowFocus(active), window, cx);
        })
        .detach();
        cx.observe_window_bounds(window, |root, window, cx| {
            let viewport = window.viewport_size();
            let current = Size::new(f32::from(viewport.width), f32::from(viewport.height));
            if root.last_size != Some(current) {
                root.last_size = Some(current);
                root.dispatch(Message::WindowResized(current), window, cx);
            }
        })
        .detach();
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |root, cx| root.dispatch(Message::Close, window, cx))
                .ok();
            false
        });
    }

    /// Starts the loops that turn what happens in the background into messages.
    fn start_events(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.pump(
            window,
            cx,
            || link::next_event().boxed_local(),
            Message::Link,
        );
        self.pump(
            window,
            cx,
            || link::next_hotkey().boxed_local(),
            Message::Hotkey,
        );
        self.pump(
            window,
            cx,
            || crate::probe::next_event().boxed_local(),
            Message::Machine,
        );
        self.pump(
            window,
            cx,
            || crate::updater::next_event().boxed_local(),
            Message::Update,
        );
        cx.spawn_in(window, async move |this, cx| {
            let mut messages = Box::pin(platform::webview::messages());
            while let Some(event) = messages.next().await {
                if this
                    .update_in(cx, |root, window, cx| {
                        root.dispatch(Message::WebEvent(event), window, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        self.tick(
            window,
            cx,
            Duration::from_millis(530),
            |wake| wake.blink,
            || Message::Blink,
        );
        self.tick(
            window,
            cx,
            Duration::from_millis(500),
            |wake| wake.directory,
            || Message::DirectoryTick,
        );
        self.tick(
            window,
            cx,
            Duration::from_millis(16),
            |wake| wake.slide,
            || Message::SlideFrame(std::time::Instant::now()),
        );
        if cfg!(target_os = "macos") {
            self.tick(
                window,
                cx,
                Duration::from_secs(1),
                |_| true,
                || Message::BackgroundTick,
            );
        }
    }

    /// Sends each item the source produces as a message, until it ends.
    fn pump<T: 'static>(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        mut next: impl FnMut() -> LocalBoxFuture<'static, Option<T>> + 'static,
        wrap: fn(T) -> Message,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            while let Some(item) = next().await {
                if this
                    .update_in(cx, |root, window, cx| {
                        root.dispatch(wrap(item), window, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Sends a message every `period` while the application wants the wakeup.
    fn tick(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        period: Duration,
        wanted: fn(&Wakeups) -> bool,
        message: fn() -> Message,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(period).await;
                let alive = this.update_in(cx, |root, window, cx| {
                    if wanted(&root.app.wakeups()) {
                        root.dispatch(message(), window, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Gives the application a message and carries out what it asks for.
    pub(crate) fn dispatch(
        &mut self,
        message: Message,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = self.app.update(message);
        self.run(task, window, cx);
        cx.notify();
    }

    fn run(&mut self, task: Task<Message>, window: &mut Window, cx: &mut Context<Self>) {
        match task {
            Task::None => {}
            Task::Batch(tasks) => {
                for task in tasks {
                    self.run(task, window, cx);
                }
            }
            Task::Chain(tasks) => self.run_chain(tasks.into(), window, cx),
            Task::Done(message) => self.dispatch(message, window, cx),
            Task::Perform(start) => {
                let future = start(self.runtime.handle());
                self.await_message(future, VecDeque::new(), window, cx);
            }
            Task::Op(op) => self.op(op, window, cx),
        }
    }

    /// Runs tasks one after the other; one that waits for work holds back the rest.
    fn run_chain(
        &mut self,
        mut tasks: VecDeque<Task<Message>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        while let Some(task) = tasks.pop_front() {
            if let Task::Perform(start) = task {
                let future = start(self.runtime.handle());
                self.await_message(future, tasks, window, cx);
                return;
            }
            self.run(task, window, cx);
        }
    }

    fn await_message(
        &mut self,
        future: LocalBoxFuture<'static, Option<Message>>,
        rest: VecDeque<Task<Message>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let message = future.await;
            this.update_in(cx, |root, window, cx| {
                if let Some(message) = message {
                    root.dispatch(message, window, cx);
                }
                root.run_chain(rest, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn op(&mut self, op: Op<Message>, window: &mut Window, cx: &mut Context<Self>) {
        match op {
            Op::Focus(id) => self.ui.focus_request = Some(id),
            Op::SelectAll(id) => {
                self.ui.focus_request = Some(id);
                self.ui.select_request = true;
            }
            Op::Exit => cx.quit(),
            Op::ClipboardWrite(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Op::ClipboardRead(next) => {
                let text = cx.read_from_clipboard().and_then(|item| item.text());
                self.dispatch(next(text), window, cx);
            }
            Op::Window(op) => self.window_op(op, window, cx),
            Op::Query(query) => {
                let next = match query {
                    Query::MonitorSize(next) => next(window.display(cx).map(|display| {
                        let bounds = display.bounds();
                        Size::new(f32::from(bounds.size.width), f32::from(bounds.size.height))
                    })),
                    Query::Size(next) => {
                        let viewport = window.viewport_size();
                        next(Size::new(
                            f32::from(viewport.width),
                            f32::from(viewport.height),
                        ))
                    }
                    Query::Minimized(next) => next(platform::window::is_minimized()),
                };
                self.run(next, window, cx);
            }
            Op::Native(run) => {
                let message = run(&*window);
                self.dispatch(message, window, cx);
            }
        }
    }

    fn window_op(&mut self, op: WindowOp, window: &mut Window, cx: &mut Context<Self>) {
        match op {
            WindowOp::Hide => platform::window::set_visible(false),
            WindowOp::Show => platform::window::set_visible(true),
            WindowOp::Minimize(true) => window.minimize_window(),
            WindowOp::Minimize(false) => platform::window::unminimize(),
            WindowOp::Raise => {
                cx.activate(true);
                window.activate_window();
            }
            WindowOp::Resize(to) => window.resize(size(px(to.width), px(to.height))),
            WindowOp::MoveTo(to) => platform::window::move_to(to),
            WindowOp::Drag => window.start_window_move(),
            WindowOp::DragResizeBottom => window.start_window_resize(ResizeEdge::Bottom),
            WindowOp::Close => cx.quit(),
            WindowOp::SetLevel(level) => {
                platform::window::set_always_on_top(level == Level::AlwaysOnTop);
            }
        }
    }

    /// Keeps the window's material in step with the application's background opacity.
    ///
    /// macOS gets a transparent window with our own dark behind-window material under it:
    /// the one GPUI's blurred appearance adds is a light selection tint. Elsewhere the
    /// toolkit's blurred appearance asks the compositor to blur what shows through.
    fn reconcile(&mut self, window: &mut Window) {
        let translucent = self.app.background_opacity() < 1.0;
        let wanted = match (translucent, cfg!(target_os = "macos")) {
            (false, _) => WindowBackgroundAppearance::Opaque,
            (true, true) => WindowBackgroundAppearance::Transparent,
            (true, false) => WindowBackgroundAppearance::Blurred,
        };
        if self.background != Some(wanted) {
            self.background = Some(wanted);
            window.set_background_appearance(wanted);
        }
        #[cfg(target_os = "macos")]
        platform::window::set_glass(translucent);
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let press = keys::press(&event.keystroke);
        self.dispatch(Message::Key(press), window, cx);
        if self.app.take_key_taken() {
            cx.stop_propagation();
        }
    }
}

impl Focusable for Root {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.ui.terminal_focus.clone()
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.reconcile(window);
        self.ui.rendered.clear();
        let content = {
            let Self { app, ui, .. } = &mut *self;
            app.view(ui, window, cx)
        };
        {
            let Self { app, ui, .. } = &mut *self;
            ui.settle_focus(app, window, cx);
        }
        div()
            .id("root")
            .size_full()
            .track_focus(&self.ui.terminal_focus)
            .capture_key_down(cx.listener(Self::key_down))
            .on_modifiers_changed(
                cx.listener(|root, event: &ModifiersChangedEvent, window, cx| {
                    let modifiers = keys::modifiers(&event.modifiers);
                    root.dispatch(Message::Modifiers(modifiers), window, cx);
                }),
            )
            .on_drop(cx.listener(|root, paths: &ExternalPaths, window, cx| {
                for path in paths.paths() {
                    let message = Message::Attachments(attachments::Message::Dropped(path.clone()));
                    root.dispatch(message, window, cx);
                }
            }))
            .child(content)
    }
}

/// The input method's view of the terminal: it only ever inserts text, which goes to the
/// pane as typed. Composition in progress is shown at the cursor.
impl EntityInputHandler for Root {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.ui
            .composing
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.ui.composing = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ui.composing = None;
        if !text.is_empty() {
            self.dispatch(Message::TerminalText(text.to_owned()), window, cx);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ui.composing = (!text.is_empty()).then(|| text.to_owned());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(self.ui.cursor_bounds.get().unwrap_or(bounds))
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
