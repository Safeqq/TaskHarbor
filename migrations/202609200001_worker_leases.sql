CREATE TABLE workers (
    id UUID PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    concurrency_limit SMALLINT NOT NULL,
    lease_duration_seconds INTEGER NOT NULL,
    started_at TIMESTAMPTZ NOT NULL,
    last_heartbeat_at TIMESTAMPTZ NOT NULL,
    heartbeat_expires_at TIMESTAMPTZ NOT NULL,
    stopped_at TIMESTAMPTZ,
    CONSTRAINT workers_name_valid CHECK (
        char_length(btrim(name)) BETWEEN 1 AND 100
    ),
    CONSTRAINT workers_concurrency_valid CHECK (
        concurrency_limit BETWEEN 1 AND 64
    ),
    CONSTRAINT workers_lease_duration_valid CHECK (
        lease_duration_seconds BETWEEN 2 AND 3600
    ),
    CONSTRAINT workers_heartbeat_window_valid CHECK (
        heartbeat_expires_at > last_heartbeat_at
    ),
    CONSTRAINT workers_stop_time_valid CHECK (
        stopped_at IS NULL OR stopped_at >= started_at
    )
);

CREATE INDEX workers_heartbeat_idx
    ON workers (heartbeat_expires_at)
    WHERE stopped_at IS NULL;

-- A process from the pre-lease version cannot prove ownership after this
-- migration. Close its attempt and return eligible jobs to the durable queue.
UPDATE job_attempts
SET state = CASE
        WHEN job.state = 'cancel_requested' THEN 'cancelled'
        ELSE 'failed'
    END,
    finished_at = CURRENT_TIMESTAMP,
    duration_ms = GREATEST(
        0,
        FLOOR(EXTRACT(EPOCH FROM (CURRENT_TIMESTAMP - job_attempts.started_at)) * 1000)::BIGINT
    ),
    error_kind = CASE
        WHEN job.state = 'cancel_requested' THEN 'cancelled'
        ELSE 'transient'
    END,
    error_message = CASE
        WHEN job.state = 'cancel_requested' THEN 'cancelled while upgrading worker ownership'
        ELSE 'worker ownership was reset during lease migration'
    END
FROM jobs AS job
WHERE job_attempts.job_id = job.id
  AND job_attempts.state = 'running';

UPDATE jobs
SET state = CASE
        WHEN jobs.state = 'cancel_requested' THEN 'cancelled'
        WHEN attempts.used < jobs.max_attempts THEN 'retry_waiting'
        ELSE 'failed'
    END,
    available_at = CASE
        WHEN jobs.state = 'running' AND attempts.used < jobs.max_attempts
            THEN CURRENT_TIMESTAMP
        ELSE jobs.available_at
    END,
    progress_completed = CASE
        WHEN jobs.state = 'running' AND attempts.used < jobs.max_attempts THEN 0
        ELSE jobs.progress_completed
    END,
    result_message = NULL,
    result_duration_ms = NULL,
    failure_message = CASE
        WHEN jobs.state = 'cancel_requested' THEN NULL
        ELSE 'worker ownership was reset during lease migration'
    END,
    finished_at = CASE
        WHEN jobs.state = 'cancel_requested' OR attempts.used >= jobs.max_attempts
            THEN CURRENT_TIMESTAMP
        ELSE NULL
    END
FROM (
    SELECT job_id, COUNT(*)::INTEGER AS used
    FROM job_attempts
    GROUP BY job_id
) AS attempts
WHERE jobs.id = attempts.job_id
  AND jobs.state IN ('running', 'cancel_requested');

ALTER TABLE job_attempts
    ADD COLUMN worker_id UUID REFERENCES workers (id) ON DELETE RESTRICT,
    ADD COLUMN claim_token UUID,
    ADD COLUMN lease_expires_at TIMESTAMPTZ,
    ADD CONSTRAINT job_attempts_claim_token_unique UNIQUE (claim_token),
    ADD CONSTRAINT job_attempts_claim_fields_valid CHECK (
        (
            worker_id IS NULL
            AND claim_token IS NULL
            AND lease_expires_at IS NULL
        )
        OR
        (
            worker_id IS NOT NULL
            AND claim_token IS NOT NULL
            AND lease_expires_at IS NOT NULL
        )
    ),
    ADD CONSTRAINT job_attempts_running_claim_valid CHECK (
        state <> 'running'
        OR (
            worker_id IS NOT NULL
            AND claim_token IS NOT NULL
            AND lease_expires_at IS NOT NULL
        )
    );

CREATE INDEX job_attempts_expired_lease_idx
    ON job_attempts (lease_expires_at, id)
    WHERE state = 'running';

CREATE INDEX job_attempts_worker_active_idx
    ON job_attempts (worker_id, id)
    WHERE state = 'running';
