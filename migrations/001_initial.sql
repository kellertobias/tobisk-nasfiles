-- nasfiles initial schema
-- Compatible with both SQLite and PostgreSQL

CREATE TABLE IF NOT EXISTS users (
    id              TEXT PRIMARY KEY,
    external_id     TEXT NOT NULL UNIQUE,
    username        TEXT NOT NULL UNIQUE,
    display_name    TEXT NOT NULL,
    picture_url     TEXT,
    is_admin        BOOLEAN NOT NULL DEFAULT FALSE,
    oidc_access_token TEXT,
    oidc_refresh_token TEXT,
    created_at      BIGINT NOT NULL,
    last_login_at   BIGINT NOT NULL
);

ALTER TABLE users ADD COLUMN oidc_access_token TEXT;
ALTER TABLE users ADD COLUMN oidc_refresh_token TEXT;

CREATE TABLE IF NOT EXISTS shares (
    id              TEXT PRIMARY KEY,
    token_hash      TEXT NOT NULL UNIQUE,
    owner_user_id   TEXT NOT NULL REFERENCES users(id),
    root_kind       TEXT NOT NULL,
    root_key        TEXT NOT NULL,
    relative_path   TEXT NOT NULL,
    is_directory    BOOLEAN NOT NULL,
    target_kind     TEXT NOT NULL,
    target_user_id  TEXT REFERENCES users(id),
    password_hash   TEXT,
    allow_upload    BOOLEAN NOT NULL DEFAULT FALSE,
    allow_download  BOOLEAN NOT NULL DEFAULT TRUE,
    expires_at      BIGINT,
    created_at      BIGINT NOT NULL,
    revoked_at      BIGINT
);

CREATE INDEX IF NOT EXISTS shares_owner_idx ON shares(owner_user_id);
CREATE INDEX IF NOT EXISTS shares_token_idx ON shares(token_hash);

CREATE TABLE IF NOT EXISTS share_access_log (
    id              TEXT PRIMARY KEY,
    share_id        TEXT NOT NULL REFERENCES shares(id),
    occurred_at     BIGINT NOT NULL,
    ip              TEXT,
    user_agent      TEXT,
    action          TEXT NOT NULL,
    path            TEXT
);

CREATE INDEX IF NOT EXISTS sal_share_idx ON share_access_log(share_id, occurred_at)
