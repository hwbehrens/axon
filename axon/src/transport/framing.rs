use anyhow::{Context, Result, anyhow};
use tokio::time::timeout;

use crate::message::{Envelope, MAX_MESSAGE_SIZE};

use super::{MAX_MESSAGE_SIZE_USIZE, REQUEST_TIMEOUT};

pub(crate) async fn send_unidirectional(
    connection: &quinn::Connection,
    envelope: Envelope,
) -> Result<()> {
    let bytes = serde_json::to_vec(&envelope).context("failed to serialize envelope")?;
    if bytes.len() > MAX_MESSAGE_SIZE_USIZE {
        return Err(anyhow!("message exceeds max size {MAX_MESSAGE_SIZE} bytes"));
    }

    let mut stream = connection
        .open_uni()
        .await
        .context("failed to open uni stream")?;
    write_framed(&mut stream, &bytes).await?;
    stream.finish().context("failed to finish uni stream")?;
    Ok(())
}

pub(crate) async fn send_request(
    connection: &quinn::Connection,
    envelope: Envelope,
) -> Result<Envelope> {
    let bytes = serde_json::to_vec(&envelope).context("failed to serialize request")?;
    if bytes.len() > MAX_MESSAGE_SIZE_USIZE {
        return Err(anyhow!("message exceeds max size {MAX_MESSAGE_SIZE} bytes"));
    }

    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .context("failed to open bidi stream")?;
    write_framed(&mut send, &bytes).await?;
    send.finish().context("failed to finish request stream")?;

    let response_bytes = timeout(REQUEST_TIMEOUT, read_framed(&mut recv))
        .await
        .context("request timed out after 30s")??;
    let response = serde_json::from_slice::<Envelope>(&response_bytes)
        .context("failed to decode response envelope")?;
    response
        .validate()
        .context("response envelope failed validation")?;
    Ok(response)
}

pub(crate) async fn write_framed(stream: &mut quinn::SendStream, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_MESSAGE_SIZE_USIZE {
        return Err(anyhow!("message too large for framing"));
    }

    stream
        .write_all(bytes)
        .await
        .context("failed to write frame body")?;
    Ok(())
}

pub(crate) async fn read_framed(stream: &mut quinn::RecvStream) -> Result<Vec<u8>> {
    let buf = stream
        .read_to_end(MAX_MESSAGE_SIZE_USIZE)
        .await
        .context("failed to read frame body")?;
    Ok(buf)
}
