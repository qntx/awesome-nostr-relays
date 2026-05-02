//! Nostr relay liveness probe.
//!
//! A probe performs, in order:
//!
//! 1. **WebSocket handshake** — the primary liveness signal. A successful
//!    handshake proves the relay is reachable and speaks the WebSocket
//!    protocol on the URL advertised in the catalog.
//! 2. **Best-effort first-frame wait** — we send a bounded `REQ` and wait up
//!    to [`FRAME_WAIT_BUDGET`] for any recognised Nostr frame
//!    (`EVENT` / `EOSE` / `NOTICE` / `AUTH` / `OK` / `CLOSED` / `COUNT`).
//!    Receiving a frame proves the server speaks Nostr, **but its absence is
//!    not a failure**: many popular relays (Damus, Primal, Nostr.Band, …)
//!    require NIP-42 AUTH and silently close anonymous subscriptions.
//! 3. **NIP-11 Relay Information Document** — HTTP GET *after* the WS path
//!    has succeeded; strictly metadata enrichment, never fatal.
//!
//! Failures are returned as typed [`ProbeError`] variants so the health
//! tracker can distinguish transient / environmental issues (DNS, WAF 403)
//! from real outages.
//!
//! Tor / onion relays are reported as [`ProbeOutcome::Skipped`] because CI
//! runners don't carry a Tor proxy; the README renderer uses a dedicated
//! icon so operators aren't misled into thinking the probe verified them.

use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{
        Error as WsError, Message,
        client::IntoClientRequest,
        http::{HeaderValue, header},
    },
};
use url::Url;

/// Mozilla-compatible `User-Agent` applied to both the HTTP (NIP-11) client
/// and the WebSocket handshake.
///
/// The `(compatible; <bot>/<ver>; +<url>)` format is the form Google, Bing
/// and similar crawlers use. Empirically it slips through Cloudflare's
/// default bot-fight rules far more reliably than a bare `relays/0.1` UA,
/// which was being served HTTP 403 on ~35% of catalog entries.
pub const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; relays-monitor/",
    env!("CARGO_PKG_VERSION"),
    "; +https://github.com/qntx/awesome-nostr-relays)"
);

/// Concrete WebSocket stream type used by the probe.
type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

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
    /// The WebSocket handshake completed. Optional frame-wait observations
    /// are attached via [`ProbeSuccess::first_frame`].
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
    /// Time from probe start to completion of the WebSocket handshake.
    pub rtt_ms: u64,
    /// Kind of the first Nostr frame observed within [`FRAME_WAIT_BUDGET`]
    /// after the handshake, if any (`EVENT` / `AUTH` / `NOTICE` / …).
    /// `None` means the handshake succeeded but no frame arrived in time;
    /// the relay is still considered healthy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_frame: Option<String>,
    /// Parsed NIP-11 Relay Information Document, if any was returned.
    #[serde(skip_serializing_if = "Option::is_none")]
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

/// Classified probe failure.
///
/// The variant determines whether a failure is treated as a real service
/// outage via [`Self::counts_as_failure`]: environmental issues (DNS hiccups,
/// upstream WAF blocks) do not bump the relay's consecutive-failure counter.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProbeError {
    /// DNS lookup failed — usually transient or runner-side.
    #[error("dns lookup failed: {0}")]
    Dns(String),
    /// Whole probe exceeded the configured hard timeout.
    #[error("probe timed out after {0:?}")]
    Timeout(Duration),
    /// Server rejected with HTTP 4xx; usually Cloudflare / WAF bot protection.
    #[error("forbidden by upstream (HTTP {status})")]
    Forbidden {
        /// HTTP status code observed during the WebSocket upgrade attempt.
        status: u16,
    },
    /// Server responded with a non-101 HTTP status that isn't a bot block.
    #[error("unexpected HTTP status {status} during WebSocket handshake")]
    UnexpectedStatus {
        /// HTTP status code observed during the WebSocket upgrade attempt.
        status: u16,
    },
    /// TLS handshake or certificate validation failed.
    #[error("tls error: {0}")]
    Tls(String),
    /// WebSocket protocol violation (bad framing, attack attempt, etc.).
    #[error("protocol error: {0}")]
    Protocol(String),
    /// URL cannot be turned into a probe target (unsupported scheme, bad path).
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    /// Any other error that doesn't fit the above categories.
    #[error("probe failed: {0}")]
    Other(String),
}

impl ProbeError {
    /// Whether this error should bump `consecutive_failures` on the relay.
    ///
    /// Environmental issues (DNS, WAF 4xx) are recorded but don't accumulate
    /// towards the dead/warn thresholds since they don't reflect the relay's
    /// actual availability.
    #[must_use]
    pub const fn counts_as_failure(&self) -> bool {
        !matches!(self, Self::Dns(_) | Self::Forbidden { .. })
    }

    /// Short machine-readable classification label for structured logs.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Dns(_) => "dns",
            Self::Timeout(_) => "timeout",
            Self::Forbidden { .. } => "forbidden",
            Self::UnexpectedStatus { .. } => "http-status",
            Self::Tls(_) => "tls",
            Self::Protocol(_) => "protocol",
            Self::InvalidUrl(_) => "invalid-url",
            Self::Other(_) => "other",
        }
    }
}

/// Result type returned by [`probe`].
pub type ProbeResult = Result<ProbeOutcome, ProbeError>;

/// Maximum per-request timeout for the NIP-11 HTTP fetch, regardless of the
/// caller-supplied overall probe timeout.
const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(8);

/// Default overall probe timeout when no caller override is provided.
///
/// Chosen to comfortably cover a cross-Atlantic TLS + WebSocket handshake
/// plus a short frame-wait window ([`FRAME_WAIT_BUDGET`]) with headroom.
/// Previously `10s`, which empirically timed out ~20 % of healthy JP / EU
/// relays measured from US GitHub runners.
const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Sub-budget reserved for the best-effort first-frame wait once the
/// WebSocket handshake has completed. Short enough to keep the probe snappy
/// for healthy relays while still giving AUTH-required relays a chance to
/// send a `["AUTH", ...]` challenge.
const FRAME_WAIT_BUDGET: Duration = Duration::from_secs(5);

/// Per-relay probe configuration.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ProbeConfig {
    /// Hard timeout for the entire probe (handshake + frame wait + NIP-11).
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
/// Returns:
///
/// * [`ProbeOutcome::Skipped`] for onion hosts (no Tor proxy available).
/// * [`ProbeOutcome::Success`] after the WebSocket handshake completes. The
///   `first_frame` field reports whether a Nostr frame was also observed
///   within [`FRAME_WAIT_BUDGET`], but its absence is not a failure.
///
/// # Errors
///
/// Returns a classified [`ProbeError`] describing the failure category.
/// Callers can use [`ProbeError::counts_as_failure`] to decide whether the
/// failure should bump the relay's `consecutive_failures` counter.
pub async fn probe(client: &reqwest::Client, relay_url: &Url, config: ProbeConfig) -> ProbeResult {
    if relay_url.host_str().is_some_and(is_onion_host) {
        return Ok(ProbeOutcome::Skipped(SkipReason::OnionUnreachable));
    }

    let started = Instant::now();

    // WebSocket handshake is the primary liveness signal.
    let mut stream = timeout(config.timeout, connect_ws(relay_url))
        .await
        .map_err(|_| ProbeError::Timeout(config.timeout))?
        .map_err(classify_ws_error)?;

    let handshake_rtt = millis_saturating(started.elapsed().as_millis());

    // Best-effort first-frame wait within the remaining time budget, capped
    // at [`FRAME_WAIT_BUDGET`]. A missing frame no longer fails the probe:
    // handshake success alone proves the relay is reachable.
    let remaining = config.timeout.saturating_sub(started.elapsed());
    let frame_budget = remaining.min(FRAME_WAIT_BUDGET);
    let first_frame = wait_for_first_frame(&mut stream, frame_budget).await;

    drop(timeout(Duration::from_secs(1), stream.close(None)).await);

    // NIP-11 is metadata-only and only attempted after the WS path succeeded.
    let nip11 = match ws_to_http(relay_url) {
        Ok(http_url) => fetch_nip11(client, &http_url, config.http_timeout)
            .await
            .ok()
            .flatten(),
        Err(_) => None,
    };

    Ok(ProbeOutcome::Success(ProbeSuccess {
        rtt_ms: handshake_rtt,
        first_frame,
        nip11,
    }))
}

/// Open a WebSocket stream with the custom [`USER_AGENT`] header set so that
/// Cloudflare / similar WAF layers are less likely to serve HTTP 403.
async fn connect_ws(relay_url: &Url) -> Result<WsStream, WsError> {
    let mut request = relay_url.as_str().into_client_request()?;
    request
        .headers_mut()
        .insert(header::USER_AGENT, HeaderValue::from_static(USER_AGENT));
    let (stream, _) = connect_async(request).await?;
    Ok(stream)
}

/// Send a bounded `REQ` and wait up to `budget` for any recognised Nostr
/// frame. Responds to `Ping` along the way to keep the connection healthy.
///
/// Returns `Some(frame_kind)` when a recognised frame arrives, or `None`
/// when the budget expires / the peer closes cleanly / an I/O error occurs.
#[allow(
    clippy::collapsible_match,
    reason = "cannot move non-Copy `payload` into a match guard (E0507); \
              inner `if` is the idiomatic form here"
)]
async fn wait_for_first_frame(stream: &mut WsStream, budget: Duration) -> Option<String> {
    const PROBE_SUB_ID: &str = "relays-probe";

    let req = serde_json::json!(["REQ", PROBE_SUB_ID, { "kinds": [1], "limit": 1 }]);
    if stream.send(text_message(&req.to_string())).await.is_err() {
        return None;
    }

    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let Ok(Some(Ok(message))) = timeout(remaining, stream.next()).await else {
            return None;
        };
        match message {
            Message::Text(text) => {
                if let Some(kind) = parse_frame_kind(&text)
                    && is_protocol_frame(&kind)
                {
                    let close = serde_json::json!(["CLOSE", PROBE_SUB_ID]).to_string();
                    drop(stream.send(text_message(&close)).await);
                    return Some(kind);
                }
            }
            Message::Ping(payload) => {
                if stream.send(Message::Pong(payload)).await.is_err() {
                    return None;
                }
            }
            Message::Close(_) => return None,
            _ => {}
        }
    }
}

/// Map a `tungstenite::Error` to a classified [`ProbeError`].
fn classify_ws_error(err: WsError) -> ProbeError {
    let display = err.to_string();
    match err {
        WsError::Http(response) => {
            let status = response.status().as_u16();
            if (400..500).contains(&status) {
                ProbeError::Forbidden { status }
            } else {
                ProbeError::UnexpectedStatus { status }
            }
        }
        WsError::Io(io) => {
            let io_msg = io.to_string();
            if is_dns_failure(&io_msg) {
                ProbeError::Dns(io_msg)
            } else if is_tls_failure(&io_msg) {
                ProbeError::Tls(io_msg)
            } else {
                ProbeError::Other(io_msg)
            }
        }
        WsError::Url(_) | WsError::HttpFormat(_) => ProbeError::InvalidUrl(display),
        WsError::Protocol(_) | WsError::AttackAttempt => ProbeError::Protocol(display),
        _ => {
            if is_tls_failure(&display) {
                ProbeError::Tls(display)
            } else {
                ProbeError::Other(display)
            }
        }
    }
}

fn is_dns_failure(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("lookup address")
        || lower.contains("name resolution")
        || lower.contains("failed to resolve")
        || lower.contains("no address associated")
}

fn is_tls_failure(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("certificate") || lower.contains("tls") || lower.contains("fatal alert")
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
) -> Result<Option<Nip11>, reqwest::Error> {
    let fut = client
        .get(url.clone())
        .header("Accept", "application/nostr+json")
        .send();
    let Ok(send_result) = timeout(http_timeout, fut).await else {
        return Ok(None);
    };
    let response = send_result?;
    if !response.status().is_success() {
        return Ok(None);
    }
    Ok(response.json::<Nip11>().await.ok())
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

/// `true` when `kind` names a Nostr protocol frame.
///
/// The white-list was widened beyond `EVENT`/`EOSE`/`NOTICE` to include
/// `AUTH` (NIP-42 challenge), `OK` (NIP-20), `CLOSED` (NIP-01 close notice),
/// and `COUNT` (NIP-45). Previously, receiving an `AUTH` challenge — as
/// Damus, Primal, Nostr.Band and most major relays do — caused the probe to
/// hang until timeout.
fn is_protocol_frame(kind: &str) -> bool {
    matches!(
        kind,
        "EVENT" | "EOSE" | "NOTICE" | "AUTH" | "OK" | "CLOSED" | "COUNT"
    )
}

fn parse_frame_kind(text: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    parsed.get(0)?.as_str().map(str::to_owned)
}

fn ws_to_http(url: &Url) -> Result<Url, ProbeError> {
    let mut http = url.clone();
    let scheme = match url.scheme() {
        "wss" => "https",
        "ws" => "http",
        other => {
            return Err(ProbeError::InvalidUrl(format!(
                "unsupported scheme: {other}"
            )));
        }
    };
    http.set_scheme(scheme)
        .map_err(|()| ProbeError::InvalidUrl(format!("cannot rewrite scheme for {url}")))?;
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
    fn default_probe_timeout_is_at_least_fifteen_seconds() {
        assert!(DEFAULT_PROBE_TIMEOUT >= Duration::from_secs(15));
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
        let err = ws_to_http(&url).expect_err("https must not be accepted");
        assert!(matches!(err, ProbeError::InvalidUrl(_)));
    }

    #[test]
    fn is_protocol_frame_covers_full_nip_set() {
        for kind in ["EVENT", "EOSE", "NOTICE", "AUTH", "OK", "CLOSED", "COUNT"] {
            assert!(is_protocol_frame(kind), "{kind} should be recognised");
        }
    }

    #[test]
    fn is_protocol_frame_rejects_unknown_kinds() {
        assert!(!is_protocol_frame("REQ"));
        assert!(!is_protocol_frame("CLOSE"));
        assert!(!is_protocol_frame(""));
        assert!(!is_protocol_frame("event"));
    }

    #[test]
    fn parse_frame_kind_extracts_first_element() {
        assert_eq!(
            parse_frame_kind(r#"["AUTH","challenge-abc"]"#).as_deref(),
            Some("AUTH")
        );
        assert_eq!(
            parse_frame_kind(r#"["EOSE","sub"]"#).as_deref(),
            Some("EOSE")
        );
        assert_eq!(parse_frame_kind("not json"), None);
        assert_eq!(parse_frame_kind("[]"), None);
    }

    #[test]
    fn skip_reason_label_is_stable() {
        assert_eq!(SkipReason::OnionUnreachable.as_str(), "onion-unreachable");
    }

    #[test]
    fn probe_error_kind_is_stable() {
        assert_eq!(ProbeError::Dns("x".into()).kind(), "dns");
        assert_eq!(ProbeError::Timeout(Duration::ZERO).kind(), "timeout");
        assert_eq!(ProbeError::Forbidden { status: 403 }.kind(), "forbidden");
        assert_eq!(
            ProbeError::UnexpectedStatus { status: 530 }.kind(),
            "http-status"
        );
        assert_eq!(ProbeError::Tls("x".into()).kind(), "tls");
        assert_eq!(ProbeError::Protocol("x".into()).kind(), "protocol");
        assert_eq!(ProbeError::InvalidUrl("x".into()).kind(), "invalid-url");
        assert_eq!(ProbeError::Other("x".into()).kind(), "other");
    }

    #[test]
    fn probe_error_environmental_errors_do_not_count() {
        assert!(!ProbeError::Dns("x".into()).counts_as_failure());
        assert!(!ProbeError::Forbidden { status: 403 }.counts_as_failure());

        assert!(ProbeError::Timeout(Duration::ZERO).counts_as_failure());
        assert!(ProbeError::Tls("x".into()).counts_as_failure());
        assert!(ProbeError::Protocol("x".into()).counts_as_failure());
        assert!(ProbeError::UnexpectedStatus { status: 502 }.counts_as_failure());
    }

    #[test]
    fn dns_heuristic_matches_common_libc_errors() {
        assert!(is_dns_failure("failed to lookup address information: foo"));
        assert!(is_dns_failure("Temporary failure in name resolution"));
        assert!(is_dns_failure("No address associated with hostname"));
        assert!(!is_dns_failure("connection refused"));
    }

    #[test]
    fn tls_heuristic_matches_common_rustls_errors() {
        assert!(is_tls_failure("invalid peer certificate: expired"));
        assert!(is_tls_failure("received fatal alert: InternalError"));
        assert!(is_tls_failure("TLS handshake failure"));
        assert!(!is_tls_failure("connection reset"));
    }

    #[test]
    fn user_agent_uses_mozilla_compatible_format() {
        assert!(USER_AGENT.starts_with("Mozilla/5.0 (compatible;"));
        assert!(USER_AGENT.contains("relays-monitor"));
        assert!(USER_AGENT.contains("github.com/qntx/awesome-nostr-relays"));
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
