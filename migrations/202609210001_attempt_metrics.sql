ALTER TABLE job_attempts
    ADD COLUMN queue_wait_ms BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN encoding_duration_ms BIGINT NOT NULL DEFAULT 0,
    ADD CONSTRAINT job_attempts_queue_wait_valid CHECK (queue_wait_ms >= 0),
    ADD CONSTRAINT job_attempts_encoding_duration_valid CHECK (encoding_duration_ms >= 0);

