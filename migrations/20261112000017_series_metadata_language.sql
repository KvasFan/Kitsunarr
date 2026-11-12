-- Sonarr's `Series.Language`: the language TheTVDB titles are read in (a language id; English
-- when NULL). Set from the preferred metadata language when a series is added.
ALTER TABLE series ADD COLUMN metadata_language INTEGER;
