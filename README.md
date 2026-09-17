# TaskHarbor

TaskHarbor is a learning-focused portfolio project for creating, scheduling, running, and monitoring background jobs. The first real job type will resize JPEG and PNG images through a Rust API and a separate Rust worker.

## Current milestone

Phase 0 provides a minimal Cargo workspace, a reusable domain library, and one executable entry point. It does not include an HTTP server, persistent storage, a worker, or a web dashboard yet.

The current program follows this small flow:

1. The API binary supplies an example job name as input.
2. The core library validates the name.
3. The binary prints a readiness message as output.

## Repository structure

```text
apps/api/       Executable entry point
crates/core/    Domain types and validation rules
docs/           Learning checkpoint and project glossary
```

The remaining target structure will be added only when its roadmap phase needs it.

## Requirements

- Rust 1.95.0 with Cargo, rustfmt, and Clippy
- Git

Node.js and Docker will be needed in later phases. Docker is not required for the current program.

## Run locally

The repository pins its Rust version in `rust-toolchain.toml`.

```bash
cargo run -p taskharbor-api
```

Expected output:

```text
TaskHarbor API foundation is ready for job "example-job".
```

`.env.example` documents planned local defaults. The Phase 0 binary does not read environment variables yet.

## Verify

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Project progress and verified commands are recorded in `docs/progress.md`.
