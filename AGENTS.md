# Repository Guidelines

## Project Structure & Module Organization

This repository is a small Rust workspace rooted at `crates/`. The workspace manifest lives at `crates/Cargo.toml`.

- `crates/nokturn-core/src`: OpenAPI loading, sanitization, reference resolution, and IR building.
- `crates/nokturn-core/tests`: integration tests and YAML fixtures in `tests/fixtures`.
- `crates/nokturn-gen/src`: CLI and generation entrypoints.
- `crates/nokturn-gen/assets`: Tera templates used by the generator.
- `open-api-specs/*`: example provider specs and matching `config.toml` files.
- `scripts/generate.sh`: helper script for local provider generation.

Keep new code inside the relevant crate, and prefer small modules over growing `lib.rs` or CLI files.

## Build, Test, and Development Commands

- `cargo build --manifest-path crates/Cargo.toml -p nokturn-gen`: build the CLI.
- `cargo test --manifest-path crates/Cargo.toml`: run workspace tests.
- `cargo check --manifest-path crates/Cargo.toml`: fast compile check before submitting changes.
- `cargo clippy --manifest-path crates/Cargo.toml --all-targets --all-features -D warnings`: lint and fix all warnings.
- `cargo run --manifest-path crates/Cargo.toml -p nokturn-gen -- generate --schema ... --config ... --output-dir ...`: run the generator locally.
- `scripts/generate.sh do` or `scripts/generate.sh cf`: generate against bundled example specs.

## Coding Style & Naming Conventions

Follow standard Rust formatting with `cargo fmt`. Use 4-space indentation, `snake_case` for functions/modules, `PascalCase` for types, and descriptive crate-local module names such as `resolver` or `utils`. Keep imports at the top of each file and group related logic by module responsibility.

## Testing Guidelines

Add unit tests near the code they cover and integration tests under `crates/nokturn-core/tests`. Prefer small fixture-driven tests using YAML files in `tests/fixtures`. Name tests after observable behavior, for example `resolves_simple_spec`.

## Commit & Pull Request Guidelines

Recent history uses short, imperative subjects with an optional scope prefix, such as `resolver: implement all refs resolvers` or `docs: update readme`. Keep commits focused. PRs should explain the change, note affected crates, list verification commands run, and include sample CLI output when behavior changes.

## Agent-Specific Notes

After code changes, run `cargo check` and `cargo clippy` before finalizing. Do not add placeholder implementations, and avoid destructive git operations such as resets or checkouts that overwrite local work.
