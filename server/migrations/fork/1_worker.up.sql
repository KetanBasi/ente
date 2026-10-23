-- fork: home AI worker. Tracked in fork_schema_migrations, separate from
-- upstream's schema_migrations, so upstream can keep numbering freely.

CREATE TABLE IF NOT EXISTS fork_worker_tokens
(
    id                UUID PRIMARY KEY,
    user_id           BIGINT NOT NULL REFERENCES users (user_id) ON DELETE CASCADE,
    worker_public_key TEXT   NOT NULL,
    -- SHA-256 of the token. The raw token is never stored.
    token_hash        BYTEA  NOT NULL UNIQUE,
    -- The token sealed to worker_public_key with crypto_box_seal.
    sealed_token      TEXT   NOT NULL,
    created_at        BIGINT NOT NULL DEFAULT now_utc_micro_seconds(),
    last_used_at      BIGINT,
    revoked_at        BIGINT
);

-- At most one live token per user and worker.
CREATE UNIQUE INDEX IF NOT EXISTS fork_worker_tokens_live_idx
    ON fork_worker_tokens (user_id, worker_public_key)
    WHERE revoked_at IS NULL;

CREATE TABLE IF NOT EXISTS fork_worker_inbox
(
    id                BIGSERIAL PRIMARY KEY,
    file_id           BIGINT NOT NULL REFERENCES files (file_id) ON DELETE CASCADE,
    owner_id          BIGINT NOT NULL REFERENCES users (user_id) ON DELETE CASCADE,
    worker_public_key TEXT   NOT NULL,
    -- The fileKey sealed to worker_public_key with crypto_box_seal.
    sealed_file_key   TEXT   NOT NULL,
    created_at        BIGINT NOT NULL DEFAULT now_utc_micro_seconds(),
    consumed_at       BIGINT,
    UNIQUE (file_id, worker_public_key)
);

CREATE INDEX IF NOT EXISTS fork_worker_inbox_owner_idx
    ON fork_worker_inbox (owner_id, worker_public_key, id);
