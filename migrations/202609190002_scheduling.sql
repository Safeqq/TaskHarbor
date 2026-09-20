CREATE TABLE schedules (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    interval_seconds INTEGER NOT NULL,
    anchor_at TIMESTAMPTZ NOT NULL,
    next_run_at TIMESTAMPTZ NOT NULL,
    priority TEXT NOT NULL DEFAULT 'normal',
    max_width INTEGER NOT NULL,
    jpeg_quality SMALLINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT schedules_name_valid CHECK (
        char_length(btrim(name)) BETWEEN 1 AND 100
    ),
    CONSTRAINT schedules_interval_valid CHECK (
        interval_seconds BETWEEN 60 AND 31536000
    ),
    CONSTRAINT schedules_priority_valid CHECK (
        priority IN ('high', 'normal', 'low')
    ),
    CONSTRAINT schedules_image_settings_valid CHECK (
        max_width BETWEEN 1 AND 8192
        AND jpeg_quality BETWEEN 1 AND 100
    )
);

CREATE INDEX schedules_due_idx
    ON schedules (next_run_at, id)
    WHERE enabled;

CREATE TABLE schedule_inputs (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    schedule_id BIGINT NOT NULL REFERENCES schedules (id) ON DELETE CASCADE,
    item_index SMALLINT NOT NULL,
    storage_key TEXT NOT NULL,
    display_name VARCHAR(255) NOT NULL,
    media_type TEXT NOT NULL,
    byte_size BIGINT NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT schedule_inputs_item_index_valid CHECK (item_index >= 0),
    CONSTRAINT schedule_inputs_media_type_valid CHECK (
        media_type IN ('image/jpeg', 'image/png')
    ),
    CONSTRAINT schedule_inputs_size_valid CHECK (byte_size > 0),
    CONSTRAINT schedule_inputs_dimensions_valid CHECK (
        width > 0 AND height > 0
    ),
    CONSTRAINT schedule_inputs_schedule_item_unique UNIQUE (schedule_id, item_index)
);

CREATE INDEX schedule_inputs_storage_key_idx ON schedule_inputs (storage_key);

ALTER TABLE jobs
    ADD COLUMN priority TEXT NOT NULL DEFAULT 'normal',
    ADD COLUMN schedule_id BIGINT REFERENCES schedules (id) ON DELETE RESTRICT,
    ADD COLUMN scheduled_for TIMESTAMPTZ,
    ADD CONSTRAINT jobs_priority_valid CHECK (
        priority IN ('high', 'normal', 'low')
    ),
    ADD CONSTRAINT jobs_schedule_origin_valid CHECK (
        (schedule_id IS NULL AND scheduled_for IS NULL)
        OR (schedule_id IS NOT NULL AND scheduled_for IS NOT NULL)
    );

DROP INDEX jobs_queue_order_idx;

CREATE INDEX jobs_queue_order_idx
    ON jobs (
        (
            CASE priority
                WHEN 'high' THEN 0
                WHEN 'normal' THEN 1
                ELSE 2
            END
        ),
        available_at,
        id
    )
    WHERE state IN ('queued', 'retry_waiting');

CREATE TABLE schedule_occurrences (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    schedule_id BIGINT NOT NULL REFERENCES schedules (id) ON DELETE CASCADE,
    scheduled_for TIMESTAMPTZ NOT NULL,
    outcome TEXT NOT NULL,
    job_id BIGINT REFERENCES jobs (id) ON DELETE RESTRICT,
    reason TEXT,
    coalesced_slots BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT schedule_occurrences_outcome_valid CHECK (
        outcome IN ('created', 'skipped_overlap')
    ),
    CONSTRAINT schedule_occurrences_result_valid CHECK (
        (outcome = 'created' AND job_id IS NOT NULL AND reason IS NULL)
        OR
        (outcome = 'skipped_overlap' AND job_id IS NULL AND reason IS NOT NULL)
    ),
    CONSTRAINT schedule_occurrences_coalesced_valid CHECK (coalesced_slots >= 0),
    CONSTRAINT schedule_occurrences_slot_unique UNIQUE (schedule_id, scheduled_for)
);

CREATE INDEX schedule_occurrences_history_idx
    ON schedule_occurrences (schedule_id, scheduled_for DESC);
