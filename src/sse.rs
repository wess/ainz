//! Line framing for `text/event-stream` bodies. Chunks can split anywhere, including inside
//! a multibyte character, so bytes are buffered and only whole lines are decoded.

/// One event off the wire. Most streams never name their events and the name is `None`; the
/// MCP SSE transport does, and its handshake is a named `endpoint` event, so the name has to
/// survive the decoder rather than being framing it throws away.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SseEvent {
  pub(crate) name: Option<String>,
  pub(crate) data: String,
}

#[derive(Default)]
pub(crate) struct SseDecoder {
  buffer: Vec<u8>,
  /// How much of `buffer` is known to hold no newline, so a long line arriving in many small
  /// chunks is scanned once rather than once per chunk.
  scanned: usize,
  name: Option<String>,
  data: String,
}

impl SseDecoder {
  pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<String> {
    self
      .push_named(chunk)
      .into_iter()
      .map(|event| event.data)
      .collect()
  }
  pub(crate) fn finish(self) -> Vec<String> {
    self
      .finish_named()
      .into_iter()
      .map(|event| event.data)
      .collect()
  }

  pub(crate) fn retained_len(&self) -> usize {
    self.buffer.len() + self.name.as_ref().map_or(0, String::len) + self.data.len()
  }

  pub(crate) fn push_named(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
    let mut buffer = std::mem::take(&mut self.buffer);
    buffer.extend_from_slice(chunk);
    let mut events = Vec::new();
    let mut start = 0;
    let mut from = self.scanned;
    while let Some(offset) = buffer[from..].iter().position(|byte| *byte == b'\n') {
      let end = from + offset;
      self.line(&buffer[start..end], &mut events);
      start = end + 1;
      from = start;
    }
    buffer.drain(..start);
    self.scanned = buffer.len();
    self.buffer = buffer;
    events
  }

  /// Flushes an event that ended with the body instead of a blank line.
  pub(crate) fn finish_named(mut self) -> Vec<SseEvent> {
    let mut events = Vec::new();
    let buffer = std::mem::take(&mut self.buffer);
    if !buffer.is_empty() {
      self.line(&buffer, &mut events);
    }
    self.flush(&mut events);
    events
  }

  fn line(&mut self, line: &[u8], events: &mut Vec<SseEvent>) {
    let line = String::from_utf8_lossy(line.strip_suffix(b"\r").unwrap_or(line));
    if line.is_empty() {
      self.flush(events);
    } else if let Some(payload) = line.strip_prefix("data:") {
      if !self.data.is_empty() {
        self.data.push('\n');
      }
      self
        .data
        .push_str(payload.strip_prefix(' ').unwrap_or(payload));
    } else if let Some(payload) = line.strip_prefix("event:") {
      self.name = Some(payload.strip_prefix(' ').unwrap_or(payload).to_string());
    }
    // id and retry are framing this decoder has no use for
  }

  fn flush(&mut self, events: &mut Vec<SseEvent>) {
    if !self.data.is_empty() {
      events.push(SseEvent {
        name: self.name.take(),
        data: std::mem::take(&mut self.data),
      });
    } else {
      // a comment or a lone event: line, with nothing under it
      self.name = None;
    }
  }
}

#[cfg(test)]
#[path = "../tests/sse/mod.rs"]
mod tests;
