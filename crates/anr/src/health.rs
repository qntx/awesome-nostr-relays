//! Utilities for mutating a [`HealthReport`] based on probe outcomes.

use std::collections::HashSet;

use chrono::Utc;

use crate::{
    model::{HealthEntry, HealthReport},
    probe::ProbeSuccess,
};

/// Apply a successful probe to the health entry identified by `url`.
pub fn record_success(report: &mut HealthReport, url: &str, success: &ProbeSuccess) {
    let entry = report.entries.entry(url.to_owned()).or_default();
    let now = Utc::now();
    entry.last_checked = Some(now);
    entry.last_success = Some(now);
    entry.consecutive_failures = 0;
    entry.rtt_ms = Some(success.rtt_ms);
    entry.last_error = None;
    if let Some(nip11) = &success.nip11 {
        entry.supported_nips.clone_from(&nip11.supported_nips);
        entry.nip11_software.clone_from(&nip11.software);
        entry.nip11_version.clone_from(&nip11.version);
    }
}

/// Apply a failed probe to the health entry identified by `url`.
pub fn record_failure(report: &mut HealthReport, url: &str, error: &str) {
    let entry = report.entries.entry(url.to_owned()).or_default();
    entry.last_checked = Some(Utc::now());
    entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
    entry.rtt_ms = None;
    entry.last_error = Some(truncate(error, 200));
}

fn truncate(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_owned();
    }
    let mut out: String = input.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// Drop health entries whose URL is no longer present in the catalog, so
/// stale data does not accumulate across months of CI runs.
pub fn prune_orphans(report: &mut HealthReport, known_urls: &[String]) {
    let known: HashSet<&str> = known_urls.iter().map(String::as_str).collect();
    report.entries.retain(|url, _| known.contains(url.as_str()));
}

/// Classify a relay as `ok`, `warn`, or `dead` for README rendering.
#[must_use]
pub const fn status_icon(entry: Option<&HealthEntry>) -> &'static str {
    let Some(entry) = entry else {
        return "❔";
    };
    if entry.consecutive_failures >= crate::DEAD_FAILURE_THRESHOLD {
        "💀"
    } else if entry.consecutive_failures >= crate::WARN_FAILURE_THRESHOLD {
        "⚠️"
    } else if entry.is_online() {
        "✅"
    } else {
        "❔"
    }
}
