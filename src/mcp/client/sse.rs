//! MCP over HTTP+SSE, the transport before Streamable HTTP and still what most hosted
//! servers speak.
//!
//! Two channels rather than one. A long-lived `GET` carries everything the server says; the
//! first thing it says is a named `endpoint` event naming the URL to `POST` to, and every
//! request goes there while its reply comes back down the `GET`. So unlike stdio, a reply is
//! not the next thing on the wire after the request. It is matched by JSON-RPC id, which is
//! why this client keeps a table of what it is waiting for.

use std::{
  collections::{BTreeMap, HashMap},
  sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicU64, Ordering},
  },
  time::Duration,
};

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::{Client, Url};
use serde_json::{Value, json};
use tokio::{
  sync::oneshot,
  task::JoinHandle,
  time::{Instant, timeout, timeout_at},
};

use crate::sse::SseDecoder;

/// How long to wait for the server to name its endpoint, and for any one call to answer.
const HANDSHAKE: Duration = Duration::from_secs(30);
const REPLY: Duration = Duration::from_secs(120);
/// An SSE event is untrusted server input. Bound an unterminated event as well as a whole
/// request, so a peer cannot retain arbitrary bytes without ever completing a message.
const MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;

type Waiting = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

pub(crate) struct SseClient {
  client: Client,
  endpoint: Url,
  headers: BTreeMap<String, String>,
  waiting: Waiting,
  next_id: AtomicU64,
  pump: Pump,
}

struct Pump(JoinHandle<()>);

impl Drop for Pump {
  fn drop(&mut self) {
    self.0.abort();
  }
}

impl SseClient {
  pub(super) async fn exchange(
    &mut self,
    message: &Value,
    id: Option<u64>,
  ) -> Result<Option<super::RpcResponse>> {
    if let Some(id) = id {
      let method = message["method"]
        .as_str()
        .context("request has no method")?;
      let result = self.call(method, message["params"].clone()).await?;
      Ok(Some(super::RpcResponse {
        id: Some(json!(id)),
        result: Some(result),
        error: None,
        method: None,
      }))
    } else {
      self.notify(message.clone()).await?;
      Ok(None)
    }
  }

  pub(crate) async fn connect(url: &str, headers: &BTreeMap<String, String>) -> Result<Self> {
    let base = Url::parse(url).with_context(|| format!("{url} is not a URL"))?;
    // No request timeout on this one: the whole point of the stream is that it stays open.
    let client = Client::builder()
      .redirect(reqwest::redirect::Policy::none())
      .connect_timeout(Duration::from_secs(15))
      .build()?;

    let mut request = client
      .get(base.clone())
      .header("accept", "text/event-stream");
    for (key, value) in headers {
      request = request.header(key, value);
    }
    let response = timeout(HANDSHAKE, request.send())
      .await
      .with_context(|| format!("{url} did not answer within {}s", HANDSHAKE.as_secs()))?
      .with_context(|| format!("open the event stream at {url}"))?;

    let status = response.status();
    if !status.is_success() {
      let body = String::from_utf8_lossy(
        &crate::output::response(response, 2048)
          .await
          .unwrap_or_default(),
      )
      .into_owned();
      bail!(
        "{url} refused the event stream: {}",
        crate::provider::wire::wire_error(status, &body)
      );
    }

    let waiting: Waiting = Arc::new(Mutex::new(HashMap::new()));
    let (found, endpoint) = oneshot::channel();
    let pump = Pump(tokio::spawn(pump(
      response,
      base.clone(),
      waiting.clone(),
      found,
    )));

    let endpoint = match timeout(HANDSHAKE, endpoint).await {
      Ok(Ok(endpoint)) => endpoint,
      // The pump ended before naming an endpoint: the stream closed, or it was never an
      // MCP SSE endpoint in the first place.
      Ok(Err(_)) => {
        pump.0.abort();
        bail!("{url} closed the event stream without naming an endpoint to post to");
      }
      Err(_) => {
        pump.0.abort();
        bail!(
          "{url} never sent an endpoint event; it may not speak the SSE transport. \
           If it is a Streamable HTTP server, its type is \"http\"."
        );
      }
    };

    Ok(Self {
      client,
      endpoint,
      headers: headers.clone(),
      waiting,
      next_id: AtomicU64::new(1),
      pump,
    })
  }

  pub(crate) async fn notify(&self, body: Value) -> Result<()> {
    self.post(body).await
  }

  pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value> {
    let id = self.next_id.fetch_add(1, Ordering::Relaxed);

    // Registered before the post, or a fast server could answer before anyone is listening.
    let (reply, waited) = oneshot::channel();
    self
      .waiting
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .insert(id, reply);

    let deadline = Instant::now() + REPLY;
    if let Err(error) = self
      .post(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
      .await
    {
      self.forget(id);
      return Err(error);
    }

    let reply = match timeout_at(deadline, waited).await {
      Ok(Ok(value)) => value,
      Ok(Err(_)) => {
        self.forget(id);
        bail!("the event stream closed while waiting for {method}");
      }
      Err(_) => {
        self.forget(id);
        bail!("{method} went unanswered for {}s", REPLY.as_secs());
      }
    };

    let response: super::RpcResponse =
      serde_json::from_value(reply).context("invalid server reply")?;
    if let Some(error) = response.error {
      bail!("server error {}: {}", error.code, error.message);
    }
    response.result.context("server reply has no result")
  }

  pub(crate) fn alive(&self) -> bool {
    !self.pump.0.is_finished()
  }

  pub(crate) fn shutdown(&self) {
    self.pump.0.abort();
  }

  fn forget(&self, id: u64) {
    self
      .waiting
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .remove(&id);
  }

  async fn post(&self, body: Value) -> Result<()> {
    let mut request = self
      .client
      .post(self.endpoint.clone())
      .header("content-type", "application/json")
      .timeout(REPLY);
    for (key, value) in &self.headers {
      request = request.header(key, value);
    }
    let response = request
      .json(&body)
      .send()
      .await
      .with_context(|| format!("post to {}", self.endpoint))?;
    let status = response.status();
    if !status.is_success() {
      let text = String::from_utf8_lossy(
        &crate::output::response(response, 2048)
          .await
          .unwrap_or_default(),
      )
      .into_owned();
      bail!(
        "{} refused the message: {}",
        self.endpoint,
        crate::provider::wire::wire_error(status, &text)
      );
    }
    Ok(())
  }
}

/// Reads the stream for as long as it lasts: the endpoint event once, then replies, which are
/// handed to whoever is waiting on that id. Anything else the server says, whether a
/// notification or a request of its own that may reuse one of our ids, is not a reply, so it
/// is dropped.
async fn pump(
  response: reqwest::Response,
  base: Url,
  waiting: Waiting,
  found: oneshot::Sender<Url>,
) {
  let mut found = Some(found);
  let mut stream = response.bytes_stream();
  let mut decoder = SseDecoder::default();

  while let Some(chunk) = stream.next().await {
    let Ok(chunk) = chunk else { break };
    if decoder.retained_len().saturating_add(chunk.len()) > MAX_EVENT_BYTES {
      break;
    }
    for event in decoder.push_named(&chunk) {
      if event.name.as_deref() == Some("endpoint") {
        // Servers send an absolute URL or a path; join covers both, and keeps whatever
        // session id the server put in the query. Another origin is refused: every post
        // carries the headers the stream was opened with, sign-in token included.
        if let Ok(url) = base.join(event.data.trim())
          && url.origin() == base.origin()
          && let Some(sender) = found.take()
        {
          let _ = sender.send(url);
        }
        continue;
      }
      let Ok(value) = serde_json::from_str::<Value>(event.data.trim()) else {
        continue;
      };
      let Some(id) = value
        .get("id")
        .and_then(Value::as_u64)
        .filter(|_| value.get("method").is_none())
      else {
        continue;
      };
      let sender = waiting
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&id);
      if let Some(sender) = sender {
        let _ = sender.send(value);
      }
    }
  }
  // Dropping the table wakes every caller still waiting with a closed channel, which is what
  // turns a dead stream into an error on the next call rather than a hang.
  waiting
    .lock()
    .unwrap_or_else(PoisonError::into_inner)
    .clear();
}

#[cfg(test)]
mod tests {
  use super::*;
  use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
  };

  /// The smallest thing that speaks the transport: answer the GET with a stream, name an
  /// endpoint, then answer whatever is posted on the stream rather than in the reply.
  async fn stub() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
      let (mut get, _) = listener.accept().await.unwrap();
      let mut buffer = [0u8; 2048];
      let _ = get.read(&mut buffer).await;
      get
        .write_all(
          b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n\
            event: endpoint\r\ndata: /messages?session=abc\r\n\r\n",
        )
        .await
        .unwrap();
      get.flush().await.unwrap();

      // Every POST is answered 202 on its own connection, and the JSON-RPC reply goes out on
      // the stream above, which is the whole shape of this transport. The 202 says the
      // connection closes, or the client may pool it and send the next POST into a socket this
      // loop has already dropped.
      loop {
        let Ok((mut post, _)) = listener.accept().await else {
          return;
        };
        let mut buffer = vec![0u8; 8192];
        let read = post.read(&mut buffer).await.unwrap_or(0);
        if read == 0 {
          continue;
        }
        let text = String::from_utf8_lossy(&buffer[..read]).to_string();
        let _ = post
          .write_all(b"HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
          .await;
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        let Ok(value) = serde_json::from_str::<Value>(&body) else {
          continue;
        };
        let Some(id) = value.get("id").and_then(Value::as_u64) else {
          continue; // a notification wants no reply
        };
        let method = value["method"].as_str().unwrap_or_default().to_string();
        let result = match method.as_str() {
          "initialize" => json!({"protocolVersion": "2024-11-05", "serverInfo": {"name": "stub"}}),
          "tools/list" => json!({"tools": [{"name": "ping", "description": "d"}]}),
          _ => json!({"echo": method}),
        };
        let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
        let _ = get
          .write_all(format!("event: message\r\ndata: {reply}\r\n\r\n").as_bytes())
          .await;
        let _ = get.flush().await;
      }
    });
    format!("http://127.0.0.1:{port}/sse")
  }

  #[tokio::test]
  async fn the_endpoint_event_is_the_handshake() {
    let url = stub().await;
    let client = SseClient::connect(&url, &BTreeMap::new()).await.unwrap();
    assert_eq!(client.endpoint.path(), "/messages");
    assert_eq!(client.endpoint.query(), Some("session=abc"));
  }

  #[tokio::test]
  async fn a_reply_arriving_on_the_other_channel_answers_the_call() {
    let url = stub().await;
    let client = SseClient::connect(&url, &BTreeMap::new()).await.unwrap();
    let result = client.call("initialize", json!({})).await.unwrap();
    assert_eq!(result["serverInfo"]["name"], "stub");
    let tools = client.call("tools/list", json!({})).await.unwrap();
    assert_eq!(tools["tools"][0]["name"], "ping");
  }

  #[tokio::test]
  async fn nothing_is_left_waiting_once_a_call_is_answered() {
    let url = stub().await;
    let client = SseClient::connect(&url, &BTreeMap::new()).await.unwrap();
    client.call("initialize", json!({})).await.unwrap();
    client
      .notify(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
      .await
      .unwrap();
    client.call("tools/list", json!({})).await.unwrap();
    assert!(
      client
        .waiting
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_empty()
    );
  }

  #[tokio::test]
  async fn a_server_that_never_names_an_endpoint_is_an_error_not_a_hang() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
      let (mut socket, _) = listener.accept().await.unwrap();
      let mut buffer = [0u8; 1024];
      let _ = socket.read(&mut buffer).await;
      // A well-formed stream that simply is not this transport, then closed.
      let _ = socket
        .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\ndata: hi\r\n\r\n")
        .await;
    });
    let Err(error) =
      SseClient::connect(&format!("http://127.0.0.1:{port}/sse"), &BTreeMap::new()).await
    else {
      panic!("a stream that names no endpoint must not connect");
    };
    assert!(error.to_string().contains("endpoint"), "{error}");
  }

  #[tokio::test]
  async fn an_endpoint_on_another_origin_is_never_posted_to() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
      let (mut socket, _) = listener.accept().await.unwrap();
      let mut buffer = [0u8; 1024];
      let _ = socket.read(&mut buffer).await;
      let _ = socket
        .write_all(
          b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n\
            event: endpoint\r\ndata: https://elsewhere.example/messages\r\n\r\n",
        )
        .await;
    });
    let connected =
      SseClient::connect(&format!("http://127.0.0.1:{port}/sse"), &BTreeMap::new()).await;
    assert!(
      connected.is_err(),
      "the token would have gone to another host"
    );
  }

  #[tokio::test]
  async fn a_refused_stream_says_what_the_server_said() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
      let (mut socket, _) = listener.accept().await.unwrap();
      let mut buffer = [0u8; 1024];
      let _ = socket.read(&mut buffer).await;
      let body = br#"{"error":{"message":"needs authorization"}}"#;
      let _ = socket
        .write_all(
          format!(
            "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
            body.len()
          )
          .as_bytes(),
        )
        .await;
      let _ = socket.write_all(body).await;
    });
    let Err(error) =
      SseClient::connect(&format!("http://127.0.0.1:{port}/sse"), &BTreeMap::new()).await
    else {
      panic!("a refused stream must not connect");
    };
    assert!(error.to_string().contains("needs authorization"), "{error}");
  }
}
