-- Native title history retains indexed pagination and deterministic ordering at equal dates.
CREATE INDEX idx_history_episode_date_id ON history (episode_id, date, id);
CREATE INDEX idx_history_download_nocase_date_id ON history (download_id COLLATE NOCASE, date, id);
