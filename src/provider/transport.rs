use super::{MAX_TRANSFER, send};
use crate::{event::EventSink, sse::SseDecoder};
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::{Client, RequestBuilder, Response};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};
/// Credentials are folded into headers so discovery and inference use the same identity.
#[derive(Clone)]
pub(crate) struct Wire {
  client: Client,
  endpoint: String,
  pub(super) model: String,
  headers: BTreeMap<String, String>,
  retries: usize,
}

impl Wire {
  /// No read timeout on purpose: a local model can sit for minutes before its first token.
  pub(super) fn new(
    endpoint: &str,
    model: String,
    headers: BTreeMap<String, String>,
    retries: usize,
  ) -> Result<Self> {
    Ok(Self {
      // No overall timeout, because a local model can sit for minutes before its first token;
      // a stream that has gone silent for five, though, is a connection that died under it
      // (a laptop that slept, a gateway that dropped it) and would otherwise wait for ever.
      client: Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(300))
        .build()?,
      endpoint: endpoint.trim_end_matches('/').into(),
      model,
      headers,
      retries,
    })
  }

  pub(super) fn with_model(&self, model: String) -> Self {
    Self {
      model,
      ..self.clone()
    }
  }

  fn request(&self, builder: RequestBuilder) -> RequestBuilder {
    self.headers.iter().fold(builder, |builder, (name, value)| {
      builder.header(name, value)
    })
  }

  /// Discovery runs during setup, before an agent and its event sink exist, so a retry here
  /// has nothing to report through.
  pub(super) async fn models(&self, path: &str) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct ModelList {
      data: Vec<Model>,
    }
    #[derive(Deserialize)]
    struct Model {
      id: String,
    }
    let url = format!("{}/{path}", self.endpoint);
    let response = send("model discovery", self.retries, None, || {
      self.request(self.client.get(&url))
    })
    .await?;
    let body = crate::output::response(response, MAX_TRANSFER).await?;
    let mut models: Vec<_> = serde_json::from_slice::<ModelList>(&body)
      .context("invalid model list")?
      .data
      .into_iter()
      .map(|model| model.id)
      .collect();
    models.sort();
    models.dedup();
    Ok(models)
  }

  pub(super) async fn post(
    &self,
    path: &str,
    body: &Value,
    events: &EventSink,
  ) -> Result<Response> {
    let url = format!("{}/{path}", self.endpoint);
    send(
      &format!("model request ({})", self.model),
      self.retries,
      Some(events),
      || self.request(self.client.post(&url)).json(body),
    )
    .await
  }
}

/// Feeds each event's data to `each` until it says the reply is complete or the body ends.
pub(super) async fn read_events(
  response: Response,
  mut each: impl FnMut(&str) -> Result<bool>,
) -> Result<()> {
  let mut stream = response.bytes_stream();
  let mut decoder = SseDecoder::default();
  let mut received = 0;
  while let Some(chunk) = stream.next().await {
    let chunk = chunk.context("read model stream")?;
    received += chunk.len();
    if received > MAX_TRANSFER {
      bail!("model stream exceeds the transfer limit");
    }
    for event in decoder.push(&chunk) {
      if each(&event)? {
        return Ok(());
      }
    }
  }
  for event in decoder.finish() {
    if each(&event)? {
      return Ok(());
    }
  }
  Ok(())
}

/// Adds a header the configuration did not already set, whatever case it was set in.
pub(super) fn default_header(headers: &mut BTreeMap<String, String>, name: &str, value: String) {
  if !headers.keys().any(|key| key.eq_ignore_ascii_case(name)) {
    headers.insert(name.into(), value);
  }
}
