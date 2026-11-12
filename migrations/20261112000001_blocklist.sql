-- Sonarr's Blocklist: releases that failed or were rejected by hand, never grabbed again.
CREATE TABLE blocklist (
    id                INTEGER PRIMARY KEY,
    series_id         INTEGER NOT NULL REFERENCES series (id) ON DELETE CASCADE,
    episode_ids       TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(episode_ids)),
    source_title      TEXT NOT NULL,
    quality_id        INTEGER,
    revision          TEXT CHECK (revision IS NULL OR json_valid(revision)),
    languages         TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(languages)),
    date              TEXT NOT NULL,
    published_date    TEXT,
    size              INTEGER,
    protocol          TEXT NOT NULL DEFAULT 'unknown',
    indexer           TEXT,
    indexer_flags     INTEGER NOT NULL DEFAULT 0,
    release_type      TEXT,
    message           TEXT,
    source            TEXT,
    torrent_info_hash TEXT
) STRICT;
CREATE INDEX idx_blocklist_series_title ON blocklist (series_id, source_title);
CREATE INDEX idx_blocklist_hash ON blocklist (series_id, torrent_info_hash);
CREATE INDEX idx_blocklist_date ON blocklist (date);
