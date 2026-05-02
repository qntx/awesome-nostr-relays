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

/// Default output directory for generated artefacts.
pub const DIST_DIR: &str = "dist";

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
pub fn default_dist_dir() -> PathBuf {
    PathBuf::from(DIST_DIR)
}

/// Default README path.
#[must_use]
pub fn default_readme_path() -> PathBuf {
    PathBuf::from(README_FILE)
}
