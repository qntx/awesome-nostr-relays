//! Utilities for mutating a [`HealthReport`] based on probe outcomes.

use std::collections::HashSet;

use chrono::{DateTime, SubsecRound, Utc};

use crate::{
    model::{HealthEntry, HealthReport, HealthState, Outcome},
    probe_types::ProbeSuccess,
};

/// Maximum number of characters retained from a failure message.
const MAX_ERROR_CHARS: usize = 200;

/// Current time truncated to whole seconds, keeping persisted timestamps and
/// their JSON diffs readable across CI runs.
fn now_secs() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(0)
}

/// Apply a successful probe to the health entry identified by `url`.
pub fn record_success(report: &mut HealthReport, url: &str, success: &ProbeSuccess) {
    let entry = report.entries.entry(url.to_owned()).or_default();
    let now = now_secs();
    entry.last_checked = Some(now);
    entry.last_success = Some(now);
    entry.consecutive_failures = 0;
    entry.last_outcome = Outcome::Success;
    entry.rtt_open_ms = Some(success.rtt_ms);
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
    entry.last_checked = Some(now_secs());
    entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
    entry.last_outcome = Outcome::Failure;
    entry.rtt_open_ms = None;
    entry.last_error = Some(truncate(error, MAX_ERROR_CHARS));
}

/// Record a failure that should **not** bump the consecutive-failure counter.
///
/// Intended for errors outside the relay's control, e.g. DNS hiccups on the
/// runner or Cloudflare bot-fight HTTP 403 — recording them for visibility
/// without wrongly pushing the relay towards the dead threshold.
pub fn record_soft_failure(report: &mut HealthReport, url: &str, error: &str) {
    let entry = report.entries.entry(url.to_owned()).or_default();
    entry.last_checked = Some(now_secs());
    entry.last_outcome = Outcome::Blocked;
    entry.rtt_open_ms = None;
    entry.last_error = Some(truncate(error, MAX_ERROR_CHARS));
}

/// Mark that the probe for `url` was deliberately skipped. Unlike a failure,
/// this does **not** increment the consecutive-failure counter: the relay
/// simply lies outside what the current runner can verify.
pub fn record_skipped(report: &mut HealthReport, url: &str) {
    let entry = report.entries.entry(url.to_owned()).or_default();
    entry.last_checked = Some(now_secs());
    entry.last_outcome = Outcome::Skipped;
    entry.rtt_open_ms = None;
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

/// Emoji for a relay's current [`HealthState`], computed against the wall
/// clock. `None` (no entry recorded) maps to [`HealthState::Unknown`].
#[must_use]
pub fn status_icon(entry: Option<&HealthEntry>) -> &'static str {
    entry
        .map_or(HealthState::Unknown, |entry| entry.state(Utc::now()))
        .icon()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe_types::ProbeSuccess;

    const URL: &str = "wss://example.com/";

    fn success() -> ProbeSuccess {
        ProbeSuccess {
            rtt_ms: 42,
            first_frame: Some("EOSE".to_owned()),
            nip11: None,
        }
    }

    #[test]
    fn success_sets_outcome_and_resets_failures() {
        let mut report = HealthReport::default();
        record_failure(&mut report, URL, "boom");
        record_failure(&mut report, URL, "boom again");
        record_success(&mut report, URL, &success());

        let entry = report.entries.get(URL).expect("entry exists");
        assert_eq!(entry.consecutive_failures, 0);
        assert_eq!(entry.last_outcome, Outcome::Success);
        assert_eq!(entry.rtt_open_ms, Some(42));
        assert!(entry.is_online());
    }

    #[test]
    fn failure_increments_counter_and_sets_outcome() {
        let mut report = HealthReport::default();
        record_skipped(&mut report, URL);
        record_failure(&mut report, URL, "network down");

        let entry = report.entries.get(URL).expect("entry exists");
        assert_eq!(entry.consecutive_failures, 1);
        assert_eq!(entry.last_outcome, Outcome::Failure);
        assert_eq!(entry.last_error.as_deref(), Some("network down"));
    }

    #[test]
    fn soft_failure_blocks_without_bumping_counter() {
        let mut report = HealthReport::default();
        record_soft_failure(&mut report, URL, "dns lookup failed");
        record_soft_failure(&mut report, URL, "forbidden (HTTP 403)");

        let entry = report.entries.get(URL).expect("entry exists");
        assert_eq!(entry.consecutive_failures, 0);
        assert_eq!(entry.last_outcome, Outcome::Blocked);
        assert_eq!(entry.last_error.as_deref(), Some("forbidden (HTTP 403)"));
        assert_eq!(entry.state(Utc::now()), HealthState::Blocked);
    }

    #[test]
    fn soft_failure_after_success_is_not_healthy() {
        // Regression: a relay that succeeded once then gets WAF-blocked must
        // not keep reporting healthy.
        let mut report = HealthReport::default();
        record_success(&mut report, URL, &success());
        record_soft_failure(&mut report, URL, "forbidden (HTTP 403)");

        let entry = report.entries.get(URL).expect("entry exists");
        assert!(entry.last_success.is_some());
        assert_eq!(entry.state(Utc::now()), HealthState::Blocked);
        assert!(!entry.is_online());
    }

    #[test]
    fn skipped_sets_skipped_outcome() {
        let mut report = HealthReport::default();
        record_skipped(&mut report, URL);

        let entry = report.entries.get(URL).expect("entry exists");
        assert_eq!(entry.consecutive_failures, 0);
        assert_eq!(entry.last_outcome, Outcome::Skipped);
        assert_eq!(entry.state(Utc::now()), HealthState::Skipped);
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
    fn status_icon_reflects_state() {
        assert_eq!(status_icon(None), HealthState::Unknown.icon());

        let mut report = HealthReport::default();
        record_success(&mut report, URL, &success());
        assert_eq!(
            status_icon(report.entries.get(URL)),
            HealthState::Healthy.icon()
        );

        record_soft_failure(&mut report, URL, "forbidden");
        assert_eq!(
            status_icon(report.entries.get(URL)),
            HealthState::Blocked.icon()
        );

        record_skipped(&mut report, URL);
        assert_eq!(
            status_icon(report.entries.get(URL)),
            HealthState::Skipped.icon()
        );
    }
}
