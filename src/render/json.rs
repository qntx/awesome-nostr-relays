//! JSON artefact generation.
//!
//! Produces three files in the `dist/` directory:
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

/// Write all three JSON artefacts into `dist_dir`, creating the directory if
/// it does not yet exist.
///
/// # Errors
///
/// Returns an error when the directory cannot be created, when serialisation
/// fails, or when any individual artefact cannot be written.
pub fn write_all(dataset: &Dataset, health: &HealthReport, dist_dir: &Path) -> Result<()> {
    fs::create_dir_all(dist_dir)
        .with_context(|| format!("failed to create dist dir {}", dist_dir.display()))?;

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
    write_json(&dist_dir.join(FULL_FILE), &full)?;

    let flat = FlatDocument {
        generated_at,
        total: dataset.relays.len(),
        urls: dataset.relays.iter().map(|r| r.url.as_str()).collect(),
    };
    write_json(&dist_dir.join(URLS_FILE), &flat)?;

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
    write_json(&dist_dir.join(COLLECTIONS_FILE), &collections)?;

    Ok(())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value).context("serialize json")?;
    text.push('\n');
    fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}
