-- Download client failure back-off (Sonarr's `DownloadClientStatus`).
CREATE TABLE download_client_status (
    download_client_id  INTEGER PRIMARY KEY REFERENCES download_clients (id) ON DELETE CASCADE,
    initial_failure     TEXT,
    most_recent_failure TEXT,
    escalation_level    INTEGER NOT NULL DEFAULT 0,
    disabled_till       TEXT
) STRICT;
