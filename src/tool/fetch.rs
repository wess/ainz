//! Reading a URL. Without this a session can only reach the web through an MCP server, which
//! is a lot of setup for the commonest thing a coding agent needs from it: the page.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Risk, Tool, ToolContext, truncate};
use crate::protocol::ToolSpec;

#[derive(Deserialize)]
struct FetchArgs {
  url: String,
  #[serde(default)]
  max_bytes: Option<usize>,
}

pub(super) fn tool() -> Arc<dyn Tool> {
  Arc::new(Fetch)
}

struct Fetch;

#[async_trait]
impl Tool for Fetch {
  fn spec(&self) -> ToolSpec {
    ToolSpec {
      name: "fetch".into(),
      description: "Read a web page or file over http(s), as text".into(),
      parameters: json!({
        "type": "object", "properties": {
          "url": {"type": "string"},
          "max_bytes": {"type": "integer", "minimum": 1}
        }, "required": ["url"], "additionalProperties": false
      }),
    }
  }

  fn risk(&self, _arguments: &Value) -> Risk {
    Risk::Network
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    let args: FetchArgs = serde_json::from_value(arguments)?;
    let url = reqwest::Url::parse(args.url.trim()).context("invalid url")?;
    guard(&url)?;
    let client = Client::builder()
      .connect_timeout(Duration::from_secs(15))
      .timeout(Duration::from_secs(30))
      .build()
      .context("build HTTP client")?;
    let response = client.get(url).send().await.context("fetch")?;
    let status = response.status();
    let final_url = response.url().to_string();
    let kind = response
      .headers()
      .get(reqwest::header::CONTENT_TYPE)
      .and_then(|value| value.to_str().ok())
      .unwrap_or("unknown")
      .to_string();
    let body = response.text().await.context("read the body")?;
    if !status.is_success() {
      bail!("{final_url} answered {status}");
    }
    let text = match kind.contains("html") {
      true => readable(&body),
      false => body,
    };
    let limit = args.max_bytes.unwrap_or(context.max_output_bytes);
    Ok(truncate(
      format!("{final_url}\n{kind}\n\n{}", text.trim()),
      limit,
    ))
  }
}

/// A coding agent that can reach the machine's own network is a way into everything the machine
/// can reach, including a cloud metadata service. The web is what this tool is for.
fn guard(url: &reqwest::Url) -> Result<()> {
  if !matches!(url.scheme(), "http" | "https") {
    bail!("fetch reads http and https, not {}", url.scheme());
  }
  let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
  let local = host == "localhost"
    || host.ends_with(".localhost")
    || host == "::1"
    || host == "[::1]"
    || host.starts_with("127.")
    || host.starts_with("10.")
    || host.starts_with("192.168.")
    || host.starts_with("169.254.")
    || (host.starts_with("172.")
      && host
        .split('.')
        .nth(1)
        .and_then(|part| part.parse::<u8>().ok())
        .is_some_and(|part| (16..=31).contains(&part)));
  if local {
    bail!("fetch will not reach {host}; it is on this machine's own network");
  }
  Ok(())
}

/// HTML as a person would read it: no markup, no script or style bodies, one space where the
/// source had a paragraph of them.
pub(crate) fn readable(html: &str) -> String {
  let mut text = String::with_capacity(html.len() / 2);
  let mut rest = html;
  while let Some(start) = rest.find('<') {
    text.push_str(&rest[..start]);
    rest = &rest[start..];
    let lowered = rest.to_ascii_lowercase();
    // a script or style body is not prose, and dropping the tags alone would leave the code
    let skipped = ["script", "style"].iter().find_map(|tag| {
      lowered
        .starts_with(&format!("<{tag}"))
        .then(|| lowered.find(&format!("</{tag}")))
        .flatten()
    });
    let end = match skipped {
      Some(end) => rest[end..].find('>').map(|close| end + close),
      None => rest.find('>'),
    };
    match end {
      Some(end) => {
        // a block tag is a break in the text, or every paragraph runs into the next
        if ["</p", "<br", "</div", "</li", "</h", "</tr"]
          .iter()
          .any(|tag| lowered.starts_with(tag))
        {
          text.push('\n');
        }
        rest = &rest[end + 1..];
      }
      None => {
        rest = "";
      }
    }
  }
  text.push_str(rest);
  collapse(&unescape(&text))
}

fn unescape(text: &str) -> String {
  text
    .replace("&lt;", "<")
    .replace("&gt;", ">")
    .replace("&quot;", "\"")
    .replace("&#39;", "'")
    .replace("&apos;", "'")
    .replace("&nbsp;", " ")
    .replace("&amp;", "&")
}

fn collapse(text: &str) -> String {
  text
    .lines()
    .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
    .filter(|line| !line.is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

#[cfg(test)]
mod tests {
  use super::{guard, readable};

  fn url(value: &str) -> reqwest::Url {
    reqwest::Url::parse(value).unwrap()
  }

  #[test]
  fn only_the_web_is_reachable() {
    assert!(guard(&url("https://example.com/x")).is_ok());
    assert!(guard(&url("file:///etc/passwd")).is_err());
    assert!(guard(&url("http://localhost:8080/")).is_err());
    assert!(guard(&url("http://127.0.0.1/")).is_err());
    assert!(guard(&url("http://169.254.169.254/latest/meta-data")).is_err());
    assert!(guard(&url("http://172.16.0.4/")).is_err());
    // 172 is only private through 31, so the rest of it is ordinary internet
    assert!(guard(&url("http://172.32.0.4/")).is_ok());
  }

  #[test]
  fn html_reads_as_text() {
    let html = "<html><head><style>body{color:red}</style>\
      <script>var x = 1 < 2;</script></head><body><h1>Title</h1>\
      <p>First &amp; best</p><p>Second&nbsp;line</p></body></html>";

    assert_eq!(readable(html), "Title\nFirst & best\nSecond line");
  }
}
