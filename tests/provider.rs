use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::{TcpListener, TcpStream},
};

use ainz::{
  EventSink, HttpProvider, PermissionMode, ProcessOutput, ProcessProvider,
  protocol::{Image, Message, Role},
  provider::ChatProvider,
};

#[tokio::test]
async fn process_provider_passes_the_transcript_over_stdin() {
  let provider = ProcessProvider::new(
    "/bin/sh".into(),
    vec!["-c".into(), "cat".into()],
    "test".into(),
    std::env::current_dir().unwrap(),
    PermissionMode::ReadOnly,
    ProcessOutput::Text,
  );

  let reply = provider
    .complete(
      &[Message::text(Role::User, "hello from stdin")],
      &[],
      &EventSink::default(),
    )
    .await
    .unwrap();

  assert!(
    reply
      .message
      .content
      .as_deref()
      .unwrap()
      .contains("hello from stdin")
  );
}

#[tokio::test]
async fn provider_lists_http_models() {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let server = tokio::spawn(async move {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut request = vec![0; 16 * 1024];
    let _ = socket.read(&mut request).await.unwrap();
    let body = r#"{"data":[{"id":"zeta"},{"id":"alpha"}]}"#;
    let response = format!(
      "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
      body.len(),
      body,
    );
    socket.write_all(response.as_bytes()).await.unwrap();
  });
  let provider = HttpProvider::new(format!("http://{address}"), "test".into(), None).unwrap();

  assert_eq!(provider.models().await.unwrap(), ["alpha", "zeta"]);
  server.await.unwrap();
}

#[tokio::test]
async fn provider_assembles_streamed_text_and_usage() {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let server = tokio::spawn(async move {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut request = vec![0; 16 * 1024];
    let _ = socket.read(&mut request).await.unwrap();
    let body = concat!(
      "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\r\n\r\n",
      "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\r\n\r\n",
      "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2}}\r\n\r\n",
      "data: [DONE]\r\n\r\n",
    );
    let response = format!(
      "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
      body.len(),
      body,
    );
    socket.write_all(response.as_bytes()).await.unwrap();
  });
  let provider = HttpProvider::new(format!("http://{address}"), "test".into(), None).unwrap();

  let reply = provider
    .complete(
      &[Message::text(Role::User, "hi")],
      &[],
      &EventSink::default(),
    )
    .await
    .unwrap();

  assert_eq!(reply.message.content.as_deref(), Some("hello"));
  assert_eq!(reply.usage.input_tokens, 4);
  assert_eq!(reply.usage.output_tokens, 2);
  server.await.unwrap();
}

#[tokio::test]
async fn provider_sends_multimodal_content_parts() {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let (sender, receiver) = tokio::sync::oneshot::channel();
  let server = tokio::spawn(async move {
    let (mut socket, _) = listener.accept().await.unwrap();
    sender.send(read_request(&mut socket).await).unwrap();
    let body = r#"{"choices":[{"message":{"content":"seen","tool_calls":[]}}]}"#;
    let response = format!(
      "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
      body.len(),
      body,
    );
    socket.write_all(response.as_bytes()).await.unwrap();
  });
  let provider = HttpProvider::new(format!("http://{address}"), "test".into(), None).unwrap();
  let message = Message::user(
    "inspect",
    vec![Image {
      url: "data:image/png;base64,AQID".into(),
      detail: Some("high".into()),
    }],
  );

  provider
    .complete(&[message], &[], &EventSink::default())
    .await
    .unwrap();
  let request = String::from_utf8(receiver.await.unwrap()).unwrap();
  assert!(request.contains(r#""type":"image_url""#));
  assert!(request.contains(r#"data:image/png;base64,AQID"#));
  assert!(request.contains(r#""detail":"high""#));
  server.await.unwrap();
}

async fn read_request(socket: &mut TcpStream) -> Vec<u8> {
  let mut request = Vec::new();
  let mut buffer = [0_u8; 4096];
  loop {
    let read = socket.read(&mut buffer).await.unwrap();
    if read == 0 {
      break;
    }
    request.extend_from_slice(&buffer[..read]);
    let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
      continue;
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let length = headers
      .lines()
      .find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name
          .eq_ignore_ascii_case("content-length")
          .then(|| value.trim().parse::<usize>().ok())
          .flatten()
      })
      .unwrap_or(0);
    if request.len() >= header_end + 4 + length {
      break;
    }
  }
  request
}

#[tokio::test]
async fn provider_reassembles_multibyte_text_split_across_chunks() {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let server = tokio::spawn(async move {
    let (mut socket, _) = listener.accept().await.unwrap();
    let request = read_request(&mut socket).await;
    // the compaction request offers no tools, so neither key may appear
    assert!(!String::from_utf8_lossy(&request).contains("\"tools\""));
    let body =
      "data: {\"choices\":[{\"delta\":{\"content\":\"caf\u{e9} \u{1f600}\"}}]}\n\ndata: [DONE]\n\n";
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
    socket.write_all(head.as_bytes()).await.unwrap();
    // split inside the four-byte emoji and inside the two-byte e-acute
    let bytes = body.as_bytes();
    let cuts = [
      bytes.iter().position(|b| *b == 0xC3).unwrap() + 1,
      bytes.iter().position(|b| *b == 0xF0).unwrap() + 2,
    ];
    socket.write_all(&bytes[..cuts[0]]).await.unwrap();
    socket.flush().await.unwrap();
    socket.write_all(&bytes[cuts[0]..cuts[1]]).await.unwrap();
    socket.flush().await.unwrap();
    socket.write_all(&bytes[cuts[1]..]).await.unwrap();
  });
  let provider = HttpProvider::new(format!("http://{address}"), "test".into(), None).unwrap();

  let reply = provider
    .complete(
      &[Message::text(Role::User, "hi")],
      &[],
      &EventSink::default(),
    )
    .await
    .unwrap();

  assert_eq!(
    reply.message.content.as_deref(),
    Some("caf\u{e9} \u{1f600}")
  );
  server.await.unwrap();
}
