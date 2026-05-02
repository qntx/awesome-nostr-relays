//! JSON artefact generation.
//!
//! Produces three files in the `api/` directory, which is published to
//! GitHub Pages:
//!
//! * `relays.json` — full dataset including curator metadata and the latest
//!   health status per relay.
//! * `urls.json` — flat list of relay URLs (the most lightweight consumer
//!   format).
//! * `collections.json` — relay URLs grouped by collection id.

use std::{fs, path::Path};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::model::{Collection, Dataset, HealthEntry, HealthReport, Relay};

/// File name of the full dataset artefact.
pub const FULL_FILE: &str = "relays.json";

/// File name of the flat URL-list artefact.
pub const URLS_FILE: &str = "urls.json";

/// File name of the per-collection index artefact.
pub const COLLECTIONS_FILE: &str = "collections.json";

#[derive(Serialize)]
struct FullDocument<'a> {
    schema_version: &'a str,
    generated_at: DateTime<Utc>,
    total: usize,
    healthy: usize,
    collections: &'a [Collection],
    relays: Vec<RelayView<'a>>,
}

#[derive(Serialize)]
struct RelayView<'a> {
    #[serde(flatten)]
    relay: &'a Relay,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'a HealthEntry>,
}

#[derive(Serialize)]
struct FlatDocument<'a> {
    generated_at: DateTime<Utc>,
    total: usize,
    urls: Vec<&'a str>,
}

#[derive(Serialize)]
struct CollectionsDocument<'a> {
    generated_at: DateTime<Utc>,
    collections: Vec<CollectionView<'a>>,
}

#[derive(Serialize)]
struct CollectionView<'a> {
    id: &'a str,
    name: &'a str,
    description: &'a str,
    relays: Vec<&'a str>,
}

/// Top-level fields excluded when comparing a newly rendered artefact to the
/// existing file. Only the wall-clock stamp is noise; everything else in the
/// payload reflects a real data change worth committing.
const VOLATILE_TOP_LEVEL_FIELDS: &[&str] = &["generated_at"];

/// Write all three JSON artefacts into `api_dir`, creating the directory if
/// it does not yet exist.
///
/// Writes are **content-addressed**: if a target file already exists and its
/// payload is equivalent to the freshly rendered value once the
/// [`VOLATILE_TOP_LEVEL_FIELDS`] are stripped, the file is left untouched.
/// This keeps CI commits and GitHub Pages deployments free of time-stamp
/// churn when the underlying catalog has not changed.
///
/// # Errors
///
/// Returns an error when the directory cannot be created, when serialisation
/// fails, or when any individual artefact cannot be written.
pub fn write_all(dataset: &Dataset, health: &HealthReport, api_dir: &Path) -> Result<()> {
    fs::create_dir_all(api_dir)
        .with_context(|| format!("failed to create output dir {}", api_dir.display()))?;

    let generated_at = Utc::now();

    let mut healthy = 0_usize;
    let relay_views: Vec<RelayView<'_>> = dataset
        .relays
        .iter()
        .map(|relay| {
            let status = health.entries.get(relay.url.as_str());
            if status.is_some_and(HealthEntry::is_online) {
                healthy = healthy.saturating_add(1);
            }
            RelayView { relay, status }
        })
        .collect();

    let full = FullDocument {
        schema_version: &dataset.schema_version,
        generated_at,
        total: dataset.relays.len(),
        healthy,
        collections: &dataset.collections,
        relays: relay_views,
    };
    write_json_if_changed(&api_dir.join(FULL_FILE), &full)?;

    let flat = FlatDocument {
        generated_at,
        total: dataset.relays.len(),
        urls: dataset.relays.iter().map(|r| r.url.as_str()).collect(),
    };
    write_json_if_changed(&api_dir.join(URLS_FILE), &flat)?;

    let collections = CollectionsDocument {
        generated_at,
        collections: dataset
            .collections
            .iter()
            .map(|collection| CollectionView {
                id: &collection.id,
                name: &collection.name,
                description: &collection.description,
                relays: dataset
                    .relays
                    .iter()
                    .filter(|r| r.collections.iter().any(|c| c == &collection.id))
                    .map(|r| r.url.as_str())
                    .collect(),
            })
            .collect(),
    };
    write_json_if_changed(&api_dir.join(COLLECTIONS_FILE), &collections)?;

    Ok(())
}

fn write_json_if_changed<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value).context("serialize json")?;
    text.push('\n');

    if let Ok(existing) = fs::read_to_string(path)
        && payload_is_equivalent(&existing, &text, VOLATILE_TOP_LEVEL_FIELDS)
    {
        return Ok(());
    }

    fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Compare two JSON payloads ignoring the given top-level keys.
///
/// Both inputs must be syntactically valid JSON objects; any parse failure
/// falls back to "not equivalent" so we always err on the side of writing.
fn payload_is_equivalent(a: &str, b: &str, ignore_keys: &[&str]) -> bool {
    let (Ok(mut av), Ok(mut bv)) = (
        serde_json::from_str::<Value>(a),
        serde_json::from_str::<Value>(b),
    ) else {
        return false;
    };
    strip_keys(&mut av, ignore_keys);
    strip_keys(&mut bv, ignore_keys);
    av == bv
}

fn strip_keys(value: &mut Value, keys: &[&str]) {
    if let Some(obj) = value.as_object_mut() {
        for key in keys {
            obj.remove(*key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_equivalent_ignores_top_level_generated_at() {
        let a = r#"{"generated_at":"2026-01-01T00:00:00Z","total":3,"urls":["a","b","c"]}"#;
        let b = r#"{"generated_at":"2026-05-02T12:00:00Z","total":3,"urls":["a","b","c"]}"#;
        assert!(payload_is_equivalent(a, b, &["generated_at"]));
    }

    #[test]
    fn payload_equivalent_detects_real_changes() {
        let a = r#"{"generated_at":"2026-01-01T00:00:00Z","total":3,"urls":["a","b","c"]}"#;
        let b = r#"{"generated_at":"2026-01-01T00:00:00Z","total":4,"urls":["a","b","c","d"]}"#;
        assert!(!payload_is_equivalent(a, b, &["generated_at"]));
    }

    #[test]
    fn payload_equivalent_returns_false_on_invalid_json() {
        assert!(!payload_is_equivalent("not json", "{}", &[]));
        assert!(!payload_is_equivalent("{}", "not json", &[]));
    }

    #[test]
    fn strip_keys_removes_only_top_level() {
        let mut value: Value =
            serde_json::from_str(r#"{"generated_at":"x","nested":{"generated_at":"y"}}"#)
                .expect("valid json");
        strip_keys(&mut value, &["generated_at"]);
        assert!(value.get("generated_at").is_none());
        assert!(
            value
                .get("nested")
                .and_then(|n| n.get("generated_at"))
                .is_some(),
            "nested generated_at must be preserved"
        );
    }
}
