# TaskHarbor

TaskHarbor is a learning-focused portfolio project for creating, scheduling, running, and monitoring background jobs. The first real job type will resize JPEG and PNG images through a Rust API and a separate Rust worker.

## Current milestone

Phase 2 persists jobs in PostgreSQL and runs them in a separate worker process. The current `demo_delay` job waits asynchronously for 500 milliseconds, then records its result and attempt history.

The current request flow is:

1. Axum parses the HTTP request and JSON body.
2. The core library validates the job name.
3. PostgreSQL assigns an ID and saves a queued job.
4. The API returns the persisted job as JSON.
5. The worker claims one eligible job in a short transaction, releases the row lock, waits asynchronously, and records completion in a new transaction.

Jobs survive API and worker restarts. Crash recovery is intentionally deferred: if the worker dies after a claim, that job remains `running` until it is reset manually in the local demo database.

## Repository structure

```text
apps/api/          HTTP transport and API executable
apps/worker/       Job execution loop and worker executable
crates/adapters/   PostgreSQL repository and atomic queue claim
crates/core/       Domain types and validation rules
migrations/        Append-only PostgreSQL schema changes
deploy/            Local PostgreSQL Compose configuration
docs/              Learning checkpoint and project glossary
```

The remaining target structure will be added only when its roadmap phase needs it.

## Requirements

- Rust 1.95.0 with Cargo, rustfmt, and Clippy
- Git
- PostgreSQL 18, either through Docker Compose or a local installation

Node.js will be needed when the dashboard is added. Docker is optional when PostgreSQL is installed directly.

## Run locally

The repository pins its Rust version in `rust-toolchain.toml`. Start the development and test databases with Docker:

```bash
docker compose -f deploy/compose.yaml up -d
```

The Compose service binds PostgreSQL to `127.0.0.1:5432` and creates both `taskharbor` and `taskharbor_test`. Set the environment variable from `.env.example`; the application does not load `.env` automatically.

Start the API:

```powershell
$env:DATABASE_URL = "postgres://taskharbor:taskharbor_dev@127.0.0.1:5432/taskharbor"
cargo run -p taskharbor-api --locked
```

Start the worker in a second terminal with the same `DATABASE_URL`:

```powershell
$env:DATABASE_URL = "postgres://taskharbor:taskharbor_dev@127.0.0.1:5432/taskharbor"
cargo run -p taskharbor-worker --locked
```

Both executables apply pending migrations before doing other work. The API listens on `127.0.0.1:3000` by default; `TASKHARBOR_BIND_ADDR` can override it.

```text
TaskHarbor API listening on http://127.0.0.1:3000
```

## API

| Method | Path | Result |
|---|---|---|
| `GET` | `/health` | API and database health |
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

List and read jobs while the worker changes their state from `queued` to `running` and then `succeeded`:

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

Database integration tests use the isolated test database and are opt-in:

```powershell
$env:TEST_DATABASE_URL = "postgres://taskharbor:taskharbor_dev@127.0.0.1:5432/taskharbor_test"
cargo test -p taskharbor-adapters --locked -- --ignored --test-threads=1
cargo test -p taskharbor-api --test jobs_api --locked -- --ignored --test-threads=1
```

Project progress and verified commands are recorded in `docs/progress.md`.
