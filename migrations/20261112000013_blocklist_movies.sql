-- Films can be blocklisted too (Tanukarr's films are series there): an entry belongs to a
-- series or to a film.
CREATE TABLE blocklist_new (
    id                INTEGER PRIMARY KEY,
    series_id         INTEGER REFERENCES series (id) ON DELETE CASCADE,
    movie_id          INTEGER REFERENCES movies (id) ON DELETE CASCADE,
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
    torrent_info_hash TEXT,
    CHECK ((series_id IS NULL) <> (movie_id IS NULL))
) STRICT;
INSERT INTO blocklist_new (id, series_id, episode_ids, source_title, quality_id, revision, languages, date,
    published_date, size, protocol, indexer, indexer_flags, release_type, message, source, torrent_info_hash)
SELECT id, series_id, episode_ids, source_title, quality_id, revision, languages, date,
    published_date, size, protocol, indexer, indexer_flags, release_type, message, source, torrent_info_hash
FROM blocklist;
DROP TABLE blocklist;
ALTER TABLE blocklist_new RENAME TO blocklist;
CREATE INDEX idx_blocklist_series_title ON blocklist (series_id, source_title);
CREATE INDEX idx_blocklist_hash ON blocklist (series_id, torrent_info_hash);
CREATE INDEX idx_blocklist_movie ON blocklist (movie_id);
CREATE INDEX idx_blocklist_date ON blocklist (date);
