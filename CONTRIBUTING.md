# Contributing to Kitsunarr

Thank you for helping! Kitsunarr is GPL-3.0-or-later; by contributing you agree that your contribution is licensed the same way.

## Ground rules

- **Working software over scaffolding.** A change should implement behaviour end to end and include tests. Do not mark a feature done in `docs/architecture/feature-matrix.md` unless it is implemented and verified.
- **Never claim a test passed without running it.** Say whether a test is simulated (mocks, replayed captures) or ran against a real service.
- **Safety first.** Code that touches the filesystem must not delete, move or rename anything outside the configured library roots. Destructive operations must be explicit and tested. Migration code must stay read-only towards the source.
- **No business logic in UI components.** Logic belongs in domain or service crates, so the web UI and the API share one implementation.
- **Attribution.** Behaviour ported from Sonarr or Tanukarr must reference the original file in a comment or doc so its provenance stays traceable.

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p kitsunarr-web --target wasm32-unknown-unknown
dx build -p kitsunarr-web --release --profile web-release   # optional: embed the UI
```

Run a local server against throwaway data:

```sh
cargo run -p kitsunarr-server -- --config-dir ./.dev --data-dir ./.dev/data --port 8990
```

## Fixtures

The migration fixtures in `tests/fixtures/` come from real Sonarr/Tanukarr containers; `tests/fixtures/README.md` explains how to regenerate them. Fixtures must never contain real secrets or personal data.

## Commits and pull requests

- Write meaningful commit messages ("Add qBittorrent client", not "wip").
- Keep pull requests focused. Describe how you verified the change.
- CI must pass: formatting, clippy with `-D warnings`, the test suite, the WebAssembly build and the Docker build.
