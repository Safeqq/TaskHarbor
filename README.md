# TaskHarbor

TaskHarbor is a learning-focused portfolio project for creating, scheduling, running, and monitoring background jobs. The first real job type will resize JPEG and PNG images through a Rust API and a separate Rust worker.

## Current milestone

Phase 3 adds a React and TypeScript dashboard to the persistent job pipeline. The dashboard creates `demo_delay` jobs, polls their state every two seconds, and shows progress, timestamps, and worker results without a manual refresh.

The current request flow is:

1. React submits the labelled create form through the frontend API client.
2. Vite proxies the development request to Axum, which validates the job name.
3. PostgreSQL assigns an ID and saves a queued job.
4. The worker claims one eligible job in a short transaction, releases the row lock, waits asynchronously, and records completion in a new transaction.
5. Sequential polling reads the new state and updates the list and selected job detail.

Jobs survive API and worker restarts. Crash recovery is intentionally deferred: if the worker dies after a claim, that job remains `running` until it is reset manually in the local demo database.

## Repository structure

```text
apps/api/          HTTP transport and API executable
apps/worker/       Job execution loop and worker executable
apps/web/          React dashboard, API client, polling, and UI tests
crates/adapters/   PostgreSQL repository and atomic queue claim
crates/core/       Domain types and validation rules
migrations/        Append-only PostgreSQL schema changes
deploy/            Local PostgreSQL Compose configuration
docs/              Learning checkpoint and project glossary
```

The remaining target structure will be added only when its roadmap phase needs it.

## Requirements

- Rust 1.95.0 with Cargo, rustfmt, and Clippy
- Node.js 24.11.1 with npm 11.6.2
- Git
- PostgreSQL 18, either through Docker Compose or a local installation

Docker is optional when PostgreSQL is installed directly.

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

Install and start the dashboard in a third terminal:

```powershell
npm ci --prefix apps/web
npm run dev --prefix apps/web
```

Open `http://127.0.0.1:5173`. The Vite development server proxies `/api` and `/health` to the API at `http://127.0.0.1:3000`, so no browser CORS configuration is needed for local development.

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
npm run typecheck --prefix apps/web
npm test --prefix apps/web
npm run build --prefix apps/web
```

Database integration tests use the isolated test database and are opt-in:

```powershell
$env:TEST_DATABASE_URL = "postgres://taskharbor:taskharbor_dev@127.0.0.1:5432/taskharbor_test"
cargo test -p taskharbor-adapters --locked -- --ignored --test-threads=1
cargo test -p taskharbor-api --test jobs_api --locked -- --ignored --test-threads=1
```

Project progress and verified commands are recorded in `docs/progress.md`.
