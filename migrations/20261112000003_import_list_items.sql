-- Import list membership (Sonarr's ImportListItems) and sync status (ImportListStatus).
CREATE TABLE import_list_items (
    id             INTEGER PRIMARY KEY,
    import_list_id INTEGER NOT NULL REFERENCES import_lists (id) ON DELETE CASCADE,
    title          TEXT NOT NULL,
    year           INTEGER,
    tvdb_id        INTEGER,
    tmdb_id        INTEGER,
    imdb_id        TEXT,
    mal_id         INTEGER,
    anilist_id     INTEGER
) STRICT;
CREATE INDEX idx_import_list_items_list ON import_list_items (import_list_id);

CREATE TABLE import_list_status (
    import_list_id     INTEGER PRIMARY KEY REFERENCES import_lists (id) ON DELETE CASCADE,
    last_info_sync     TEXT,
    initial_failure    TEXT,
    most_recent_failure TEXT,
    escalation_level   INTEGER NOT NULL DEFAULT 0,
    disabled_till      TEXT,
    last_error         TEXT,
    has_removed_item_since_last_clean INTEGER NOT NULL DEFAULT 0 CHECK (has_removed_item_since_last_clean IN (0, 1))
) STRICT;
