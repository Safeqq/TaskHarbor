CREATE TABLE users (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    username VARCHAR(32) NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT users_username_valid CHECK (
        username ~ '^[a-z0-9][a-z0-9_-]{2,31}$'
    ),
    CONSTRAINT users_password_hash_present CHECK (
        char_length(password_hash) BETWEEN 1 AND 512
    )
);

-- ID 1 is a disabled-by-password bootstrap owner. The API replaces the
-- sentinel with an Argon2id PHC string from TASKHARBOR_OWNER_PASSWORD before
-- it starts listening. No usable credential is committed to the repository.
INSERT INTO users (username, password_hash)
VALUES ('owner', '!unconfigured');

CREATE TABLE sessions (
    id UUID PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL UNIQUE,
    csrf_token_hash BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    CONSTRAINT sessions_token_hash_valid CHECK (octet_length(token_hash) = 32),
    CONSTRAINT sessions_csrf_hash_valid CHECK (octet_length(csrf_token_hash) = 32),
    CONSTRAINT sessions_expiry_valid CHECK (expires_at > created_at),
    CONSTRAINT sessions_last_seen_valid CHECK (last_seen_at >= created_at),
    CONSTRAINT sessions_revoke_time_valid CHECK (
        revoked_at IS NULL OR revoked_at >= created_at
    )
);

CREATE INDEX sessions_active_token_idx
    ON sessions (token_hash, expires_at)
    WHERE revoked_at IS NULL;

ALTER TABLE jobs
    ADD COLUMN owner_user_id BIGINT NOT NULL DEFAULT 1
        REFERENCES users (id) ON DELETE RESTRICT,
    ADD COLUMN idempotency_key VARCHAR(128),
    ADD COLUMN request_fingerprint BYTEA,
    ADD COLUMN outputs_expired_at TIMESTAMPTZ,
    ADD CONSTRAINT jobs_idempotency_fields_valid CHECK (
        (idempotency_key IS NULL AND request_fingerprint IS NULL)
        OR
        (
            idempotency_key IS NOT NULL
            AND char_length(idempotency_key) BETWEEN 1 AND 128
            AND request_fingerprint IS NOT NULL
            AND octet_length(request_fingerprint) = 32
        )
    );

CREATE INDEX jobs_owner_created_idx
    ON jobs (owner_user_id, created_at DESC, id DESC);

CREATE UNIQUE INDEX jobs_owner_idempotency_unique
    ON jobs (owner_user_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

ALTER TABLE artifacts
    ADD COLUMN checksum_sha256 BYTEA,
    ADD CONSTRAINT artifacts_checksum_valid CHECK (
        checksum_sha256 IS NULL OR octet_length(checksum_sha256) = 32
    );

ALTER TABLE schedule_inputs
    ADD COLUMN checksum_sha256 BYTEA,
    ADD CONSTRAINT schedule_inputs_checksum_valid CHECK (
        checksum_sha256 IS NULL OR octet_length(checksum_sha256) = 32
    );

ALTER TABLE schedules
    ADD COLUMN owner_user_id BIGINT NOT NULL DEFAULT 1
        REFERENCES users (id) ON DELETE RESTRICT;

CREATE INDEX schedules_owner_created_idx
    ON schedules (owner_user_id, created_at DESC, id DESC);
