-- Per-profile size limits (Sonarr v5 / Tanukarr profile item sizes) and Tanukarr's codec size
-- factors (migration 251 `CodecMultipliers`).
ALTER TABLE quality_profiles ADD COLUMN size_limits TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(size_limits));
ALTER TABLE quality_profiles ADD COLUMN codec_multipliers TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(codec_multipliers));
