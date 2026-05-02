//! Core library for the awesome-nostr-relays catalog.
//!
//! The crate exposes a single [`Dataset`] type that is loaded from
//! [`relays.toml`](../relays.toml) and optionally enriched with health data
//! from `health.json`. All subcommands (`validate`, `build`, `check`) operate
//! on a dataset to produce JSON artefacts, Markdown, or updated health
//! snapshots.

// These crates are consumed only by the `anr` binary target; they appear
// in `[dependencies]` because Cargo has no per-target dep table for binaries
// in the same crate. Silencing the `unused_crate_dependencies` lint here keeps
// the lint meaningful for all other deps.
#[allow(
    unused_imports,
    reason = "deps are consumed by the `anr` binary target"
)]
use {clap as _, indicatif as _, tracing as _, tracing_subscriber as _};

pub mod health;
pub mod model;
pub mod probe;
pub mod render;
pub mod source;
pub mod validate;

pub use model::{Collection, Dataset, HealthEntry, HealthReport, Relay};

/// Number of consecutive failures that flag a relay as `⚠️` unhealthy.
pub const WARN_FAILURE_THRESHOLD: u32 = 3;

/// Number of consecutive failures that flag a relay as `💀` dead (candidate
/// for manual removal).
pub const DEAD_FAILURE_THRESHOLD: u32 = 14;

/// Start marker for the auto-generated section of `README.md`.
pub const README_START_MARKER: &str = "<!-- RELAYS:START -->";

/// End marker for the auto-generated section of `README.md`.
pub const README_END_MARKER: &str = "<!-- RELAYS:END -->";
