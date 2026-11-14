-- An explicit removal from a collection disables automatic reattachment on refresh.
CREATE TABLE collection_grouping_exclusions (
    series_id INTEGER UNIQUE REFERENCES series (id) ON DELETE CASCADE,
    movie_id INTEGER UNIQUE REFERENCES movies (id) ON DELETE CASCADE,
    CHECK ((series_id IS NOT NULL) != (movie_id IS NOT NULL))
) STRICT;
