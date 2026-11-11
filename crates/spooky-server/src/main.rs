//! Persistent Unix PTY sessions and the SSH stdio bridge.

#[cfg(unix)]
#[macro_use]
mod log;
#[cfg(unix)]
mod agent;
#[cfg(unix)]
mod cli;
#[cfg(unix)]
mod daemon;
#[cfg(unix)]
mod pane;
#[cfg(unix)]
mod persist;
#[cfg(unix)]
mod search;
#[cfg(unix)]
mod server;
#[cfg(unix)]
mod settings;
#[cfg(unix)]
mod shim;
#[cfg(unix)]
mod state;

#[cfg(unix)]
fn main() -> anyhow::Result<()> {
    cli::main()
}
#[cfg(not(unix))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("spooky-server requires Unix; Windows is unsupported")
}
