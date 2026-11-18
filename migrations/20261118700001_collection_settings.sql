-- Tanukarr's collection settings: the group's kind and how member films are foldered.
ALTER TABLE collections ADD COLUMN group_kind TEXT NOT NULL DEFAULT 'series'
    CHECK (group_kind IN ('series', 'movieSaga'));
ALTER TABLE collections ADD COLUMN movie_folder_style TEXT NOT NULL DEFAULT 'automatic'
    CHECK (movie_folder_style IN ('automatic', 'numbered', 'title'));
-- TMDB collections are film sagas (Tanukarr's `TmdbMovieProxy`).
UPDATE collections SET group_kind = 'movieSaga'
    WHERE kind = 'provider' AND metadata_provider = 'tmdbMovie';
