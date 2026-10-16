// line framing for text/event-stream bodies. chunks can split anywhere, including inside a
// multibyte character, so bytes are buffered and only whole lines are decoded

#[derive(Default)]
pub(crate) struct SseDecoder {
  buffer: Vec<u8>,
  // how much of `buffer` is already known to hold no newline, so a long line that arrives in
  // many small chunks is scanned once rather than once per chunk
  scanned: usize,
  data: String,
}

impl SseDecoder {
  // feeds one chunk and returns the data payload of every event it completes
  pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<String> {
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
    // one shift per chunk, not one per line
    buffer.drain(..start);
    self.scanned = buffer.len();
    self.buffer = buffer;
    events
  }

  // flushes an event that ended with the body instead of a blank line
  pub(crate) fn finish(mut self) -> Vec<String> {
    let mut events = Vec::new();
    let buffer = std::mem::take(&mut self.buffer);
    if !buffer.is_empty() {
      self.line(&buffer, &mut events);
    }
    if !self.data.is_empty() {
      events.push(std::mem::take(&mut self.data));
    }
    events
  }

  fn line(&mut self, line: &[u8], events: &mut Vec<String>) {
    let line = String::from_utf8_lossy(line.strip_suffix(b"\r").unwrap_or(line));
    if line.is_empty() {
      if !self.data.is_empty() {
        events.push(std::mem::take(&mut self.data));
      }
    } else if let Some(payload) = line.strip_prefix("data:") {
      if !self.data.is_empty() {
        self.data.push('\n');
      }
      self
        .data
        .push_str(payload.strip_prefix(' ').unwrap_or(payload));
    }
  }
}

#[cfg(test)]
#[path = "../tests/sse/mod.rs"]
mod tests;
