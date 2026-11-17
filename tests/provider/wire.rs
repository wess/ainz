use super::*;

fn replay(lines: &[&str]) -> Result<ProviderReply> {
  let events = EventSink::default();
  let mut stream = ChatStream::default();
  for line in lines {
    if stream.push(line, &events)? {
      break;
    }
  }
  stream.finish()
}

#[test]
fn null_fields_in_a_chunk_read_as_absent() {
  let reply = replay(&[
    r#"{"choices":null,"usage":null}"#,
    r#"{"choices":[{"delta":null,"finish_reason":null}]}"#,
    r#"{"choices":[{"delta":{"content":"hi","tool_calls":null},"finish_reason":null}]}"#,
    r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
  ])
  .unwrap();
  assert_eq!(reply.message.content.as_deref(), Some("hi"));
  assert!(reply.message.tool_calls.is_empty());
}

#[test]
fn usage_with_missing_counts_defaults_to_zero() {
  let reply = replay(&[
    r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4}}"#,
  ])
  .unwrap();
  assert_eq!(reply.usage.input_tokens, 4);
  assert_eq!(reply.usage.output_tokens, 0);
}

#[test]
fn calls_without_an_index_split_on_a_new_id() {
  let reply = replay(&[
    r#"{"choices":[{"delta":{"tool_calls":[{"id":"a","function":{"name":"read","arguments":"{\"path\":"}}]}}]}"#,
    r#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"\"x\"}"}}]}}]}"#,
    r#"{"choices":[{"delta":{"tool_calls":[{"id":"b","function":{"name":"list","arguments":null}}]}}]}"#,
    r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
  ])
  .unwrap();
  let calls = &reply.message.tool_calls;
  assert_eq!(calls.len(), 2);
  assert_eq!(calls[0].id, "a");
  assert_eq!(calls[0].arguments, json!({"path": "x"}));
  assert_eq!(calls[1].name, "list");
  assert_eq!(calls[1].arguments, json!({}));
}

#[test]
fn unparseable_arguments_keep_the_call_with_the_raw_text() {
  let reply = replay(&[
    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"write","arguments":"{\"path\":\"x\",\"content\":\"hal"}}]}}]}"#,
    r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#,
  ])
  .unwrap();
  let call = &reply.message.tool_calls[0];
  assert_eq!(call.name, "write");
  assert_eq!(
    call.arguments,
    json!({INVALID_ARGUMENTS: r#"{"path":"x","content":"hal"#})
  );
}

#[test]
fn arguments_that_are_json_but_not_an_object_are_kept_as_invalid() {
  assert_eq!(
    arguments("[1, 2]".into()),
    json!({INVALID_ARGUMENTS: "[1, 2]"})
  );
}

#[test]
fn a_reply_cut_off_at_the_length_limit_is_kept_and_marked() {
  let reply =
    replay(&[r#"{"choices":[{"delta":{"content":"half"},"finish_reason":"length"}]}"#]).unwrap();
  assert!(reply.truncated);
  let content = reply.message.content.unwrap();
  assert!(content.starts_with("half"));
  assert!(content.ends_with(CUT_OFF));
}

#[test]
fn a_completed_reply_quoting_the_cutoff_marker_is_not_cut_off() {
  let quoted = serde_json::json!({
    "choices": [{"delta": {"content": format!("the marker reads{CUT_OFF}")}, "finish_reason": "stop"}],
  })
  .to_string();
  let reply = replay(&[&quoted]).unwrap();
  assert!(!reply.truncated);
}

#[test]
fn a_whole_reply_cut_off_at_its_length_limit_is_flagged() {
  let body = br#"{"choices":[{"message":{"content":"half"},"finish_reason":"length"}]}"#;
  let reply = parse_response(body, &EventSink::default()).unwrap();
  assert!(reply.truncated);
  assert!(reply.message.content.unwrap().ends_with(CUT_OFF));
}

#[test]
fn a_filtered_reply_is_an_error() {
  let error =
    replay(&[r#"{"choices":[{"delta":{"content":"x"},"finish_reason":"content_filter"}]}"#])
      .unwrap_err();
  assert!(error.to_string().contains("content_filter"), "{error}");
}

#[test]
fn an_unknown_finish_reason_still_ends_the_reply() {
  let reply =
    replay(&[r#"{"choices":[{"delta":{"content":"done"},"finish_reason":"end_turn"}]}"#]).unwrap();
  assert_eq!(reply.message.content.as_deref(), Some("done"));
}

#[test]
fn an_error_event_in_the_stream_surfaces_its_message() {
  let error = replay(&[
    r#"{"choices":[{"delta":{"content":"he"}}]}"#,
    r#"{"error":{"message":"upstream overloaded","type":"server_error"}}"#,
  ])
  .unwrap_err();
  assert!(error.to_string().contains("upstream overloaded"), "{error}");
}

#[test]
fn an_error_body_in_a_successful_response_surfaces_its_message() {
  let body = br#"{"error":{"message":"model not found"}}"#;
  let error = parse_response(body, &EventSink::default()).unwrap_err();
  assert!(error.to_string().contains("model not found"), "{error}");
}

#[test]
fn a_whole_response_tolerates_nulls_and_bad_arguments() {
  let body = br#"{"choices":[{"message":{"content":"ok","tool_calls":[{"id":"a","function":{"name":"read","arguments":"nope"}}]},"finish_reason":"tool_calls"}],"usage":null}"#;
  let reply = parse_response(body, &EventSink::default()).unwrap();
  assert_eq!(
    reply.message.tool_calls[0].arguments,
    json!({INVALID_ARGUMENTS: "nope"})
  );
}

#[test]
fn the_error_sentence_is_found_wherever_the_provider_put_it() {
  assert_eq!(
    error_message(r#"{"error":{"message":"bad key"}}"#),
    "bad key"
  );
  assert_eq!(error_message(r#"{"message":"slow down"}"#), "slow down");
  assert_eq!(error_message(r#"{"detail":"not found"}"#), "not found");
  assert_eq!(error_message(r#"{"error":"plain"}"#), "plain");
  assert_eq!(error_message("gateway timeout"), "gateway timeout");
}

#[test]
fn an_upstream_error_quoted_inside_a_message_is_unwrapped() {
  let body = r#"{"error":{"message":"upstream - {\"error\":{\"message\":\"quota exceeded\"}}"}}"#;
  assert_eq!(error_message(body), "upstream: quota exceeded");
}

#[test]
fn a_status_with_an_empty_body_reads_as_the_status() {
  assert_eq!(wire_error(StatusCode::BAD_GATEWAY, ""), "502 Bad Gateway");
  assert_eq!(
    wire_error(
      StatusCode::UNAUTHORIZED,
      r#"{"error":{"message":"bad key"}}"#
    ),
    "401 Unauthorized: bad key"
  );
}
