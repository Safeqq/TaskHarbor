# ADR 0002: Recover work with leases and fencing tokens

- Status: Accepted
- Date: 2026-09-21

## Context

A worker can stop after claiming a job, lose its database connection, or continue computing after another worker has recovered its expired attempt. A status flag alone cannot distinguish the current owner from that stale process.

## Decision

Each claim creates an attempt with a worker ID, lease expiry, and random fencing token. The active worker renews its lease while it processes the job. Progress, failure, cancellation, duration, and completion updates include the attempt ID, worker ID, and token in their conditional update.

After expiry, a worker closes the abandoned attempt and either schedules another attempt, completes a pending cancellation, or fails a job that exhausted its attempt limit. Output is written below an attempt-specific storage prefix. Only the current fenced attempt may publish the final artifact manifest; stale output is removed.

## Alternatives considered

- Process heartbeats alone show worker liveness but cannot prove ownership of one attempt.
- A lease without a token still permits an old process to write after the row has been reclaimed.
- Holding a database transaction or advisory lock during image processing would tie up connections and locks for unbounded CPU and filesystem work.
- Exactly-once computation would require transactional coordination with the image processor and filesystem that these components do not provide.

## Consequences

- Execution is at least once: physical work can repeat after a crash, while publication remains single-winner.
- Lease and renewal intervals must leave enough margin for temporary database latency.
- Cancellation is cooperative at safe boundaries; a running blocking image operation cannot be forcibly aborted.
- Every state-changing repository method must preserve the fencing predicate.
- Attempt-specific paths make stale cleanup possible without deleting a valid result.
