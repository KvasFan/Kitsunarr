//! Tanukarr's series index: one list of series, anime and movies (Tanukarr's movies are
//! series), filtered and sorted together, with each anime group shown once when grouped.

use std::collections::{HashMap, HashSet};

use kitsunarr_domain::api::library::{
    JumpBarItem, LibraryEntry, LibraryFooter, LibraryGroup, LibraryIndex, LibraryKey, LibraryQuery,
};
use kitsunarr_domain::{MediaKind, SeriesType, SortDirection};
use sqlx::{FromRow, QueryBuilder, Sqlite};

use super::{collections, like_pattern, movies, series};
use crate::{Database, DbResult};

/// The group a series belongs to: a manual group first (the user's choice), else its anime
/// group (Tanukarr: one group per title).
const SERIES_GROUP: &str = "(SELECT gm.collection_id FROM collection_members gm \
     JOIN collections gc ON gc.id = gm.collection_id AND gc.kind IN ('manual', 'animeGroup') \
     WHERE gm.series_id = s.id ORDER BY CASE gc.kind WHEN 'manual' THEN 0 ELSE 1 END, gc.id LIMIT 1)";
const MOVIE_GROUP: &str = "(SELECT gm.collection_id FROM collection_members gm \
     JOIN collections gc ON gc.id = gm.collection_id AND gc.kind IN ('manual', 'animeGroup') \
     WHERE gm.movie_id = m.id ORDER BY CASE gc.kind WHEN 'manual' THEN 0 ELSE 1 END, gc.id LIMIT 1)";

const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%SZ','now')";

/// A movie's release dates, for the airing columns.
fn movie_release(op: &str, pick: &str) -> String {
    format!(
        "(SELECT {pick}(d) FROM (SELECT m.in_cinemas AS d UNION ALL SELECT m.digital_release UNION ALL SELECT m.physical_release) \
         WHERE d IS NOT NULL AND d {op} {NOW})"
    )
}

/// Sort values of Tanukarr's sort keys (`useSeries.ts` `SORT_PREDICATES`) for a series and a
/// movie, and whether titles without a value go last in both directions.
fn sort_values(key: &str) -> (String, String, bool) {
    let s_files = "(SELECT COUNT(*) FROM episodes e WHERE e.series_id = s.id AND e.episode_file_id IS NOT NULL)";
    let s_episodes =
        "(SELECT COUNT(*) FROM episodes e WHERE e.series_id = s.id AND e.season_number > 0)";
    let s_size = "(SELECT COALESCE(SUM(f.size), 0) FROM episode_files f WHERE f.series_id = s.id)";
    let m_size = "(SELECT COALESCE(SUM(f.size), 0) FROM movie_files f WHERE f.movie_id = m.id)";
    let m_file = "EXISTS (SELECT 1 FROM movie_files f WHERE f.movie_id = m.id)";
    let tag = |table: &str, column: &str, alias: &str| {
        format!(
            "(SELECT MIN(t.label) FROM {table} x JOIN tags t ON t.id = x.tag_id WHERE x.{column} = {alias}.id)"
        )
    };
    let (a, b, nulls_last): (String, String, bool) = match key {
        // Monitored first, then continuing (Tanukarr's status sort).
        "status" => (
            "(s.monitored * 2 + (s.status = 'continuing'))".into(),
            "(m.monitored * 2 + (m.status = 'inCinemas'))".into(),
            false,
        ),
        "seriesType" => ("s.series_type".into(), "'movie'".into(), false),
        "network" => (
            "lower(COALESCE(s.network, ''))".into(),
            "lower(COALESCE(m.studio, ''))".into(),
            false,
        ),
        "qualityProfile" | "qualityProfileId" => (
            "(SELECT lower(name) FROM quality_profiles WHERE id = s.quality_profile_id)".into(),
            "(SELECT lower(name) FROM quality_profiles WHERE id = m.quality_profile_id)".into(),
            false,
        ),
        "nextAiring" => (
            format!(
                "(SELECT MIN(e.air_date_utc) FROM episodes e WHERE e.series_id = s.id AND e.air_date_utc > {NOW})"
            ),
            movie_release(">", "MIN"),
            true,
        ),
        "previousAiring" => (
            format!(
                "(SELECT MAX(e.air_date_utc) FROM episodes e WHERE e.series_id = s.id AND e.air_date_utc <= {NOW})"
            ),
            movie_release("<=", "MAX"),
            true,
        ),
        "added" | "addedAt" => ("s.added_at".into(), "m.added_at".into(), false),
        "seasonCount" => (
            "(SELECT COUNT(*) FROM seasons x WHERE x.series_id = s.id AND x.season_number > 0)"
                .into(),
            "0".into(),
            false,
        ),
        "seasonFolder" => ("s.season_folder".into(), "0".into(), false),
        // Share of episodes with a file, then the episode count (Tanukarr's progress sort).
        "episodeProgress" => (
            format!(
                "(CASE WHEN {s_episodes} = 0 THEN 100.0 ELSE {s_files} * 100.0 / {s_episodes} END + {s_episodes} / 1000000.0)"
            ),
            format!("((CASE WHEN {m_file} THEN 100.0 ELSE 0.0 END) + 0.000001)"),
            false,
        ),
        "episodeCount" => (s_episodes.into(), "1".into(), false),
        "year" => ("s.year".into(), "m.year".into(), false),
        "path" => ("lower(s.path)".into(), "lower(m.path)".into(), false),
        "size" | "sizeOnDisk" => (s_size.into(), m_size.into(), false),
        "averageSize" | "averageSizePerEpisode" => (
            format!("(CASE WHEN {s_files} > 0 THEN {s_size} / {s_files} ELSE 0 END)"),
            m_size.into(),
            false,
        ),
        "ratings" => (
            "COALESCE(json_extract(s.ratings, '$.value'), 0)".into(),
            "COALESCE(json_extract(m.ratings, '$.value'), 0)".into(),
            false,
        ),
        "tags" => (
            tag("series_tags", "series_id", "s"),
            tag("movie_tags", "movie_id", "m"),
            false,
        ),
        _ => ("s.sort_title".into(), "m.sort_title".into(), false),
    };
    (a, b, nulls_last)
}

/// Whether the sort is by title (Tanukarr shows the jump bar then).
fn sorted_by_title(key: Option<&str>) -> bool {
    matches!(key.unwrap_or("title"), "title" | "sortTitle")
}

/// Movie statuses read as Tanukarr's series statuses (a released movie has ended).
fn movie_statuses(status: &str) -> &'static [&'static str] {
    match status {
        "ended" => &["released"],
        "continuing" => &["inCinemas"],
        "upcoming" => &["announced"],
        "deleted" => &["deleted"],
        _ => &[],
    }
}

/// The filters on the movie aliased `m` (each starts with ` AND`), as the series filters read
/// for a movie.
fn push_movie_conditions(qb: &mut QueryBuilder<Sqlite>, q: &LibraryQuery) {
    if let Some(m) = q.monitored {
        qb.push(" AND m.monitored = ").push_bind(m);
    }
    if let Some(w) = q.watched {
        qb.push(" AND m.watched = ").push_bind(w);
    }
    if let Some(status) = q.status.as_deref().filter(|s| !s.is_empty()) {
        let list = movie_statuses(status);
        if list.is_empty() {
            qb.push(" AND m.status = ").push_bind(status.to_string());
        } else {
            qb.push(" AND m.status IN (");
            let mut sep = qb.separated(", ");
            for s in list {
                sep.push_bind(*s);
            }
            qb.push(")");
        }
    }
    if let Some(tag) = q.tag_id {
        qb.push(" AND EXISTS (SELECT 1 FROM movie_tags t WHERE t.movie_id = m.id AND t.tag_id = ")
            .push_bind(tag)
            .push(")");
    }
    if let Some(p) = q.quality_profile_id {
        qb.push(" AND m.quality_profile_id = ").push_bind(p);
    }
    if let Some(p) = q.provider {
        qb.push(" AND m.metadata_provider = ").push_bind(p.as_str());
    }
    // An anime movie: from an anime catalogue, or a member of an anime group.
    let anime = "(m.metadata_provider IN ('aniList', 'myAnimeList') OR EXISTS (SELECT 1 FROM collection_members x \
                 JOIN collections c ON c.id = x.collection_id AND c.kind = 'animeGroup' WHERE x.movie_id = m.id))";
    match q.series_type {
        Some(SeriesType::Anime) => {
            qb.push(format_args!(" AND {anime}"));
        }
        Some(SeriesType::Standard) => {
            qb.push(format_args!(" AND NOT {anime}"));
        }
        Some(SeriesType::Daily) => {
            qb.push(" AND 0");
        }
        None => {}
    }
    if let Some(path) = q
        .path
        .as_deref()
        .map(|p| {
            p.replace('\\', "/")
                .trim_end_matches('/')
                .to_ascii_lowercase()
        })
        .filter(|p| !p.is_empty())
    {
        let column = "lower(replace(m.path, '\\', '/'))";
        let prefix = format!("{path}/");
        qb.push(format_args!(" AND ({column} = "))
            .push_bind(path)
            .push(format_args!(" OR substr({column}, 1, "))
            .push_bind(prefix.chars().count() as i64)
            .push(") = ")
            .push_bind(prefix)
            .push(")");
    }
    // Missing: monitored, released (or in cinemas) and without a file.
    if let Some(missing) = q.missing {
        qb.push(format_args!(
            " AND {} (m.monitored = 1 AND m.status IN ('inCinemas', 'released') \
             AND NOT EXISTS (SELECT 1 FROM movie_files f WHERE f.movie_id = m.id))",
            if missing { "" } else { "NOT" }
        ));
    }
    if let Some(term) = q.search.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        qb.push(" AND (m.title LIKE ")
            .push_bind(like_pattern(term))
            .push(" ESCAPE '\\'");
        qb.push(" OR m.clean_title LIKE ")
            .push_bind(like_pattern(&kitsunarr_domain::text::clean_title(term)))
            .push(" ESCAPE '\\'");
        qb.push(" OR m.original_title LIKE ")
            .push_bind(like_pattern(term))
            .push(" ESCAPE '\\')");
    }
}

#[derive(FromRow)]
struct Candidate {
    movie: bool,
    id: i64,
    sort_title: String,
    group_id: Option<i64>,
}

impl Candidate {
    fn key(&self) -> LibraryKey {
        LibraryKey {
            movie: self.movie,
            id: self.id,
        }
    }
}

/// Every title the filters keep, in the index order.
async fn candidates(db: &Database, q: &LibraryQuery) -> DbResult<Vec<Candidate>> {
    let paging = q.page_request();
    let (series_sort, movie_sort, nulls_last) =
        sort_values(paging.sort_key.as_deref().unwrap_or("title"));
    // The watched state is a movie's; a daily series type never matches a movie.
    let with_series = q.watched.is_none() && q.kind != Some(MediaKind::Movie);
    let with_movies =
        matches!(q.kind, None | Some(MediaKind::Movie)) && q.series_type != Some(SeriesType::Daily);
    if !with_series && !with_movies {
        return Ok(Vec::new());
    }
    let mut qb = QueryBuilder::<Sqlite>::new("SELECT movie, id, sort_title, group_id FROM (");
    if with_series {
        qb.push(format_args!(
            "SELECT 0 AS movie, s.id AS id, s.sort_title AS sort_title, {SERIES_GROUP} AS group_id, {series_sort} AS sort_value \
             FROM series s WHERE 1 = 1"
        ));
        series::push_conditions(&mut qb, q, "s");
    }
    if with_series && with_movies {
        qb.push(" UNION ALL ");
    }
    if with_movies {
        qb.push(format_args!(
            "SELECT 1 AS movie, m.id AS id, m.sort_title AS sort_title, {MOVIE_GROUP} AS group_id, {movie_sort} AS sort_value \
             FROM movies m WHERE 1 = 1"
        ));
        push_movie_conditions(&mut qb, q);
    }
    let dir = if paging.direction() == SortDirection::Desc {
        "DESC"
    } else {
        "ASC"
    };
    // Tanukarr's secondary sort: the title, ascending.
    qb.push(format_args!(
        ") ORDER BY {}sort_value {dir}, sort_title COLLATE NOCASE ASC, movie, id",
        if nulls_last {
            "sort_value IS NULL, "
        } else {
            ""
        }
    ));
    Ok(qb.build_query_as().fetch_all(db.pool()).await?)
}

/// What the index shows: each title, or each anime group once at its first member's place
/// (Tanukarr's `groupAnimeSeries`, applied after filtering and sorting).
enum Shown {
    Title(LibraryKey),
    Group(i64),
}

/// Tanukarr's footer over the whole library.
async fn footer(db: &Database, kind: Option<MediaKind>) -> DbResult<LibraryFooter> {
    type Row = (i64, i64, i64, i64, i64, i64);
    let mut f = LibraryFooter::default();
    if kind != Some(MediaKind::Movie) {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT COUNT(*), COALESCE(SUM(status = 'ended'), 0), COALESCE(SUM(monitored), 0), \
             (SELECT COUNT(*) FROM episodes e JOIN series x ON x.id = e.series_id WHERE e.season_number > 0",
        );
        if let Some(k) = kind {
            qb.push(" AND x.media_kind = ").push_bind(k.as_str());
        }
        qb.push(
            "), (SELECT COUNT(*) FROM episodes e JOIN series x ON x.id = e.series_id WHERE e.episode_file_id IS NOT NULL",
        );
        if let Some(k) = kind {
            qb.push(" AND x.media_kind = ").push_bind(k.as_str());
        }
        qb.push(
            "), (SELECT COALESCE(SUM(f.size), 0) FROM episode_files f JOIN series x ON x.id = f.series_id",
        );
        if let Some(k) = kind {
            qb.push(" WHERE x.media_kind = ").push_bind(k.as_str());
        }
        qb.push(") FROM series");
        if let Some(k) = kind {
            qb.push(" WHERE media_kind = ").push_bind(k.as_str());
        }
        let (count, ended, monitored, episodes, files, size): Row =
            qb.build_query_as().fetch_one(db.pool()).await?;
        f.titles += count;
        f.ended += ended;
        f.monitored += monitored;
        f.episodes += episodes;
        f.files += files;
        f.size_on_disk += size;
    }
    if matches!(kind, None | Some(MediaKind::Movie)) {
        // A movie counts as one episode, as Tanukarr's movies do.
        let (count, ended, monitored, episodes, files, size): Row = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(status = 'released'), 0), COALESCE(SUM(monitored), 0), COUNT(*), \
             (SELECT COUNT(*) FROM movie_files), (SELECT COALESCE(SUM(size), 0) FROM movie_files) FROM movies",
        )
        .fetch_one(db.pool())
        .await?;
        f.titles += count;
        f.ended += ended;
        f.monitored += monitored;
        f.episodes += episodes;
        f.files += files;
        f.size_on_disk += size;
    }
    f.continuing = f.titles - f.ended;
    f.unmonitored = f.titles - f.monitored;
    Ok(f)
}

/// A grouped entry: the members the filters keep, counted as the collection page shows them.
async fn group_entry(
    db: &Database,
    collection_id: i64,
    kept: &[LibraryKey],
) -> DbResult<LibraryGroup> {
    let detail = collections::detail(db, collection_id).await?;
    let kept_set: HashSet<LibraryKey> = kept.iter().copied().collect();
    let key_of = |series: Option<i64>, movie: Option<i64>| match (series, movie) {
        (Some(id), _) => LibraryKey { movie: false, id },
        (_, Some(id)) => LibraryKey { movie: true, id },
        _ => LibraryKey {
            movie: false,
            id: 0,
        },
    };
    let entries: Vec<_> = detail
        .entries
        .iter()
        .filter(|e| kept_set.contains(&key_of(e.series_id, e.movie_id)))
        .collect();
    let mut members: Vec<LibraryKey> = Vec::new();
    for e in &entries {
        let k = key_of(e.series_id, e.movie_id);
        if !members.contains(&k) {
            members.push(k);
        }
    }
    for k in kept {
        if !members.contains(k) {
            members.push(*k);
        }
    }
    let count = |section: &str| entries.iter().filter(|e| e.section == section).count() as i64;
    let seasons: Vec<_> = entries.iter().filter(|e| e.section == "seasons").collect();
    let counted = entries.iter().filter(|e| e.season_number != Some(0));
    let poster = detail
        .poster_url
        .clone()
        .or_else(|| entries.iter().find_map(|e| e.poster_url.clone()));
    Ok(LibraryGroup {
        collection_id,
        title: detail.title,
        poster_url: poster,
        season_count: seasons.len() as i64,
        movie_count: count("movies"),
        special_count: count("specials"),
        episode_count: counted.clone().map(|e| e.episode_count).sum(),
        episode_file_count: counted.map(|e| e.episode_file_count).sum(),
        single_season: (members.len() == 1 && seasons.len() == 1)
            .then(|| seasons[0].display_number),
        members,
    })
}

/// A page of the library index.
pub async fn list(db: &Database, q: &LibraryQuery) -> DbResult<LibraryIndex> {
    let paging = q.page_request();
    let all = candidates(db, q).await?;
    let grouped = q.group_anime_seasons == Some(true);
    let mut members: HashMap<i64, Vec<LibraryKey>> = HashMap::new();
    let mut shown: Vec<(Shown, &str)> = Vec::new();
    for c in &all {
        match c.group_id.filter(|_| grouped) {
            Some(g) => {
                let list = members.entry(g).or_default();
                if list.is_empty() {
                    shown.push((Shown::Group(g), &c.sort_title));
                }
                list.push(c.key());
            }
            None => shown.push((Shown::Title(c.key()), &c.sort_title)),
        }
    }
    let descending = paging.direction() == SortDirection::Desc;
    let mut jump_bar = Vec::new();
    if sorted_by_title(paging.sort_key.as_deref()) {
        let mut first: Vec<(String, u64)> = Vec::new();
        for (i, (_, title)) in shown.iter().enumerate() {
            let c = title.chars().next().unwrap_or('#');
            let c = if c.is_numeric() {
                "#".to_string()
            } else {
                c.to_uppercase().to_string()
            };
            if !first.iter().any(|(x, _)| *x == c) {
                first.push((c, i as u64));
            }
        }
        first.sort();
        if descending {
            first.reverse();
        }
        jump_bar = first
            .into_iter()
            .map(|(character, index)| JumpBarItem { character, index })
            .collect();
    }
    let total_records = shown.len() as u64;
    let page_entries: Vec<&Shown> = shown
        .iter()
        .map(|(s, _)| s)
        .skip(paging.offset() as usize)
        .take(paging.page_size() as usize)
        .collect();
    let series_ids: Vec<i64> = page_entries
        .iter()
        .filter_map(|s| match s {
            Shown::Title(k) if !k.movie => Some(k.id),
            _ => None,
        })
        .collect();
    let movie_ids: Vec<i64> = page_entries
        .iter()
        .filter_map(|s| match s {
            Shown::Title(k) if k.movie => Some(k.id),
            _ => None,
        })
        .collect();
    let mut series_items: HashMap<i64, _> = series::list_items_by_ids(db, &series_ids)
        .await?
        .into_iter()
        .map(|i| (i.id, i))
        .collect();
    let mut movie_items: HashMap<i64, _> = movies::list_items_by_ids(db, &movie_ids)
        .await?
        .into_iter()
        .map(|i| (i.id, i))
        .collect();
    let mut records = Vec::with_capacity(page_entries.len());
    for s in page_entries {
        match s {
            Shown::Title(k) if k.movie => {
                if let Some(item) = movie_items.remove(&k.id) {
                    records.push(LibraryEntry::Movie(item));
                }
            }
            Shown::Title(k) => {
                if let Some(item) = series_items.remove(&k.id) {
                    records.push(LibraryEntry::Series(item));
                }
            }
            Shown::Group(g) => {
                let kept = members.get(g).cloned().unwrap_or_default();
                records.push(LibraryEntry::Group(group_entry(db, *g, &kept).await?));
            }
        }
    }
    let total_items = {
        let series: i64 = if q.kind == Some(MediaKind::Movie) {
            0
        } else if let Some(k) = q.kind {
            sqlx::query_scalar("SELECT COUNT(*) FROM series WHERE media_kind = ?")
                .bind(k.as_str())
                .fetch_one(db.pool())
                .await?
        } else {
            sqlx::query_scalar("SELECT COUNT(*) FROM series")
                .fetch_one(db.pool())
                .await?
        };
        let movies: i64 = if matches!(q.kind, None | Some(MediaKind::Movie)) {
            sqlx::query_scalar("SELECT COUNT(*) FROM movies")
                .fetch_one(db.pool())
                .await?
        } else {
            0
        };
        (series + movies) as u64
    };
    Ok(LibraryIndex {
        page: paging.page(),
        page_size: paging.page_size(),
        total_records,
        records,
        filtered_items: all.len() as u64,
        total_items,
        has_groups: all.iter().any(|c| c.group_id.is_some()),
        jump_bar,
        footer: footer(db, q.kind).await?,
        keys: if q.include_keys == Some(true) {
            all.iter().map(Candidate::key).collect()
        } else {
            Vec::new()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::movies::tests::sample as movie_sample;
    use crate::library::series::tests::insert_sample;

    fn titles(index: &LibraryIndex) -> Vec<String> {
        index
            .records
            .iter()
            .map(|r| match r {
                LibraryEntry::Series(s) => s.title.clone(),
                LibraryEntry::Movie(m) => m.title.clone(),
                LibraryEntry::Group(g) => format!("group {}", g.title),
            })
            .collect()
    }

    #[tokio::test]
    async fn one_list_of_series_and_movies_with_groups_once() {
        let db = crate::test_support::db().await;
        let bb = insert_sample(&db, "Breaking Bad", "81189", 3).await;
        let s1 = insert_sample(&db, "Frieren", "1", 0).await;
        let s2 = insert_sample(&db, "Frieren S2", "2", 0).await;
        let mut conn = db.pool().acquire().await.unwrap();
        let film = movies::insert(&mut conn, &movie_sample("Frieren Film", "10"))
            .await
            .unwrap();
        movies::insert(&mut conn, &movie_sample("Cowboy Bebop Film", "11"))
            .await
            .unwrap();
        drop(conn);
        let group: i64 = sqlx::query_scalar(
            "INSERT INTO collections (title, sort_title, kind, created_at) VALUES ('Frieren Saga', 'frieren saga', 'animeGroup', '2026-01-01') RETURNING id",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        for (position, series, movie, role) in [
            (1, Some(s1), None, "season"),
            (2, Some(s2), None, "season"),
            (3, None, Some(film), "movie"),
        ] {
            sqlx::query(
                "INSERT INTO collection_members (collection_id, position, series_id, movie_id, role) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(group)
            .bind(position)
            .bind(series)
            .bind(movie)
            .bind(role)
            .execute(db.pool())
            .await
            .unwrap();
        }

        // Every title, sorted by title: series and movies together.
        let all = list(&db, &LibraryQuery::default()).await.unwrap();
        assert_eq!(
            titles(&all),
            [
                "Breaking Bad",
                "Cowboy Bebop Film",
                "Frieren",
                "Frieren Film",
                "Frieren S2"
            ]
        );
        assert_eq!((all.total_records, all.total_items), (5, 5));
        assert!(all.has_groups);
        assert_eq!(all.footer.titles, 5);
        assert_eq!(
            all.jump_bar
                .iter()
                .map(|j| (j.character.as_str(), j.index))
                .collect::<Vec<_>>(),
            [("B", 0), ("C", 1), ("F", 2)]
        );

        // Grouped: the group once, at its first member's place, with what the filters keep.
        let grouped = LibraryQuery {
            group_anime_seasons: Some(true),
            ..Default::default()
        };
        let index = list(&db, &grouped).await.unwrap();
        assert_eq!(
            titles(&index),
            ["Breaking Bad", "Cowboy Bebop Film", "group Frieren Saga"]
        );
        let LibraryEntry::Group(g) = &index.records[2] else {
            panic!("group entry");
        };
        assert_eq!(
            g.members,
            [
                LibraryKey {
                    movie: false,
                    id: s1
                },
                LibraryKey {
                    movie: false,
                    id: s2
                },
                LibraryKey {
                    movie: true,
                    id: film
                },
            ]
        );
        assert_eq!((g.season_count, g.movie_count, g.special_count), (2, 1, 0));
        assert_eq!(g.single_season, None);
        // Sorted the other way, the group moves to its first member in that order.
        let index = list(
            &db,
            &LibraryQuery {
                sort_direction: Some(SortDirection::Desc),
                ..grouped.clone()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            titles(&index),
            ["group Frieren Saga", "Cowboy Bebop Film", "Breaking Bad"]
        );
        // A filter keeping one member: a group of one season ("Season 2").
        let index = list(
            &db,
            &LibraryQuery {
                search: Some("S2".into()),
                include_keys: Some(true),
                ..grouped.clone()
            },
        )
        .await
        .unwrap();
        let LibraryEntry::Group(g) = &index.records[0] else {
            panic!("group entry");
        };
        assert_eq!(g.single_season, Some(2));
        assert_eq!(
            index.keys,
            [LibraryKey {
                movie: false,
                id: s2
            }]
        );

        // Movies only (the old movie library), watched state, the anime filter.
        let movies_only = list(
            &db,
            &LibraryQuery {
                kind: Some(MediaKind::Movie),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(titles(&movies_only), ["Cowboy Bebop Film", "Frieren Film"]);
        let unwatched = list(
            &db,
            &LibraryQuery {
                watched: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            unwatched.total_records, 2,
            "only movies have a watched state"
        );
        let anime = list(
            &db,
            &LibraryQuery {
                series_type: Some(SeriesType::Anime),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(titles(&anime), ["Cowboy Bebop Film", "Frieren Film"]);

        // Tanukarr's status sort: monitored first, then continuing; missing airings last.
        sqlx::query("UPDATE series SET monitored = 0 WHERE id = ?")
            .bind(bb)
            .execute(db.pool())
            .await
            .unwrap();
        let by_status = list(
            &db,
            &LibraryQuery {
                sort_key: Some("status".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(titles(&by_status)[0], "Breaking Bad");
        for dir in [SortDirection::Asc, SortDirection::Desc] {
            let by_next = list(
                &db,
                &LibraryQuery {
                    sort_key: Some("nextAiring".into()),
                    sort_direction: Some(dir),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(by_next.total_records, 5);
            assert!(by_next.jump_bar.is_empty(), "jump bar only by title");
        }
    }
}
