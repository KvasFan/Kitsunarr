-- Sonarr's custom filters (saved filters of the series, history, queue… lists): the filter
-- items are stored verbatim as Sonarr's JSON array of {key, value, type}.
CREATE TABLE custom_filters (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    type TEXT NOT NULL,
    label TEXT NOT NULL,
    filters TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(filters) AND json_type(filters) = 'array')
);
