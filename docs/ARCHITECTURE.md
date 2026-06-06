# Architecture

## Workspace

- **`relays-core`** — IO-free domain model, validation, the health state
  machine, and renderers (JSON, Markdown, NIP-66, JSON Schema). No network
  dependencies, so it is cheap to reuse and fast to test.
- **`relays-probe`** — async liveness probe (WebSocket handshake + NIP-11
  enrichment). Depends on `relays-core` for the result types.
- **`relays`** — the CLI binary orchestrating the subcommands `validate`,
  `build`, `check`, and `dead`.

## Data flow

```text
relays.toml (main, curated)
   |  relays build
   |-> README RELAYS block                                   -> main
   |-> relays.json / urls.json / collections.json
       nip66.json / badge.json / schema/                     -> data branch

relays check (probes)  ->  health.json                       -> data branch

GitHub Pages = web/ (main) + artefacts (data)  ->  https://relays.meowl.social
```

## Branches

- **`main`** — curated source (`relays.toml`), the Rust workspace, the `web/`
  shell, hand-written docs, and the generated README catalog block.
- **`data`** (orphan) — `health.json` and every generated JSON artefact,
  refreshed by the Health Check workflow. This keeps `main` history free of
  high-frequency machine commits.

## Health state machine

`HealthEntry::state(now)` maps a persisted `Outcome` (plus the consecutive
failure count and data freshness) to a `HealthState`:

- never probed -> `unknown`
- `last_checked` older than `STALE_AFTER_HOURS` -> `stale`
- `skipped` outcome -> `skipped`
- `blocked` outcome (WAF 4xx / DNS) -> `blocked` (never `healthy`)
- `failure`: `>= DEAD` -> `dead`; `>= WARN` -> `warn`; otherwise `flaky`
- `success` -> `healthy`

`blocked` and `stale` together fix the previous "pseudo-healthy" behaviour where
a relay that succeeded once but is now WAF-blocked kept reporting healthy.

## NIP-66 mapping

Catalog/health fields map onto NIP-66 `kind 30166` tags (see `nip66.json`):

- `url` -> `d`
- `network` -> `n`
- `requirements.{auth,payment,pow,writes}` -> `R` (`!`-prefixed when false)
- `topics[]` -> `t`
- `geohash` -> `g`
- `health.supported_nips[]` -> `N`
- `health.rtt_open_ms` -> `rtt-open`

The `monitor` block in `relays.json` mirrors NIP-66 `kind 10166` (frequency,
timeout, and the checks performed).

## Future work

- **Tor probing** — onion relays are currently `skipped` because the CI runner
  has no Tor route. A self-hosted runner or an in-CI Tor proxy could probe them
  via `network = tor` and report real `rtt-open`.
- **Signed NIP-66 events** — `nip66.json` ships unsigned templates; a monitor
  key could sign and publish them to relays for native client consumption.
