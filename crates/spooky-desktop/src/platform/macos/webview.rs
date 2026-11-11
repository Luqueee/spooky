//! The agent view's web page, a WKWebView that is a child of the main window.
//!
//! The view is created inside `iced::window::run`, which runs on the event loop's thread
//! with a safe window handle, and kept in a thread-local because a `WebView` is bound to
//! that thread. Everything here is called from that thread.
//!
//! The page is a bundled, self-contained file. It may not navigate anywhere, open windows
//! or fetch anything; the only way out is the message channel, whose receiving end is read
//! by [`messages`].

use iced::Rectangle;
use iced::futures::{StreamExt, channel::mpsc};
use std::{
    cell::RefCell,
    sync::{LazyLock, Mutex},
};
use wry::{
    NewWindowResponse, Rect, WebView, WebViewBuilder,
    dpi::{LogicalPosition, LogicalSize},
};

/// The page, built from `web/agent` and committed.
const PAGE: &str = include_str!("../../../../../web/agent/dist/index.html");
/// Messages from the page that wait to be read. A page that outpaces the application
/// loses messages instead of growing memory.
const QUEUE: usize = 64;

thread_local! {
    static VIEW: RefCell<Option<WebView>> = const { RefCell::new(None) };
}

struct Channel {
    tx: mpsc::Sender<String>,
    rx: Mutex<Option<mpsc::Receiver<String>>>,
}

static CHANNEL: LazyLock<Channel> = LazyLock::new(|| {
    let (tx, rx) = mpsc::channel(QUEUE);
    Channel {
        tx,
        rx: Mutex::new(Some(rx)),
    }
});

/// Whether the web agent view is used: always, unless `SPOOKY_AGENT_UI=native` asks for
/// the native one.
pub(crate) fn supported() -> bool {
    std::env::var_os("SPOOKY_AGENT_UI").is_none_or(|value| value != "native")
}

fn rect(area: Rectangle) -> Rect {
    Rect {
        position: LogicalPosition::new(f64::from(area.x), f64::from(area.y)).into(),
        size: LogicalSize::new(
            f64::from(area.width).max(1.0),
            f64::from(area.height).max(1.0),
        )
        .into(),
    }
}

/// Creates the page as a hidden child of `window`; a second call does nothing.
pub(crate) fn create(window: &dyn iced::window::Window) -> Result<(), String> {
    VIEW.with(|view| {
        if view.borrow().is_some() {
            return Ok(());
        }
        let built = WebViewBuilder::new()
            .with_html(PAGE)
            .with_visible(false)
            .with_devtools(false)
            .with_accept_first_mouse(true)
            .with_background_color((34, 37, 46, 255))
            .with_ipc_handler(move |request| {
                // A full queue drops the message: the page is not waited for.
                let _ = CHANNEL.tx.clone().try_send(request.into_body());
            })
            // The page is loaded from a string and never leaves it.
            .with_navigation_handler(|url| url.starts_with("about:") || url.starts_with("data:"))
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .build_as_child(&window)
            .map_err(|error| format!("The agent view could not start: {error}"))?;
        *view.borrow_mut() = Some(built);
        Ok(())
    })
}

fn with_view(action: impl FnOnce(&WebView) -> wry::Result<()>) {
    VIEW.with(|view| {
        if let Some(view) = view.borrow().as_ref() {
            // A view that cannot be adjusted is left as it was; the next change retries.
            let _ = action(view);
        }
    });
}

/// Places the page over `area`, in window coordinates.
pub(crate) fn place(area: Rectangle) {
    with_view(|view| view.set_bounds(rect(area)));
}

/// Shows or hides the page. Hiding gives the keyboard back to the window.
pub(crate) fn show(visible: bool) {
    with_view(|view| {
        view.set_visible(visible)?;
        if !visible {
            view.focus_parent()?;
        }
        Ok(())
    });
}

/// Gives the page the keyboard.
pub(crate) fn focus() {
    with_view(WebView::focus);
}

/// Gives the page its new state: `json` is the argument of the page's `update` function.
pub(crate) fn push(json: &str) {
    with_view(|view| view.evaluate_script(&format!("window.spooky.update({json})")));
}

/// Gives the page text from the clipboard to insert at its caret.
pub(crate) fn paste(text: &str) {
    // A JSON string is also a JavaScript string literal.
    if let Ok(literal) = serde_json::to_string(text) {
        with_view(|view| view.evaluate_script(&format!("window.spooky.paste({literal})")));
    }
}

/// Puts the keyboard focus in the page's composer.
pub(crate) fn focus_composer() {
    with_view(|view| view.evaluate_script("window.spooky.focusComposer()"));
}

/// What the page posts, in order. The stream exists once: a second caller gets an empty one.
pub(crate) fn messages() -> impl iced::futures::Stream<Item = String> {
    let taken = CHANNEL.rx.lock().ok().and_then(|mut slot| slot.take());
    iced::futures::stream::iter(taken).flatten()
}
