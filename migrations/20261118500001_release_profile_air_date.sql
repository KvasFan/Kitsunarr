-- Sonarr's release profile excluded tags (migration 221), air-date restriction (226) and
-- "allow season packs before all episodes aired" (232).
ALTER TABLE release_profiles ADD COLUMN excluded_tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE release_profiles ADD COLUMN air_date_restriction INTEGER NOT NULL DEFAULT 0;
ALTER TABLE release_profiles ADD COLUMN air_date_grace_period INTEGER NOT NULL DEFAULT 0;
ALTER TABLE release_profiles ADD COLUMN allow_season_pack_without_all_episodes_aired INTEGER NOT NULL DEFAULT 0;
