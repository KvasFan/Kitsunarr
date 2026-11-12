-- Tanukarr's per-series Library tools options: renaming mode and season folder titles.
ALTER TABLE series ADD COLUMN renaming_mode TEXT NOT NULL DEFAULT 'default'
    CHECK (renaming_mode IN ('default', 'manual', 'automatic'));
ALTER TABLE series ADD COLUMN season_titles TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(season_titles));
