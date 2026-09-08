use std::{
  net::{IpAddr, SocketAddr},
  time::Duration,
};

use anyhow::{Context, Result, bail};
use reqwest::{Client, Url, redirect::Policy};

pub(crate) struct Response {
  pub url: String,
  pub kind: String,
  pub text: String,
}

pub(crate) fn guard(url: &Url) -> Result<()> {
  if !matches!(url.scheme(), "http" | "https") {
    bail!("fetch only supports http and https");
  }
  if !url.username().is_empty() || url.password().is_some() {
    bail!("fetch URLs must not contain credentials");
  }
  match url.host().context("URL has no host")? {
    url::Host::Ipv4(ip) => ensure_public(ip.into())?,
    url::Host::Ipv6(ip) => ensure_public(ip.into())?,
    url::Host::Domain(host) => {
      let host = host.trim_end_matches('.').to_ascii_lowercase();
      if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        bail!("fetch will not reach this machine's own network");
      }
    }
  }
  Ok(())
}

fn ensure_public(ip: IpAddr) -> Result<()> {
  let public = match ip {
    IpAddr::V4(ip) => {
      let [a, b, c, _] = ip.octets();
      !(matches!(a, 0 | 10 | 127 | 224..=255)
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && (b == 168 || (b == 0 && matches!(c, 0 | 2))))
        || (a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
        || (a == 203 && b == 0 && c == 113))
    }
    IpAddr::V6(ip) => {
      let parts = ip.segments();
      // only global unicast; mapped, local, multicast and transition ranges stay out
      (parts[0] & 0xe000) == 0x2000
        && parts[0] != 0x2002
        && !(parts[0] == 0x2001 && (parts[1] <= 0x1ff || parts[1] == 0xdb8))
        && !(parts[0] == 0x3fff && parts[1] < 0x1000)
    }
  };
  if !public {
    bail!("fetch will not reach {ip}; it is not a public address");
  }
  Ok(())
}

pub(crate) async fn fetch(url: &str, limit: usize, duration: Duration) -> Result<Response> {
  tokio::time::timeout(duration, fetch_inner(url, limit))
    .await
    .context("fetch timed out")?
}

async fn fetch_inner(input: &str, limit: usize) -> Result<Response> {
  let mut url = Url::parse(input.trim()).context("invalid url")?;
  for hop in 0..=5 {
    guard(&url)?;
    let host = url.host_str().context("URL has no host")?;
    let port = url.port_or_known_default().context("URL has no port")?;
    let addresses: Vec<SocketAddr> = match url.host().context("URL has no host")? {
      url::Host::Ipv4(ip) => vec![(IpAddr::V4(ip), port).into()],
      url::Host::Ipv6(ip) => vec![(IpAddr::V6(ip), port).into()],
      url::Host::Domain(host) => tokio::net::lookup_host((host, port)).await?.collect(),
    };
    validate_addresses(&addresses)?;
    // pin the checked addresses so DNS cannot change between validation and connection.
    // ambient proxies would bypass that destination check.
    let client = Client::builder()
      .no_proxy()
      .redirect(Policy::none())
      .connect_timeout(Duration::from_secs(15))
      .resolve_to_addrs(host, &addresses)
      .build()?;
    let mut response = client.get(url.clone()).send().await.context("fetch")?;
    if response.status().is_redirection() {
      if hop == 5 {
        bail!("fetch exceeded the redirect limit");
      }
      let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .context("redirect has no location")?
        .to_str()?;
      url = redirect(&url, location)?;
      continue;
    }
    response = response.error_for_status()?;
    let kind = response
      .headers()
      .get(reqwest::header::CONTENT_TYPE)
      .and_then(|value| value.to_str().ok())
      .unwrap_or("unknown")
      .to_string();
    if response
      .content_length()
      .is_some_and(|length| length > limit as u64)
    {
      bail!("response exceeds the {limit} byte transfer limit");
    }
    let bytes = crate::output::response(response, limit).await?;
    return Ok(Response {
      url: url.to_string(),
      kind,
      text: String::from_utf8_lossy(&bytes).into_owned(),
    });
  }
  unreachable!()
}

fn validate_addresses(addresses: &[SocketAddr]) -> Result<()> {
  if addresses.is_empty() {
    bail!("fetch hostname resolved to no addresses");
  }
  for address in addresses {
    ensure_public(address.ip())?;
  }
  Ok(())
}

fn redirect(url: &Url, location: &str) -> Result<Url> {
  let next = url.join(location)?;
  guard(&next)?;
  Ok(next)
}

#[cfg(test)]
#[path = "../tests/network/mod.rs"]
mod tests;
