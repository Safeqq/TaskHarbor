ALTER TABLE jobs
    DROP CONSTRAINT jobs_state_valid,
    ADD COLUMN max_attempts INTEGER NOT NULL DEFAULT 3,
    ADD COLUMN retry_of_job_id BIGINT REFERENCES jobs (id) ON DELETE SET NULL,
    ADD COLUMN cancel_requested_at TIMESTAMPTZ,
    ADD CONSTRAINT jobs_state_valid CHECK (
        state IN (
            'queued',
            'running',
            'retry_waiting',
            'cancel_requested',
            'succeeded',
            'failed',
            'cancelled'
        )
    ),
    ADD CONSTRAINT jobs_max_attempts_valid CHECK (
        max_attempts BETWEEN 1 AND 10
    ),
    ADD CONSTRAINT jobs_retry_parent_valid CHECK (
        retry_of_job_id IS NULL OR retry_of_job_id <> id
    );

DROP INDEX jobs_queue_order_idx;

CREATE INDEX jobs_queue_order_idx
    ON jobs (available_at, id)
    WHERE state IN ('queued', 'retry_waiting');

ALTER TABLE job_attempts
    DROP CONSTRAINT job_attempts_state_valid,
    ADD COLUMN error_kind TEXT,
    ADD CONSTRAINT job_attempts_state_valid CHECK (
        state IN ('running', 'succeeded', 'failed', 'cancelled')
    );

UPDATE job_attempts
SET error_kind = 'permanent'
WHERE state = 'failed';

ALTER TABLE job_attempts
    ADD CONSTRAINT job_attempts_error_kind_valid CHECK (
        (state IN ('running', 'succeeded') AND error_kind IS NULL)
        OR (state = 'failed' AND error_kind IN ('transient', 'permanent'))
        OR (state = 'cancelled' AND error_kind = 'cancelled')
    );

ALTER TABLE artifacts
    DROP CONSTRAINT artifacts_storage_key_key;

CREATE INDEX artifacts_storage_key_idx ON artifacts (storage_key);
