//! Reading a URL. Without this a session can only reach the web through an MCP server, which
//! is a lot of setup for the commonest thing a coding agent needs from it: the page.

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Risk, Tool, ToolContext, truncate};
use crate::{network, protocol::ToolSpec};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
    let limit = args
      .max_bytes
      .unwrap_or(context.max_output_bytes)
      .min(context.max_output_bytes);
    let response = network::fetch(&args.url, limit, Duration::from_secs(30)).await?;
    let text = if response.kind.contains("html") {
      readable(&response.text)
    } else {
      response.text
    };
    Ok(truncate(
      format!("{}\n{}\n\n{}", response.url, response.kind, text.trim()),
      limit,
    ))
  }
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
  use super::readable;
  use crate::network::guard;

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
