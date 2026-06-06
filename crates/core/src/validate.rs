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

    if dataset.schema_version != "2" {
        errors.push(format!(
            "schema_version must be \"2\", got {:?}",
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
            || trimmed == "XX";
        if !ok {
            errors.push(format!(
                "relay {} has invalid country {country:?} (expected ISO-3166 alpha-2 uppercase, or XX)",
                relay.url
            ));
        }
    }
    if let Some(geohash) = &relay.geohash
        && !is_valid_geohash(geohash.trim())
    {
        errors.push(format!(
            "relay {} has invalid geohash {geohash:?} (expected 1–12 base32 geohash chars)",
            relay.url
        ));
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

/// `true` when `value` is a syntactically valid NIP-52 geohash: 1–12
/// characters drawn from the base32 geohash alphabet (no `a`, `i`, `l`, `o`).
fn is_valid_geohash(value: &str) -> bool {
    const GEOHASH_ALPHABET: &str = "0123456789bcdefghjkmnpqrstuvwxyz";
    !value.is_empty() && value.len() <= 12 && value.chars().all(|c| GEOHASH_ALPHABET.contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Collection, Relay, Requirements};

    fn dataset(collections: Vec<Collection>, relays: Vec<Relay>) -> Dataset {
        Dataset {
            schema_version: "2".to_owned(),
            collections,
            relays,
        }
    }

    fn collection(id: &str) -> Collection {
        Collection {
            id: id.to_owned(),
            name: format!("Label {id}"),
            description: format!("Description for {id}"),
        }
    }

    fn relay(url: &str, collections: &[&str]) -> Relay {
        Relay {
            url: url::Url::parse(url).expect("valid url"),
            name: None,
            description: None,
            operator: None,
            country: None,
            geohash: None,
            network: None,
            software: None,
            requirements: Requirements::default(),
            collections: collections.iter().copied().map(str::to_owned).collect(),
            topics: Vec::new(),
            added_at: None,
        }
    }

    #[test]
    fn kebab_case_rules() {
        assert!(is_kebab_case("featured"));
        assert!(is_kebab_case("regional-americas"));
        assert!(is_kebab_case("bot-01"));
        assert!(!is_kebab_case(""));
        assert!(!is_kebab_case("-leading"));
        assert!(!is_kebab_case("trailing-"));
        assert!(!is_kebab_case("Upper"));
        assert!(!is_kebab_case("has_underscore"));
        assert!(!is_kebab_case("has space"));
    }

    #[test]
    fn accepts_minimal_valid_dataset() {
        let ds = dataset(
            vec![collection("featured")],
            vec![relay("wss://a.example/", &["featured"])],
        );
        let summary = validate(&ds).expect("valid");
        assert_eq!(summary.relay_count, 1);
        assert_eq!(summary.collection_count, 1);
        assert_eq!(summary.relays_per_collection.get("featured"), Some(&1));
    }

    #[test]
    fn rejects_wrong_schema_version() {
        let mut ds = dataset(vec![], vec![]);
        ds.schema_version = "1".to_owned();
        let err = validate(&ds).expect_err("must reject");
        assert!(matches!(err, ValidationError::Failed { .. }));
    }

    #[test]
    fn collects_multiple_violations_in_one_pass() {
        let ds = dataset(
            vec![
                collection("Featured"),
                collection("Featured"),
                Collection {
                    id: "global".to_owned(),
                    name: String::new(),
                    description: String::new(),
                },
            ],
            vec![
                relay("https://wrong-scheme/", &["global"]),
                relay("wss://a.example/", &["missing-collection"]),
                relay("wss://a.example/", &["global"]),
            ],
        );
        let err = validate(&ds).expect_err("must reject");
        let ValidationError::Failed { count, .. } = err;
        assert!(count >= 5, "expected many violations, got {count}");
    }

    #[test]
    fn rejects_relay_without_collection() {
        let ds = dataset(
            vec![collection("featured")],
            vec![relay("wss://a.example/", &[])],
        );
        assert!(validate(&ds).is_err());
    }

    #[test]
    fn rejects_duplicate_collection_on_relay() {
        let ds = dataset(
            vec![collection("featured")],
            vec![relay("wss://a.example/", &["featured", "featured"])],
        );
        assert!(validate(&ds).is_err());
    }

    #[test]
    fn country_code_rules() {
        let build = |country: &str| {
            let mut r = relay("wss://a.example/", &["featured"]);
            r.country = Some(country.to_owned());
            dataset(vec![collection("featured")], vec![r])
        };
        assert!(validate(&build("US")).is_ok());
        assert!(validate(&build("JP")).is_ok());
        assert!(validate(&build("XX")).is_ok());
        assert!(validate(&build("T1")).is_err());
        assert!(validate(&build("us")).is_err());
        assert!(validate(&build("USA")).is_err());
        assert!(validate(&build("U")).is_err());
    }

    #[test]
    fn geohash_rules() {
        let build = |geohash: &str| {
            let mut r = relay("wss://a.example/", &["featured"]);
            r.geohash = Some(geohash.to_owned());
            dataset(vec![collection("featured")], vec![r])
        };
        assert!(validate(&build("ww8p1r4t8")).is_ok());
        assert!(validate(&build("u4pruydqqvj")).is_ok());
        assert!(validate(&build("")).is_err());
        assert!(validate(&build("ABC")).is_err());
        assert!(validate(&build("ail")).is_err());
    }

    #[test]
    fn rejects_duplicate_relay_url_case_insensitive() {
        let ds = dataset(
            vec![collection("featured")],
            vec![
                relay("wss://a.example/", &["featured"]),
                relay("wss://A.EXAMPLE/", &["featured"]),
            ],
        );
        assert!(validate(&ds).is_err());
    }
}
