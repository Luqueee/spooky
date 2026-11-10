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

/// Whether `error` is a [`Cancelled`] connection attempt rather than a failure.
pub fn was_cancelled(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Cancelled>().is_some()
}

/// Whether `error` is [`Unrecoverable`]: retrying the same target is pointless.
pub fn is_unrecoverable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Unrecoverable>().is_some()
}
