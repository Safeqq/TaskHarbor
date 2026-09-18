# TaskHarbor

TaskHarbor is a learning-focused portfolio project for creating, scheduling, running, and monitoring background jobs. The first real job type will resize JPEG and PNG images through a Rust API and a separate Rust worker.

## Current milestone

Phase 1 provides an HTTP API that creates and reads jobs from an in-memory store. Every new job starts in the `queued` state.

The current request flow is:

1. Axum parses the HTTP request and JSON body.
2. The core library validates the job name.
3. The in-memory store assigns an ID and saves a queued job.
4. The API returns a JSON response.

Data is intentionally temporary in this phase. Restarting the API removes every job. The API does not execute jobs yet.

## Repository structure

```text
apps/api/       HTTP transport, in-memory store, and executable entry point
crates/core/    Domain types and validation rules
docs/           Learning checkpoint and project glossary
```

The remaining target structure will be added only when its roadmap phase needs it.

## Requirements

- Rust 1.95.0 with Cargo, rustfmt, and Clippy
- Git

Node.js and Docker will be needed in later phases. Docker is not required for the in-memory API.

## Run locally

The repository pins its Rust version in `rust-toolchain.toml`.

```bash
cargo run -p taskharbor-api
```

The server listens on `127.0.0.1:3000` by default. `TASKHARBOR_BIND_ADDR` can override this value.

```text
TaskHarbor API listening on http://127.0.0.1:3000
```

`.env.example` documents the local default. Environment files are not loaded automatically, so set an override in the shell when needed.

## API

| Method | Path | Result |
|---|---|---|
| `GET` | `/health` | Service health |
| `POST` | `/api/v1/jobs` | Create a queued job |
| `GET` | `/api/v1/jobs` | List jobs by ascending ID |
| `GET` | `/api/v1/jobs/{id}` | Read one job or return `404` |

Create a job:

```bash
curl -i -X POST http://127.0.0.1:3000/api/v1/jobs \
  -H "content-type: application/json" \
  -d '{"name":"resize avatars"}'
```

Windows PowerShell passes quotes to native programs differently. This equivalent command was verified on Windows:

```powershell
curl.exe --request POST --header "content-type: application/json" --data '{\"name\":\"resize-avatars\"}' http://127.0.0.1:3000/api/v1/jobs
```

List and read jobs:

```bash
curl -i http://127.0.0.1:3000/api/v1/jobs
curl -i http://127.0.0.1:3000/api/v1/jobs/1
```

Validation and parsing errors use a consistent envelope:

```json
{
  "error": {
    "code": "validation_error",
    "message": "job name must contain at least one non-whitespace character",
    "field": "name"
  }
}
```

## Verify

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Project progress and verified commands are recorded in `docs/progress.md`.
