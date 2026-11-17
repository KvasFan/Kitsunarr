-- Notification failure back-off (Sonarr's `NotificationStatus`).
CREATE TABLE notification_status (
    notification_id     INTEGER PRIMARY KEY REFERENCES notifications (id) ON DELETE CASCADE,
    initial_failure     TEXT,
    most_recent_failure TEXT,
    escalation_level    INTEGER NOT NULL DEFAULT 0,
    disabled_till       TEXT
) STRICT;
