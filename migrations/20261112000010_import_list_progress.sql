-- Watched progress sent to import lists (Tanukarr's `ImportListProgressStates`, migration 249).
CREATE TABLE import_list_progress_states (
    id                    INTEGER PRIMARY KEY,
    import_list_id        INTEGER NOT NULL REFERENCES import_lists (id) ON DELETE CASCADE,
    series_id             INTEGER NOT NULL REFERENCES series (id) ON DELETE CASCADE,
    target_fingerprint    TEXT NOT NULL,
    pending_progress      INTEGER NOT NULL DEFAULT 0,
    acknowledged_progress INTEGER NOT NULL DEFAULT 0,
    acknowledged_identity TEXT NOT NULL DEFAULT '',
    failure_count         INTEGER NOT NULL DEFAULT 0,
    next_attempt_at       TEXT NOT NULL,
    last_attempt_at       TEXT,
    last_error            TEXT,
    UNIQUE (import_list_id, series_id)
) STRICT;
