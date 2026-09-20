# TaskHarbor

TaskHarbor is a learning-focused portfolio project for creating, running, and monitoring background jobs. Its first real job type resizes JPEG and PNG images through a Rust API and a separate Rust worker.

## Current milestone

Phase 6 adds one-off and recurring scheduling to the resilient `image_resize` pipeline. Jobs can be assigned `high`, `normal`, or `low` priority, while recurring schedules keep an image template and a UTC-anchored fixed interval.

The current request flow is:

1. React sends multipart form data with the job settings and source images.
2. Axum streams each file to a server-generated storage key while enforcing count and byte limits.
3. Image headers are inspected to verify JPEG or PNG content, dimensions, megapixels, and the absence of PNG animation before PostgreSQL records the queued job.
4. A one-off job remains `queued` until `available_at`. The dashboard presents a future queued job as **Scheduled**.
5. Before claiming work, the worker materializes due recurring slots. Each occurrence snapshots the schedule's settings and input metadata into a new job.
6. The worker claims the highest-priority eligible job, then orders equal priorities by `available_at` and ID. Priority does not interrupt work that is already running.
7. A transient image I/O failure records the attempt and schedules another one with exponential backoff. A permanent input/settings failure, or an exhausted attempt limit, ends the job as `failed`.
8. PostgreSQL publishes the complete output manifest only after every item succeeds. The dashboard shows lifecycle actions, attempt history, schedules, and occurrence history.

The default `max_attempts` is three, including the first attempt. Manual retry leaves a terminal job and its history unchanged, creates a linked job through `retry_of_job_id`, and reuses the same source files.

Recurring intervals range from one minute to one year. If the scheduler was offline, it coalesces missed time into at most the latest due slot and records how many older slots were skipped. A due slot is recorded as `skipped_overlap` when an earlier occurrence from the same schedule is still nonterminal. Schedule edits affect future occurrences only.

Jobs survive API and worker restarts. Crash recovery is intentionally deferred: if the worker dies after a claim, that job remains `running` until it is reset manually in the local demo database. Cancellation is cooperative and cannot interrupt image code already running inside `spawn_blocking`; it takes effect at the next safe boundary.

## Repository structure

```text
apps/api/          HTTP transport and API executable
apps/worker/       Job execution loop and worker executable
apps/web/          React dashboard, API client, polling, and UI tests
crates/adapters/   PostgreSQL, local storage, and image processing adapters
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

Both executables apply pending migrations before doing other work. They share `TASKHARBOR_STORAGE_DIR`, which defaults to `var/storage`. The worker bounds CPU-heavy image tasks with `TASKHARBOR_MAX_BLOCKING_TASKS`, which defaults to `2`. The API listens on `127.0.0.1:3000`; `TASKHARBOR_BIND_ADDR` can override it.

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
| `POST` | `/api/v1/jobs/{id}/cancel` | Cancel a pending job or request cancellation of a running job |
| `POST` | `/api/v1/jobs/{id}/retry` | Create a linked retry of a failed or cancelled job |
| `POST` | `/api/v1/schedules` | Create a recurring image schedule from multipart input |
| `GET` | `/api/v1/schedules` | List schedules with occurrence history |
| `GET` | `/api/v1/schedules/{id}` | Read one schedule or return `404` |
| `PUT` | `/api/v1/schedules/{id}` | Edit future settings or enable/disable a schedule |
| `GET` | `/api/v1/artifacts/{id}/download` | Download a published JPEG output |

Create a job:

```bash
curl -i -X POST http://127.0.0.1:3000/api/v1/jobs \
  -F "name=resize avatars" \
  -F "max_width=1600" \
  -F "jpeg_quality=85" \
  -F "priority=high" \
  -F "available_at=2026-09-20T08:00:00Z" \
  -F "images=@avatar.png" \
  -F "images=@portrait.jpg"
```

Create an hourly recurring schedule. `anchor_at` and API timestamps use RFC 3339; the dashboard converts local form input to UTC.

```bash
curl -i -X POST http://127.0.0.1:3000/api/v1/schedules \
  -F "name=hourly catalog previews" \
  -F "interval_seconds=3600" \
  -F "anchor_at=2026-09-20T08:00:00Z" \
  -F "priority=normal" \
  -F "max_width=1600" \
  -F "jpeg_quality=85" \
  -F "images=@catalog.png"
```

Windows PowerShell passes quotes to native programs differently. This equivalent command was verified on Windows:

```powershell
curl.exe --request POST --form "name=resize avatars" --form "max_width=1600" --form "jpeg_quality=85" --form "images=@avatar.png" http://127.0.0.1:3000/api/v1/jobs
```

List and read jobs while the worker changes their state. The possible states are `queued`, `running`, `retry_waiting`, `cancel_requested`, `succeeded`, `failed`, and `cancelled`:

```bash
curl -i http://127.0.0.1:3000/api/v1/jobs
curl -i http://127.0.0.1:3000/api/v1/jobs/1
```

Cancel a pending/running job, or create a new linked job after failure or cancellation:

```bash
curl -i -X POST http://127.0.0.1:3000/api/v1/jobs/1/cancel
curl -i -X POST http://127.0.0.1:3000/api/v1/jobs/1/retry
```

Validation and parsing errors use a consistent envelope:

```json
{
  "error": {
    "code": "invalid_image",
    "message": "file content is not a supported JPEG or PNG image",
    "field": "images"
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
cargo test -p taskharbor-api --tests --locked -- --ignored --test-threads=1
cargo test -p taskharbor-worker --tests --locked -- --ignored --test-threads=1
```

Project progress and verified commands are recorded in `docs/progress.md`.
