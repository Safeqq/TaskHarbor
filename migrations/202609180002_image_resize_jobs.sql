ALTER TABLE jobs
    DROP CONSTRAINT jobs_type_valid,
    ADD COLUMN max_width INTEGER,
    ADD COLUMN jpeg_quality SMALLINT,
    ADD COLUMN failure_message TEXT,
    ADD CONSTRAINT jobs_type_valid CHECK (
        job_type IN ('demo_delay', 'image_resize')
    ),
    ADD CONSTRAINT jobs_image_settings_valid CHECK (
        (
            job_type = 'demo_delay'
            AND max_width IS NULL
            AND jpeg_quality IS NULL
        )
        OR
        (
            job_type = 'image_resize'
            AND max_width BETWEEN 1 AND 8192
            AND jpeg_quality BETWEEN 1 AND 100
        )
    ),
    ADD CONSTRAINT jobs_image_item_count_valid CHECK (
        job_type <> 'image_resize' OR progress_total BETWEEN 1 AND 10
    );

CREATE TABLE artifacts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    job_id BIGINT NOT NULL REFERENCES jobs (id) ON DELETE CASCADE,
    attempt_id BIGINT REFERENCES job_attempts (id) ON DELETE CASCADE,
    source_artifact_id BIGINT REFERENCES artifacts (id) ON DELETE RESTRICT,
    kind TEXT NOT NULL,
    item_index SMALLINT NOT NULL,
    storage_key TEXT NOT NULL UNIQUE,
    display_name VARCHAR(255) NOT NULL,
    media_type TEXT NOT NULL,
    byte_size BIGINT NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT artifacts_kind_valid CHECK (kind IN ('input', 'output')),
    CONSTRAINT artifacts_item_index_valid CHECK (item_index BETWEEN 0 AND 9),
    CONSTRAINT artifacts_name_valid CHECK (char_length(btrim(display_name)) BETWEEN 1 AND 255),
    CONSTRAINT artifacts_media_type_valid CHECK (
        media_type IN ('image/jpeg', 'image/png')
    ),
    CONSTRAINT artifacts_size_valid CHECK (byte_size > 0),
    CONSTRAINT artifacts_dimensions_valid CHECK (width > 0 AND height > 0),
    CONSTRAINT artifacts_relation_valid CHECK (
        (
            kind = 'input'
            AND attempt_id IS NULL
            AND source_artifact_id IS NULL
        )
        OR
        (
            kind = 'output'
            AND attempt_id IS NOT NULL
            AND source_artifact_id IS NOT NULL
            AND media_type = 'image/jpeg'
        )
    ),
    CONSTRAINT artifacts_job_kind_item_unique UNIQUE (job_id, kind, item_index)
);

CREATE INDEX artifacts_job_idx ON artifacts (job_id, kind, item_index);
