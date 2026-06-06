//! Core library for the awesome-nostr-relays catalog.
//!
//! This crate is free of network IO: it defines the domain model parsed from
//! [`relays.toml`](../relays.toml), validation rules, health-tracking logic,
//! and the JSON / Markdown renderers. Actual relay probing lives in the
//! `relays-probe` crate, which depends on the probe result types
//! ([`probe_types`]) re-exported here.

pub mod health;
pub mod model;
pub mod probe_types;
pub mod render;
pub mod source;
pub mod validate;

pub use model::{
    Collection, Dataset, HealthEntry, HealthReport, HealthState, Network, Outcome, Relay,
    Requirements,
};
pub use probe_types::{
    Nip11, ProbeConfig, ProbeError, ProbeOutcome, ProbeResult, ProbeSuccess, SkipReason,
};

/// Number of consecutive failures that flag a relay as `⚠️` unhealthy.
pub const WARN_FAILURE_THRESHOLD: u32 = 3;

/// Number of consecutive failures that flag a relay as `💀` dead (candidate
/// for manual removal).
pub const DEAD_FAILURE_THRESHOLD: u32 = 14;

/// Monitoring freshness window in hours.
///
/// Entries whose most recent probe is older than this are reported as
/// [`model::HealthState::Stale`] regardless of their last outcome, guarding
/// consumers against a silently stalled pipeline.
pub const STALE_AFTER_HOURS: i64 = 72;

/// Standard probe cadence in seconds, advertised in the JSON `monitor` block
/// (NIP-66 kind 10166). Keep in sync with the health-check workflow schedule.
pub const MONITOR_FREQUENCY_SECONDS: u64 = 10_800;

/// Standard per-relay probe timeout in milliseconds, advertised in `monitor`.
pub const MONITOR_TIMEOUT_MS: u64 = 30_000;

/// Start marker for the auto-generated section of `README.md`.
pub const README_START_MARKER: &str = "<!-- RELAYS:START -->";

/// End marker for the auto-generated section of `README.md`.
pub const README_END_MARKER: &str = "<!-- RELAYS:END -->";
