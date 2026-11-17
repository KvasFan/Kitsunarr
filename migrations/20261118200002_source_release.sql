-- Tanukarr's `preserve_source_release_context` (migration 237): the grabbed release's title and
-- size stay on the imported file, so a 30 GB season pack does not become a preferred 1 GB
-- release for custom formats once it is imported (or re-encoded).
ALTER TABLE episode_files ADD COLUMN source_release_size INTEGER;
ALTER TABLE episode_files ADD COLUMN source_release_title TEXT;
ALTER TABLE movie_files ADD COLUMN source_release_size INTEGER;
ALTER TABLE movie_files ADD COLUMN source_release_title TEXT;

-- Existing imports: the newest grab of the download that imported each file.
WITH candidates AS (
    SELECT CAST(json_extract(i.data, '$.fileId') AS INTEGER) AS file_id,
           i.series_id AS series_id,
           CAST(json_extract(g.data, '$.size') AS INTEGER) AS release_size,
           g.source_title AS release_title,
           ROW_NUMBER() OVER (PARTITION BY i.series_id, CAST(json_extract(i.data, '$.fileId') AS INTEGER)
                              ORDER BY i.date DESC, g.date DESC, i.id DESC, g.id DESC) AS preference
    FROM history i JOIN history g
      ON upper(g.download_id) = upper(i.download_id) AND g.series_id = i.series_id AND g.episode_id = i.episode_id
    WHERE i.event_type = 'downloadFolderImported' AND g.event_type = 'grabbed'
      AND i.download_id IS NOT NULL AND i.download_id <> '' AND i.series_id IS NOT NULL
      AND g.date <= i.date AND CAST(json_extract(g.data, '$.size') AS INTEGER) > 0
)
UPDATE episode_files SET source_release_size = c.release_size, source_release_title = c.release_title
FROM candidates c
WHERE c.preference = 1 AND c.file_id = episode_files.id AND c.series_id = episode_files.series_id;
