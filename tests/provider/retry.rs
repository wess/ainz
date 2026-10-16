use std::sync::{
  Arc,
  atomic::{AtomicUsize, Ordering},
};

use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::TcpListener,
};

use super::*;

// answers with `codes` in order, one per connection, then 200 for ever, and counts what it was
// asked, which is the thing each test is really asserting about
async fn server(codes: Vec<u16>, body: &'static str) -> (String, Arc<AtomicUsize>) {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let port = listener.local_addr().unwrap().port();
  let seen = Arc::new(AtomicUsize::new(0));
  let counted = seen.clone();
  tokio::spawn(async move {
    loop {
      let Ok((mut socket, _)) = listener.accept().await else {
        return;
      };
      let index = counted.fetch_add(1, Ordering::SeqCst);
      let mut buffer = [0u8; 2048];
      let _ = socket.read(&mut buffer).await;
      let code = codes.get(index).copied().unwrap_or(200);
      let head = format!(
        "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
      );
      let _ = socket.write_all(head.as_bytes()).await;
      let _ = socket.write_all(body.as_bytes()).await;
    }
  });
  (format!("http://127.0.0.1:{port}/"), seen)
}

const UPSTREAM: &str = r#"{"error":{"message":"upstream said so"}}"#;

#[tokio::test]
async fn a_transient_failure_is_tried_again() {
  let (url, seen) = server(vec![503, 500], UPSTREAM).await;
  let http = Client::new();
  let response = send("model request", 3, None, || http.get(&url))
    .await
    .unwrap();
  assert!(response.status().is_success());
  assert_eq!(
    seen.load(Ordering::SeqCst),
    3,
    "two failures then the answer"
  );
}

#[tokio::test]
async fn a_client_error_is_not_tried_again() {
  let (url, seen) = server(vec![401], UPSTREAM).await;
  let http = Client::new();
  let error = send("model request", 3, None, || http.get(&url))
    .await
    .expect_err("a 401 must not be retried")
    .to_string();
  assert_eq!(seen.load(Ordering::SeqCst), 1);
  assert!(error.contains("upstream said so"), "{error}");
  assert!(
    !error.contains("attempts"),
    "a single try should not say attempts: {error}"
  );
}

#[tokio::test]
async fn a_permanent_refusal_is_not_tried_again_whatever_its_status() {
  let body = r#"{"error":{"code":"insufficient_quota","message":"out of credit"}}"#;
  let (url, seen) = server(vec![429, 429], body).await;
  let http = Client::new();
  let error = send("model request", 3, None, || http.get(&url))
    .await
    .expect_err("a quota refusal must not be retried")
    .to_string();
  assert_eq!(seen.load(Ordering::SeqCst), 1);
  assert!(error.contains("out of credit"), "{error}");
}

#[tokio::test]
async fn giving_up_says_how_many_times_it_tried() {
  let (url, seen) = server(vec![500, 500, 500, 500, 500], UPSTREAM).await;
  let http = Client::new();
  let error = send("model request", 2, None, || http.get(&url))
    .await
    .expect_err("three 500s with two retries must fail")
    .to_string();
  assert_eq!(seen.load(Ordering::SeqCst), 3, "one try and two retries");
  assert!(error.contains("after 3 attempts"), "{error}");
}

#[tokio::test]
async fn a_request_that_cannot_be_built_fails_without_waiting() {
  let http = Client::new();
  let started = std::time::Instant::now();
  assert!(
    send("model request", 3, None, || http.get("not a url"))
      .await
      .is_err()
  );
  assert!(started.elapsed() < Duration::from_millis(300));
}

#[tokio::test]
async fn zero_retries_means_one_attempt() {
  let (url, seen) = server(vec![503], UPSTREAM).await;
  let http = Client::new();
  assert!(
    send("model request", 0, None, || http.get(&url))
      .await
      .is_err()
  );
  assert_eq!(seen.load(Ordering::SeqCst), 1);
}

#[test]
fn the_backoff_grows_and_stays_inside_its_jitter() {
  // 500ms, 1s, 2s, 4s, then capped at 8s, each within a quarter either side
  for (attempt, base) in [(1usize, 500u64), (2, 1000), (3, 2000), (4, 4000), (9, 8000)] {
    let wait = backoff(attempt).as_millis() as u64;
    assert!(
      wait >= base - base / 4 && wait <= base + base / 4,
      "attempt {attempt} waited {wait}ms, outside the jitter around {base}ms"
    );
  }
}

#[test]
fn only_the_transient_statuses_are_retried() {
  for code in [408, 429, 500, 502, 503, 504] {
    assert!(
      retryable_status(StatusCode::from_u16(code).unwrap()),
      "{code}"
    );
  }
  for code in [400, 401, 403, 404, 422] {
    assert!(
      !retryable_status(StatusCode::from_u16(code).unwrap()),
      "{code}"
    );
  }
}
