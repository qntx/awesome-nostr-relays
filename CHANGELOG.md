# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-06-06

A ground-up refactor toward an ecosystem-native, production-ready relay catalog.
**Breaking**: the catalog and public JSON API are now `schema_version: 2`.

### Added

- **NIP-66 alignment** for the data model, plus a new `nip66.json` endpoint that
  emits unsigned `kind 30166` relay-discovery event templates.
- **Explicit health state machine** (`healthy`, `flaky`, `warn`, `dead`,
  `stale`, `blocked`, `skipped`, `unknown`) surfaced as `health.state` in
  `relays.json`, replacing consumer-side inference.
- **`monitor`** (probe cadence/checks, NIP-66 `kind 10166`) and **`summary`**
  (per-state relay counts) blocks in `relays.json`.
- **`badge.json`** shields.io endpoint and a live health badge.
- **JSON Schema** export under `schema/` (`relay`, `collection`, `health-entry`,
  `health-report`), generated from the Rust types via `schemars`.
- **`relays dead`** subcommand and a weekly **Dead Relay Report** workflow that
  opens/updates a removal-candidate tracking issue.
- `network` (clearnet/tor/i2p/loki), `geohash`, `requirements` and `topics`
  fields on catalog entries.

### Changed

- **Workspace split** into `relays-core` (IO-free domain/render), `relays-probe`
  (async probing) and `relays` (CLI).
- **Data layout**: `health.json` and `api/` are no longer committed to `main`.
  CI publishes them to the orphan `data` branch; GitHub Pages serves `web/`
  (main) + `api/` (data). This removes the high-frequency machine commits that
  previously dominated `main` history.
- The README catalog block no longer embeds per-relay health (now a badge + API
  link), so it only changes when the curated catalog changes.
- Persisted timestamps are truncated to whole seconds for readable diffs.

### Fixed

- **Pseudo-healthy relays**: a WAF/DNS-blocked relay that once succeeded no
  longer reports healthy — it is now `blocked`. A staleness guard reports
  `stale` when monitoring data is older than `STALE_AFTER_HOURS`.
- Probed-but-below-threshold relays are now `flaky`, distinct from the
  never-probed `unknown` state.

### Removed

- `country = "T1"` Tor hack (replaced by the `network` field).
- `*.json merge=union` git attribute (an anti-pattern for structured files).
- Top-level `paid` / `tags` / `status` JSON fields (replaced by
  `requirements` / `topics` / `health`).
