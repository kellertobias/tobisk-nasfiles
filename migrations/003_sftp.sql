ALTER TABLE users ADD COLUMN folder_permissions_json TEXT;
ALTER TABLE users ADD COLUMN has_home BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE IF NOT EXISTS user_public_keys (
    id                  TEXT PRIMARY KEY,
    user_id             TEXT NOT NULL REFERENCES users(id),
    key_fingerprint     TEXT NOT NULL UNIQUE,
    public_key          TEXT NOT NULL,
    label               TEXT,
    created_at          BIGINT NOT NULL,
    last_used_at        BIGINT,
    revoked_at          BIGINT
);

CREATE INDEX IF NOT EXISTS user_public_keys_user_idx ON user_public_keys(user_id);

CREATE TABLE IF NOT EXISTS sftp_temp_users (
    id                  TEXT PRIMARY KEY,
    created_by_user_id  TEXT NOT NULL REFERENCES users(id),
    display_name        TEXT NOT NULL,
    root_kind           TEXT NOT NULL,
    root_key            TEXT NOT NULL,
    relative_path       TEXT NOT NULL,
    can_write           BOOLEAN NOT NULL DEFAULT FALSE,
    expires_at          BIGINT NOT NULL,
    revoked_at          BIGINT,
    created_at          BIGINT NOT NULL,
    restored_from_id    TEXT
);

CREATE INDEX IF NOT EXISTS sftp_temp_users_creator_idx ON sftp_temp_users(created_by_user_id);
CREATE INDEX IF NOT EXISTS sftp_temp_users_expiry_idx ON sftp_temp_users(expires_at);

CREATE TABLE IF NOT EXISTS sftp_temp_user_keys (
    id                  TEXT PRIMARY KEY,
    temp_user_id        TEXT NOT NULL REFERENCES sftp_temp_users(id),
    key_fingerprint     TEXT NOT NULL UNIQUE,
    public_key          TEXT NOT NULL,
    created_at          BIGINT NOT NULL,
    last_used_at        BIGINT,
    revoked_at          BIGINT
);

CREATE INDEX IF NOT EXISTS sftp_temp_user_keys_temp_user_idx ON sftp_temp_user_keys(temp_user_id);

CREATE TABLE IF NOT EXISTS sftp_access_log (
    id                  TEXT PRIMARY KEY,
    principal_kind      TEXT NOT NULL,
    principal_id        TEXT NOT NULL,
    occurred_at         BIGINT NOT NULL,
    action              TEXT NOT NULL,
    root_key            TEXT,
    path                TEXT,
    ip                  TEXT,
    success             BOOLEAN NOT NULL,
    error               TEXT
);

CREATE INDEX IF NOT EXISTS sftp_access_log_principal_idx ON sftp_access_log(principal_kind, principal_id, occurred_at);
