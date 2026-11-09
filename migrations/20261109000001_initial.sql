-- Kitsunarr initial schema.
--
-- Conventions
--   * timestamps: RFC 3339 UTC text (`2026-11-09T17:34:40Z`); local air dates: `YYYY-MM-DD`.
--   * booleans: INTEGER 0/1 with CHECK constraints.
--   * small denormalised lists (provider tags, genres, images) are JSON text.
--   * media <-> tag links use join tables so filtering stays indexable.
--   * quality ids follow the Sonarr v4 catalogue (see kitsunarr-domain::quality).

CREATE TABLE settings (
    key        TEXT PRIMARY KEY NOT NULL,
    value      TEXT NOT NULL CHECK (json_valid(value)),
    updated_at TEXT NOT NULL
) STRICT;

-- ---------------------------------------------------------------------------------------------
-- Authentication
-- ---------------------------------------------------------------------------------------------
CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT NOT NULL COLLATE NOCASE UNIQUE,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL DEFAULT 'admin' CHECK (role IN ('admin')),
    created_at    TEXT NOT NULL,
    last_login_at TEXT
) STRICT;

CREATE TABLE sessions (
    id           INTEGER PRIMARY KEY,
    token_hash   TEXT NOT NULL UNIQUE,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    user_agent   TEXT,
    remote_addr  TEXT
) STRICT;
CREATE INDEX idx_sessions_user ON sessions (user_id);
CREATE INDEX idx_sessions_expires ON sessions (expires_at);

-- ---------------------------------------------------------------------------------------------
-- Shared configuration entities
-- ---------------------------------------------------------------------------------------------
CREATE TABLE tags (
    id    INTEGER PRIMARY KEY,
    label TEXT NOT NULL COLLATE NOCASE UNIQUE
) STRICT;

CREATE TABLE root_folders (
    id         INTEGER PRIMARY KEY,
    path       TEXT NOT NULL UNIQUE,
    media_kind TEXT CHECK (media_kind IN ('series', 'anime', 'movie')),
    created_at TEXT NOT NULL
) STRICT;

-- Size limits are megabytes per minute of runtime; NULL max = unlimited.
CREATE TABLE quality_definitions (
    quality_id     INTEGER PRIMARY KEY,
    title          TEXT NOT NULL,
    min_size       REAL,
    preferred_size REAL,
    max_size       REAL
) STRICT;

CREATE TABLE quality_profiles (
    id                       INTEGER PRIMARY KEY,
    name                     TEXT NOT NULL COLLATE NOCASE UNIQUE,
    upgrade_allowed          INTEGER NOT NULL DEFAULT 1 CHECK (upgrade_allowed IN (0, 1)),
    cutoff                   INTEGER NOT NULL,
    items                    TEXT NOT NULL CHECK (json_valid(items)),
    min_format_score         INTEGER NOT NULL DEFAULT 0,
    cutoff_format_score      INTEGER NOT NULL DEFAULT 0,
    min_upgrade_format_score INTEGER NOT NULL DEFAULT 1,
    format_items             TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(format_items))
) STRICT;

CREATE TABLE custom_formats (
    id                    INTEGER PRIMARY KEY,
    name                  TEXT NOT NULL COLLATE NOCASE UNIQUE,
    include_when_renaming INTEGER NOT NULL DEFAULT 0 CHECK (include_when_renaming IN (0, 1)),
    specifications        TEXT NOT NULL CHECK (json_valid(specifications))
) STRICT;

CREATE TABLE delay_profiles (
    id                                  INTEGER PRIMARY KEY,
    enable_usenet                       INTEGER NOT NULL DEFAULT 1 CHECK (enable_usenet IN (0, 1)),
    enable_torrent                      INTEGER NOT NULL DEFAULT 1 CHECK (enable_torrent IN (0, 1)),
    preferred_protocol                  TEXT NOT NULL DEFAULT 'usenet' CHECK (preferred_protocol IN ('usenet', 'torrent')),
    usenet_delay                        INTEGER NOT NULL DEFAULT 0,
    torrent_delay                       INTEGER NOT NULL DEFAULT 0,
    bypass_if_highest_quality           INTEGER NOT NULL DEFAULT 1 CHECK (bypass_if_highest_quality IN (0, 1)),
    bypass_if_above_custom_format_score INTEGER NOT NULL DEFAULT 0 CHECK (bypass_if_above_custom_format_score IN (0, 1)),
    minimum_custom_format_score         INTEGER NOT NULL DEFAULT 0,
    sort_order                          INTEGER NOT NULL,
    tags                                TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(tags))
) STRICT;

CREATE TABLE release_profiles (
    id         INTEGER PRIMARY KEY,
    name       TEXT,
    enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    required   TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(required)),
    ignored    TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(ignored)),
    -- Empty list = all indexers.
    indexer_ids TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(indexer_ids)),
    tags       TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(tags))
) STRICT;

-- Providers keep implementation-specific settings verbatim as JSON so that settings of
-- implementations Kitsunarr does not run natively yet are preserved, never dropped.
CREATE TABLE download_clients (
    id                         INTEGER PRIMARY KEY,
    name                       TEXT NOT NULL COLLATE NOCASE UNIQUE,
    implementation             TEXT NOT NULL,
    protocol                   TEXT NOT NULL CHECK (protocol IN ('usenet', 'torrent', 'unknown')),
    settings                   TEXT NOT NULL CHECK (json_valid(settings)),
    enabled                    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    priority                   INTEGER NOT NULL DEFAULT 1,
    remove_completed_downloads INTEGER NOT NULL DEFAULT 1 CHECK (remove_completed_downloads IN (0, 1)),
    remove_failed_downloads    INTEGER NOT NULL DEFAULT 1 CHECK (remove_failed_downloads IN (0, 1)),
    tags                       TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(tags)),
    created_at                 TEXT NOT NULL
) STRICT;

CREATE TABLE indexers (
    id                                       INTEGER PRIMARY KEY,
    name                                     TEXT NOT NULL COLLATE NOCASE UNIQUE,
    implementation                           TEXT NOT NULL,
    protocol                                 TEXT NOT NULL CHECK (protocol IN ('usenet', 'torrent', 'unknown')),
    settings                                 TEXT NOT NULL CHECK (json_valid(settings)),
    enable_rss                               INTEGER NOT NULL DEFAULT 1 CHECK (enable_rss IN (0, 1)),
    enable_automatic_search                  INTEGER NOT NULL DEFAULT 1 CHECK (enable_automatic_search IN (0, 1)),
    enable_interactive_search                INTEGER NOT NULL DEFAULT 1 CHECK (enable_interactive_search IN (0, 1)),
    priority                                 INTEGER NOT NULL DEFAULT 25,
    download_client_id                       INTEGER REFERENCES download_clients (id) ON DELETE SET NULL,
    season_search_maximum_single_episode_age INTEGER NOT NULL DEFAULT 0,
    tags                                     TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(tags)),
    created_at                               TEXT NOT NULL
) STRICT;

-- `triggers` holds the event switches (onGrab, onDownload, onUpgrade, ...).
CREATE TABLE notifications (
    id             INTEGER PRIMARY KEY,
    name           TEXT NOT NULL COLLATE NOCASE UNIQUE,
    implementation TEXT NOT NULL,
    settings       TEXT NOT NULL CHECK (json_valid(settings)),
    triggers       TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(triggers)),
    tags           TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(tags)),
    created_at     TEXT NOT NULL
) STRICT;

-- `options` holds list behaviour (enableAutomaticAdd, shouldMonitor, rootFolderPath, ...).
CREATE TABLE import_lists (
    id             INTEGER PRIMARY KEY,
    name           TEXT NOT NULL COLLATE NOCASE UNIQUE,
    implementation TEXT NOT NULL,
    settings       TEXT NOT NULL CHECK (json_valid(settings)),
    options        TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(options)),
    enabled        INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    tags           TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(tags)),
    created_at     TEXT NOT NULL
) STRICT;

CREATE TABLE import_list_exclusions (
    id          INTEGER PRIMARY KEY,
    provider    TEXT NOT NULL,
    external_id TEXT NOT NULL,
    title       TEXT,
    UNIQUE (provider, external_id)
) STRICT;

CREATE TABLE remote_path_mappings (
    id          INTEGER PRIMARY KEY,
    host        TEXT NOT NULL,
    remote_path TEXT NOT NULL,
    local_path  TEXT NOT NULL,
    UNIQUE (host, remote_path)
) STRICT;

-- ---------------------------------------------------------------------------------------------
-- Library: series (TV and anime)
-- ---------------------------------------------------------------------------------------------
CREATE TABLE series (
    id                 INTEGER PRIMARY KEY,
    media_kind         TEXT NOT NULL CHECK (media_kind IN ('series', 'anime')),
    title              TEXT NOT NULL,
    sort_title         TEXT NOT NULL,
    clean_title        TEXT NOT NULL,
    title_slug         TEXT NOT NULL,
    year               INTEGER,
    status             TEXT NOT NULL DEFAULT 'unknown',
    overview           TEXT,
    network            TEXT,
    air_time           TEXT,
    runtime            INTEGER,
    certification      TEXT,
    original_language  INTEGER,
    genres             TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(genres)),
    images             TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(images)),
    ratings            TEXT CHECK (ratings IS NULL OR json_valid(ratings)),
    first_aired        TEXT,
    last_aired         TEXT,
    series_type        TEXT NOT NULL DEFAULT 'standard' CHECK (series_type IN ('standard', 'daily', 'anime')),
    -- Owning metadata catalogue and its native identifier (provider-specific identity).
    metadata_provider  TEXT NOT NULL,
    metadata_id        TEXT NOT NULL,
    episode_ordering   TEXT,
    monitored          INTEGER NOT NULL DEFAULT 1 CHECK (monitored IN (0, 1)),
    monitor_new_items  TEXT NOT NULL DEFAULT 'all' CHECK (monitor_new_items IN ('all', 'none')),
    season_folder      INTEGER NOT NULL DEFAULT 1 CHECK (season_folder IN (0, 1)),
    use_scene_numbering INTEGER NOT NULL DEFAULT 0 CHECK (use_scene_numbering IN (0, 1)),
    quality_profile_id INTEGER NOT NULL REFERENCES quality_profiles (id) ON DELETE RESTRICT,
    root_folder_path   TEXT NOT NULL,
    path               TEXT NOT NULL UNIQUE,
    added_at           TEXT NOT NULL,
    last_refreshed_at  TEXT,
    last_disk_scan_at  TEXT,
    UNIQUE (metadata_provider, metadata_id)
) STRICT;
CREATE INDEX idx_series_kind_sort ON series (media_kind, sort_title);
CREATE INDEX idx_series_clean_title ON series (clean_title);
CREATE INDEX idx_series_profile ON series (quality_profile_id);

CREATE TABLE series_external_ids (
    series_id   INTEGER NOT NULL REFERENCES series (id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    external_id TEXT NOT NULL,
    PRIMARY KEY (series_id, kind, external_id)
) STRICT;
CREATE INDEX idx_series_external_lookup ON series_external_ids (kind, external_id);

CREATE TABLE series_alternate_titles (
    series_id     INTEGER NOT NULL REFERENCES series (id) ON DELETE CASCADE,
    title         TEXT NOT NULL,
    clean_title   TEXT NOT NULL,
    season_number INTEGER,
    source        TEXT,
    PRIMARY KEY (series_id, title)
) STRICT;
CREATE INDEX idx_series_alt_clean ON series_alternate_titles (clean_title);

CREATE TABLE series_tags (
    series_id INTEGER NOT NULL REFERENCES series (id) ON DELETE CASCADE,
    tag_id    INTEGER NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    PRIMARY KEY (series_id, tag_id)
) STRICT;
CREATE INDEX idx_series_tags_tag ON series_tags (tag_id);

CREATE TABLE seasons (
    series_id     INTEGER NOT NULL REFERENCES series (id) ON DELETE CASCADE,
    season_number INTEGER NOT NULL,
    monitored     INTEGER NOT NULL DEFAULT 1 CHECK (monitored IN (0, 1)),
    title         TEXT,
    images        TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(images)),
    PRIMARY KEY (series_id, season_number)
) STRICT;

CREATE TABLE episode_files (
    id               INTEGER PRIMARY KEY,
    series_id        INTEGER NOT NULL REFERENCES series (id) ON DELETE CASCADE,
    season_number    INTEGER NOT NULL,
    relative_path    TEXT NOT NULL,
    size             INTEGER NOT NULL DEFAULT 0,
    added_at         TEXT NOT NULL,
    scene_name       TEXT,
    release_group    TEXT,
    quality_id       INTEGER NOT NULL DEFAULT 0,
    revision_version INTEGER NOT NULL DEFAULT 1,
    revision_real    INTEGER NOT NULL DEFAULT 0,
    is_repack        INTEGER NOT NULL DEFAULT 0 CHECK (is_repack IN (0, 1)),
    languages        TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(languages)),
    media_info       TEXT CHECK (media_info IS NULL OR json_valid(media_info)),
    original_path    TEXT,
    indexer_flags    INTEGER NOT NULL DEFAULT 0,
    release_type     INTEGER NOT NULL DEFAULT 0,
    UNIQUE (series_id, relative_path)
) STRICT;

CREATE TABLE episodes (
    id                            INTEGER PRIMARY KEY,
    series_id                     INTEGER NOT NULL REFERENCES series (id) ON DELETE CASCADE,
    season_number                 INTEGER NOT NULL,
    episode_number                INTEGER NOT NULL,
    absolute_episode_number       INTEGER,
    scene_season_number           INTEGER,
    scene_episode_number          INTEGER,
    scene_absolute_episode_number INTEGER,
    unverified_scene_numbering    INTEGER NOT NULL DEFAULT 0 CHECK (unverified_scene_numbering IN (0, 1)),
    title                         TEXT,
    overview                      TEXT,
    air_date                      TEXT,
    air_date_utc                  TEXT,
    runtime                       INTEGER,
    monitored                     INTEGER NOT NULL DEFAULT 1 CHECK (monitored IN (0, 1)),
    episode_file_id               INTEGER REFERENCES episode_files (id) ON DELETE SET NULL,
    provider_episode_id           TEXT,
    last_search_at                TEXT,
    UNIQUE (series_id, season_number, episode_number)
) STRICT;
CREATE INDEX idx_episodes_air ON episodes (air_date_utc);
CREATE INDEX idx_episodes_file ON episodes (episode_file_id);
CREATE INDEX idx_episodes_absolute ON episodes (series_id, absolute_episode_number);
-- Fast "monitored and missing" scans.
CREATE INDEX idx_episodes_missing ON episodes (series_id, monitored) WHERE episode_file_id IS NULL;

-- ---------------------------------------------------------------------------------------------
-- Library: movies
-- ---------------------------------------------------------------------------------------------
CREATE TABLE movies (
    id                   INTEGER PRIMARY KEY,
    title                TEXT NOT NULL,
    sort_title           TEXT NOT NULL,
    clean_title          TEXT NOT NULL,
    title_slug           TEXT NOT NULL,
    original_title       TEXT,
    year                 INTEGER,
    status               TEXT NOT NULL DEFAULT 'unknown',
    overview             TEXT,
    studio               TEXT,
    runtime              INTEGER,
    certification        TEXT,
    original_language    INTEGER,
    genres               TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(genres)),
    images               TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(images)),
    ratings              TEXT CHECK (ratings IS NULL OR json_valid(ratings)),
    in_cinemas           TEXT,
    digital_release      TEXT,
    physical_release     TEXT,
    metadata_provider    TEXT NOT NULL,
    metadata_id          TEXT NOT NULL,
    monitored            INTEGER NOT NULL DEFAULT 1 CHECK (monitored IN (0, 1)),
    minimum_availability TEXT NOT NULL DEFAULT 'released' CHECK (minimum_availability IN ('announced', 'inCinemas', 'released')),
    quality_profile_id   INTEGER NOT NULL REFERENCES quality_profiles (id) ON DELETE RESTRICT,
    root_folder_path     TEXT NOT NULL,
    path                 TEXT NOT NULL UNIQUE,
    added_at             TEXT NOT NULL,
    last_refreshed_at    TEXT,
    last_search_at       TEXT,
    UNIQUE (metadata_provider, metadata_id)
) STRICT;
CREATE INDEX idx_movies_sort ON movies (sort_title);
CREATE INDEX idx_movies_clean_title ON movies (clean_title);
CREATE INDEX idx_movies_profile ON movies (quality_profile_id);

-- One file per movie; deleting the movie removes the file record (never the file on disk).
CREATE TABLE movie_files (
    id               INTEGER PRIMARY KEY,
    movie_id         INTEGER NOT NULL UNIQUE REFERENCES movies (id) ON DELETE CASCADE,
    relative_path    TEXT NOT NULL,
    size             INTEGER NOT NULL DEFAULT 0,
    added_at         TEXT NOT NULL,
    scene_name       TEXT,
    release_group    TEXT,
    quality_id       INTEGER NOT NULL DEFAULT 0,
    revision_version INTEGER NOT NULL DEFAULT 1,
    revision_real    INTEGER NOT NULL DEFAULT 0,
    is_repack        INTEGER NOT NULL DEFAULT 0 CHECK (is_repack IN (0, 1)),
    languages        TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(languages)),
    media_info       TEXT CHECK (media_info IS NULL OR json_valid(media_info)),
    original_path    TEXT,
    edition          TEXT
) STRICT;

CREATE TABLE movie_external_ids (
    movie_id    INTEGER NOT NULL REFERENCES movies (id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    external_id TEXT NOT NULL,
    PRIMARY KEY (movie_id, kind, external_id)
) STRICT;
CREATE INDEX idx_movie_external_lookup ON movie_external_ids (kind, external_id);

CREATE TABLE movie_tags (
    movie_id INTEGER NOT NULL REFERENCES movies (id) ON DELETE CASCADE,
    tag_id   INTEGER NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    PRIMARY KEY (movie_id, tag_id)
) STRICT;
CREATE INDEX idx_movie_tags_tag ON movie_tags (tag_id);

-- ---------------------------------------------------------------------------------------------
-- Collections (provider collections, anime groups and manual groups of series and movies)
-- ---------------------------------------------------------------------------------------------
CREATE TABLE collections (
    id                INTEGER PRIMARY KEY,
    title             TEXT NOT NULL,
    sort_title        TEXT NOT NULL,
    kind              TEXT NOT NULL DEFAULT 'manual' CHECK (kind IN ('manual', 'provider', 'animeGroup')),
    metadata_provider TEXT,
    metadata_id       TEXT,
    overview          TEXT,
    images            TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(images)),
    created_at        TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX idx_collections_provider ON collections (metadata_provider, metadata_id)
    WHERE metadata_provider IS NOT NULL;

CREATE TABLE collection_members (
    id            INTEGER PRIMARY KEY,
    collection_id INTEGER NOT NULL REFERENCES collections (id) ON DELETE CASCADE,
    position      INTEGER NOT NULL,
    series_id     INTEGER REFERENCES series (id) ON DELETE CASCADE,
    movie_id      INTEGER REFERENCES movies (id) ON DELETE CASCADE,
    role          TEXT,
    label         TEXT,
    CHECK ((series_id IS NULL) <> (movie_id IS NULL))
) STRICT;
CREATE INDEX idx_collection_members_collection ON collection_members (collection_id, position);
CREATE UNIQUE INDEX idx_collection_members_series ON collection_members (collection_id, series_id) WHERE series_id IS NOT NULL;
CREATE UNIQUE INDEX idx_collection_members_movie ON collection_members (collection_id, movie_id) WHERE movie_id IS NOT NULL;

-- ---------------------------------------------------------------------------------------------
-- Activity
-- ---------------------------------------------------------------------------------------------
CREATE TABLE history (
    id                  INTEGER PRIMARY KEY,
    event_type          TEXT NOT NULL,
    date                TEXT NOT NULL,
    source_title        TEXT NOT NULL,
    series_id           INTEGER REFERENCES series (id) ON DELETE CASCADE,
    episode_id          INTEGER REFERENCES episodes (id) ON DELETE CASCADE,
    movie_id            INTEGER REFERENCES movies (id) ON DELETE CASCADE,
    quality_id          INTEGER,
    revision            TEXT CHECK (revision IS NULL OR json_valid(revision)),
    languages           TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(languages)),
    custom_format_score INTEGER,
    download_id         TEXT,
    data                TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(data))
) STRICT;
CREATE INDEX idx_history_date ON history (date);
CREATE INDEX idx_history_series ON history (series_id, date);
CREATE INDEX idx_history_movie ON history (movie_id, date);
CREATE INDEX idx_history_download ON history (download_id);

-- ---------------------------------------------------------------------------------------------
-- Scheduler
-- ---------------------------------------------------------------------------------------------
CREATE TABLE scheduled_tasks (
    name             TEXT PRIMARY KEY NOT NULL,
    interval_minutes INTEGER NOT NULL,
    enabled          INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    last_run_at      TEXT,
    last_success_at  TEXT,
    last_duration_ms INTEGER,
    last_status      TEXT
) STRICT;

CREATE TABLE task_runs (
    id          INTEGER PRIMARY KEY,
    task_name   TEXT NOT NULL,
    trigger     TEXT NOT NULL CHECK (trigger IN ('scheduled', 'manual', 'startup')),
    status      TEXT NOT NULL CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
    started_at  TEXT NOT NULL,
    finished_at TEXT,
    progress    REAL,
    message     TEXT,
    error       TEXT
) STRICT;
CREATE INDEX idx_task_runs_started ON task_runs (started_at);
CREATE INDEX idx_task_runs_task ON task_runs (task_name, started_at);

-- ---------------------------------------------------------------------------------------------
-- Migration bookkeeping (Sonarr / Tanukarr imports)
-- ---------------------------------------------------------------------------------------------
CREATE TABLE migration_runs (
    id                 INTEGER PRIMARY KEY,
    source_app         TEXT NOT NULL CHECK (source_app IN ('sonarr', 'tanukarr')),
    source_method      TEXT NOT NULL CHECK (source_method IN ('api', 'backup')),
    source_version     TEXT NOT NULL,
    source_instance    TEXT,
    source_fingerprint TEXT NOT NULL,
    status             TEXT NOT NULL CHECK (status IN ('running', 'completed', 'failed')),
    options            TEXT NOT NULL CHECK (json_valid(options)),
    summary            TEXT CHECK (summary IS NULL OR json_valid(summary)),
    report             TEXT CHECK (report IS NULL OR json_valid(report)),
    backup_path        TEXT,
    started_at         TEXT NOT NULL,
    finished_at        TEXT,
    error              TEXT
) STRICT;

-- Source record → Kitsunarr record, so re-running a migration updates instead of duplicating.
CREATE TABLE migration_entity_map (
    source_fingerprint TEXT NOT NULL,
    entity             TEXT NOT NULL,
    source_id          TEXT NOT NULL,
    target_id          INTEGER NOT NULL,
    run_id             INTEGER NOT NULL REFERENCES migration_runs (id) ON DELETE CASCADE,
    PRIMARY KEY (source_fingerprint, entity, source_id)
) STRICT;

-- Source values Kitsunarr could not map onto its model; kept so nothing is silently lost.
CREATE TABLE migration_unmapped (
    id        INTEGER PRIMARY KEY,
    run_id    INTEGER NOT NULL REFERENCES migration_runs (id) ON DELETE CASCADE,
    entity    TEXT NOT NULL,
    source_id TEXT,
    field     TEXT NOT NULL,
    value     TEXT NOT NULL CHECK (json_valid(value))
) STRICT;
CREATE INDEX idx_migration_unmapped_run ON migration_unmapped (run_id);
