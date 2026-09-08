use super::{guard, redirect, validate_addresses};
use reqwest::Url;
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::TcpListener,
};

#[test]
fn private_and_special_addresses_are_rejected() {
  for host in [
    "0.0.0.0",
    "127.0.0.2",
    "10.0.0.1",
    "172.31.0.1",
    "192.168.1.1",
    "169.254.169.254",
    "100.64.0.1",
    "198.18.0.1",
    "224.0.0.1",
    "[::]",
    "[::1]",
    "[::ffff:127.0.0.1]",
    "[fc00::1]",
    "[fe80::1]",
    "[2002:7f00:1::]",
    "[2001:db8::1]",
    "localhost.",
  ] {
    assert!(
      guard(&Url::parse(&format!("http://{host}/")).unwrap()).is_err(),
      "{host}"
    );
  }
  for host in ["example.com", "8.8.8.8", "[2606:4700:4700::1111]"] {
    assert!(
      guard(&Url::parse(&format!("https://{host}/")).unwrap()).is_ok(),
      "{host}"
    );
  }
}

#[test]
fn a_private_dns_answer_rejects_the_entire_destination() {
  assert!(validate_addresses(&[]).is_err());
  assert!(validate_addresses(&["8.8.8.8:80".parse().unwrap()]).is_ok());
  assert!(
    validate_addresses(&[
      "8.8.8.8:80".parse().unwrap(),
      "127.0.0.1:80".parse().unwrap()
    ])
    .is_err()
  );
}

#[test]
fn redirects_reapply_the_destination_policy() {
  let origin = Url::parse("https://example.com/start").unwrap();
  assert_eq!(
    redirect(&origin, "/next").unwrap().as_str(),
    "https://example.com/next"
  );
  for target in [
    "http://127.0.0.1/",
    "//[::ffff:127.0.0.1]/",
    "http://169.254.169.254/",
    "file:///etc/passwd",
    "https://user:password@example.com/",
  ] {
    assert!(redirect(&origin, target).is_err(), "{target}");
  }
}

#[tokio::test]
async fn streaming_responses_are_bounded_without_a_content_length() {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let server = tokio::spawn(async move {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut request = [0; 8192];
    let _ = socket.read(&mut request).await.unwrap();
    socket
      .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n800\r\n")
      .await
      .unwrap();
    socket.write_all(&vec![b'x'; 2048]).await.unwrap();
    socket.write_all(b"\r\n0\r\n\r\n").await.unwrap();
  });
  let response = reqwest::get(format!("http://{address}/")).await.unwrap();
  let error = crate::output::response(response, 1024).await.unwrap_err();
  assert!(error.to_string().contains("transfer limit"));
  server.await.unwrap();
}
