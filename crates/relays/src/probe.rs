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
//!
//! Tor / onion relays are reported as [`ProbeOutcome::Skipped`] because the
//! CI runners do not carry a Tor proxy. The downstream renderer shows them
//! with a dedicated icon so operators are not misled into thinking the probe
//! actually verified the relay.

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

/// Terminal outcome of a single probe attempt.
///
/// A [`Self::Skipped`] result is **not** a failure: it signals that the probe
/// could not meaningfully run (e.g. the target is an onion host and the CI
/// runner has no Tor route), and the health tracker must not penalise the
/// relay's failure counter.
///
/// Intentionally exhaustive: every probe either succeeds or is skipped; any
/// runtime error is returned via [`Result::Err`]. Callers may therefore
/// match this enum without a wildcard arm.
#[derive(Debug, Clone)]
pub enum ProbeOutcome {
    /// The WebSocket handshake completed and the relay replied with a valid
    /// Nostr frame.
    Success(ProbeSuccess),
    /// The probe deliberately did not run; the caller should record the
    /// skip without altering liveness statistics.
    Skipped(SkipReason),
}

/// Reason why a probe was skipped rather than executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkipReason {
    /// The target is a `.onion` host and the runner has no Tor proxy.
    OnionUnreachable,
}

impl SkipReason {
    /// Short machine-readable label suitable for structured logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OnionUnreachable => "onion-unreachable",
        }
    }
}

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

/// Maximum per-request timeout for the NIP-11 HTTP fetch, regardless of the
/// caller-supplied overall probe timeout.
const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(8);

/// Default overall probe timeout when no caller override is provided.
const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-relay probe configuration.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ProbeConfig {
    /// Hard timeout for the entire probe (HTTP + WebSocket combined).
    pub timeout: Duration,
    /// Per-request timeout for the NIP-11 HTTP fetch.
    pub http_timeout: Duration,
}

impl ProbeConfig {
    /// Build a config from a single overall timeout. The NIP-11 HTTP request
    /// is capped at [`DEFAULT_HTTP_TIMEOUT`] so a flaky information document
    /// cannot consume the whole probe budget.
    #[must_use]
    pub fn from_timeout(timeout: Duration) -> Self {
        Self {
            timeout,
            http_timeout: timeout.min(DEFAULT_HTTP_TIMEOUT),
        }
    }
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self::from_timeout(DEFAULT_PROBE_TIMEOUT)
    }
}

/// Probe a single relay.
///
/// Returns [`ProbeOutcome::Skipped`] for onion hosts without attempting any
/// connectivity, or [`ProbeOutcome::Success`] after a WebSocket handshake
/// and Nostr-protocol frame exchange.
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
) -> Result<ProbeOutcome> {
    if relay_url.host_str().is_some_and(is_onion_host) {
        return Ok(ProbeOutcome::Skipped(SkipReason::OnionUnreachable));
    }

    let http_url = ws_to_http(relay_url)?;
    let nip11_future = fetch_nip11(client, &http_url, config.http_timeout);
    let ws_future = probe_websocket(relay_url, config.timeout);

    let (nip11_result, ws_result) = tokio::join!(nip11_future, ws_future);
    let rtt_ms = ws_result?;
    let nip11 = nip11_result.ok().flatten();
    Ok(ProbeOutcome::Success(ProbeSuccess { rtt_ms, nip11 }))
}

fn is_onion_host(host: &str) -> bool {
    matches!(
        host.rsplit_once('.'),
        Some((_, tld)) if tld.eq_ignore_ascii_case("onion")
    )
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

    let subscription_id = "relays-probe";
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_config_from_timeout_caps_http_budget() {
        let config = ProbeConfig::from_timeout(Duration::from_secs(30));
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(config.http_timeout, DEFAULT_HTTP_TIMEOUT);
    }

    #[test]
    fn probe_config_from_timeout_shrinks_below_cap() {
        let config = ProbeConfig::from_timeout(Duration::from_secs(3));
        assert_eq!(config.timeout, Duration::from_secs(3));
        assert_eq!(config.http_timeout, Duration::from_secs(3));
    }

    #[test]
    fn is_onion_host_detects_tld() {
        assert!(is_onion_host("abc.onion"));
        assert!(is_onion_host("ABC.ONION"));
        assert!(is_onion_host("sub.abc.onion"));
    }

    #[test]
    fn is_onion_host_rejects_non_tor() {
        assert!(!is_onion_host("example.com"));
        assert!(!is_onion_host("onion.example.com"));
        assert!(!is_onion_host(""));
        assert!(!is_onion_host("onion"));
    }

    #[test]
    fn ws_to_http_rewrites_scheme() {
        let ws = Url::parse("ws://example.com/").expect("valid ws url");
        let wss = Url::parse("wss://example.com/").expect("valid wss url");
        assert_eq!(ws_to_http(&ws).expect("rewrite ws").scheme(), "http");
        assert_eq!(ws_to_http(&wss).expect("rewrite wss").scheme(), "https");
    }

    #[test]
    fn ws_to_http_rejects_unsupported_scheme() {
        let url = Url::parse("https://example.com/").expect("valid https url");
        assert!(ws_to_http(&url).is_err());
    }

    #[test]
    fn is_protocol_frame_accepts_nostr_replies() {
        assert!(is_protocol_frame(r#"["EVENT","sub",{}]"#));
        assert!(is_protocol_frame(r#"["EOSE","sub"]"#));
        assert!(is_protocol_frame(r#"["NOTICE","hi"]"#));
    }

    #[test]
    fn is_protocol_frame_rejects_other_frames() {
        assert!(!is_protocol_frame(r#"["OK","id",true,""]"#));
        assert!(!is_protocol_frame(r#"["CLOSED","sub",""]"#));
        assert!(!is_protocol_frame("not json"));
        assert!(!is_protocol_frame("{}"));
    }

    #[test]
    fn skip_reason_label_is_stable() {
        assert_eq!(SkipReason::OnionUnreachable.as_str(), "onion-unreachable");
    }

    #[tokio::test]
    async fn probe_skips_onion_hosts() {
        let client = reqwest::Client::new();
        let url = Url::parse("ws://abc.onion/").expect("valid onion url");
        let outcome = probe(&client, &url, ProbeConfig::default())
            .await
            .expect("onion skip is not an error");
        assert!(matches!(
            outcome,
            ProbeOutcome::Skipped(SkipReason::OnionUnreachable)
        ));
    }
}
