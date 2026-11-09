//! Read-only discovery through the user's installed Tailscale client.
use serde::Deserialize;
use std::{collections::HashMap, net::IpAddr, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

#[derive(Clone, Debug)]
pub(super) struct Peer {
    pub name: String,
    pub host: String,
    pub online: bool,
    pub os: String,
}

#[derive(Clone, Debug)]
pub(super) struct Network {
    pub peers: Vec<Peer>,
}

#[derive(Default)]
pub(super) enum Discovery {
    #[default]
    Idle,
    Loading,
    Ready(Network),
    Failed(String),
}

#[derive(Deserialize)]
struct Status {
    #[serde(rename = "BackendState")]
    state: String,
    #[serde(rename = "Peer")]
    peers: Option<HashMap<String, Node>>,
    #[serde(rename = "Self")]
    this_node: Option<Node>,
}

#[derive(Deserialize)]
struct Node {
    #[serde(rename = "ID", default)]
    id: String,
    #[serde(rename = "HostName", default)]
    name: String,
    #[serde(rename = "DNSName", default)]
    dns: String,
    #[serde(rename = "TailscaleIPs", default)]
    ips: Vec<IpAddr>,
    #[serde(rename = "Online", default)]
    online: bool,
    #[serde(rename = "OS", default)]
    os: String,
}

fn parse(bytes: &[u8]) -> Result<Network, String> {
    let status: Status =
        serde_json::from_slice(bytes).map_err(|e| format!("Invalid Tailscale status: {e}"))?;
    match status.state.as_str() {
        "Running" => {}
        "NeedsLogin" | "NeedsMachineAuth" => {
            return Err("Finish signing in or approving this device in Tailscale first.".into());
        }
        state => {
            return Err(format!(
                "Tailscale is {state}. Connect in the Tailscale app first."
            ));
        }
    }
    let mut peers = Vec::new();
    for node in status.peers.unwrap_or_default().into_values() {
        if status.this_node.as_ref().is_some_and(|own| {
            (!own.id.is_empty() && own.id == node.id)
                || node.ips.iter().any(|ip| own.ips.contains(ip))
        }) {
            continue;
        }
        let ip = node.ips.iter().find(|ip| ip.is_ipv4()).or(node.ips.first());
        let Some(ip) = ip else {
            return Err(format!(
                "Tailscale returned a device without an address: {}",
                node.name
            ));
        };
        let name = if !node.name.is_empty() {
            node.name
        } else if !node.dns.is_empty() {
            node.dns.trim_end_matches('.').to_owned()
        } else {
            ip.to_string()
        };
        peers.push(Peer {
            name,
            host: ip.to_string(),
            online: node.online,
            os: node.os,
        });
    }
    peers.sort_by(|a, b| {
        b.online
            .cmp(&a.online)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.host.cmp(&b.host))
    });
    Ok(Network { peers })
}

async fn bounded_output(
    reader: impl tokio::io::AsyncRead + Unpin,
    limit: u64,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Tailscale status exceeded the output limit.".into());
    }
    Ok(bytes)
}

async fn run() -> Result<Network, String> {
    // Finder may not inherit the shell's PATH. Both macOS distributions expose a CLI.
    let candidates = if cfg!(target_os = "macos") {
        &[
            "tailscale",
            "/usr/local/bin/tailscale",
            "/opt/homebrew/bin/tailscale",
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        ][..]
    } else {
        &["tailscale"][..]
    };
    for executable in candidates {
        let child = Command::new(executable)
            .args(["status", "--json"])
            .env("TAILSCALE_BE_CLI", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("Cannot start Tailscale: {e}")),
        };
        let stdout = child.stdout.take().ok_or("Tailscale stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("Tailscale stderr unavailable")?;
        let (out, err, exit) = iced::futures::future::try_join3(
            bounded_output(stdout, 8 * 1024 * 1024),
            bounded_output(stderr, 16 * 1024),
            async { child.wait().await.map_err(|e| e.to_string()) },
        )
        .await?;
        if !exit.success() {
            return Err(format!(
                "Tailscale status failed: {}",
                String::from_utf8_lossy(&err).trim()
            ));
        }
        return parse(&out);
    }
    Err(
        "Tailscale was not found. Install its client and enable CLI integration, then refresh."
            .into(),
    )
}

pub(super) async fn discover() -> Result<Network, String> {
    tokio::time::timeout(Duration::from_secs(10), run())
        .await
        .map_err(|_| {
            "Tailscale did not respond within 10 seconds. Check its client and refresh.".to_owned()
        })?
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn discovery_excludes_self_and_uses_addresses_not_display_names() {
        let network = parse(br#"{"BackendState":"Running","Self":{"ID":"self","TailscaleIPs":["100.64.0.1"]},"Peer":{"self":{"ID":"self","TailscaleIPs":["100.64.0.1"]},"offline":{"HostName":"sleeping","Online":false,"TailscaleIPs":["fd7a:115c:a1e0::2"]},"online":{"HostName":"-oProxyCommand=bad","Online":true,"TailscaleIPs":["fd7a:115c:a1e0::3","100.64.0.3"]}}}"#).expect("valid status");
        assert_eq!(
            network
                .peers
                .iter()
                .map(|p| p.host.as_str())
                .collect::<Vec<_>>(),
            ["100.64.0.3", "fd7a:115c:a1e0::2"]
        );
        assert!(network.peers[0].online);
        assert!(!network.peers[1].online);
    }

    #[test]
    fn discovery_rejects_unusable_or_unauthenticated_status() {
        for status in [
            r#"{"BackendState":"NeedsLogin","Peer":null}"#,
            r#"{"BackendState":"Running","Peer":{"bad":{"TailscaleIPs":["host;command"]}}}"#,
            r#"{"BackendState":"Running","Peer":{"missing":{"HostName":"missing"}}}"#,
        ] {
            assert!(parse(status.as_bytes()).is_err());
        }
        assert!(
            parse(br#"{"BackendState":"Running","Peer":null}"#)
                .expect("empty tailnet")
                .peers
                .is_empty()
        );
    }
}
