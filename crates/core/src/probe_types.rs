//! Probe result and configuration types.
//!
//! These types are free of network IO so that `relays-core` carries no async
//! or networking dependencies. The actual probing logic lives in the
//! `relays-probe` crate, which depends on these definitions and returns them.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

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
    /// Kind of the first Nostr frame observed within the frame-wait budget
    /// after the handshake, if any (`EVENT` / `AUTH` / `NOTICE` / …).
    /// `None` means the handshake succeeded but no frame arrived in time;
    /// the relay is still considered healthy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_frame: Option<String>,
    /// Parsed NIP-11 Relay Information Document, if any was returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nip11: Option<Nip11>,
}

impl ProbeSuccess {
    /// Construct a successful probe result from its measured parts.
    ///
    /// Provided because the type is `#[non_exhaustive]`, which forbids struct
    /// literals from other crates such as `relays-probe`.
    #[must_use]
    pub const fn new(rtt_ms: u64, first_frame: Option<String>, nip11: Option<Nip11>) -> Self {
        Self {
            rtt_ms,
            first_frame,
            nip11,
        }
    }
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

/// Result type returned by a probe.
pub type ProbeResult = Result<ProbeOutcome, ProbeError>;

/// Maximum per-request timeout for the NIP-11 HTTP fetch, regardless of the
/// caller-supplied overall probe timeout.
pub const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(8);

/// Default overall probe timeout when no caller override is provided.
///
/// Chosen to comfortably cover a cross-Atlantic TLS + WebSocket handshake
/// plus a short frame-wait window with headroom.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

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
}
