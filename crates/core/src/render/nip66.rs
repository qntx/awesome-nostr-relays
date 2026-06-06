//! NIP-66 relay discovery export.
//!
//! Emits `nip66.json`: an array of unsigned `kind 30166` (relay discovery)
//! event templates, one per catalogued relay, ready for a NIP-66 monitor to
//! sign and publish. Tags follow NIP-66: `d` (normalised URL), `n` (network),
//! `N` (supported NIPs), `R` (requirements, `!`-prefixed when false), `t`
//! (topics), `g` (geohash) and `rtt-open` (open round-trip time).

use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use serde::Serialize;

use crate::model::{Dataset, HealthReport, Relay};

/// File name of the NIP-66 discovery export.
pub const NIP66_FILE: &str = "nip66.json";

/// Addressable relay discovery event kind defined by NIP-66.
const KIND_RELAY_DISCOVERY: u16 = 30166;

/// Unsigned NIP-66 `kind 30166` event template.
#[derive(Serialize)]
struct DiscoveryEvent {
    kind: u16,
    created_at: i64,
    content: &'static str,
    tags: Vec<Vec<String>>,
}

/// Write `nip66.json` into `api_dir`, reusing the content-addressed writer so
/// the file is only rewritten when the underlying templates change.
///
/// # Errors
///
/// Returns an error if serialisation or the file write fails.
pub fn write(dataset: &Dataset, health: &HealthReport, api_dir: &Path) -> Result<()> {
    let now = Utc::now().timestamp();
    let events: Vec<DiscoveryEvent> = dataset
        .relays
        .iter()
        .map(|relay| discovery_event(relay, health, now))
        .collect();
    super::json::write_json_if_changed(&api_dir.join(NIP66_FILE), &events)
}

fn discovery_event(relay: &Relay, health: &HealthReport, now: i64) -> DiscoveryEvent {
    let entry = health.entries.get(relay.url.as_str());
    let created_at = entry
        .and_then(|entry| entry.last_checked)
        .map_or(now, |checked| checked.timestamp());

    let mut tags: Vec<Vec<String>> = vec![
        vec!["d".to_owned(), relay.url.as_str().to_owned()],
        vec![
            "n".to_owned(),
            relay.effective_network().as_str().to_owned(),
        ],
    ];

    if let Some(entry) = entry {
        if let Some(nips) = &entry.supported_nips {
            for nip in nips {
                tags.push(vec!["N".to_owned(), nip.to_string()]);
            }
        }
        if let Some(rtt) = entry.rtt_open_ms {
            tags.push(vec!["rtt-open".to_owned(), rtt.to_string()]);
        }
    }

    push_requirement(&mut tags, "auth", relay.requirements.auth);
    push_requirement(&mut tags, "payment", relay.requirements.payment);
    push_requirement(&mut tags, "pow", relay.requirements.pow);
    push_requirement(&mut tags, "writes", relay.requirements.writes);

    for topic in &relay.topics {
        tags.push(vec!["t".to_owned(), topic.clone()]);
    }
    if let Some(geohash) = &relay.geohash {
        tags.push(vec!["g".to_owned(), geohash.clone()]);
    }

    DiscoveryEvent {
        kind: KIND_RELAY_DISCOVERY,
        created_at,
        content: "",
        tags,
    }
}

/// Append a NIP-66 `R` requirement tag, using a `!`-prefix for `false`.
fn push_requirement(tags: &mut Vec<Vec<String>>, key: &str, value: Option<bool>) {
    if let Some(enabled) = value {
        let label = if enabled {
            key.to_owned()
        } else {
            format!("!{key}")
        };
        tags.push(vec!["R".to_owned(), label]);
    }
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;
    use crate::model::{HealthReport, Relay, Requirements};

    fn relay() -> Relay {
        Relay {
            url: Url::parse("wss://relay.example/").expect("valid url"),
            name: None,
            description: None,
            operator: None,
            country: None,
            geohash: Some("ww8p1r4t8".to_owned()),
            network: None,
            software: None,
            requirements: Requirements {
                payment: Some(true),
                auth: Some(false),
                ..Requirements::default()
            },
            collections: vec!["global".to_owned()],
            topics: vec!["bitcoin".to_owned()],
            added_at: None,
        }
    }

    fn has_tag(event: &DiscoveryEvent, expected: &[&str]) -> bool {
        let expected: Vec<String> = expected.iter().map(|s| (*s).to_owned()).collect();
        event.tags.iter().any(|tag| tag == &expected)
    }

    #[test]
    fn discovery_event_emits_nip66_tags() {
        let event = discovery_event(&relay(), &HealthReport::default(), 1_000);
        assert_eq!(event.kind, KIND_RELAY_DISCOVERY);
        assert_eq!(event.created_at, 1_000);
        assert!(has_tag(&event, &["d", "wss://relay.example/"]));
        assert!(has_tag(&event, &["n", "clearnet"]));
        assert!(has_tag(&event, &["R", "payment"]));
        assert!(has_tag(&event, &["R", "!auth"]));
        assert!(has_tag(&event, &["t", "bitcoin"]));
        assert!(has_tag(&event, &["g", "ww8p1r4t8"]));
    }

    #[test]
    fn discovery_event_derives_tor_network_from_onion() {
        let mut relay = relay();
        relay.url = Url::parse("ws://abc.onion/").expect("valid onion url");
        let event = discovery_event(&relay, &HealthReport::default(), 1_000);
        assert!(has_tag(&event, &["n", "tor"]));
    }
}
