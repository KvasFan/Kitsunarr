//! Dashboard statistics.

use kitsunarr_domain::api::library::LibraryStats;
use sqlx::FromRow;

use crate::{Database, DbResult};

#[derive(FromRow)]
struct Row {
    series: i64,
    anime: i64,
    movies: i64,
    collections: i64,
    episodes: i64,
    episode_files: i64,
    missing: i64,
    movie_files: i64,
    size: i64,
}

pub async fn library_stats(db: &Database) -> DbResult<LibraryStats> {
    let r: Row = sqlx::query_as(
        "SELECT \
           (SELECT COUNT(*) FROM series WHERE media_kind = 'series') AS series, \
           (SELECT COUNT(*) FROM series WHERE media_kind = 'anime') AS anime, \
           (SELECT COUNT(*) FROM movies) AS movies, \
           (SELECT COUNT(*) FROM collections) AS collections, \
           (SELECT COUNT(*) FROM episodes WHERE season_number > 0) AS episodes, \
           (SELECT COUNT(*) FROM episode_files) AS episode_files, \
           (SELECT COUNT(*) FROM episodes e JOIN series s ON s.id = e.series_id \
              WHERE e.monitored = 1 AND s.monitored = 1 AND e.episode_file_id IS NULL AND e.season_number > 0 \
                AND e.air_date_utc IS NOT NULL AND e.air_date_utc <= strftime('%Y-%m-%dT%H:%M:%SZ','now')) AS missing, \
           (SELECT COUNT(*) FROM movie_files) AS movie_files, \
           (SELECT COALESCE(SUM(size), 0) FROM episode_files) + (SELECT COALESCE(SUM(size), 0) FROM movie_files) AS size",
    )
    .fetch_one(db.pool())
    .await?;
    Ok(LibraryStats {
        series: r.series,
        anime: r.anime,
        movies: r.movies,
        collections: r.collections,
        episodes: r.episodes,
        episode_files: r.episode_files,
        monitored_missing_episodes: r.missing,
        movie_files: r.movie_files,
        size_on_disk: r.size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn counts() {
        let db = crate::test_support::db().await;
        crate::library::series::tests::insert_sample(&db, "Breaking Bad", "81189", 3).await;
        let s = library_stats(&db).await.unwrap();
        assert_eq!(
            (
                s.series,
                s.episodes,
                s.episode_files,
                s.monitored_missing_episodes,
                s.size_on_disk
            ),
            (1, 3, 1, 2, 1000)
        );
    }
}
