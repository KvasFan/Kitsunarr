-- Tanukarr's companion app keys: scoped, expiring API keys for the Sonarr v3 API. Only the
-- SHA-256 of the token is stored (uppercase hex, as Tanukarr does, so migrated keys still work).
CREATE TABLE companion_keys (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    hash       TEXT NOT NULL UNIQUE,
    permission TEXT NOT NULL CHECK (permission IN ('read-library', 'add-series')),
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
) STRICT;
