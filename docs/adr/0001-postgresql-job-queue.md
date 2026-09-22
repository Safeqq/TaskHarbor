# ADR 0001: Use PostgreSQL as the durable job queue

- Status: Accepted
- Date: 2026-09-21

## Context

TaskHarbor needs job state to survive process restarts, API and worker processes to share one source of truth, and concurrent workers to claim different eligible jobs. The first release is a single-host portfolio application, so operating a separate message broker would add another failure mode before its throughput is needed.

## Decision

Store jobs, attempts, schedules, ownership, and artifacts in PostgreSQL. A worker claims one eligible row inside a short transaction using `FOR UPDATE SKIP LOCKED`, ordered by priority, availability, and ID. The transaction changes the job to `running` and inserts its attempt before committing. Image processing and filesystem I/O happen after that commit.

PostgreSQL remains the authority for status. The shared filesystem stores image bytes, while database rows store their safe generated keys and checksums.

## Alternatives considered

- Redis lists or streams would provide a dedicated queue, but would require reconciling broker delivery with relational job state and operating another durable service.
- An in-process channel would be simpler, but queued work would disappear on restart and could not be shared by multiple processes.
- Polling without row locks would allow two workers to claim the same job before either update became visible.

## Consequences

- A local deployment needs only PostgreSQL in addition to the application processes.
- Claim transactions remain small and never cover image processing.
- `SKIP LOCKED` improves concurrent claiming but does not promise global FIFO order.
- Database polling has a practical scale ceiling. A broker becomes reasonable when measured database contention, polling load, or cross-region delivery requires it.
- Database and filesystem backup must be coordinated because they are separate durability domains.
