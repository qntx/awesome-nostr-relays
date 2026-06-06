//! Domain types for the catalog.
//!
//! The data model is intentionally minimal: [`Relay`] carries curator-provided
//! metadata from `relays.toml`, while [`HealthEntry`] carries CI-measured
//! runtime facts from `health.json`. The two are kept in separate files so
//! that human edits never race with automated probe runs.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

/// Root document parsed from `relays.toml`.
///
/// Deserialised from TOML; never serialised back, so it intentionally
/// implements only [`Deserialize`].
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Dataset {
    /// Layout version; bumped when incompatible schema changes land.
    pub schema_version: String,
    /// Discoverability buckets referenced by relay entries.
    #[serde(default)]
    pub collections: Vec<Collection>,
    /// All relays tracked by the catalog.
    #[serde(default)]
    pub relays: Vec<Relay>,
}

/// A discoverability bucket referenced by one or more relays.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct Collection {
    /// Unique, lowercase kebab-case identifier.
    pub id: String,
    /// Human-friendly title displayed in the README and JSON outputs.
    pub name: String,
    /// One-paragraph description of what belongs in this collection.
    pub description: String,
}

/// Network transport a relay is reachable over (NIP-66 `n` tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Network {
    /// Reachable over the public internet.
    #[default]
    Clearnet,
    /// Tor hidden service (`.onion`).
    Tor,
    /// I2P eepsite (`.i2p`).
    I2p,
    /// Lokinet service (`.loki`).
    Loki,
}

impl Network {
    /// Lowercase NIP-66 `n` label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clearnet => "clearnet",
            Self::Tor => "tor",
            Self::I2p => "i2p",
            Self::Loki => "loki",
        }
    }

    /// Derive the network from a relay URL's host suffix, defaulting to
    /// [`Network::Clearnet`].
    #[must_use]
    pub fn from_url(url: &Url) -> Self {
        match url.host_str().and_then(|host| host.rsplit_once('.')) {
            Some((_, tld)) if tld.eq_ignore_ascii_case("onion") => Self::Tor,
            Some((_, tld)) if tld.eq_ignore_ascii_case("i2p") => Self::I2p,
            Some((_, tld)) if tld.eq_ignore_ascii_case("loki") => Self::Loki,
            _ => Self::Clearnet,
        }
    }
}

/// Posting requirements a relay enforces, aligned with NIP-66's `R` tag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct Requirements {
    /// Requires NIP-42 AUTH before serving reads/writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<bool>,
    /// Requires payment (admission, subscription, or per-event) to post.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment: Option<bool>,
    /// Requires Proof-of-Work (NIP-13) on submitted events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pow: Option<bool>,
    /// Accepts writes from the general public (`false` = read-only/restricted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writes: Option<bool>,
}

impl Requirements {
    /// `true` when no requirement is declared; used to skip serialization.
    #[must_use]
    #[allow(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde `skip_serializing_if` requires a `fn(&T) -> bool` predicate"
    )]
    pub const fn is_empty(&self) -> bool {
        self.auth.is_none() && self.payment.is_none() && self.pow.is_none() && self.writes.is_none()
    }
}

/// A single relay entry as described by a curator.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct Relay {
    /// Normalised WebSocket URL (`wss://...` or `ws://...onion`).
    pub url: Url,
    /// Short human-friendly name; falls back to the URL host when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional one-sentence description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Operator name or pubkey; either an org label or a Nostr npub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    /// ISO-3166 alpha-2 uppercase country code, or `XX` if unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// Optional NIP-52 geohash for finer-grained location (NIP-66 `g`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geohash: Option<String>,
    /// Optional curator override for the network transport (NIP-66 `n`). The
    /// public JSON instead exposes [`Relay::effective_network`], so this raw
    /// override is deserialize-only to avoid emitting a duplicate `network` key.
    #[serde(default, skip_serializing)]
    pub network: Option<Network>,
    /// Relay software implementation (e.g. `strfry`, `khatru`, `nostream`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub software: Option<String>,
    /// Posting requirements declared by the curator (NIP-66 `R`).
    #[serde(default, skip_serializing_if = "Requirements::is_empty")]
    pub requirements: Requirements,
    /// IDs of the collections this relay belongs to.
    #[serde(default)]
    pub collections: Vec<String>,
    /// Free-form lowercase topics for fine-grained discovery (NIP-66 `t`).
    #[serde(default)]
    pub topics: Vec<String>,
    /// Date the relay was first added to the catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<NaiveDate>,
}

impl Relay {
    /// Display name for Markdown rendering, falling back to the URL host when
    /// `name` is missing.
    #[must_use]
    pub fn display_name(&self) -> String {
        if let Some(name) = &self.name
            && !name.is_empty()
        {
            return name.clone();
        }
        self.url.host_str().unwrap_or("<unknown>").to_owned()
    }

    /// Effective network transport: the explicit [`Relay::network`] override
    /// or, when absent, the value derived from the URL host.
    #[must_use]
    pub fn effective_network(&self) -> Network {
        self.network.unwrap_or_else(|| Network::from_url(&self.url))
    }

    /// `true` when the relay requires payment to post events.
    #[must_use]
    pub const fn is_paid(&self) -> bool {
        matches!(self.requirements.payment, Some(true))
    }
}

/// Disposition of the most recent probe attempt, persisted to drive
/// [`HealthEntry::state`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Outcome {
    /// No probe has been recorded yet.
    #[default]
    Unknown,
    /// The WebSocket handshake succeeded.
    Success,
    /// A hard failure attributable to the relay (timeout, TLS, protocol, 5xx).
    Failure,
    /// An environmental block outside the relay's control (WAF 4xx, DNS).
    Blocked,
    /// The probe was deliberately skipped (e.g. onion without a Tor proxy).
    Skipped,
}

/// Coarse health classification derived from a [`HealthEntry`] at an instant.
///
/// Computed, never stored, because staleness is relative to the current time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum HealthState {
    /// Last probe succeeded and the data is fresh.
    Healthy,
    /// Hard-failing but still below [`crate::WARN_FAILURE_THRESHOLD`].
    Flaky,
    /// At/above the warn threshold of consecutive hard failures.
    Warn,
    /// At/above the dead threshold; a candidate for manual removal.
    Dead,
    /// Monitoring data is older than [`crate::STALE_AFTER_HOURS`]; the pipeline
    /// may have stalled, so the last outcome can no longer be trusted.
    Stale,
    /// The last probe was blocked by the environment (WAF 4xx / DNS); the
    /// relay's real availability could not be verified.
    Blocked,
    /// The probe was deliberately skipped (e.g. onion without a Tor proxy).
    Skipped,
    /// Never probed.
    Unknown,
}

impl HealthState {
    /// Lowercase machine-readable label, matching the JSON serialization.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Flaky => "flaky",
            Self::Warn => "warn",
            Self::Dead => "dead",
            Self::Stale => "stale",
            Self::Blocked => "blocked",
            Self::Skipped => "skipped",
            Self::Unknown => "unknown",
        }
    }

    /// Emoji used in Markdown / dashboard rendering.
    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Healthy => "✅",
            Self::Flaky => "🟡",
            Self::Warn => "⚠️",
            Self::Dead => "💀",
            Self::Stale => "🕒",
            Self::Blocked => "🚫",
            Self::Skipped => "🧅",
            Self::Unknown => "❔",
        }
    }

    /// `true` only for [`Self::Healthy`]; the single source of truth for
    /// "is this relay usable right now".
    #[must_use]
    pub const fn is_online(self) -> bool {
        matches!(self, Self::Healthy)
    }
}

/// Runtime health aggregate for a single relay URL, persisted across CI runs.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct HealthEntry {
    /// Timestamp of the most recent probe attempt, regardless of outcome.
    pub last_checked: Option<DateTime<Utc>>,
    /// Timestamp of the most recent successful probe.
    pub last_success: Option<DateTime<Utc>>,
    /// Number of consecutive hard failures; reset to 0 on success.
    #[serde(default)]
    pub consecutive_failures: u32,
    /// Disposition of the most recent probe attempt.
    #[serde(default)]
    pub last_outcome: Outcome,
    /// Open round-trip time of the latest successful handshake (NIP-66
    /// `rtt-open`), in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_open_ms: Option<u64>,
    /// Short, truncated error message from the latest failed probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Supported NIP numbers as reported by the relay's NIP-11 document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_nips: Option<Vec<u16>>,
    /// `software` string from the NIP-11 document, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nip11_software: Option<String>,
    /// `version` string from the NIP-11 document, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nip11_version: Option<String>,
}

impl HealthEntry {
    /// Classify this entry at instant `now`.
    ///
    /// Priority: never-probed → `Unknown`; stalled monitoring → `Stale`; then
    /// the last outcome (`Skipped` / `Blocked` / `Failure` by severity /
    /// `Success`). A `Blocked` outcome never reports `Healthy`, fixing the
    /// previous "pseudo-healthy" behaviour for relays behind a WAF or failing
    /// DNS that had succeeded once long ago.
    #[must_use]
    pub fn state(&self, now: DateTime<Utc>) -> HealthState {
        let Some(last_checked) = self.last_checked else {
            return HealthState::Unknown;
        };
        if now.signed_duration_since(last_checked)
            > chrono::TimeDelta::hours(crate::STALE_AFTER_HOURS)
        {
            return HealthState::Stale;
        }
        match self.last_outcome {
            Outcome::Unknown => HealthState::Unknown,
            Outcome::Skipped => HealthState::Skipped,
            Outcome::Blocked => HealthState::Blocked,
            Outcome::Failure => {
                if self.consecutive_failures >= crate::DEAD_FAILURE_THRESHOLD {
                    HealthState::Dead
                } else if self.consecutive_failures >= crate::WARN_FAILURE_THRESHOLD {
                    HealthState::Warn
                } else {
                    HealthState::Flaky
                }
            }
            Outcome::Success => HealthState::Healthy,
        }
    }

    /// Convenience wrapper around [`HealthEntry::state`] using the current wall
    /// clock; `true` only when the relay is [`HealthState::Healthy`].
    #[must_use]
    pub fn is_online(&self) -> bool {
        self.state(Utc::now()).is_online()
    }
}

/// Top-level health snapshot persisted at `health.json`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[non_exhaustive]
pub struct HealthReport {
    /// Timestamp of the most recent [`crate::probe`] run across the entire
    /// catalog.
    pub last_run: Option<DateTime<Utc>>,
    /// Per-URL entries keyed by the relay URL string.
    #[serde(default)]
    pub entries: BTreeMap<String, HealthEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay(url: &str, name: Option<&str>) -> Relay {
        Relay {
            url: Url::parse(url).expect("valid url"),
            name: name.map(str::to_owned),
            description: None,
            operator: None,
            country: None,
            geohash: None,
            network: None,
            software: None,
            requirements: Requirements::default(),
            collections: Vec::new(),
            topics: Vec::new(),
            added_at: None,
        }
    }

    #[test]
    fn display_name_prefers_curator_label() {
        let r = relay("wss://example.com/", Some("Example"));
        assert_eq!(r.display_name(), "Example");
    }

    #[test]
    fn display_name_falls_back_to_host() {
        let r = relay("wss://example.com/", None);
        assert_eq!(r.display_name(), "example.com");
    }

    #[test]
    fn display_name_ignores_empty_label() {
        let r = relay("wss://example.com/", Some(""));
        assert_eq!(r.display_name(), "example.com");
    }

    #[test]
    fn network_derives_tor_from_onion_url() {
        let r = relay("ws://abc.onion/", None);
        assert_eq!(r.effective_network(), Network::Tor);
    }

    #[test]
    fn network_defaults_to_clearnet() {
        let r = relay("wss://example.com/", None);
        assert_eq!(r.effective_network(), Network::Clearnet);
    }

    #[test]
    fn network_explicit_override_wins() {
        let mut r = relay("wss://example.com/", None);
        r.network = Some(Network::I2p);
        assert_eq!(r.effective_network(), Network::I2p);
    }

    #[test]
    fn is_paid_reflects_payment_requirement() {
        let mut r = relay("wss://example.com/", None);
        assert!(!r.is_paid());
        r.requirements.payment = Some(true);
        assert!(r.is_paid());
    }

    #[test]
    fn state_unknown_when_never_probed() {
        let entry = HealthEntry::default();
        assert_eq!(entry.state(Utc::now()), HealthState::Unknown);
        assert!(!entry.is_online());
    }

    #[test]
    fn state_healthy_on_fresh_success() {
        let now = Utc::now();
        let entry = HealthEntry {
            last_checked: Some(now),
            last_success: Some(now),
            last_outcome: Outcome::Success,
            ..HealthEntry::default()
        };
        assert_eq!(entry.state(now), HealthState::Healthy);
        assert!(entry.state(now).is_online());
    }

    #[test]
    fn state_blocked_never_reports_healthy() {
        let now = Utc::now();
        let entry = HealthEntry {
            last_checked: Some(now),
            last_success: Some(now - chrono::TimeDelta::days(30)),
            last_outcome: Outcome::Blocked,
            ..HealthEntry::default()
        };
        assert_eq!(entry.state(now), HealthState::Blocked);
        assert!(!entry.state(now).is_online());
    }

    #[test]
    fn state_tiers_failures_by_threshold() {
        let now = Utc::now();
        let mut entry = HealthEntry {
            last_checked: Some(now),
            last_outcome: Outcome::Failure,
            consecutive_failures: 1,
            ..HealthEntry::default()
        };
        assert_eq!(entry.state(now), HealthState::Flaky);
        entry.consecutive_failures = crate::WARN_FAILURE_THRESHOLD;
        assert_eq!(entry.state(now), HealthState::Warn);
        entry.consecutive_failures = crate::DEAD_FAILURE_THRESHOLD;
        assert_eq!(entry.state(now), HealthState::Dead);
    }

    #[test]
    fn state_stale_when_pipeline_stalls() {
        let now = Utc::now();
        let old = now - chrono::TimeDelta::hours(crate::STALE_AFTER_HOURS + 1);
        let entry = HealthEntry {
            last_checked: Some(old),
            last_success: Some(old),
            last_outcome: Outcome::Success,
            ..HealthEntry::default()
        };
        assert_eq!(entry.state(now), HealthState::Stale);
    }

    #[test]
    fn state_skipped_outcome() {
        let now = Utc::now();
        let entry = HealthEntry {
            last_checked: Some(now),
            last_outcome: Outcome::Skipped,
            ..HealthEntry::default()
        };
        assert_eq!(entry.state(now), HealthState::Skipped);
    }
}
