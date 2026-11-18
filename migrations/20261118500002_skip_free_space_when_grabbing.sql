-- The free-space check was one switch; it is now split between grabbing and importing
-- (Sonarr's SkipFreeSpaceCheckWhenGrabbing and SkipFreeSpaceCheckWhenImporting). Keep an
-- existing installation's choice for both.
UPDATE settings
SET value = json_set(value, '$.skipFreeSpaceCheckWhenGrabbing',
                     json(CASE WHEN json_extract(value, '$.skipFreeSpaceCheck') THEN 'true' ELSE 'false' END))
WHERE key = 'mediaManagement'
  AND json_type(value, '$.skipFreeSpaceCheckWhenGrabbing') IS NULL
  AND json_type(value, '$.skipFreeSpaceCheck') IS NOT NULL;
