use crate::daemon::{serve, write_response};
use anyhow::{Context, Result, bail};
use spooky_core::{MAX_MESSAGE_BYTES, Request, Response, ServerStatus};
use std::{
    env,
    io::{self, BufRead, Read},
    thread,
    time::Duration,
};

fn bridge() -> Result<()> {
    use spooky_core::client::{Client, Target};
    let mut client = Client::connect(&Target::Local)?;
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    loop {
        let mut line = Vec::new();
        let n = (&mut input)
            .take((MAX_MESSAGE_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if n == 0 {
            break;
        }
        if n > MAX_MESSAGE_BYTES || !line.ends_with(b"\n") {
            write_response(
                &mut output,
                &Response::Error("request too large or unterminated".into()),
            )?;
            break;
        }
        let response = match serde_json::from_slice::<Request>(&line) {
            Ok(request) => match client.request(&request) {
                Ok(response) => response,
                Err(error) => Response::Error(format!("server request failed: {error:#}")),
            },
            Err(error) => Response::Error(format!("invalid request: {error}")),
        };
        write_response(&mut output, &response)?;
    }
    Ok(())
}
/// The running local server, if any. Never starts one.
fn running_server() -> Result<Option<spooky_core::client::Client>> {
    spooky_core::client::Client::attach_local().context(
        "talking to the running server; one from an older Spooky cannot be asked to stop, end its process instead",
    )
}
fn server_status(client: &mut spooky_core::client::Client) -> Result<ServerStatus> {
    match client.request(&Request::Status)? {
        Response::Status(status) => Ok(status),
        Response::Error(message) => bail!("server refused: {message}"),
        other => bail!("unexpected reply to status: {other:?}"),
    }
}
pub(crate) fn status() -> Result<()> {
    let Some(mut client) = running_server()? else {
        println!("no Spooky server is running");
        return Ok(());
    };
    let status = server_status(&mut client)?;
    println!(
        "Spooky server pid {}, protocol {}, up {}s: {} workspace(s), {} tab(s), {} shell(s) of which {} running",
        status.pid,
        status.version,
        status.uptime_secs,
        status.workspaces,
        status.tabs,
        status.panes,
        status.running
    );
    Ok(())
}
/// Stops the local server. Refuses while shells are running unless `force`,
/// because stopping ends them.
pub(crate) fn stop(force: bool) -> Result<()> {
    let Some(mut client) = running_server()? else {
        println!("no Spooky server is running");
        return Ok(());
    };
    let status = server_status(&mut client)?;
    if status.running > 0 && !force {
        bail!(
            "{} shell(s) are running in {} workspace(s) and stopping the server ends them. \
             Run `spooky-server stop --force` to stop anyway; the workspaces come back, with \
             new shells, the next time a server starts",
            status.running,
            status.workspaces
        );
    }
    match client.request(&Request::Shutdown)? {
        Response::Ok => {}
        Response::Error(message) => bail!("server refused to stop: {message}"),
        other => bail!("unexpected reply to shutdown: {other:?}"),
    }
    for _ in 0..200 {
        if matches!(spooky_core::client::Client::attach_local(), Ok(None)) {
            println!("stopped the Spooky server (pid {})", status.pid);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    bail!("the server acknowledged the request but is still running after 5 seconds")
}
pub fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref().unwrap_or("serve") {
        "serve" => serve(),
        "bridge" => bridge(),
        "status" => status(),
        "stop" => match args.next().as_deref() {
            None => stop(false),
            Some("--force") => stop(true),
            Some(other) => bail!("unknown option for stop: {other}"),
        },
        mode => bail!("unknown server mode: {mode} (expected serve, bridge, status or stop)"),
    }
}
