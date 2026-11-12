-- Tanukarr's durable Jellyfin refresh queue (migration 243): imports bump a generation that
-- immediate refreshes and interval ticks acknowledge.
CREATE TABLE jellyfin_refresh_states (
    notification_id      INTEGER PRIMARY KEY REFERENCES notifications (id) ON DELETE CASCADE,
    target_fingerprint   TEXT NOT NULL,
    state_token          TEXT NOT NULL,
    last_refresh_mode    INTEGER NOT NULL DEFAULT 1,
    imported_generation  INTEGER NOT NULL DEFAULT 0,
    immediate_generation INTEGER NOT NULL DEFAULT 0,
    tick_generation      INTEGER NOT NULL DEFAULT 0,
    last_tick_at         TEXT NOT NULL
) STRICT;
