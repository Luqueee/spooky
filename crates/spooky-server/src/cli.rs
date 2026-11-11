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
        "Spooky server pid {}, protocol {}, up {}s: {} workspace(s), {} tab(s), {} shell(s) of which {} running, {} agent(s) running",
        status.pid,
        status.version,
        status.uptime_secs,
        status.workspaces,
        status.tabs,
        status.panes,
        status.running,
        status.agents
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
    if status.running + status.agents > 0 && !force {
        bail!(
            "{} shell(s) and {} agent(s) are running in {} workspace(s) and stopping the server ends them. \
             Run `spooky-server stop --force` to stop anyway; the workspaces come back, with \
             new shells and agents resumed from their sessions, the next time a server starts",
            status.running,
            status.agents,
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
/// Asks the running server to open an agent tab next to the pane this command runs in,
/// in the current directory. The `omp` shim runs this.
fn agent_open(backend: &str, args: Vec<String>) -> Result<()> {
    let pane: u64 = env::var("SPOOKY_PANE")
        .context("SPOOKY_PANE is not set: this only works inside a Spooky pane")?
        .parse()
        .context("SPOOKY_PANE is not a pane number")?;
    let cwd = env::current_dir().context("reading the current directory")?;
    let Some(mut client) = running_server()? else {
        bail!("no Spooky server is running");
    };
    match client.request(&Request::OpenAgent {
        pane,
        backend: backend.to_owned(),
        cwd: cwd.to_string_lossy().into_owned(),
        args,
    })? {
        Response::Ok => {
            println!("Opened {backend} in this pane.");
            Ok(())
        }
        Response::Error(message) => bail!("the server refused: {message}"),
        other => bail!("unexpected reply to agent-open: {other:?}"),
    }
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
        "agent-open" => {
            let backend = args.next().unwrap_or_else(|| "omp".into());
            // `--` ends this command's own words; everything after is the user's.
            let rest: Vec<String> = args.skip_while(|a| a != "--").skip(1).collect();
            agent_open(&backend, rest)
        }
        mode => bail!(
            "unknown server mode: {mode} (expected serve, bridge, status, stop or agent-open)"
        ),
    }
}
