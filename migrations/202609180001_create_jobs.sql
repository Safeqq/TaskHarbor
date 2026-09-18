CREATE TABLE jobs (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    job_type TEXT NOT NULL DEFAULT 'demo_delay',
    state TEXT NOT NULL DEFAULT 'queued',
    available_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    progress_completed INTEGER NOT NULL DEFAULT 0,
    progress_total INTEGER NOT NULL DEFAULT 1,
    delay_ms INTEGER NOT NULL DEFAULT 500,
    result_message TEXT,
    result_duration_ms BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    CONSTRAINT jobs_name_valid CHECK (
        char_length(btrim(name)) BETWEEN 1 AND 100
    ),
    CONSTRAINT jobs_type_valid CHECK (job_type = 'demo_delay'),
    CONSTRAINT jobs_state_valid CHECK (
        state IN ('queued', 'running', 'succeeded', 'failed')
    ),
    CONSTRAINT jobs_progress_valid CHECK (
        progress_total > 0
        AND progress_completed BETWEEN 0 AND progress_total
    ),
    CONSTRAINT jobs_delay_valid CHECK (delay_ms BETWEEN 1 AND 30000),
    CONSTRAINT jobs_result_duration_valid CHECK (
        result_duration_ms IS NULL OR result_duration_ms >= 0
    )
);

CREATE INDEX jobs_queue_order_idx
    ON jobs (available_at, id)
    WHERE state = 'queued';

CREATE TABLE job_attempts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    job_id BIGINT NOT NULL REFERENCES jobs (id) ON DELETE CASCADE,
    attempt_number INTEGER NOT NULL,
    state TEXT NOT NULL,
    progress_completed INTEGER NOT NULL DEFAULT 0,
    progress_total INTEGER NOT NULL DEFAULT 1,
    started_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at TIMESTAMPTZ,
    duration_ms BIGINT,
    error_message TEXT,
    CONSTRAINT job_attempts_number_valid CHECK (attempt_number > 0),
    CONSTRAINT job_attempts_state_valid CHECK (
        state IN ('running', 'succeeded', 'failed')
    ),
    CONSTRAINT job_attempts_progress_valid CHECK (
        progress_total > 0
        AND progress_completed BETWEEN 0 AND progress_total
    ),
    CONSTRAINT job_attempts_duration_valid CHECK (
        duration_ms IS NULL OR duration_ms >= 0
    ),
    CONSTRAINT job_attempts_job_number_unique UNIQUE (job_id, attempt_number)
);
