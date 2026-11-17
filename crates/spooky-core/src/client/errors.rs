use std::fmt;

/// A connection attempt abandoned because the caller no longer wants it.
#[derive(Debug)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("connection attempt cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// A connection failure that retrying cannot fix until the user changes
/// something, such as authorizing a key or installing `spooky-server`. It
/// displays as the explanation to show the user.
#[derive(Debug)]
pub struct Unrecoverable(pub(super) String);

impl fmt::Display for Unrecoverable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unrecoverable {}

/// What a reachable remote machine lacks, which Spooky can offer to put right by
/// installing or updating `spooky-server` there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The account answers, but has no `spooky-server` on its path.
    ServerMissing,
    /// A server answers, but speaks another protocol version than this client.
    Incompatible {
        /// The protocol version the server on the machine speaks.
        server: u32,
        /// The protocol version this client speaks.
        client: u32,
    },
}

/// A remote machine that needs `spooky-server` installed or updated. Like
/// [`Unrecoverable`], retrying the same target cannot fix it.
#[derive(Debug)]
pub struct RemoteProblem {
    pub(super) problem: Problem,
    pub(super) message: String,
}

impl fmt::Display for RemoteProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RemoteProblem {}

/// What a remote lacks, if `error` says so: the cue to offer an installation.
pub fn remote_problem(error: &anyhow::Error) -> Option<Problem> {
    error
        .downcast_ref::<RemoteProblem>()
        .map(|remote| remote.problem)
}

/// Whether `error` is a [`Cancelled`] connection attempt rather than a failure.
pub fn was_cancelled(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Cancelled>().is_some()
}

/// Whether `error` is [`Unrecoverable`]: retrying the same target is pointless.
pub fn is_unrecoverable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Unrecoverable>().is_some() || remote_problem(error).is_some()
}
