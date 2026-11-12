-- The newest release of each indexer's last RSS sync (Sonarr's `IndexerStatus.LastRssSyncReleaseInfo`),
-- as JSON: the next sync pages back until it reaches it.
ALTER TABLE indexer_status ADD COLUMN last_rss_sync_release TEXT;
