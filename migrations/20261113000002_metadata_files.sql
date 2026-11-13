-- Files written by Sonarr's metadata consumers (`MetadataFiles`), and the episode screenshot
-- their episode thumbnails come from.
CREATE TABLE metadata_files (
    id              INTEGER PRIMARY KEY,
    series_id       INTEGER NOT NULL REFERENCES series (id) ON DELETE CASCADE,
    season_number   INTEGER,
    episode_file_id INTEGER REFERENCES episode_files (id) ON DELETE CASCADE,
    -- Sonarr's consumer type name (`XbmcMetadata`, `RoksboxMetadata`, `WdtvMetadata`, `PlexMetadata`).
    consumer        TEXT NOT NULL,
    -- Sonarr's `MetadataType`: 1 series metadata, 2 episode metadata, 3 series image,
    -- 4 season image, 5 episode image.
    type            INTEGER NOT NULL CHECK (type BETWEEN 1 AND 5),
    relative_path   TEXT NOT NULL,
    extension       TEXT NOT NULL,
    hash            TEXT,
    added_at        TEXT NOT NULL,
    last_updated    TEXT NOT NULL
) STRICT;
CREATE INDEX idx_metadata_files_series ON metadata_files (series_id);
CREATE INDEX idx_metadata_files_episode_file ON metadata_files (episode_file_id);

ALTER TABLE episodes ADD COLUMN screenshot TEXT;
