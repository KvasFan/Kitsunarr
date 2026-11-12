-- Extra files imported with episodes (Sonarr's `SubtitleFiles` and `ExtraFiles`).
CREATE TABLE extra_files (
    id              INTEGER PRIMARY KEY,
    series_id       INTEGER NOT NULL REFERENCES series (id) ON DELETE CASCADE,
    episode_file_id INTEGER REFERENCES episode_files (id) ON DELETE SET NULL,
    season_number   INTEGER NOT NULL DEFAULT 0,
    relative_path   TEXT NOT NULL,
    extension       TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('subtitle', 'other')),
    language        INTEGER,
    language_tags   TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(language_tags)),
    title           TEXT,
    added_at        TEXT NOT NULL
) STRICT;
CREATE INDEX idx_extra_files_episode_file ON extra_files (episode_file_id);
CREATE INDEX idx_extra_files_series ON extra_files (series_id);
