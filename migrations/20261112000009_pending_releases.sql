-- Sonarr's pending releases: releases a delay profile holds back, decided again by each RSS sync
-- until the delay is over (or they are no longer wanted).
CREATE TABLE pending_releases (
    id          INTEGER PRIMARY KEY,
    series_id   INTEGER NOT NULL REFERENCES series (id) ON DELETE CASCADE,
    indexer_id  INTEGER NOT NULL,
    guid        TEXT NOT NULL,
    title       TEXT NOT NULL,
    release     TEXT NOT NULL CHECK (json_valid(release)),
    episode_ids TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(episode_ids)),
    added_at    TEXT NOT NULL,
    UNIQUE (indexer_id, guid)
) STRICT;
