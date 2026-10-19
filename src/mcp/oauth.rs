//! Signing in to a remote MCP server.
//!
//! Servers like Atlassian's answer every unauthenticated request with `401` and a
//! `WWW-Authenticate: Bearer` header. Getting past it is the MCP authorization flow, which is
//! plain OAuth 2.1 for a native client: find the authorization server, register with it, send
//! the person to their browser with a PKCE challenge, catch the redirect on a loopback port,
//! and trade the code for tokens.
//!
//! Tokens live in `mcpauth.json` beside the config, owner-readable only, one record per
//! declared server. A record keeps the endpoints it was issued from, so a refresh costs no
//! discovery and can be read back without the server being reachable.

use std::{
  collections::BTreeMap,
  path::PathBuf,
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::TcpListener,
};

/// How long the browser leg may take before the listener gives up.
const PATIENCE: Duration = Duration::from_secs(300);
/// Renewed this long before the server would reject it, so a token cannot expire between the
/// check and the call it was checked for.
const EARLY: u64 = 60;
/// How long a connection to the loopback port may stay silent before it is passed over.
const QUIET: Duration = Duration::from_secs(2);

fn now() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}

/// What a signed-in server left behind.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
  #[serde(default)]
  pub server_url: Option<String>,
  pub access: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub refresh: Option<String>,
  /// Seconds since the epoch. Absent when the server issued a token that does not expire.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub expires_at: Option<u64>,
  pub token_endpoint: String,
  pub client_id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub client_secret: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub resource: Option<String>,
}

impl Record {
  fn stale(&self) -> bool {
    self.expires_at.is_some_and(|at| at <= now() + EARLY)
  }
}

pub fn path() -> Result<PathBuf> {
  if let Some(path) = std::env::var_os("AINZ_MCP_AUTH") {
    return Ok(PathBuf::from(path));
  }
  Ok(
    crate::config::Config::path()?
      .parent()
      .context("no config directory")?
      .to_path_buf()
      .join("mcpauth.json"),
  )
}

async fn all() -> BTreeMap<String, Record> {
  match path() {
    Ok(path) => read(&path).await,
    Err(_) => BTreeMap::new(),
  }
}

async fn read(path: &std::path::Path) -> BTreeMap<String, Record> {
  tokio::fs::read_to_string(path)
    .await
    .ok()
    .and_then(|raw| serde_json::from_str(&raw).ok())
    .unwrap_or_default()
}

pub async fn load(server: &str) -> Option<Record> {
  all().await.remove(server)
}

/// Which servers Ainz holds a token for.
pub async fn held() -> Vec<String> {
  all().await.into_keys().collect()
}

pub async fn save(server: &str, record: &Record) -> Result<()> {
  change(&path()?, |records| {
    records.insert(server.to_string(), record.clone());
    true
  })
  .await
  .map(drop)
}

pub async fn forget(server: &str) -> Result<bool> {
  change(&path()?, |records| records.remove(server).is_some()).await
}

/// One read-modify-write of the whole file at a time. Servers connect concurrently, and two
/// renewals landing together would otherwise each write back a file missing the other's token,
/// and a rotated refresh token lost that way is a sign-in lost with it.
async fn change(
  path: &std::path::Path,
  edit: impl FnOnce(&mut BTreeMap<String, Record>) -> bool,
) -> Result<bool> {
  static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
  let _held = LOCK.lock().await;
  let mut records = read(path).await;
  if !edit(&mut records) {
    return Ok(false);
  }
  // Token refreshes must replace the private file atomically.
  write(path, serde_json::to_vec_pretty(&records)?).await?;
  Ok(true)
}

/// The `Authorization` header for a server, renewed first if it is about to expire.
///
/// `None` means no token was ever stored, which is the ordinary case: most servers need none.
/// A refresh that fails is reported rather than swallowed, because connecting without the
/// header would surface as a bare `401` and send somebody looking in the wrong place.
pub async fn bearer(server: &str, url: &str, http: &reqwest::Client) -> Result<Option<String>> {
  let Some(record) = load(server).await else {
    return Ok(None);
  };
  if record.server_url.as_deref() != Some(url) {
    bail!("the stored sign-in for {server} belongs to a different endpoint; sign in again");
  }
  if !record.stale() {
    return Ok(Some(format!("Bearer {}", record.access)));
  }
  let renewed = renew(http, &record)
    .await
    .with_context(|| format!("the stored sign-in for {server} could not be renewed"))?;
  save(server, &renewed).await?;
  Ok(Some(format!("Bearer {}", renewed.access)))
}

/// Where the authorization server is and what it offers.
#[derive(Clone, Debug)]
pub struct Meta {
  pub authorize: String,
  pub token: String,
  pub register: Option<String>,
  pub scopes: Option<String>,
  /// The resource indicator to ask the token for, when the server named one.
  pub resource: Option<String>,
}

/// Finds the authorization server for an MCP endpoint.
///
/// The spec's route is the protected-resource document, which names its authorization servers.
/// Plenty of servers, Atlassian's among them, publish only the authorization-server document
/// at their own origin, so that is tried too.
pub async fn discover(http: &reqwest::Client, url: &str) -> Result<Meta> {
  let (origin, path) = split(url);
  let mut resource = None;
  let mut issuers: Vec<String> = Vec::new();

  for candidate in [
    format!("{origin}/.well-known/oauth-protected-resource{path}"),
    format!("{origin}/.well-known/oauth-protected-resource"),
  ] {
    let Some(doc) = fetch(http, &candidate).await else {
      continue;
    };
    resource = text(&doc, "resource");
    issuers = strings(&doc, "authorization_servers")
      .map(str::to_string)
      .collect();
    break;
  }

  issuers.push(origin.clone());

  for issuer in issuers {
    for candidate in documents(&issuer) {
      let Some(doc) = fetch(http, &candidate).await else {
        continue;
      };
      let (Some(authorize), Some(token)) = (
        text(&doc, "authorization_endpoint"),
        text(&doc, "token_endpoint"),
      ) else {
        continue;
      };
      let scopes = strings(&doc, "scopes_supported").collect::<Vec<_>>();
      return Ok(Meta {
        authorize,
        token,
        register: text(&doc, "registration_endpoint"),
        scopes: (!scopes.is_empty()).then(|| scopes.join(" ")),
        resource,
      });
    }
  }

  bail!("{origin} does not say where to sign in; it publishes no OAuth metadata")
}

/// Where an issuer's metadata may be. RFC 8414 puts the well-known segment between the host
/// and the issuer's path; OpenID Connect appends it to the path, and plenty of servers do
/// that for both.
fn documents(issuer: &str) -> Vec<String> {
  let (origin, path) = split(issuer);
  let mut documents = vec![
    format!("{origin}/.well-known/oauth-authorization-server{path}"),
    format!("{origin}/.well-known/openid-configuration{path}"),
  ];
  if !path.is_empty() {
    documents.push(format!("{origin}{path}/.well-known/openid-configuration"));
  }
  documents
}

fn text(doc: &Value, key: &str) -> Option<String> {
  doc.get(key).and_then(Value::as_str).map(str::to_string)
}

fn strings<'a>(doc: &'a Value, key: &str) -> impl Iterator<Item = &'a str> {
  doc
    .get(key)
    .and_then(Value::as_array)
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
}

async fn fetch(http: &reqwest::Client, url: &str) -> Option<Value> {
  let response = http
    .get(url)
    .timeout(Duration::from_secs(15))
    .send()
    .await
    .ok()?;
  if !response.status().is_success() {
    return None;
  }
  response.json().await.ok()
}

/// Splits `https://host/v1/sse` into its origin and its path.
fn split(url: &str) -> (String, String) {
  let (scheme, rest) = url.split_once("://").unwrap_or(("https", url));
  match rest.split_once('/') {
    Some((host, path)) => (
      format!("{scheme}://{host}"),
      format!("/{}", path.trim_end_matches('/')),
    ),
    None => (format!("{scheme}://{rest}"), String::new()),
  }
}

/// A sign-in waiting for the browser.
pub struct Pending {
  server_url: String,
  /// The address to visit. Shown, and opened if this machine will open it.
  pub url: String,
  server: String,
  listener: TcpListener,
  state: String,
  verifier: String,
  redirect: String,
  meta: Meta,
  client_id: String,
  client_secret: Option<String>,
}

/// Discovers, registers if needed, and prepares the browser leg.
pub async fn begin(http: &reqwest::Client, server: &str, url: &str) -> Result<Pending> {
  let meta = discover(http, url).await?;

  // Bound to port zero: RFC 8252 requires an authorization server to accept any port on a
  // loopback redirect, which is what makes a native client possible without a registered one.
  let listener = TcpListener::bind("127.0.0.1:0")
    .await
    .context("no loopback port to catch the redirect on")?;
  let port = listener.local_addr()?.port();
  let redirect = format!("http://127.0.0.1:{port}/callback");

  // A dynamic registration binds the exact loopback callback, and a reconnect gets a new
  // random port, so it also needs a fresh registration. A static client has no registration
  // endpoint and must reuse the id it was given.
  let previous = load(server).await;
  let (client_id, client_secret) = match reusable(&meta, previous.as_ref()) {
    Some(client) => client,
    None => register(http, &meta, &redirect, server).await?,
  };

  let verifier = format!("{}{}", nonce()?, nonce()?);
  let state = nonce()?;
  let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
    .encode(<sha2::Sha256 as sha2::Digest>::digest(verifier.as_bytes()));

  let mut query = vec![
    ("response_type", "code".to_string()),
    ("client_id", client_id.clone()),
    ("redirect_uri", redirect.clone()),
    ("state", state.clone()),
    ("code_challenge", challenge),
    ("code_challenge_method", "S256".to_string()),
  ];
  if let Some(scopes) = &meta.scopes {
    query.push(("scope", scopes.clone()));
  }
  if let Some(resource) = &meta.resource {
    query.push(("resource", resource.clone()));
  }

  Ok(Pending {
    server_url: url.to_string(),
    url: format!(
      "{}{}{}",
      meta.authorize,
      if meta.authorize.contains('?') {
        '&'
      } else {
        '?'
      },
      encode(&query)
    ),
    server: server.to_string(),
    listener,
    state,
    verifier,
    redirect,
    meta,
    client_id,
    client_secret,
  })
}

fn reusable(meta: &Meta, previous: Option<&Record>) -> Option<(String, Option<String>)> {
  if meta.register.is_some() {
    return None;
  }
  previous
    .filter(|record| record.token_endpoint == meta.token)
    .map(|record| (record.client_id.clone(), record.client_secret.clone()))
}

impl Pending {
  /// Waits for the browser to come back, trades the code for tokens, and stores them.
  pub async fn finish(self, http: &reqwest::Client) -> Result<Record> {
    let code = tokio::time::timeout(PATIENCE, catch(&self.listener, &self.state, &self.server))
      .await
      .context("the browser never came back")??;

    let mut form = vec![
      ("grant_type", "authorization_code".to_string()),
      ("code", code),
      ("redirect_uri", self.redirect.clone()),
      ("client_id", self.client_id.clone()),
      ("code_verifier", self.verifier.clone()),
    ];
    if let Some(secret) = &self.client_secret {
      form.push(("client_secret", secret.clone()));
    }
    if let Some(resource) = &self.meta.resource {
      form.push(("resource", resource.clone()));
    }

    let granted = post(http.post(&self.meta.token).form(&form)).await?;
    let record = issued(
      &granted,
      Record {
        server_url: Some(self.server_url),
        access: String::new(),
        refresh: None,
        expires_at: None,
        token_endpoint: self.meta.token,
        client_id: self.client_id,
        client_secret: self.client_secret,
        resource: self.meta.resource,
      },
    )?;
    save(&self.server, &record).await?;
    Ok(record)
  }
}

/// One request on the loopback port: the redirect carrying the code.
///
/// Anything without a query is a browser asking for something else (the icon, most often) and
/// is answered without ending the wait.
///
/// A browser opens speculative connections it may never send anything on, so a connection
/// that says nothing promptly is dropped rather than left to hold up the one carrying the code.
async fn catch(listener: &TcpListener, state: &str, server: &str) -> Result<String> {
  loop {
    let (mut socket, _) = listener.accept().await?;
    let mut buffer = [0u8; 8192];
    let Ok(read) = tokio::time::timeout(QUIET, socket.read(&mut buffer)).await else {
      continue;
    };
    let read = read.unwrap_or(0);
    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
    let target = request
      .lines()
      .next()
      .and_then(|line| line.split_whitespace().nth(1))
      .unwrap_or_default()
      .to_string();

    let Some((_, query)) = target.split_once('?') else {
      let _ = socket
        .write_all(page("Waiting for the sign-in to come back.").as_bytes())
        .await;
      let _ = socket.shutdown().await;
      continue;
    };

    let answer = answer(&params(query), state);
    let body = match &answer {
      Ok(_) => page(&format!("Ainz is signed in to {server}.")),
      Err(error) => page(&format!("Sign-in failed: {error}")),
    };
    let _ = socket.write_all(body.as_bytes()).await;
    let _ = socket.shutdown().await;
    return answer;
  }
}

fn params(query: &str) -> BTreeMap<String, String> {
  query
    .split('&')
    .filter_map(|pair| pair.split_once('='))
    .map(|(key, value)| (key.to_string(), decode(value)))
    .collect()
}

/// What the redirect meant. An authorization server reports refusal in the query rather than
/// by status, so a missing code is not always an error worth guessing at.
fn answer(params: &BTreeMap<String, String>, state: &str) -> Result<String> {
  if let Some(error) = params.get("error") {
    let detail = params
      .get("error_description")
      .map(|detail| format!(": {detail}"))
      .unwrap_or_default();
    bail!("{error}{detail}");
  }
  let code = params
    .get("code")
    .context("the browser came back without a code")?;
  ensure!(
    params.get("state").map(String::as_str) == Some(state),
    "the browser came back with the wrong state"
  );
  Ok(code.clone())
}

/// Trades a refresh token for a fresh one.
pub async fn renew(http: &reqwest::Client, record: &Record) -> Result<Record> {
  let refresh = record
    .refresh
    .clone()
    .context("the server issued no refresh token, so signing in again is the only way back")?;

  let mut form = vec![
    ("grant_type", "refresh_token".to_string()),
    ("refresh_token", refresh),
    ("client_id", record.client_id.clone()),
  ];
  if let Some(secret) = &record.client_secret {
    form.push(("client_secret", secret.clone()));
  }
  if let Some(resource) = &record.resource {
    form.push(("resource", resource.clone()));
  }

  let granted = post(http.post(&record.token_endpoint).form(&form)).await?;
  // A refresh may or may not rotate the refresh token; keeping the old one when it does not
  // is the difference between one renewal and every renewal.
  issued(&granted, record.clone())
}

/// Registers Ainz with an authorization server that accepts new clients.
async fn register(
  http: &reqwest::Client,
  meta: &Meta,
  redirect: &str,
  server: &str,
) -> Result<(String, Option<String>)> {
  let endpoint = meta.register.as_ref().with_context(|| {
    format!("{server} needs a client id, and its authorization server does not register one")
  })?;

  let body = serde_json::json!({
    "client_name": "Ainz",
    "redirect_uris": [redirect],
    "grant_types": ["authorization_code", "refresh_token"],
    "response_types": ["code"],
    "token_endpoint_auth_method": "none",
    "application_type": "native",
  });

  let doc = post(http.post(endpoint).json(&body))
    .await
    .context("registration refused")?;
  Ok((
    text(&doc, "client_id").context("the registration reply carried no client_id")?,
    text(&doc, "client_secret"),
  ))
}

async fn post(request: reqwest::RequestBuilder) -> Result<Value> {
  // Token and registration endpoints come from whatever the server's metadata said, so they
  // are checked where they are sent rather than where they were found.
  let (http, request) = request.timeout(Duration::from_secs(30)).build_split();
  let request = request?;
  let response = http
    .execute(request)
    .await
    .context("the authorization server did not answer")?;
  let status = response.status();
  let url = response.url().clone();
  let doc: Value = response
    .json()
    .await
    .with_context(|| format!("{url} answered with something other than JSON"))?;
  if !status.is_success() {
    bail!("{status}: {}", detail(&doc));
  }
  Ok(doc)
}

/// The sentence in an OAuth error body, not the punctuation around it.
fn detail(doc: &Value) -> String {
  doc
    .get("error_description")
    .or_else(|| doc.get("error"))
    .and_then(Value::as_str)
    .map(str::to_string)
    .unwrap_or_else(|| doc.to_string())
}

/// A token reply laid over what the record already knows. Whatever the reply leaves out, most
/// often a refresh token, is kept from `base`.
fn issued(granted: &Value, base: Record) -> Result<Record> {
  Ok(Record {
    access: text(granted, "access_token").context("the reply carried no access token")?,
    refresh: text(granted, "refresh_token").or(base.refresh),
    expires_at: granted
      .get("expires_in")
      .and_then(Value::as_u64)
      .map(|seconds| now() + seconds),
    ..base
  })
}

/// 128 bits of unreserved characters, so it needs no escaping wherever it is used.
///
/// From the operating system, not from a hash of the clock: a PKCE verifier and a state
/// parameter are the two things in this flow that must not be guessable, so without it the
/// sign-in stops rather than going ahead with something weaker.
fn nonce() -> Result<String> {
  let mut bytes = [0u8; 16];
  std::fs::File::open("/dev/urandom")
    .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut bytes))
    .context("no randomness to sign in with")?;
  Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Percent-encoding for a query value: everything but the unreserved set of RFC 3986.
fn encode(query: &[(&str, String)]) -> String {
  query
    .iter()
    .map(|(key, value)| format!("{key}={}", escape(value)))
    .collect::<Vec<_>>()
    .join("&")
}

fn escape(value: &str) -> String {
  let mut out = String::with_capacity(value.len());
  for byte in value.as_bytes() {
    match byte {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
        out.push(*byte as char)
      }
      other => out.push_str(&format!("%{other:02X}")),
    }
  }
  out
}

fn decode(value: &str) -> String {
  let bytes = value.replace('+', " ").into_bytes();
  let mut out = Vec::with_capacity(bytes.len());
  let mut index = 0;
  while index < bytes.len() {
    if bytes[index] == b'%' && index + 2 < bytes.len() {
      let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
      if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
        out.push(byte);
        index += 3;
        continue;
      }
    }
    out.push(bytes[index]);
    index += 1;
  }
  String::from_utf8_lossy(&out).into_owned()
}

fn page(message: &str) -> String {
  let body = format!(
    "<!doctype html><meta charset=utf-8><title>Ainz</title>\
     <body style=\"font:16px/1.6 -apple-system,system-ui,sans-serif;display:grid;\
     place-items:center;height:100vh;margin:0;background:#0b0812;color:#d6d2e0\">\
     <main style=\"text-align:center\">\
     <p style=\"color:#ffb03d;font-weight:600;letter-spacing:.16em\">AINZ</p>\
     <p>{message}</p><p style=\"opacity:.6\">You can close this tab.</p></main>"
  );
  format!(
    "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\n\
     content-length: {}\r\nconnection: close\r\n\r\n{body}",
    body.len()
  )
}

/// Opens the browser, and says so rather than failing when it cannot: the URL is printed
/// either way, and a machine reached over ssh has no browser to open.
pub fn open(url: &str) -> bool {
  let command = if cfg!(target_os = "macos") {
    "open"
  } else {
    "xdg-open"
  };
  std::process::Command::new(command)
    .arg(url)
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .status()
    .map(|status| status.success())
    .unwrap_or(false)
}

async fn write(path: &std::path::Path, bytes: impl AsRef<[u8]>) -> Result<()> {
  let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::now_v7()));
  let result = async {
    if let Some(parent) = path.parent() {
      tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::OpenOptions::new()
      .write(true)
      .create_new(true)
      .mode(0o600)
      .open(&temp)
      .await?;
    file.write_all(bytes.as_ref()).await?;
    file.sync_all().await?;
    tokio::fs::rename(&temp, path).await?;
    Ok::<_, anyhow::Error>(())
  }
  .await;
  if result.is_err() {
    let _ = tokio::fs::remove_file(temp).await;
  }
  result
}

#[cfg(test)]
mod tests {
  use super::*;

  fn record() -> Record {
    Record {
      server_url: Some("https://tools.test/mcp".into()),
      access: "a".into(),
      refresh: Some("r".into()),
      expires_at: None,
      token_endpoint: "https://t".into(),
      client_id: "c".into(),
      client_secret: None,
      resource: None,
    }
  }

  #[test]
  fn an_endpoint_splits_into_an_origin_and_a_path() {
    assert_eq!(
      split("https://mcp.atlassian.com/v1/sse"),
      ("https://mcp.atlassian.com".into(), "/v1/sse".into())
    );
    assert_eq!(
      split("https://example.com"),
      ("https://example.com".into(), String::new())
    );
    // A trailing slash is not part of the path the well-known documents hang off.
    assert_eq!(
      split("https://example.com/mcp/"),
      ("https://example.com".into(), "/mcp".into())
    );
  }

  #[test]
  fn a_query_value_is_escaped_and_comes_back_whole() {
    // Scopes are space-separated and redirect URIs have a colon and slashes; both go in a
    // query, so both have to survive it.
    for value in [
      "read:jira-work write:jira-work",
      "http://127.0.0.1:53127/callback",
      "a+b&c=d",
      "café",
    ] {
      assert_eq!(decode(&escape(value)), value, "{value}");
    }
    assert_eq!(escape("a b"), "a%20b");
    // A + in a query means a space, which is not the same rule as percent-decoding.
    assert_eq!(decode("a+b"), "a b");
  }

  #[test]
  fn the_redirect_is_read_for_what_it_says() {
    let state = "abc";
    let ok = params("code=xyz&state=abc");
    assert_eq!(answer(&ok, state).unwrap(), "xyz");

    // An authorization server reports refusal in the query, with a 200.
    let refused = params("error=access_denied&error_description=User%20said%20no");
    let error = answer(&refused, state).unwrap_err().to_string();
    assert!(error.contains("access_denied"), "{error}");
    assert!(error.contains("User said no"), "{error}");

    // A code arriving with the wrong state is the attack this parameter exists to stop.
    let wrong = params("code=xyz&state=somebody-elses");
    assert!(answer(&wrong, state).is_err());
    assert!(answer(&params("state=abc"), state).is_err());
  }

  #[test]
  fn a_nonce_is_unguessable_and_needs_no_escaping() {
    let one = nonce().unwrap();
    let two = nonce().unwrap();
    assert_ne!(one, two);
    assert_eq!(one.len(), 32);
    assert!(one.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(escape(&one), one);
  }

  #[test]
  fn a_token_is_renewed_before_it_expires_not_after() {
    let base = record();
    // No expiry at all: nothing to renew.
    assert!(!base.stale());
    // Comfortably in the future.
    assert!(
      !Record {
        expires_at: Some(now() + 3600),
        ..base.clone()
      }
      .stale()
    );
    // Inside the early window: renewed now, so it cannot expire between the check and the
    // call it was checked for.
    assert!(
      Record {
        expires_at: Some(now() + 30),
        ..base.clone()
      }
      .stale()
    );
    assert!(
      Record {
        expires_at: Some(now().saturating_sub(1)),
        ..base
      }
      .stale()
    );
  }

  #[test]
  fn a_static_client_is_reused_and_a_registering_one_is_not() {
    let previous = Record {
      client_id: "known".into(),
      client_secret: Some("s".into()),
      ..record()
    };
    let statik = Meta {
      authorize: "https://a".into(),
      token: "https://t".into(),
      register: None,
      scopes: None,
      resource: None,
    };
    assert_eq!(
      reusable(&statik, Some(&previous)),
      Some(("known".into(), Some("s".into())))
    );
    // A server that registers gets a fresh registration every time, because the loopback
    // port it must bind changes on every sign-in.
    let dynamic = Meta {
      register: Some("https://r".into()),
      ..statik.clone()
    };
    assert_eq!(reusable(&dynamic, Some(&previous)), None);
    // A record issued by a different authorization server is not this server's client.
    let moved = Meta {
      token: "https://elsewhere".into(),
      ..statik
    };
    assert_eq!(reusable(&moved, Some(&previous)), None);
  }

  #[test]
  fn an_issuer_with_a_path_is_looked_up_where_rfc_8414_puts_it() {
    let documents = documents("https://auth.example.com/tenant");
    assert_eq!(
      documents[0],
      "https://auth.example.com/.well-known/oauth-authorization-server/tenant"
    );
    assert!(
      documents
        .contains(&"https://auth.example.com/tenant/.well-known/openid-configuration".into())
    );
    assert_eq!(
      super::documents("https://mcp.atlassian.com")[0],
      "https://mcp.atlassian.com/.well-known/oauth-authorization-server"
    );
  }

  #[tokio::test]
  async fn tokens_saved_at_the_same_time_are_all_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcpauth.json");
    let saves = (0..16).map(|index| {
      let (path, record) = (path.clone(), record());
      tokio::spawn(async move {
        change(&path, |records| {
          records.insert(format!("server{index}"), record);
          true
        })
        .await
      })
    });
    for save in futures_util::future::join_all(saves).await {
      save.unwrap().unwrap();
    }
    assert_eq!(read(&path).await.len(), 16);
  }

  #[tokio::test]
  async fn a_connection_that_says_nothing_does_not_hold_up_the_redirect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
      // What a browser's preconnect looks like: open, and then nothing.
      let _silent = tokio::net::TcpStream::connect(address).await.unwrap();
      let mut redirect = tokio::net::TcpStream::connect(address).await.unwrap();
      redirect
        .write_all(b"GET /callback?code=xyz&state=abc HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
      tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let code = tokio::time::timeout(Duration::from_secs(10), catch(&listener, "abc", "test"))
      .await
      .expect("the redirect was never read")
      .unwrap();
    assert_eq!(code, "xyz");
  }

  #[test]
  fn an_oauth_error_body_reads_as_its_sentence() {
    let doc = serde_json::json!({"error": "invalid_grant", "error_description": "code expired"});
    assert_eq!(detail(&doc), "code expired");
    assert_eq!(
      detail(&serde_json::json!({"error": "invalid_grant"})),
      "invalid_grant"
    );
  }
}
