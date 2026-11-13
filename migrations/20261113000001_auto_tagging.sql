-- Sonarr's auto tagging (`AutoTagging`, migration 179) and the original country it can
-- match on (`Series.OriginalCountry`, migration 227).
CREATE TABLE auto_tags (
    id                         INTEGER PRIMARY KEY,
    name                       TEXT NOT NULL COLLATE NOCASE UNIQUE,
    remove_tags_automatically  INTEGER NOT NULL DEFAULT 0 CHECK (remove_tags_automatically IN (0, 1)),
    specifications             TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(specifications)),
    tags                       TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(tags))
) STRICT;

ALTER TABLE series ADD COLUMN original_country TEXT;
ALTER TABLE movies ADD COLUMN original_country TEXT;
