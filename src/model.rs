//! Domain types for the catalog.
//!
//! The data model is intentionally minimal: [`Relay`] carries curator-provided
//! metadata from `relays.toml`, while [`HealthEntry`] carries CI-measured
//! runtime facts from `health.json`. The two are kept in separate files so
//! that human edits never race with automated probe runs.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

/// Root document parsed from `relays.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
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
#[derive(Debug, Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Collection {
    /// Unique, lowercase kebab-case identifier.
    pub id: String,
    /// Human-friendly title displayed in the README and JSON outputs.
    pub name: String,
    /// One-paragraph description of what belongs in this collection.
    pub description: String,
}

/// A single relay entry as described by a curator.
#[derive(Debug, Clone, Deserialize, Serialize)]
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
    /// ISO-3166 alpha-2 uppercase country code, `XX` if unknown, `T1` for
    /// tor-only services.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// Relay software implementation (e.g. `strfry`, `khatru`, `nostream`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub software: Option<String>,
    /// `true` when the relay requires payment to post events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paid: Option<bool>,
    /// IDs of the collections this relay belongs to.
    #[serde(default)]
    pub collections: Vec<String>,
    /// Free-form lowercase tags for fine-grained discovery.
    #[serde(default)]
    pub tags: Vec<String>,
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
}

/// Runtime health aggregate for a single relay URL, persisted across CI runs.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[non_exhaustive]
pub struct HealthEntry {
    /// Timestamp of the most recent probe attempt, regardless of outcome.
    pub last_checked: Option<DateTime<Utc>>,
    /// Timestamp of the most recent successful probe.
    pub last_success: Option<DateTime<Utc>>,
    /// Number of consecutive failed probes; reset to 0 on success.
    #[serde(default)]
    pub consecutive_failures: u32,
    /// Round-trip time of the latest successful WebSocket handshake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<u64>,
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
    /// `true` when the most recent probe succeeded and no consecutive failures
    /// are recorded.
    #[must_use]
    pub const fn is_online(&self) -> bool {
        self.consecutive_failures == 0 && self.last_success.is_some()
    }
}

/// Top-level health snapshot persisted at `health.json`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[non_exhaustive]
pub struct HealthReport {
    /// Timestamp of the most recent [`crate::probe`] run across the entire
    /// catalog.
    pub last_run: Option<DateTime<Utc>>,
    /// Per-URL entries keyed by the relay URL string.
    #[serde(default)]
    pub entries: BTreeMap<String, HealthEntry>,
}
