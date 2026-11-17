-- Browser sessions signed in through an OpenID Connect provider (Settings › General ›
-- Security › Authentication: OIDC). They belong to the configured OIDC user rather than to a
-- local account, so they live apart from `sessions`.
CREATE TABLE sso_sessions (
    id           INTEGER PRIMARY KEY,
    token_hash   TEXT NOT NULL UNIQUE,
    -- The configured OIDC user (verified e-mail or subject) the provider's identity matched.
    subject      TEXT NOT NULL,
    -- The provider's ID token, sent back as `id_token_hint` when signing out.
    id_token     TEXT,
    created_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    user_agent   TEXT,
    remote_addr  TEXT
) STRICT;
CREATE INDEX idx_sso_sessions_expires ON sso_sessions (expires_at);
