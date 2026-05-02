//! Utilities for mutating a [`HealthReport`] based on probe outcomes.

use std::collections::HashSet;

use chrono::Utc;

use crate::{
    model::{HealthEntry, HealthReport},
    probe::ProbeSuccess,
};

/// Maximum number of characters retained from a failure message.
const MAX_ERROR_CHARS: usize = 200;

/// Apply a successful probe to the health entry identified by `url`.
pub fn record_success(report: &mut HealthReport, url: &str, success: &ProbeSuccess) {
    let entry = report.entries.entry(url.to_owned()).or_default();
    let now = Utc::now();
    entry.last_checked = Some(now);
    entry.last_success = Some(now);
    entry.consecutive_failures = 0;
    entry.skipped = false;
    entry.rtt_ms = Some(success.rtt_ms);
    entry.last_error = None;
    if let Some(nip11) = &success.nip11 {
        entry.supported_nips.clone_from(&nip11.supported_nips);
        entry.nip11_software.clone_from(&nip11.software);
        entry.nip11_version.clone_from(&nip11.version);
    }
}

/// Apply a hard failed probe to the health entry identified by `url`.
///
/// Bumps `consecutive_failures`; use [`record_soft_failure`] for errors the
/// probe already classified as transient / environmental.
pub fn record_failure(report: &mut HealthReport, url: &str, error: &str) {
    let entry = report.entries.entry(url.to_owned()).or_default();
    entry.last_checked = Some(Utc::now());
    entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
    entry.skipped = false;
    entry.rtt_ms = None;
    entry.last_error = Some(truncate(error, MAX_ERROR_CHARS));
}

/// Record a failure that should **not** bump the consecutive-failure counter.
///
/// Intended for errors outside the relay's control, e.g. DNS hiccups on the
/// runner or Cloudflare bot-fight HTTP 403 — recording them for visibility
/// without wrongly pushing the relay towards the dead threshold.
pub fn record_soft_failure(report: &mut HealthReport, url: &str, error: &str) {
    let entry = report.entries.entry(url.to_owned()).or_default();
    entry.last_checked = Some(Utc::now());
    entry.skipped = false;
    entry.rtt_ms = None;
    entry.last_error = Some(truncate(error, MAX_ERROR_CHARS));
}

/// Mark that the probe for `url` was deliberately skipped. Unlike a failure,
/// this does **not** increment the consecutive-failure counter: the relay
/// simply lies outside what the current runner can verify.
pub fn record_skipped(report: &mut HealthReport, url: &str) {
    let entry = report.entries.entry(url.to_owned()).or_default();
    entry.last_checked = Some(Utc::now());
    entry.skipped = true;
    entry.rtt_ms = None;
    entry.last_error = None;
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

/// Classify a relay for README rendering.
///
/// Returns one of: `❔` never probed, `🧅` skipped (e.g. onion), `✅` healthy,
/// `⚠️` warn (>= [`WARN_FAILURE_THRESHOLD`](crate::WARN_FAILURE_THRESHOLD)
/// consecutive failures), `💀` dead
/// (>= [`DEAD_FAILURE_THRESHOLD`](crate::DEAD_FAILURE_THRESHOLD)).
#[must_use]
pub const fn status_icon(entry: Option<&HealthEntry>) -> &'static str {
    let Some(entry) = entry else {
        return "❔";
    };
    if entry.skipped {
        return "🧅";
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::ProbeSuccess;

    const URL: &str = "wss://example.com/";

    fn success() -> ProbeSuccess {
        ProbeSuccess {
            rtt_ms: 42,
            first_frame: Some("EOSE".to_owned()),
            nip11: None,
        }
    }

    #[test]
    fn success_resets_failure_counter_and_skipped() {
        let mut report = HealthReport::default();
        record_failure(&mut report, URL, "boom");
        record_failure(&mut report, URL, "boom again");
        record_success(&mut report, URL, &success());

        let entry = report.entries.get(URL).expect("entry exists");
        assert_eq!(entry.consecutive_failures, 0);
        assert!(!entry.skipped);
        assert_eq!(entry.rtt_ms, Some(42));
        assert!(entry.is_online());
    }

    #[test]
    fn failure_increments_counter_and_clears_skipped() {
        let mut report = HealthReport::default();
        record_skipped(&mut report, URL);
        record_failure(&mut report, URL, "network down");

        let entry = report.entries.get(URL).expect("entry exists");
        assert_eq!(entry.consecutive_failures, 1);
        assert!(!entry.skipped);
        assert_eq!(entry.last_error.as_deref(), Some("network down"));
    }

    #[test]
    fn soft_failure_records_error_without_bumping_counter() {
        let mut report = HealthReport::default();
        record_soft_failure(&mut report, URL, "dns lookup failed");
        record_soft_failure(&mut report, URL, "forbidden (HTTP 403)");

        let entry = report.entries.get(URL).expect("entry exists");
        assert_eq!(entry.consecutive_failures, 0);
        assert!(!entry.skipped);
        assert_eq!(entry.last_error.as_deref(), Some("forbidden (HTTP 403)"));
    }

    #[test]
    fn skipped_marks_entry_without_failure_count() {
        let mut report = HealthReport::default();
        record_skipped(&mut report, URL);

        let entry = report.entries.get(URL).expect("entry exists");
        assert_eq!(entry.consecutive_failures, 0);
        assert!(entry.skipped);
        assert!(!entry.is_online());
    }

    #[test]
    fn truncate_keeps_short_messages_verbatim() {
        assert_eq!(truncate("short", 10), "short");
    }

    #[test]
    fn truncate_appends_ellipsis_for_long_messages() {
        let long = "x".repeat(300);
        let trimmed = truncate(&long, 200);
        assert_eq!(trimmed.chars().count(), 201);
        assert!(trimmed.ends_with('…'));
    }

    #[test]
    fn prune_orphans_removes_unknown_urls() {
        let mut report = HealthReport::default();
        record_success(&mut report, "wss://a.example/", &success());
        record_success(&mut report, "wss://b.example/", &success());

        prune_orphans(&mut report, &["wss://a.example/".to_owned()]);
        assert!(report.entries.contains_key("wss://a.example/"));
        assert!(!report.entries.contains_key("wss://b.example/"));
    }

    #[test]
    fn status_icon_handles_all_states() {
        assert_eq!(status_icon(None), "❔");

        let mut entry = HealthEntry::default();
        assert_eq!(status_icon(Some(&entry)), "❔");

        entry.last_success = Some(Utc::now());
        assert_eq!(status_icon(Some(&entry)), "✅");

        entry.consecutive_failures = crate::WARN_FAILURE_THRESHOLD;
        assert_eq!(status_icon(Some(&entry)), "⚠️");

        entry.consecutive_failures = crate::DEAD_FAILURE_THRESHOLD;
        assert_eq!(status_icon(Some(&entry)), "💀");

        entry.skipped = true;
        assert_eq!(status_icon(Some(&entry)), "🧅");
    }
}
