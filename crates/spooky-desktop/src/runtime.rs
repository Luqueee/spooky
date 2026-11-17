//! What the application asks its shell to do, as data.
//!
//! Update functions return a [`Task`] instead of acting: running a future, putting the
//! keyboard focus in a field, writing the clipboard, moving the window. The shell
//! (`gui::shell`) carries the tasks out and feeds the messages they produce back into
//! `App::update`. Keeping effects as values is what lets the update logic be tested
//! without a display and lets this module be the only vocabulary the logic shares with
//! the toolkit.

use crate::geometry::{Point, Size};
use futures::future::LocalBoxFuture;
use std::{future::Future, rc::Rc};

/// How the window stacks against others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Normal,
    AlwaysOnTop,
}

/// A change to the window that has no answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WindowOp {
    /// Take the window off screen.
    Hide,
    /// Put the window back on screen.
    Show,
    Minimize(bool),
    /// Bring the window to the front and give it the keyboard.
    Raise,
    Resize(Size),
    MoveTo(Point),
    /// The pointer is on the window's title area: let the system move it.
    Drag,
    /// The pointer is on the window's bottom edge: let the system resize it.
    DragResizeBottom,
    Close,
    SetLevel(Level),
}

/// A question about the window, and what to do with the answer.
pub(crate) enum Query<M> {
    /// The size of the display the window is on, when it is known.
    MonitorSize(Box<dyn FnOnce(Option<Size>) -> Task<M>>),
    /// The window's size.
    Size(Box<dyn FnOnce(Size) -> Task<M>>),
    /// Whether the window is minimized, when that can be told.
    Minimized(Box<dyn FnOnce(Option<bool>) -> Task<M>>),
}

/// One thing the shell does.
pub(crate) enum Op<M> {
    /// Give the keyboard to the text field with this id.
    Focus(String),
    /// Select all the text of the field with this id.
    SelectAll(String),
    Exit,
    ClipboardWrite(String),
    ClipboardRead(Box<dyn FnOnce(Option<String>) -> M>),
    Window(WindowOp),
    Query(Query<M>),
    /// Run something that needs the window's native handle, on the main thread.
    Native(Native<M>),
}

/// A closure that turns a native window handle into a message.
pub(crate) type Native<M> = Box<dyn FnOnce(&dyn raw_window_handle::HasWindowHandle) -> M>;

/// A closure that starts background work on the runtime and yields its message.
pub(crate) type Start<M> =
    Box<dyn FnOnce(&tokio::runtime::Handle) -> LocalBoxFuture<'static, Option<M>>>;

/// Work for the shell, producing zero or more messages.
pub(crate) enum Task<M> {
    None,
    /// Independent tasks.
    Batch(Vec<Task<M>>),
    /// Tasks that start one after the other, each when the one before has finished.
    Chain(Vec<Task<M>>),
    /// A message that is ready now.
    Done(M),
    /// Work on the background runtime, then its message.
    Perform(Start<M>),
    Op(Op<M>),
}

impl<M: 'static> Task<M> {
    pub(crate) fn none() -> Self {
        Self::None
    }

    pub(crate) fn done(message: M) -> Self {
        Self::Done(message)
    }

    pub(crate) fn batch(tasks: impl IntoIterator<Item = Self>) -> Self {
        Self::Batch(tasks.into_iter().collect())
    }

    /// Runs `future` on the background runtime and turns what it returns into a message.
    pub(crate) fn perform<O: Send + 'static>(
        future: impl Future<Output = O> + Send + 'static,
        message: impl FnOnce(O) -> M + 'static,
    ) -> Self {
        Self::Perform(Box::new(move |runtime| {
            let handle = runtime.spawn(future);
            Box::pin(async move { handle.await.ok().map(message) })
        }))
    }

    /// Runs `self`, and `next` once `self` has finished.
    pub(crate) fn chain(self, next: Self) -> Self {
        match (self, next) {
            (Self::None, next) => next,
            (task, Self::None) => task,
            (Self::Chain(mut tasks), next) => {
                tasks.push(next);
                Self::Chain(tasks)
            }
            (task, next) => Self::Chain(vec![task, next]),
        }
    }

    pub(crate) fn window(op: WindowOp) -> Self {
        Self::Op(Op::Window(op))
    }

    pub(crate) fn focus(id: impl Into<String>) -> Self {
        Self::Op(Op::Focus(id.into()))
    }

    pub(crate) fn select_all(id: impl Into<String>) -> Self {
        Self::Op(Op::SelectAll(id.into()))
    }

    pub(crate) fn exit() -> Self {
        Self::Op(Op::Exit)
    }

    pub(crate) fn clipboard_write(text: String) -> Self {
        Self::Op(Op::ClipboardWrite(text))
    }

    pub(crate) fn clipboard_read(message: impl FnOnce(Option<String>) -> M + 'static) -> Self {
        Self::Op(Op::ClipboardRead(Box::new(message)))
    }

    pub(crate) fn monitor_size(next: impl FnOnce(Option<Size>) -> Self + 'static) -> Self {
        Self::Op(Op::Query(Query::MonitorSize(Box::new(next))))
    }

    pub(crate) fn window_size(next: impl FnOnce(Size) -> Self + 'static) -> Self {
        Self::Op(Op::Query(Query::Size(Box::new(next))))
    }

    pub(crate) fn minimized(next: impl FnOnce(Option<bool>) -> Self + 'static) -> Self {
        Self::Op(Op::Query(Query::Minimized(Box::new(next))))
    }

    pub(crate) fn native(
        run: impl FnOnce(&dyn raw_window_handle::HasWindowHandle) -> M + 'static,
    ) -> Self {
        Self::Op(Op::Native(Box::new(run)))
    }

    /// The same work, with its messages turned into another message type.
    pub(crate) fn map<N: 'static>(self, f: impl Fn(M) -> N + 'static) -> Task<N> {
        self.map_with(Rc::new(f))
    }

    fn map_with<N: 'static>(self, f: Rc<dyn Fn(M) -> N>) -> Task<N> {
        match self {
            Self::None => Task::None,
            Self::Batch(tasks) => {
                Task::Batch(tasks.into_iter().map(|t| t.map_with(f.clone())).collect())
            }
            Self::Chain(tasks) => {
                Task::Chain(tasks.into_iter().map(|t| t.map_with(f.clone())).collect())
            }
            Self::Done(message) => Task::Done(f(message)),
            Self::Perform(run) => Task::Perform(Box::new(move |runtime| {
                let future = run(runtime);
                Box::pin(async move { future.await.map(|message| f(message)) })
            })),
            Self::Op(op) => Task::Op(match op {
                Op::Focus(id) => Op::Focus(id),
                Op::SelectAll(id) => Op::SelectAll(id),
                Op::Exit => Op::Exit,
                Op::ClipboardWrite(text) => Op::ClipboardWrite(text),
                Op::ClipboardRead(read) => Op::ClipboardRead(Box::new(move |text| f(read(text)))),
                Op::Window(op) => Op::Window(op),
                Op::Query(query) => Op::Query(match query {
                    Query::MonitorSize(next) => {
                        Query::MonitorSize(Box::new(move |v| next(v).map_with(f)))
                    }
                    Query::Size(next) => Query::Size(Box::new(move |v| next(v).map_with(f))),
                    Query::Minimized(next) => {
                        Query::Minimized(Box::new(move |v| next(v).map_with(f)))
                    }
                }),
                Op::Native(run) => Op::Native(Box::new(move |handle| f(run(handle)))),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The messages a task holds that need nothing from the shell, in order.
    fn ready<M>(task: Task<M>, out: &mut Vec<M>) {
        match task {
            Task::Done(message) => out.push(message),
            Task::Batch(tasks) | Task::Chain(tasks) => {
                for task in tasks {
                    ready(task, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn map_turns_every_message_and_keeps_the_structure() {
        let task = Task::batch([
            Task::done(1),
            Task::done(2).chain(Task::done(3)),
            Task::none(),
        ]);
        let mut out = Vec::new();
        ready(task.map(|n| n * 10), &mut out);
        assert_eq!(out, [10, 20, 30]);
    }

    #[test]
    fn chaining_with_nothing_adds_no_step() {
        assert!(matches!(Task::<u8>::none().chain(Task::none()), Task::None));
        assert!(matches!(Task::done(1).chain(Task::none()), Task::Done(1)));
        assert!(matches!(Task::none().chain(Task::done(2)), Task::Done(2)));
        let three = Task::done(1).chain(Task::done(2)).chain(Task::done(3));
        assert!(matches!(three, Task::Chain(steps) if steps.len() == 3));
    }

    #[test]
    fn a_query_answer_continues_in_the_mapped_message_type() {
        let task: Task<u32> =
            Task::monitor_size(|size| Task::done(size.map_or(0, |size| size.width as u32)));
        let Task::Op(Op::Query(Query::MonitorSize(next))) = task.map(|n| n + 1) else {
            panic!("a mapped query stays a query");
        };
        let mut out = Vec::new();
        ready(next(Some(Size::new(1920.0, 1080.0))), &mut out);
        assert_eq!(out, [1921]);
    }
}
