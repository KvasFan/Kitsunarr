-- Tanukarr's custom quality presets (migration 251): criteria on quality definitions, and
-- presets of their own (ids from 10000).
ALTER TABLE quality_definitions ADD COLUMN source TEXT;
ALTER TABLE quality_definitions ADD COLUMN resolution INTEGER;
ALTER TABLE quality_definitions ADD COLUMN codec TEXT NOT NULL DEFAULT '';
ALTER TABLE quality_definitions ADD COLUMN title_pattern TEXT NOT NULL DEFAULT '';
ALTER TABLE quality_definitions ADD COLUMN is_retired INTEGER NOT NULL DEFAULT 0 CHECK (is_retired IN (0, 1));
