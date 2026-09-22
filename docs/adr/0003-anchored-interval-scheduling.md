# ADR 0003: Use anchored interval scheduling with coalescing

- Status: Accepted
- Date: 2026-09-21

## Context

Recurring image work must remain predictable across restarts and concurrent schedulers. Creating every missed slot after downtime could flood the queue, while calculating the next run from the current time would cause schedules to drift.

## Decision

Represent a schedule with an interval, UTC anchor, and next run timestamp. Derive every slot from the anchor. When a scheduler observes a due schedule, it locks that schedule row, records one unique occurrence, and advances the next run in the same transaction.

If several slots were missed, create at most the latest due slot and record the number of older coalesced slots. If an earlier occurrence from that schedule is still nonterminal, record the latest slot as `skipped_overlap`. Each created job snapshots the schedule name, priority, settings, input metadata, and scheduled time. Schedule edits affect future snapshots only.

## Alternatives considered

- Calculating `next_run_at` from completion time would accumulate drift.
- Replaying every missed slot could create an unbounded backlog after downtime.
- Silently dropping overlap would hide why expected work did not run.
- Cron expressions would add timezone and daylight-saving semantics that the first release does not need.

## Consequences

- Restarts and concurrent scheduler ticks do not duplicate a `(schedule, slot)` occurrence.
- UTC anchored intervals are simple to explain and test with a controlled clock.
- Coalescing favors current work over complete historical replay.
- A schedule cannot intentionally run overlapping occurrences.
- Cron calendars and daylight-saving policies remain outside the first release.
