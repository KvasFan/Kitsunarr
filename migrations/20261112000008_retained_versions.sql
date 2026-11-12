-- Tanukarr's retained episode versions (migration 239): files replaced by upgrades kept under
-- the series folder's "Plex Versions", with a snapshot of their record. Rows go with the series.
ALTER TABLE series ADD COLUMN keep_versions INTEGER NOT NULL DEFAULT 0 CHECK (keep_versions IN (0, 1));
CREATE TABLE retained_versions (
    id            INTEGER PRIMARY KEY,
    series_id     INTEGER NOT NULL REFERENCES series (id) ON DELETE CASCADE,
    relative_path TEXT NOT NULL,
    archive_path  TEXT NOT NULL,
    metadata      TEXT NOT NULL CHECK (json_valid(metadata)),
    episode_ids   TEXT NOT NULL CHECK (json_valid(episode_ids)),
    created_at    TEXT NOT NULL
) STRICT;
CREATE INDEX idx_retained_versions_series ON retained_versions (series_id);
