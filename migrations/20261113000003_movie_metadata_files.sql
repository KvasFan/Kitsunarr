-- Files the Kodi metadata consumer wrote for a film (`movie.nfo`, poster, fanart).
CREATE TABLE movie_metadata_files (
    id            INTEGER PRIMARY KEY,
    movie_id      INTEGER NOT NULL REFERENCES movies (id) ON DELETE CASCADE,
    consumer      TEXT NOT NULL,
    -- 1 film metadata, 3 film image.
    type          INTEGER NOT NULL CHECK (type IN (1, 3)),
    relative_path TEXT NOT NULL,
    hash          TEXT,
    added_at      TEXT NOT NULL,
    last_updated  TEXT NOT NULL,
    UNIQUE (movie_id, relative_path)
) STRICT;
