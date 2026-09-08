use tokio::io::{AsyncRead, AsyncReadExt};

pub(crate) struct Capture {
  pub bytes: Vec<u8>,
  pub truncated: bool,
}

// reads to the end but keeps only `limit` bytes, so a runaway child cannot grow host memory
pub(crate) async fn capture(
  mut reader: impl AsyncRead + Unpin,
  limit: usize,
) -> std::io::Result<Capture> {
  let mut bytes = Vec::with_capacity(limit.min(8192));
  let mut buffer = [0_u8; 8192];
  let mut truncated = false;
  loop {
    let read = reader.read(&mut buffer).await?;
    if read == 0 {
      break;
    }
    let remaining = limit.saturating_sub(bytes.len());
    bytes.extend_from_slice(&buffer[..read.min(remaining)]);
    truncated |= read > remaining;
  }
  Ok(Capture { bytes, truncated })
}

pub(crate) async fn response(
  mut response: reqwest::Response,
  limit: usize,
) -> anyhow::Result<Vec<u8>> {
  let mut bytes = Vec::new();
  while let Some(chunk) = response.chunk().await? {
    if chunk.len() > limit.saturating_sub(bytes.len()) {
      anyhow::bail!("response exceeds the {limit} byte transfer limit");
    }
    bytes.extend_from_slice(&chunk);
  }
  Ok(bytes)
}
