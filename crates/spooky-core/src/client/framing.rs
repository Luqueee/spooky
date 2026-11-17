use super::errors::{Problem, RemoteProblem};
use crate::{MAX_MESSAGE_BYTES, PROTOCOL_VERSION, Request, Response};
use anyhow::{Context, Result, bail};
use std::io::BufRead;

pub(super) fn encode_request(request: &Request) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(request).context("serializing spooky request")?;
    if bytes.len() + 1 > MAX_MESSAGE_BYTES {
        bail!("serialized spooky request exceeds the {MAX_MESSAGE_BYTES}-byte protocol limit");
    }
    bytes.push(b'\n');
    Ok(bytes)
}

pub(super) fn read_frame(reader: &mut impl BufRead) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().context("reading protocol frame")?;
        if available.is_empty() {
            bail!("server closed the connection before completing a response frame");
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        if bytes.len() + consumed > MAX_MESSAGE_BYTES {
            bail!("server response exceeds the {MAX_MESSAGE_BYTES}-byte protocol limit");
        }
        bytes.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if newline.is_some() {
            bytes.truncate(bytes.len() - 1);
            return Ok(bytes);
        }
    }
}

pub(super) fn decode_response(frame: &[u8]) -> Result<Response> {
    serde_json::from_slice(frame).context("invalid JSON response")
}

pub(super) fn check_pong(response: Response) -> Result<()> {
    match response {
        Response::Pong { version } if version == PROTOCOL_VERSION => Ok(()),
        Response::Pong { version } => Err(incompatible(version, PROTOCOL_VERSION).into()),
        Response::Error(message) => bail!("server rejected protocol handshake: {message}"),
        _ => bail!("server returned an unexpected response to protocol handshake"),
    }
}

const INCOMPATIBLE_SERVER: &str = "server protocol version ";
const INCOMPATIBLE_CLIENT: &str = " is incompatible with client version ";

/// The failed handshake between a client speaking `client` and a server speaking
/// `server`. Its wording is what [`reported_incompatibility`] recognizes.
pub(super) fn incompatible(server: u32, client: u32) -> RemoteProblem {
    RemoteProblem {
        problem: Problem::Incompatible { server, client },
        message: format!(
            "{INCOMPATIBLE_SERVER}{server}{INCOMPATIBLE_CLIENT}{client}. \
             Update both ends and restart the server, which ends its shells."
        ),
    }
}

/// The incompatibility a remote `spooky-server bridge` reported in `text`, its
/// diagnostics. A bridge newer than the server running on its machine fails its
/// own handshake there and exits with the [`incompatible`] message on stderr,
/// before it answers the client's handshake.
pub(super) fn reported_incompatibility(text: &str) -> Option<RemoteProblem> {
    let (_, rest) = text.split_once(INCOMPATIBLE_SERVER)?;
    let (server, rest) = leading_number(rest)?;
    let (client, _) = leading_number(rest.strip_prefix(INCOMPATIBLE_CLIENT)?)?;
    Some(incompatible(server, client))
}

fn leading_number(text: &str) -> Option<(u32, &str)> {
    let end = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());
    Some((text[..end].parse().ok()?, &text[end..]))
}
