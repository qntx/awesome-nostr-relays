//! JSON Schema export for the public catalog data model.
//!
//! Emits draft schemas under `<api_dir>/schema/` so consumers can validate
//! catalog entries and the health snapshot, and generate typed clients. The
//! schemas are produced from the Rust types via `schemars`, so they never drift
//! from the code.

use std::path::Path;

use anyhow::{Context, Result};

use crate::model::{Collection, HealthEntry, HealthReport, Relay};

/// Sub-directory (under the API output dir) that holds the schema files.
pub const SCHEMA_DIR: &str = "schema";

/// Write every JSON Schema file into `<api_dir>/schema/`.
///
/// Writes are content-addressed, so unchanged schemas are left untouched.
///
/// # Errors
///
/// Returns an error if the schema directory cannot be created or any schema
/// file cannot be written.
pub fn write(api_dir: &Path) -> Result<()> {
    let dir = api_dir.join(SCHEMA_DIR);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create schema dir {}", dir.display()))?;

    super::json::write_json_if_changed(
        &dir.join("relay.schema.json"),
        &schemars::schema_for!(Relay),
    )?;
    super::json::write_json_if_changed(
        &dir.join("collection.schema.json"),
        &schemars::schema_for!(Collection),
    )?;
    super::json::write_json_if_changed(
        &dir.join("health-entry.schema.json"),
        &schemars::schema_for!(HealthEntry),
    )?;
    super::json::write_json_if_changed(
        &dir.join("health-report.schema.json"),
        &schemars::schema_for!(HealthReport),
    )?;
    Ok(())
}
