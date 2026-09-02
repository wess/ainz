// line framing for text/event-stream bodies. chunks can split anywhere, including inside a
// multibyte character, so bytes are buffered and only whole lines are decoded

#[derive(Default)]
pub(crate) struct SseDecoder {
  buffer: Vec<u8>,
  data: String,
}

impl SseDecoder {
  // feeds one chunk and returns the data payload of every event it completes
  pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<String> {
    self.buffer.extend_from_slice(chunk);
    let mut events = Vec::new();
    while let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
      let line = String::from_utf8_lossy(&self.buffer[..end])
        .trim_end_matches('\r')
        .to_string();
      self.buffer.drain(..=end);
      self.line(&line, &mut events);
    }
    events
  }

  // flushes an event that ended with the body instead of a blank line
  pub(crate) fn finish(mut self) -> Vec<String> {
    let mut events = Vec::new();
    if !self.buffer.is_empty() {
      let line = String::from_utf8_lossy(&self.buffer)
        .trim_end_matches('\r')
        .to_string();
      self.line(&line, &mut events);
    }
    if !self.data.is_empty() {
      events.push(std::mem::take(&mut self.data));
    }
    events
  }

  fn line(&mut self, line: &str, events: &mut Vec<String>) {
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
