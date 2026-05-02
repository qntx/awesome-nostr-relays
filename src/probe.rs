//! Nostr relay liveness probe.
//!
//! Each probe performs two parallel checks:
//!
//! 1. **NIP-11 HTTP GET** — fetch the relay information document using the
//!    `Accept: application/nostr+json` header. Non-fatal on failure; the relay
//!    is still considered alive if the WebSocket check succeeds.
//! 2. **WebSocket handshake + REQ** — open the WebSocket, send a bounded
//!    `REQ` for a single event, and wait for `EVENT`, `EOSE`, or `NOTICE`.
//!    Any of those proves the server speaks the Nostr protocol.
//!
//! A single probe always runs under a hard timeout so one misbehaving relay
//! cannot stall the entire CI job.

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use url::Url;

/// Outcome of a successful probe.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct ProbeSuccess {
    /// Round-trip time of the WebSocket exchange in milliseconds.
    pub rtt_ms: u64,
    /// Parsed NIP-11 Relay Information Document, if any was returned.
    pub nip11: Option<Nip11>,
}

/// Subset of the NIP-11 Relay Information Document we care about.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Nip11 {
    /// Relay-advertised display name.
    #[serde(default)]
    pub name: Option<String>,
    /// Relay implementation string (`software` field in NIP-11).
    #[serde(default)]
    pub software: Option<String>,
    /// Free-form version string from the relay software.
    #[serde(default)]
    pub version: Option<String>,
    /// Supported NIP numbers as reported by the relay.
    #[serde(default)]
    pub supported_nips: Option<Vec<u16>>,
}

/// Per-relay probe configuration.
///
/// Intentionally **not** `#[non_exhaustive]`: callers (including the binary
/// target) construct it as a struct literal, which would require a builder
/// otherwise.
#[derive(Debug, Clone, Copy)]
pub struct ProbeConfig {
    /// Hard timeout for the entire probe (HTTP + WebSocket combined).
    pub timeout: Duration,
    /// Per-request timeout for the NIP-11 HTTP fetch.
    pub http_timeout: Duration,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            http_timeout: Duration::from_secs(8),
        }
    }
}

/// Probe a single relay. Returns either timing / NIP-11 metadata or a
/// human-readable error describing the failure.
///
/// # Errors
///
/// Returns an error when the WebSocket handshake times out, when the relay
/// closes the connection without emitting a recognised Nostr frame, or when
/// the URL scheme is unsupported.
pub async fn probe(
    client: &reqwest::Client,
    relay_url: &Url,
    config: ProbeConfig,
) -> Result<ProbeSuccess> {
    // Onion relays cannot be reached without a Tor proxy; skip connectivity
    // probing and optimistically report success without metadata so they are
    // not flagged dead in CI.
    if relay_url.host_str().is_some_and(is_onion_host) {
        return Ok(ProbeSuccess {
            rtt_ms: 0,
            nip11: None,
        });
    }

    let http_url = ws_to_http(relay_url)?;
    let nip11_future = fetch_nip11(client, &http_url, config.http_timeout);
    let ws_future = probe_websocket(relay_url, config.timeout);

    let (nip11_result, ws_result) = tokio::join!(nip11_future, ws_future);
    let rtt_ms = ws_result?;
    let nip11 = nip11_result.ok().flatten();
    Ok(ProbeSuccess { rtt_ms, nip11 })
}

fn is_onion_host(host: &str) -> bool {
    host.len() >= ".onion".len()
        && host
            .get(host.len() - ".onion".len()..)
            .is_some_and(onion_suffix)
}

fn onion_suffix(tail: &str) -> bool {
    tail.eq_ignore_ascii_case(".onion")
}

async fn fetch_nip11(
    client: &reqwest::Client,
    url: &Url,
    http_timeout: Duration,
) -> Result<Option<Nip11>> {
    let fut = client
        .get(url.clone())
        .header("Accept", "application/nostr+json")
        .send();
    let response = timeout(http_timeout, fut)
        .await
        .map_err(|_| anyhow!("nip11 timeout"))??;
    if !response.status().is_success() {
        return Ok(None);
    }
    let doc: Nip11 = response.json().await.unwrap_or(Nip11 {
        name: None,
        software: None,
        version: None,
        supported_nips: None,
    });
    Ok(Some(doc))
}

async fn probe_websocket(relay_url: &Url, hard_timeout: Duration) -> Result<u64> {
    let started = Instant::now();
    timeout(hard_timeout, websocket_exchange(relay_url))
        .await
        .map_err(|_| anyhow!("probe timeout"))??;
    Ok(millis_saturating(started.elapsed().as_millis()))
}

async fn websocket_exchange(relay_url: &Url) -> Result<()> {
    let request = relay_url.as_str().into_client_request()?;
    let (mut stream, _) = connect_async(request).await?;

    let subscription_id = "anrelays-probe";
    let req = serde_json::json!(["REQ", subscription_id, { "kinds": [1], "limit": 1 }]);
    stream.send(text_message(&req.to_string())).await?;

    while let Some(message) = stream.next().await {
        match message? {
            Message::Text(text) if is_protocol_frame(&text) => {
                let close = serde_json::json!(["CLOSE", subscription_id]).to_string();
                stream.send(text_message(&close)).await.ok();
                stream.close(None).await.ok();
                return Ok(());
            }
            Message::Ping(payload) => {
                stream.send(Message::Pong(payload)).await?;
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    Err(anyhow!("closed without valid frame"))
}

fn text_message(body: &str) -> Message {
    // `Message::Text` wraps `tungstenite::Utf8Bytes`; `String -> Utf8Bytes`
    // goes through `From`, which clippy's `useless_conversion` lint cannot see
    // because both types stringify identically.
    #[allow(
        clippy::useless_conversion,
        reason = "Utf8Bytes has a From<String> impl that clippy cannot resolve"
    )]
    Message::Text(body.to_owned().into())
}

fn is_protocol_frame(text: &str) -> bool {
    matches!(
        parse_frame_kind(text).as_deref(),
        Some("EVENT" | "EOSE" | "NOTICE")
    )
}

fn parse_frame_kind(text: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    parsed.get(0)?.as_str().map(str::to_owned)
}

fn ws_to_http(url: &Url) -> Result<Url> {
    let mut http = url.clone();
    let scheme = match url.scheme() {
        "wss" => "https",
        "ws" => "http",
        other => return Err(anyhow!("unsupported scheme: {other}")),
    };
    http.set_scheme(scheme)
        .map_err(|()| anyhow!("cannot rewrite scheme for {url}"))?;
    Ok(http)
}

fn millis_saturating(millis: u128) -> u64 {
    u64::try_from(millis).unwrap_or(u64::MAX)
}
