-- Tanukarr stores film progress on the one feature episode. Native movies need the same
-- watched/source and automatic-unmonitor ownership fields as episodes.
ALTER TABLE movies ADD COLUMN watched INTEGER NOT NULL DEFAULT 0 CHECK (watched IN (0, 1));
ALTER TABLE movies ADD COLUMN watched_at TEXT;
ALTER TABLE movies ADD COLUMN watched_source TEXT;
ALTER TABLE movies ADD COLUMN jellyfin_unmonitor_owner TEXT;
CREATE INDEX idx_movies_watched ON movies (watched);
