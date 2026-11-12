-- Indexer failure back-off (Sonarr's `IndexerStatus`).
CREATE TABLE indexer_status (
    indexer_id          INTEGER PRIMARY KEY REFERENCES indexers (id) ON DELETE CASCADE,
    initial_failure     TEXT,
    most_recent_failure TEXT,
    escalation_level    INTEGER NOT NULL DEFAULT 0,
    disabled_till       TEXT
) STRICT;
