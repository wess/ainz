//! The output limit a chat-completions request names, and what happens when a server will not
//! take it.

use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::{TcpListener, TcpStream},
  sync::mpsc,
};

use ainz::{
  Event, EventSink, HttpProvider,
  protocol::{Message, Role},
  provider::ChatProvider,
};

const REPLY: &str = r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#;
const REFUSED: &str = r#"{"error":{"message":"max_tokens is too large: 32000. This model supports at most 16384 completion tokens, whereas you provided 32000."}}"#;

fn response(status: &str, body: &str) -> String {
  format!(
    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
    body.len()
  )
}

async fn body(socket: &mut TcpStream) -> String {
  let mut request = Vec::new();
  let mut buffer = [0_u8; 4096];
  loop {
    let read = socket.read(&mut buffer).await.unwrap();
    if read == 0 {
      break;
    }
    request.extend_from_slice(&buffer[..read]);
    let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
      continue;
    };
    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
    let length = headers
      .lines()
      .find_map(|line| line.strip_prefix("content-length:"))
      .and_then(|value| value.trim().parse::<usize>().ok())
      .unwrap_or(0);
    if request.len() >= end + 4 + length {
      return String::from_utf8_lossy(&request[end + 4..]).into_owned();
    }
  }
  String::new()
}

/// Answers each request in turn and hands back the body each one carried.
async fn serve(responses: Vec<String>) -> (String, mpsc::UnboundedReceiver<String>) {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let (sender, bodies) = mpsc::unbounded_channel();
  tokio::spawn(async move {
    for response in responses {
      let (mut socket, _) = listener.accept().await.unwrap();
      drop(sender.send(body(&mut socket).await));
      socket.write_all(response.as_bytes()).await.unwrap();
    }
  });
  (format!("http://{address}"), bodies)
}

async fn ask(provider: &HttpProvider, events: &EventSink) -> anyhow::Result<String> {
  let reply = provider
    .complete(&[&Message::text(Role::User, "hi")], &[], events)
    .await?;
  Ok(reply.message.content.unwrap_or_default())
}

#[tokio::test]
async fn a_request_names_the_default_output_limit() {
  let (endpoint, mut bodies) = serve(vec![response("200 OK", REPLY)]).await;
  let provider = HttpProvider::new(endpoint, "m".into(), None, 0).unwrap();

  ask(&provider, &EventSink::default()).await.unwrap();

  let body = bodies.recv().await.unwrap();
  assert!(body.contains(r#""max_completion_tokens":32000"#), "{body}");
}

#[tokio::test]
async fn a_profile_s_own_limit_replaces_the_default_and_zero_sends_none() {
  let (endpoint, mut bodies) =
    serve(vec![response("200 OK", REPLY), response("200 OK", REPLY)]).await;
  let larger = HttpProvider::new(endpoint.clone(), "m".into(), None, 0)
    .unwrap()
    .with_output_limit(Some(64_000));
  let none = HttpProvider::new(endpoint, "m".into(), None, 0)
    .unwrap()
    .with_output_limit(Some(0));

  ask(&larger, &EventSink::default()).await.unwrap();
  ask(&none, &EventSink::default()).await.unwrap();

  assert!(
    bodies
      .recv()
      .await
      .unwrap()
      .contains(r#""max_completion_tokens":64000"#)
  );
  assert!(
    !bodies
      .recv()
      .await
      .unwrap()
      .contains("max_completion_tokens")
  );
}

#[tokio::test]
async fn a_refused_limit_is_dropped_once_and_stays_dropped() {
  let (endpoint, mut bodies) = serve(vec![
    response("400 Bad Request", REFUSED),
    response("200 OK", REPLY),
    response("200 OK", REPLY),
  ])
  .await;
  let provider = HttpProvider::new(endpoint, "m".into(), None, 0).unwrap();
  let (events, mut received) = EventSink::channel();

  assert_eq!(ask(&provider, &events).await.unwrap(), "ok");
  assert_eq!(ask(&provider, &events).await.unwrap(), "ok");

  assert!(
    bodies
      .recv()
      .await
      .unwrap()
      .contains("max_completion_tokens")
  );
  assert!(
    !bodies
      .recv()
      .await
      .unwrap()
      .contains("max_completion_tokens")
  );
  assert!(
    !bodies
      .recv()
      .await
      .unwrap()
      .contains("max_completion_tokens")
  );
  let mut noticed = 0;
  while let Ok(event) = received.try_recv() {
    noticed +=
      usize::from(matches!(event, Event::Notice { message } if message.contains("output limit")));
  }
  assert_eq!(noticed, 1);
}

#[tokio::test]
async fn a_request_too_long_for_the_context_keeps_its_limit() {
  let refused = r#"{"error":{"message":"input length and `max_tokens` exceed context limit"}}"#;
  let (endpoint, mut bodies) = serve(vec![response("400 Bad Request", refused)]).await;
  let provider = HttpProvider::new(endpoint, "m".into(), None, 0).unwrap();

  let error = ask(&provider, &EventSink::default()).await.unwrap_err();

  assert!(error.to_string().contains("context limit"), "{error}");
  assert!(
    bodies
      .recv()
      .await
      .unwrap()
      .contains("max_completion_tokens")
  );
  assert!(bodies.try_recv().is_err());
}
