//! Schema and semantic validation for [`Dataset`].
//!
//! Collects *all* violations before returning, so CI output lists every issue
//! in a single run instead of erroring out after the first mistake.

use std::collections::{BTreeMap, HashSet};

use thiserror::Error;

use crate::model::{Dataset, Relay};

/// Validation failure produced by [`validate`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ValidationError {
    /// Validation collected one or more rule violations.
    #[error("validation failed with {count} error(s):\n{details}")]
    Failed {
        /// Number of individual rule violations encountered.
        count: usize,
        /// Newline-delimited, indented list of violations.
        details: String,
    },
}

/// Summary returned by a successful [`validate`] run.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct ValidationSummary {
    /// Total number of relays in the dataset.
    pub relay_count: usize,
    /// Total number of declared collections.
    pub collection_count: usize,
    /// Map of `collection_id -> number of relays referencing it`.
    pub relays_per_collection: BTreeMap<String, usize>,
}

/// Validate the full [`Dataset`], collecting every rule violation.
///
/// # Errors
///
/// Returns [`ValidationError::Failed`] when any schema, URL normalisation,
/// duplicate, or collection-reference rule is violated.
pub fn validate(dataset: &Dataset) -> Result<ValidationSummary, ValidationError> {
    let mut errors: Vec<String> = Vec::new();

    if dataset.schema_version != "1" {
        errors.push(format!(
            "schema_version must be \"1\", got {:?}",
            dataset.schema_version
        ));
    }

    let collection_ids = validate_collections(dataset, &mut errors);
    let counts = validate_relays(dataset, &collection_ids, &mut errors);

    if errors.is_empty() {
        Ok(ValidationSummary {
            relay_count: dataset.relays.len(),
            collection_count: dataset.collections.len(),
            relays_per_collection: counts,
        })
    } else {
        let details = errors
            .iter()
            .map(|msg| format!("  - {msg}"))
            .collect::<Vec<_>>()
            .join("\n");
        Err(ValidationError::Failed {
            count: errors.len(),
            details,
        })
    }
}

fn validate_collections<'a>(dataset: &'a Dataset, errors: &mut Vec<String>) -> HashSet<&'a str> {
    let mut ids: HashSet<&str> = HashSet::new();
    for collection in &dataset.collections {
        if collection.id.trim().is_empty() {
            errors.push("collection with empty id".to_owned());
            continue;
        }
        if !ids.insert(collection.id.as_str()) {
            errors.push(format!("duplicate collection id: {}", collection.id));
        }
        if !is_kebab_case(&collection.id) {
            errors.push(format!(
                "collection id {:?} must be lowercase kebab-case",
                collection.id
            ));
        }
        if collection.name.trim().is_empty() {
            errors.push(format!("collection {:?} has empty name", collection.id));
        }
    }
    ids
}

fn validate_relays(
    dataset: &Dataset,
    collection_ids: &HashSet<&str>,
    errors: &mut Vec<String>,
) -> BTreeMap<String, usize> {
    let mut urls: HashSet<String> = HashSet::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for relay in &dataset.relays {
        validate_relay(relay, collection_ids, errors);
        let key = relay.url.as_str().to_ascii_lowercase();
        if !urls.insert(key) {
            errors.push(format!("duplicate relay url: {}", relay.url));
        }
        for coll in &relay.collections {
            *counts.entry(coll.clone()).or_insert(0) += 1;
        }
    }
    counts
}

fn validate_relay(relay: &Relay, collection_ids: &HashSet<&str>, errors: &mut Vec<String>) {
    let scheme = relay.url.scheme();
    if scheme != "wss" && scheme != "ws" {
        errors.push(format!(
            "relay {} must use wss:// or ws:// (got {scheme}://)",
            relay.url
        ));
    }
    if relay.url.host_str().is_none() {
        errors.push(format!("relay {} has no host", relay.url));
    }
    if relay.collections.is_empty() {
        errors.push(format!("relay {} has no collections", relay.url));
    }
    let mut seen = HashSet::new();
    for coll in &relay.collections {
        if !collection_ids.contains(coll.as_str()) {
            errors.push(format!(
                "relay {} references unknown collection {coll:?}",
                relay.url
            ));
        }
        if !seen.insert(coll.as_str()) {
            errors.push(format!(
                "relay {} lists collection {coll:?} more than once",
                relay.url
            ));
        }
    }
    if let Some(country) = &relay.country {
        let trimmed = country.trim();
        let ok = (trimmed.len() == 2 && trimmed.chars().all(|c| c.is_ascii_uppercase()))
            || trimmed == "T1"
            || trimmed == "XX";
        if !ok {
            errors.push(format!(
                "relay {} has invalid country {country:?} (expected ISO-3166 alpha-2 uppercase, or XX/T1)",
                relay.url
            ));
        }
    }
}

fn is_kebab_case(identifier: &str) -> bool {
    !identifier.is_empty()
        && identifier
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !identifier.starts_with('-')
        && !identifier.ends_with('-')
}
