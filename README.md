# Nokturn – Rust OpenAPI v3 SDK Generator

Nokturn is a Rust toolchain that turns an OpenAPI v3 specification into an idiomatic Rust SDK. The `nokturn-gen` CLI can generate SDKs for Cloudflare, DigitalOcean, Exoscale, Hetzner, Scaleway and any provider that publishes an OpenAPI spec.

## Workspace layout

- `crates/nokturn-core`: Loads OpenAPI v3 (JSON/YAML), applies a small sanitization pass, resolves component references, and defines the canonical IR for code generation.
- `crates/nokturn-gen`: CLI wrapper (binary name: `nokturn-gen`) that reads the config/spec, lowers the schema, and emits the generated crate layout.

## Status

Code generation is in progress. Today `nokturn-gen generate` emits a compilable crate layout with package metadata, `src/lib.rs`, generated model types, operation modules, and a `reqwest`-backed client surface. Typed execution is currently limited to the subset of response shapes the generator can decode safely.

## Build

```bash
cargo build --manifest-path crates/Cargo.toml -p nokturn-gen
cargo install --path ./crates/nokturn-gen
```

## Usage

```bash
cargo run --manifest-path crates/Cargo.toml -p nokturn-gen -- \
  --log-level debug \
  generate \
  --schema open-api-specs/digitalocean/digitalocean.yaml \
  --config open-api-specs/digitalocean/config.toml \
  --output-dir generated/digitalocean
nokturn-gen generate --schema <schema-file> --output-dir <output-directory> --config <config-file>
```

You can also run the binary directly after a release build:

```bash
(cd crates && cargo build --release)
./crates/target/release/nokturn-gen --log-level info generate \
  --schema open-api-specs/digitalocean/digitalocean.yaml \
  --config open-api-specs/digitalocean/config.toml \
  --output-dir generated/digitalocean
nokturn-gen --log-level debug generate --schema openapi.yaml --config config.toml --output-dir ./output
```

Supported log levels: `trace`, `debug`, `info`, `warn`, `error`.

## Configuration

Nokturn uses a TOML configuration file to customize the generated SDK. Here's an example configuration:

```toml
crate_name = "digitalocean"
version = "0.1.0-alpha"
edition = "2024" # optional
description = "Unofficial Rust SDK for DigitalOcean"
lib_status = "experimental"
keywords = ["digitalocean"]
api_url = "https://api.digitalocean.com/v2"
authors = ["Cloudflavor GmbH <foss@cloudflavor.io>"]

# Optional filtering
include_only = ["User", "Account"]
exclude = ["InternalModel"]
```

In-tree examples live under `open-api-specs/*/config.toml`.

### Configuration Options

- `crate_name`: The name of the generated Rust crate
- `version`: The version of the generated crate
- `edition`: The Rust edition to use (defaults to "2021")
- `description`: A brief description of the crate
- `lib_status`: The maintenance status of the library (e.g., "active", "deprecated", "experimental")
- `keywords`: A list of keywords for the crate
- `api_url`: The base URL for the API
- `authors`: A list of authors for the crate
- `include_only`: (Optional) A list of models to include (if not specified, all models are included)
- `exclude`: (Optional) A list of models to exclude

## Generating the SDK

```bash
nokturn-gen generate \
  --schema path/to/openapi.yaml \
  --config config.toml \
  --output-dir ./output
```

The output will contain a Cargo crate with a typical `src/` layout and a `Cargo.toml` that exposes optional features like `rustls`, `native-tls` and `http2`.
