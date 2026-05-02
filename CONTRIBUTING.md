# Contributing to awesome-nostr-relays

Thanks for taking the time to improve the catalogue. This repository has one
golden rule:

> `relays.toml` is the **single source of truth**. Every other file in the
> repo is either curator-written prose (README intro, this document) or
> generated output (`api/*.json`, the `<!-- RELAYS:* -->` block of the
> README).

If you only want to **add / fix / remove a relay**, you will almost certainly
only need to edit `relays.toml`.

## Workflow

1. **Fork** the repo and create a feature branch.
2. Add your change to `relays.toml`. Keep entries sorted alphabetically by
   URL within their section to minimise merge conflicts.
3. Run `cargo run -- validate` locally. CI runs the same check.
4. Open a PR. A reviewer will probe the new URLs before merging.

## Relay entry schema

Required fields:

| Field         | Notes                                                         |
|---------------|---------------------------------------------------------------|
| `url`         | `wss://` or `ws://` (onion only). Include the trailing slash. |
| `collections` | Non-empty array of known collection ids (see below).          |

Optional fields:

| Field         | Notes                                                                               |
|---------------|-------------------------------------------------------------------------------------|
| `name`        | Short human-readable label.                                                         |
| `description` | One-sentence pitch describing the relay's focus.                                    |
| `operator`    | Organisation or pubkey running the relay.                                           |
| `country`     | ISO-3166 alpha-2 uppercase (`US`, `JP`, …). Use `XX` if unknown, `T1` for tor-only. |
| `software`    | Relay implementation (e.g. `strfry`, `khatru`, `nostream`).                         |
| `paid`        | `true` if the relay requires payment to post.                                       |
| `tags`        | Free-form strings, lower-case, for fine-grained discovery.                          |
| `added_at`    | ISO date (`YYYY-MM-DD`) when the relay was first catalogued.                        |

## Relay acceptance criteria

- **Liveness**: the relay SHOULD be online. CI will flag relays after 3
  consecutive failures and mark them as dead after 14. PRs adding permanently
  offline relays will be rejected.
- **Protocol compliance**: the relay MUST respond to a basic `REQ` within the
  probe timeout.
- **No malicious content**: relays hosting obvious spam or CSAM content will
  not be accepted.
- **Accessibility**: public relays preferred. Invite-only relays (like
  `pyramid.fiatjaf.com`) are OK if they implement a sensible WoT policy.

## Adding a new collection

If none of the existing collections fit, add a new `[[collections]]` entry at
the top of `relays.toml`. Guidelines:

- `id` MUST be lower-case kebab-case (e.g. `regional-oceania`).
- Each collection should end up with **at least 2–3 relays**; otherwise tag
  the relay and reuse an existing collection.
- Keep the taxonomy shallow. Prefer tags over a deep collection tree.

## Local commands

```bash
# Lint
cargo run -- validate

# Regenerate artefacts
cargo run -- build

# Probe a small batch (good for smoke tests)
cargo run -- check --limit 5 --timeout 8

# Probe everything (takes ~1 minute with default concurrency)
cargo run -- check --concurrency 32 --timeout 10
```

## Removing a relay

Relays are **not** removed on a single failure. CI attaches health state to
each entry and only hints at removal (`💀`) after 14 consecutive failures.
Once CI marks a relay as dead, open a PR to remove it (or revive it with new
URL/metadata if the operator has migrated).

## Code of conduct

- Be respectful and constructive.
- Focus on data quality and discoverability.
- No spam, astro-turfing, or promotional content.

Thank you for helping the Nostr ecosystem stay discoverable! 🚀
