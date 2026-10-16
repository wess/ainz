use super::*;

#[test]
fn an_event_is_its_data() {
  let mut decoder = SseDecoder::default();
  assert_eq!(decoder.push(b"data: hello\n\n"), ["hello"]);
}

#[test]
fn a_chunk_may_split_anywhere_including_mid_character() {
  let mut decoder = SseDecoder::default();
  let mut events = Vec::new();
  for byte in "event: message\ndata: café\n\n".as_bytes() {
    events.extend(decoder.push(&[*byte]));
  }
  assert_eq!(events, ["café"]);
}

#[test]
fn multiple_data_lines_join_with_newlines() {
  let mut decoder = SseDecoder::default();
  assert_eq!(decoder.push(b"data: one\ndata: two\n\n"), ["one\ntwo"]);
}

#[test]
fn crlf_line_endings_split_across_chunks_still_frame_events() {
  let mut decoder = SseDecoder::default();
  assert!(decoder.push(b"data:one\r").is_empty());
  assert!(decoder.push(b"\ndata: two\r\n\r").is_empty());
  assert_eq!(decoder.push(b"\n"), ["one\ntwo"]);
}

#[test]
fn a_long_line_in_many_chunks_arrives_whole() {
  let mut decoder = SseDecoder::default();
  let payload = "x".repeat(10_000);
  decoder.push(b"data: ");
  for piece in payload.as_bytes().chunks(7) {
    assert!(decoder.push(piece).is_empty());
  }
  assert_eq!(decoder.push(b"\n\n"), [payload]);
}

#[test]
fn several_events_in_one_chunk_all_come_out() {
  let mut decoder = SseDecoder::default();
  assert_eq!(
    decoder.push(b"data: one\n\ndata: two\n\ndata: thr"),
    ["one", "two"]
  );
  assert_eq!(decoder.push(b"ee\n\n"), ["three"]);
}

#[test]
fn a_body_that_ends_without_a_blank_line_still_flushes() {
  let mut decoder = SseDecoder::default();
  assert!(decoder.push(b"data: last").is_empty());
  assert_eq!(decoder.finish(), ["last"]);
}
