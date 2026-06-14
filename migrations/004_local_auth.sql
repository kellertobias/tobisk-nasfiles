ALTER TABLE users ADD COLUMN auth_provider TEXT NOT NULL DEFAULT 'oidc';
ALTER TABLE users ADD COLUMN username_normalized TEXT;
ALTER TABLE users ADD COLUMN password_hash TEXT;
ALTER TABLE users ADD COLUMN setup_password_fingerprint TEXT;
ALTER TABLE users ADD COLUMN setup_password_source TEXT;
ALTER TABLE users ADD COLUMN password_changed_at BIGINT;

UPDATE users
SET username_normalized = lower(username)
WHERE username_normalized IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS users_local_username_norm_idx
ON users(username_normalized)
WHERE auth_provider = 'local';

CREATE TABLE IF NOT EXISTS local_passkeys (
    id                  TEXT PRIMARY KEY,
    user_id             TEXT NOT NULL REFERENCES users(id),
    credential_id       TEXT NOT NULL UNIQUE,
    credential_json     TEXT NOT NULL,
    label               TEXT,
    created_at          BIGINT NOT NULL,
    last_used_at        BIGINT,
    revoked_at          BIGINT
);

CREATE INDEX IF NOT EXISTS local_passkeys_user_idx
ON local_passkeys(user_id);

CREATE TABLE IF NOT EXISTS local_totp (
    user_id             TEXT PRIMARY KEY REFERENCES users(id),
    secret_enc          TEXT NOT NULL,
    created_at          BIGINT NOT NULL,
    confirmed_at        BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS local_totp_trusted_devices (
    id                  TEXT PRIMARY KEY,
    user_id             TEXT NOT NULL REFERENCES users(id),
    secret_enc          TEXT NOT NULL,
    secret_hash         TEXT NOT NULL,
    label               TEXT,
    created_at          BIGINT NOT NULL,
    last_used_at        BIGINT,
    expires_at          BIGINT,
    revoked_at          BIGINT
);

CREATE INDEX IF NOT EXISTS local_totp_trusted_devices_user_idx
ON local_totp_trusted_devices(user_id);

CREATE TABLE IF NOT EXISTS local_auth_attempts (
    id                  TEXT PRIMARY KEY,
    username_normalized TEXT NOT NULL,
    ip                  TEXT,
    occurred_at         BIGINT NOT NULL,
    success             BOOLEAN NOT NULL,
    reason              TEXT
);

CREATE INDEX IF NOT EXISTS local_auth_attempts_lookup_idx
ON local_auth_attempts(username_normalized, occurred_at);
