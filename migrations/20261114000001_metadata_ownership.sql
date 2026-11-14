-- Episodes retained during a catalogue change must not be silently reused by a later refresh.
-- Their row ids, file associations and history remain intact until explicitly matched.
CREATE TABLE retained_metadata_episodes (
    episode_id INTEGER PRIMARY KEY REFERENCES episodes (id) ON DELETE CASCADE
) STRICT;
