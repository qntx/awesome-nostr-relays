//! File-system loaders for the TOML catalog and JSON health snapshot.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::model::{Dataset, HealthReport};

/// Default filename of the TOML catalog.
pub const RELAYS_FILE: &str = "relays.toml";

/// Default filename of the JSON health snapshot.
pub const HEALTH_FILE: &str = "health.json";

/// Default output directory for generated artefacts (published to GitHub
/// Pages as a static JSON API).
pub const API_DIR: &str = "api";

/// Default README path that [`crate::render::markdown`] will rewrite.
pub const README_FILE: &str = "README.md";

/// Load a [`Dataset`] from the given path (typically `./relays.toml`).
///
/// # Errors
///
/// Returns an error if the file cannot be read or contains invalid TOML.
pub fn load_dataset(path: &Path) -> Result<Dataset> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read relay catalog from {}", path.display()))?;
    let dataset: Dataset = toml::from_str(&text)
        .with_context(|| format!("failed to parse TOML catalog at {}", path.display()))?;
    Ok(dataset)
}

/// Load a [`HealthReport`] if it exists, otherwise return an empty report.
///
/// # Errors
///
/// Returns an error if the file exists but cannot be read or parsed as JSON.
pub fn load_health(path: &Path) -> Result<HealthReport> {
    if !path.exists() {
        return Ok(HealthReport::default());
    }
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read health snapshot from {}", path.display()))?;
    let report: HealthReport = serde_json::from_str(&text)
        .with_context(|| format!("failed to parse health snapshot at {}", path.display()))?;
    Ok(report)
}

/// Persist a [`HealthReport`] as pretty JSON followed by a trailing newline.
///
/// # Errors
///
/// Returns an error if the parent directory cannot be created or the file
/// cannot be written.
pub fn save_health(path: &Path, report: &HealthReport) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent dir {}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(report).context("serialize health report")?;
    text.push('\n');
    fs::write(path, text)
        .with_context(|| format!("failed to write health snapshot to {}", path.display()))?;
    Ok(())
}

/// Default location of the relay catalog, relative to the current working
/// directory.
#[must_use]
pub fn default_relays_path() -> PathBuf {
    PathBuf::from(RELAYS_FILE)
}

/// Default location of the health snapshot.
#[must_use]
pub fn default_health_path() -> PathBuf {
    PathBuf::from(HEALTH_FILE)
}

/// Default output directory for generated JSON artefacts.
#[must_use]
pub fn default_api_dir() -> PathBuf {
    PathBuf::from(API_DIR)
}

/// Default README path.
#[must_use]
pub fn default_readme_path() -> PathBuf {
    PathBuf::from(README_FILE)
}

#[cfg(test)]
mod tests {
    use std::{
        env, process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;
    use crate::model::{HealthEntry, HealthReport};

    /// Monotonic counter ensuring each test gets a unique temp filename even
    /// under parallel execution.
    static TEST_SEQ: AtomicU64 = AtomicU64::new(0);

    fn unique_tempfile(suffix: &str) -> PathBuf {
        let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!("relays-{}-{}-{}", process::id(), seq, suffix))
    }

    /// Best-effort cleanup; ignore any error (e.g. already-absent file).
    fn cleanup(path: &PathBuf) {
        drop(fs::remove_file(path));
    }

    #[test]
    fn load_dataset_parses_minimal_toml() {
        let toml = r#"
schema_version = "1"

[[collections]]
id = "featured"
name = "Featured"
description = "Hand-picked"

[[relays]]
url = "wss://example.com/"
name = "Example"
collections = ["featured"]
"#;
        let tmp = unique_tempfile("dataset.toml");
        fs::write(&tmp, toml).expect("write fixture");
        let dataset = load_dataset(&tmp).expect("load dataset");
        assert_eq!(dataset.schema_version, "1");
        assert_eq!(dataset.collections.len(), 1);
        assert_eq!(dataset.relays.len(), 1);
        let first = dataset.relays.first().expect("one relay present");
        assert_eq!(first.name.as_deref(), Some("Example"));
        cleanup(&tmp);
    }

    #[test]
    fn load_health_returns_default_for_missing_file() {
        let tmp = unique_tempfile("missing.json");
        cleanup(&tmp);
        let report = load_health(&tmp).expect("load health");
        assert!(report.entries.is_empty());
        assert!(report.last_run.is_none());
    }

    #[test]
    fn save_and_load_health_roundtrips() {
        let tmp = unique_tempfile("roundtrip.json");
        let mut report = HealthReport::default();
        report
            .entries
            .insert("wss://a.example/".to_owned(), HealthEntry::default());
        save_health(&tmp, &report).expect("save health");
        let loaded = load_health(&tmp).expect("load health");
        assert!(loaded.entries.contains_key("wss://a.example/"));
        cleanup(&tmp);
    }
}
