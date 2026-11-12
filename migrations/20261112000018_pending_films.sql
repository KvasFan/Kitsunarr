-- Pending releases of films (Tanukarr's films go through the same delay profiles): a held
-- release belongs to a series or to a film.
CREATE TABLE pending_releases_new (
    id          INTEGER PRIMARY KEY,
    series_id   INTEGER REFERENCES series (id) ON DELETE CASCADE,
    movie_id    INTEGER REFERENCES movies (id) ON DELETE CASCADE,
    indexer_id  INTEGER NOT NULL,
    guid        TEXT NOT NULL,
    title       TEXT NOT NULL,
    release     TEXT NOT NULL CHECK (json_valid(release)),
    episode_ids TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(episode_ids)),
    added_at    TEXT NOT NULL,
    UNIQUE (indexer_id, guid),
    CHECK ((series_id IS NULL) <> (movie_id IS NULL))
) STRICT;
INSERT INTO pending_releases_new (id, series_id, indexer_id, guid, title, release, episode_ids, added_at)
    SELECT id, series_id, indexer_id, guid, title, release, episode_ids, added_at FROM pending_releases;
DROP TABLE pending_releases;
ALTER TABLE pending_releases_new RENAME TO pending_releases;
