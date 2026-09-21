# TaskHarbor

TaskHarbor is a learning-focused portfolio project for creating, running, and monitoring background jobs. Its first real job type resizes JPEG and PNG images through a Rust API and a separate Rust worker.

## Current milestone

Phase 8 makes the single-owner dashboard suitable for a limited demo. The API now requires an Argon2id-backed owner login, expiring server-side sessions, strict cookies, CSRF tokens, ownership checks, rate limits, and resource budgets. The worker emits structured JSON logs, reports queue measurements, expires old output, removes safe orphans, and supports a tested database-plus-files backup workflow.

The current request flow is:

1. React restores an owner session or presents the login form. The API keeps only hashes of random session and CSRF tokens in PostgreSQL.
2. React sends authenticated multipart form data with the job settings, a CSRF header, and source images.
3. Axum streams each file to a server-generated storage key while enforcing count, byte, active-job, and disk limits.
4. Image headers are inspected to verify JPEG or PNG content, dimensions, megapixels, and the absence of PNG animation before PostgreSQL records the queued job.
5. A one-off job remains `queued` until `available_at`. The dashboard presents a future queued job as **Scheduled**.
6. Before claiming work, the worker materializes due recurring slots. Each occurrence snapshots the schedule's settings and input metadata into a new job.
7. Each worker registers its name and capacity, sends heartbeats, and claims up to its local concurrency limit. A claim creates an attempt with a lease and a random fencing token.
8. Active workers renew leases while processing. If a lease expires, any worker can transactionally close that attempt and schedule a retry, finish a pending cancellation, or fail the job when its attempt limit is exhausted.
9. Every progress, failure, cancellation, and completion write checks the attempt owner and token. A stale worker can finish physical computation, but PostgreSQL rejects its write and its per-attempt output directory is discarded.
10. PostgreSQL publishes the complete output manifest only after every item succeeds. Maintenance preserves referenced input and active attempt paths while applying output retention and orphan grace periods.

The default `max_attempts` is three, including the first attempt. Manual retry leaves a terminal job and its history unchanged, creates a linked job through `retry_of_job_id`, and reuses the same source files.

Recurring intervals range from one minute to one year. If the scheduler was offline, it coalesces missed time into at most the latest due slot and records how many older slots were skipped. A due slot is recorded as `skipped_overlap` when an earlier occurrence from the same schedule is still nonterminal. Schedule edits affect future occurrences only.

Jobs survive API and worker restarts. Execution is at-least-once: a crashed attempt may have performed physical work before its lease expires, but only the current token can publish the final manifest. Cancellation is cooperative and cannot interrupt image code already running inside `spawn_blocking`; it takes effect at the next safe boundary. Multiple workers currently require the same host and shared storage directory.

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
$env:TASKHARBOR_OWNER_PASSWORD = "choose-a-local-password-with-12-or-more-bytes"
cargo run -p taskharbor-api --locked
```

Start the worker in a second terminal with the same `DATABASE_URL`:

```powershell
$env:DATABASE_URL = "postgres://taskharbor:taskharbor_dev@127.0.0.1:5432/taskharbor"
cargo run -p taskharbor-worker --locked
```

To run two named workers against the same database and storage, start this command in two terminals and use a different name in each:

```powershell
$env:DATABASE_URL = "postgres://taskharbor:taskharbor_dev@127.0.0.1:5432/taskharbor"
$env:TASKHARBOR_WORKER_NAME = "worker-a"
$env:TASKHARBOR_WORKER_CONCURRENCY = "2"
cargo run -p taskharbor-worker --locked
```

Install and start the dashboard in a third terminal:

```powershell
npm ci --prefix apps/web
npm run dev --prefix apps/web
```

Open `http://127.0.0.1:5173`. The Vite development server proxies `/api` and `/health` to the API at `http://127.0.0.1:3000`, so no browser CORS configuration is needed for local development.

Both executables apply pending migrations before doing other work. They share `TASKHARBOR_STORAGE_DIR`, which defaults to `var/storage`. The worker defaults to two concurrent jobs and two CPU-heavy image tasks. `TASKHARBOR_WORKER_CONCURRENCY` and `TASKHARBOR_MAX_BLOCKING_TASKS` configure those independent limits.

The default worker timing is a 15-second lease renewed every 5 seconds, a heartbeat every 2 seconds with a 10-second liveness window, and a 30-second shutdown drain. These can be changed with `TASKHARBOR_LEASE_SECONDS`, `TASKHARBOR_LEASE_RENEWAL_SECONDS`, `TASKHARBOR_HEARTBEAT_SECONDS`, `TASKHARBOR_HEARTBEAT_TTL_SECONDS`, and `TASKHARBOR_SHUTDOWN_GRACE_SECONDS`.

The API refuses to start without `TASKHARBOR_OWNER_PASSWORD` and accepts passwords from 12 through 128 bytes. It binds to `127.0.0.1:3000` by default. A non-loopback bind requires both `TASKHARBOR_ALLOW_REMOTE=true` and `TASKHARBOR_SECURE_COOKIES=true`; place it behind HTTPS. Local HTTP development keeps secure cookies disabled. Keep the API and worker on the same storage budget, which defaults to 1 GiB.

```text
{"level":"INFO","message":"TaskHarbor API listening","bind_addr":"127.0.0.1:3000"}
```

## API

| Method | Path | Result |
|---|---|---|
| `GET` | `/health/live` | Process liveness without dependencies |
| `GET` | `/health/ready` | Database and writable-storage readiness |
| `POST` | `/api/v1/session/login` | Create an expiring owner session |
| `GET` | `/api/v1/session` | Read the current session and CSRF token |
| `DELETE` | `/api/v1/session` | Revoke the current session |
| `POST` | `/api/v1/jobs` | Create a queued job |
| `GET` | `/api/v1/jobs` | List jobs by ascending ID |
| `GET` | `/api/v1/jobs/{id}` | Read one job or return `404` |
| `POST` | `/api/v1/jobs/{id}/cancel` | Cancel a pending job or request cancellation of a running job |
| `POST` | `/api/v1/jobs/{id}/retry` | Create a linked retry of a failed or cancelled job |
| `GET` | `/api/v1/workers` | List worker heartbeat state, capacity, and active attempt count |
| `POST` | `/api/v1/schedules` | Create a recurring image schedule from multipart input |
| `GET` | `/api/v1/schedules` | List schedules with occurrence history |
| `GET` | `/api/v1/schedules/{id}` | Read one schedule or return `404` |
| `PUT` | `/api/v1/schedules/{id}` | Edit future settings or enable/disable a schedule |
| `GET` | `/api/v1/artifacts/{id}/download` | Download a published JPEG output |

All job, schedule, worker, and artifact routes require the session cookie. Login first, save both cookies, and copy `csrf_token` from the JSON response into the shell variable:

```bash
curl -i -c taskharbor.cookies \
  -H "Content-Type: application/json" \
  -d '{"username":"owner","password":"your-local-password"}' \
  http://127.0.0.1:3000/api/v1/session/login

export CSRF_TOKEN="copy-csrf_token-from-the-response"
```

Create a job. `Idempotency-Key` is optional; replaying the same key and body returns the original job, while different data returns `409 Conflict`:

```bash
curl -i -X POST http://127.0.0.1:3000/api/v1/jobs \
  -b taskharbor.cookies \
  -H "X-CSRF-Token: $CSRF_TOKEN" \
  -H "Idempotency-Key: resize-avatars-20260920" \
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
  -b taskharbor.cookies \
  -H "X-CSRF-Token: $CSRF_TOKEN" \
  -F "name=hourly catalog previews" \
  -F "interval_seconds=3600" \
  -F "anchor_at=2026-09-20T08:00:00Z" \
  -F "priority=normal" \
  -F "max_width=1600" \
  -F "jpeg_quality=85" \
  -F "images=@catalog.png"
```

List and read jobs while the worker changes their state. The possible states are `queued`, `running`, `retry_waiting`, `cancel_requested`, `succeeded`, `failed`, and `cancelled`:

```bash
curl -i -b taskharbor.cookies http://127.0.0.1:3000/api/v1/jobs
curl -i -b taskharbor.cookies http://127.0.0.1:3000/api/v1/jobs/1
```

Cancel a pending/running job, or create a new linked job after failure or cancellation:

```bash
curl -i -X POST -b taskharbor.cookies -H "X-CSRF-Token: $CSRF_TOKEN" http://127.0.0.1:3000/api/v1/jobs/1/cancel
curl -i -X POST -b taskharbor.cookies -H "X-CSRF-Token: $CSRF_TOKEN" http://127.0.0.1:3000/api/v1/jobs/1/retry
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

## Operations

The worker runs maintenance at startup and then hourly by default. It expires published output after seven days, removes unreferenced files only after a one-hour grace period, preserves every database-referenced input, and protects output prefixes belonging to running attempts. Configure these periods with `TASKHARBOR_MAINTENANCE_SECONDS`, `TASKHARBOR_OUTPUT_RETENTION_SECONDS`, and `TASKHARBOR_ORPHAN_GRACE_SECONDS`.

API and worker logs are JSON. Worker events include `job_id`, `attempt_id`, `worker_id`, queue depth, oldest eligible wait, attempt duration, and failure kind. Set `RUST_LOG` to change the filter. Request logs include method, path, status, and duration; credentials, cookies, CSRF tokens, filenames, and file contents are not logged.

A consistent backup needs PostgreSQL and the shared storage tree from the same quiet point. Stop the API and every worker before backup or restore; the scripts require the explicit `-ConfirmQuiesced` switch. Restore into an empty storage directory and a disposable or otherwise prepared database:

```powershell
.\scripts\backup.ps1 `
  -DatabaseUrl $env:DATABASE_URL `
  -StorageDir "var/storage" `
  -Destination "var/backups/demo-20260920" `
  -ConfirmQuiesced `
  -PgBin "C:\Program Files\PostgreSQL\18\bin"

.\scripts\restore.ps1 `
  -BackupDir "var/backups/demo-20260920" `
  -DatabaseUrl $env:DATABASE_URL `
  -StorageDir "var/restored-storage" `
  -ConfirmQuiesced `
  -PgBin "C:\Program Files\PostgreSQL\18\bin"
```

The backup manifest contains SHA-256 checksums for the custom-format database dump and storage ZIP. It contains no database URL or password. `restore.ps1` verifies both checksums and rejects unsafe ZIP paths before running `pg_restore`.

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
