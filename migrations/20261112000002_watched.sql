-- Watched state (Tanukarr's migration 248): set by Jellyfin watched sync or migrated.
ALTER TABLE episodes ADD COLUMN watched INTEGER NOT NULL DEFAULT 0;
ALTER TABLE episodes ADD COLUMN watched_at TEXT;
-- The authority that set it (a Jellyfin connection's fingerprint, or 'migrated').
ALTER TABLE episodes ADD COLUMN watched_source TEXT;
-- Set when watched sync unmonitored the episode, so it may monitor it again.
ALTER TABLE episodes ADD COLUMN jellyfin_unmonitor_owner TEXT;

CREATE TABLE jellyfin_watch_states (
    notification_id            INTEGER PRIMARY KEY REFERENCES notifications (id) ON DELETE CASCADE,
    target_fingerprint         TEXT NOT NULL,
    last_sync_at               TEXT NOT NULL,
    unmonitor_watched_episodes INTEGER NOT NULL DEFAULT 0
) STRICT;
