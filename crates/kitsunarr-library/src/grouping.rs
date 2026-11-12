//! Automatic anime grouping: Tanukarr's `AnimeGroupingResolver` (AniList relations, or
//! MyAnimeList's through Jikan) and TMDB
//! movie collections.
//!
//! An AniList title is walked back through its PREQUEL links to the franchise's first TV entry;
//! the title joins the collection `anilist:{root}` as "Season N" (TV, TV short, ONA) or "Movie N"
//! (films), N counting the entries of its kind from the root. Ambiguous chains (two prequels,
//! links that are not reciprocal, cycles) are left ungrouped rather than guessed. Membership is
//! decided once, when a title is added; existing members are never moved.

use std::collections::HashSet;
use std::future::Future;

use kitsunarr_db::Database;
pub use kitsunarr_db::library::collections::Member;
use kitsunarr_db::library::collections::{self, NewCollection};
use kitsunarr_domain::MetadataProvider;
use kitsunarr_metadata::{AnimeNode, Metadata, MetadataResult};

/// Distinct nodes fetched per resolution (Tanukarr's budget).
const MAX_FETCHES: usize = 16;
/// Relations of one kind per node beyond which a chain is ambiguous.
const MAX_CANDIDATES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
    Season,
    Movie,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// `anilist:{root id}`.
    pub id: String,
    pub root: i64,
    pub title: String,
    pub member_kind: MemberKind,
    /// 1-based position among the group's entries of the same kind.
    pub ordinal: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Resolved(Group),
    /// A standalone title: no prequel and no sequel.
    Unrelated,
    /// The title's format is not grouped (OVA, special, music...).
    Unsupported,
    Ambiguous(String),
    LimitReached,
}

fn eligible(format: &str) -> bool {
    matches!(format, "TV" | "TV_SHORT" | "ONA" | "MOVIE")
}

fn is_movie(format: &str) -> bool {
    format == "MOVIE"
}

/// Resolves an AniList title's group; `fetch` reads one node (relations included).
pub async fn resolve<F, Fut>(start: i64, fetch: F) -> MetadataResult<Resolution>
where
    F: Fn(i64) -> Fut,
    Fut: Future<Output = MetadataResult<AnimeNode>>,
{
    let mut fetched = 0usize;
    let mut get = async |id: i64| -> MetadataResult<Option<AnimeNode>> {
        if fetched >= MAX_FETCHES {
            return Ok(None);
        }
        fetched += 1;
        fetch(id).await.map(Some)
    };
    let Some(first) = get(start).await? else {
        return Ok(Resolution::LimitReached);
    };
    if !eligible(&first.format) || first.title.trim().is_empty() {
        return Ok(Resolution::Unsupported);
    }
    let related = |n: &AnimeNode, kind: &str| -> Vec<(i64, String)> {
        n.relations
            .iter()
            .filter(|r| r.kind == kind && eligible(&r.format))
            .map(|r| (r.id, r.format.clone()))
            .collect()
    };
    if related(&first, "PREQUEL").is_empty() && related(&first, "SEQUEL").is_empty() {
        return Ok(Resolution::Unrelated);
    }
    // Backwards through prequels: chain[0] is the start, the last entry the earliest.
    let mut chain = vec![first];
    let mut seen = HashSet::from([start]);
    loop {
        let current = chain.last().expect("non-empty chain");
        let mut prequels = related(current, "PREQUEL");
        if prequels.len() > MAX_CANDIDATES {
            return Ok(Resolution::Ambiguous("too many prequels".into()));
        }
        if prequels.is_empty() {
            break;
        }
        let current_id = current.id;
        let current_movie = is_movie(&current.format);
        let mut nodes = Vec::new();
        for (id, _) in &prequels {
            match get(*id).await? {
                Some(n) => nodes.push(n),
                None => return Ok(Resolution::LimitReached),
            }
        }
        // A prequel that is another prequel's side story (reciprocally: it names that prequel
        // as its parent) is not the main line.
        let links = |n: &AnimeNode, kind: &str, id: i64| {
            n.relations.iter().any(|r| r.kind == kind && r.id == id)
        };
        let side: HashSet<i64> = nodes
            .iter()
            .filter(|p| {
                nodes.iter().any(|q| {
                    q.id != p.id && links(q, "SIDE_STORY", p.id) && links(p, "PARENT", q.id)
                })
            })
            .map(|p| p.id)
            .collect();
        prequels.retain(|(id, _)| !side.contains(id));
        let mut main: Vec<AnimeNode> = nodes
            .into_iter()
            .filter(|n| !side.contains(&n.id))
            .collect();
        if main.len() != 1 {
            return Ok(Resolution::Ambiguous("several prequels".into()));
        }
        let parent = main.remove(0);
        if !seen.insert(parent.id) {
            return Ok(Resolution::Ambiguous("a relation cycle".into()));
        }
        // Reciprocity: the parent names the child as its sequel, and only one sequel of the
        // child's kind.
        let sequels = related(&parent, "SEQUEL");
        if !sequels.iter().any(|(id, _)| *id == current_id) {
            return Ok(Resolution::Ambiguous(
                "the prequel does not list this title as its sequel".into(),
            ));
        }
        if sequels
            .iter()
            .filter(|(_, f)| is_movie(f) == current_movie)
            .count()
            != 1
        {
            return Ok(Resolution::Ambiguous(
                "the prequel has several sequels".into(),
            ));
        }
        if !eligible(&parent.format) || parent.title.trim().is_empty() {
            return Ok(Resolution::Ambiguous(
                "an entry of the chain cannot be grouped".into(),
            ));
        }
        chain.push(parent);
    }
    // Earliest first; the root is the earliest TV entry (or the earliest film for a saga).
    chain.reverse();
    let member = chain.len() - 1;
    let root = chain.iter().position(|n| !is_movie(&n.format)).unwrap_or(0);
    let kind = if is_movie(&chain[member].format) {
        MemberKind::Movie
    } else {
        MemberKind::Season
    };
    if member < root {
        // A film before the first TV entry: Tanukarr groups it from the TV root forward only.
        return Ok(Resolution::Ambiguous(
            "a film before the franchise's first season".into(),
        ));
    }
    let ordinal = chain[root..=member]
        .iter()
        .filter(|n| is_movie(&n.format) == (kind == MemberKind::Movie))
        .count() as i32;
    Ok(Resolution::Resolved(Group {
        id: format!("anilist:{}", chain[root].id),
        root: chain[root].id,
        title: chain[root].title.clone(),
        member_kind: kind,
        ordinal,
    }))
}

/// Tanukarr's `AnimeGroupPathBuilder.BuildPath` for a new AniList or MyAnimeList series: its
/// group is resolved first, and the series goes to `{root}/{group folder}/{member folder}`.
/// The group folder of a member already in the library is reused; a path another series uses,
/// or a group folder another series occupies, falls back to the usual folder (`None`).
#[allow(clippy::too_many_arguments)]
pub async fn plan_series_path(
    db: &Database,
    metadata: &Metadata,
    source: MetadataProvider,
    anime_id: i64,
    title: &str,
    root: &str,
    naming_settings: &kitsunarr_domain::api::settings::NamingSettings,
) -> Option<(String, Group)> {
    let resolved = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        if source == MetadataProvider::AniList {
            resolve(anime_id, |n| metadata.anime_relations(n)).await
        } else {
            resolve(anime_id, |n| metadata.mal_relations(n)).await
        }
    })
    .await
    .ok()?
    .ok()?;
    let Resolution::Resolved(mut g) = resolved else {
        return None;
    };
    if g.member_kind != MemberKind::Season {
        return None;
    }
    if source == MetadataProvider::MyAnimeList {
        g.id = format!("mal:{}", g.root);
    }
    let root_path = std::path::Path::new(root);
    // A member already in the library names the group folder.
    let member_paths: Vec<String> = sqlx::query_scalar(
        "SELECT s.path FROM collection_members m JOIN collections c ON c.id = m.collection_id \
         JOIN series s ON s.id = m.series_id WHERE c.kind = 'animeGroup' AND c.metadata_provider = ? AND c.metadata_id = ?",
    )
    .bind(source.as_str())
    .bind(g.root.to_string())
    .fetch_all(db.pool())
    .await
    .ok()?;
    let existing = member_paths.iter().find_map(|p| {
        let parent = std::path::Path::new(p).parent()?;
        (parent.parent()? == root_path)
            .then(|| parent.file_name().map(|n| n.to_string_lossy().into_owned()))
            .flatten()
    });
    let series = crate::file_names::Series {
        title: title.to_string(),
        group: Some(crate::file_names::GroupMembership {
            title: g.title.clone(),
            season: Some(g.ordinal),
            movie: None,
        }),
        ..Default::default()
    };
    let group_folder = match existing {
        Some(f) => f,
        None => crate::file_names::group_folder(
            &naming_settings.anime_group_folder_format,
            &series,
            naming_settings,
        ),
    };
    let member_format = if crate::file_names::is_story_arc(title, &g.title) {
        &naming_settings.anime_group_arc_folder_format
    } else {
        &naming_settings.anime_group_season_folder_format
    };
    let member_folder = crate::file_names::group_folder(member_format, &series, naming_settings);
    if group_folder.is_empty() || member_folder.is_empty() {
        return None;
    }
    let group_path = kitsunarr_domain::naming::join(root, &group_folder);
    let path = kitsunarr_domain::naming::join(&group_path, &member_folder);
    // Not a path in use, and the group folder is not another series' folder (nor holds one
    // outside this group).
    let clash: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series WHERE path = ?1 OR path = ?2")
        .bind(&path)
        .bind(&group_path)
        .fetch_one(db.pool())
        .await
        .ok()?;
    if clash > 0 {
        return None;
    }
    let inside: Vec<String> = sqlx::query_scalar("SELECT path FROM series WHERE path LIKE ?")
        .bind(format!("{group_path}/%"))
        .fetch_all(db.pool())
        .await
        .ok()?;
    if inside.iter().any(|p| !member_paths.contains(p)) {
        return None;
    }
    Some((path, g))
}

/// Tanukarr's path for a film of a TMDB collection: `{root}/{collection folder}/{Film NN}`
/// (the collection's release order); the folder of a film of it already in the library is
/// reused, and a path in use falls back to the usual folder.
pub async fn plan_movie_path(
    db: &Database,
    metadata: &Metadata,
    tmdb_id: &str,
    title: &str,
    root: &str,
    naming_settings: &kitsunarr_domain::api::settings::NamingSettings,
) -> Option<String> {
    let c = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        metadata.movie_collection(tmdb_id),
    )
    .await
    .ok()?
    .ok()??;
    let index = c.parts.iter().position(|p| p.to_string() == tmdb_id)?;
    let root_path = std::path::Path::new(root);
    let member_paths: Vec<String> = sqlx::query_scalar(
        "SELECT v.path FROM collection_members m JOIN collections c ON c.id = m.collection_id \
         JOIN movies v ON v.id = m.movie_id WHERE c.metadata_provider = ? AND c.metadata_id = ?",
    )
    .bind(MetadataProvider::TmdbMovie.as_str())
    .bind(c.id.to_string())
    .fetch_all(db.pool())
    .await
    .ok()?;
    let existing = member_paths.iter().find_map(|p| {
        let parent = std::path::Path::new(p).parent()?;
        (parent.parent()? == root_path)
            .then(|| parent.file_name().map(|n| n.to_string_lossy().into_owned()))
            .flatten()
    });
    let series = crate::file_names::Series {
        title: title.to_string(),
        group: Some(crate::file_names::GroupMembership {
            title: c.name.clone(),
            season: None,
            movie: Some(index as i32 + 1),
        }),
        ..Default::default()
    };
    let group_folder = existing.unwrap_or_else(|| {
        crate::file_names::group_folder(
            &naming_settings.anime_group_folder_format,
            &series,
            naming_settings,
        )
    });
    let member = crate::file_names::group_folder(
        &naming_settings.anime_group_movie_saga_folder_format,
        &series,
        naming_settings,
    );
    if group_folder.is_empty() || member.is_empty() {
        return None;
    }
    let path = kitsunarr_domain::naming::join(
        &kitsunarr_domain::naming::join(root, &group_folder),
        &member,
    );
    let used: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM movies WHERE path = ?1) + (SELECT COUNT(*) FROM series WHERE path = ?1)",
    )
    .bind(&path)
    .fetch_one(db.pool())
    .await
    .ok()?;
    (used == 0).then_some(path)
}

/// Joins a group already resolved (by [`plan_series_path`]).
pub async fn join_resolved(db: &Database, source: MetadataProvider, g: &Group, member: Member) {
    if let Err(e) = join(db, source, g, member).await {
        tracing::warn!(error = %e, "the anime group could not be saved");
    }
}

/// Puts a newly added AniList-owned title into its franchise group (creating the collection on
/// first use). Titles already in an anime group are left alone; failures only log.
pub async fn group_new_title(db: &Database, metadata: &Metadata, member: Member) {
    let (id, lookup, membership) = match member {
        Member::Series(id) => (
            id,
            "SELECT metadata_provider, metadata_id FROM series WHERE id = ?",
            "SELECT COUNT(*) FROM collection_members m JOIN collections c ON c.id = m.collection_id \
             WHERE c.kind = 'animeGroup' AND m.series_id = ?",
        ),
        Member::Movie(id) => (
            id,
            "SELECT metadata_provider, metadata_id FROM movies WHERE id = ?",
            "SELECT COUNT(*) FROM collection_members m JOIN collections c ON c.id = m.collection_id \
             WHERE c.kind = 'animeGroup' AND m.movie_id = ?",
        ),
    };
    let row: Option<(String, String)> = sqlx::query_as(lookup)
        .bind(id)
        .fetch_optional(db.pool())
        .await
        .ok()
        .flatten();
    let Some((provider, metadata_id)) = row else {
        return;
    };
    if provider == MetadataProvider::TmdbMovie.as_str()
        && let Member::Movie(movie) = member
    {
        if let Err(e) = join_tmdb_collection(db, metadata, movie, &metadata_id).await {
            tracing::warn!(movie, error = %e, "the TMDB collection could not be read");
        }
        return;
    }
    // AniList relations, or MyAnimeList's through Jikan; groups never cross catalogues.
    let source = if provider == MetadataProvider::AniList.as_str() {
        MetadataProvider::AniList
    } else if provider == MetadataProvider::MyAnimeList.as_str() {
        MetadataProvider::MyAnimeList
    } else {
        return;
    };
    let Ok(anime_id) = metadata_id.parse::<i64>() else {
        return;
    };
    let grouped: i64 = sqlx::query_scalar(membership)
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap_or(1);
    if grouped > 0 {
        return;
    }
    let resolved = if source == MetadataProvider::AniList {
        resolve(anime_id, |n| metadata.anime_relations(n)).await
    } else {
        resolve(anime_id, |n| metadata.mal_relations(n)).await
    };
    let resolution = match resolved {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(title = id, error = %e, "the anime group could not be resolved");
            return;
        }
    };
    let Resolution::Resolved(mut g) = resolution else {
        tracing::debug!(title = id, ?resolution, "not grouped");
        return;
    };
    if source == MetadataProvider::MyAnimeList {
        g.id = format!("mal:{}", g.root);
    }
    if let Err(e) = join(db, source, &g, member).await {
        tracing::warn!(title = id, error = %e, "the anime group could not be saved");
    }
}

/// A TMDB film joins its TMDB collection (`provider` kind), at its release-order position in the
/// whole collection (Tanukarr's "Film NN").
async fn join_tmdb_collection(
    db: &Database,
    metadata: &Metadata,
    movie: i64,
    tmdb_id: &str,
) -> Result<(), String> {
    let Some(c) = metadata
        .movie_collection(tmdb_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let Some(index) = c.parts.iter().position(|p| p.to_string() == tmdb_id) else {
        return Ok(());
    };
    let err = |e: kitsunarr_db::DbError| e.to_string();
    let mut tx = db.pool().begin().await.map_err(|e| e.to_string())?;
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM collections WHERE metadata_provider = ? AND metadata_id = ?",
    )
    .bind(MetadataProvider::TmdbMovie.as_str())
    .bind(c.id.to_string())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    let collection = match existing {
        Some(id) => id,
        None => collections::insert(
            &mut tx,
            &NewCollection {
                title: c.name.clone(),
                kind: "provider".into(),
                metadata_provider: Some(MetadataProvider::TmdbMovie),
                metadata_id: Some(c.id.to_string()),
                overview: c.overview.clone(),
                images: Vec::new(),
            },
        )
        .await
        .map_err(err)?,
    };
    let n = index as i32 + 1;
    collections::add_member(
        &mut tx,
        collection,
        Member::Movie(movie),
        n,
        Some("movie"),
        Some(&format!("Film {n:02}")),
    )
    .await
    .map_err(err)?;
    tx.commit().await.map_err(|e| e.to_string())
}

async fn join(
    db: &Database,
    source: MetadataProvider,
    g: &Group,
    member: Member,
) -> kitsunarr_db::DbResult<()> {
    let mut tx = db.pool().begin().await?;
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM collections WHERE kind = 'animeGroup' AND metadata_provider = ? AND metadata_id = ?",
    )
    .bind(source.as_str())
    .bind(g.root.to_string())
    .fetch_optional(&mut *tx)
    .await?;
    let collection = match existing {
        Some(id) => id,
        None => {
            collections::insert(
                &mut tx,
                &NewCollection {
                    title: g.title.clone(),
                    kind: "animeGroup".into(),
                    metadata_provider: Some(source),
                    metadata_id: Some(g.root.to_string()),
                    overview: None,
                    images: Vec::new(),
                },
            )
            .await?
        }
    };
    // Seasons first, then films (Tanukarr orders Season → Movie → Special).
    let (position, role, label) = match g.member_kind {
        MemberKind::Season => (g.ordinal, "season", format!("Season {}", g.ordinal)),
        MemberKind::Movie => (1000 + g.ordinal, "movie", format!("Movie {}", g.ordinal)),
    };
    collections::add_member(
        &mut tx,
        collection,
        member,
        position,
        Some(role),
        Some(&label),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use kitsunarr_metadata::{AnimeNode, AnimeRelation, MetadataError};

    use super::*;

    fn node(id: i64, format: &str, rels: &[(&str, i64, &str)]) -> AnimeNode {
        AnimeNode {
            id,
            format: format.into(),
            title: format!("Title {id}"),
            relations: rels
                .iter()
                .map(|(k, i, f)| AnimeRelation {
                    kind: (*k).into(),
                    id: *i,
                    format: (*f).into(),
                    title: format!("Title {i}"),
                })
                .collect(),
        }
    }

    async fn run(nodes: Vec<AnimeNode>, start: i64) -> Resolution {
        let map: HashMap<i64, AnimeNode> = nodes.into_iter().map(|n| (n.id, n)).collect();
        resolve(start, |id| {
            let n = map.get(&id).cloned();
            async move { n.ok_or(MetadataError::NotFound(MetadataProvider::AniList)) }
        })
        .await
        .unwrap()
    }

    /// TV 1 → film 2 → TV 3: Season 1, Movie 1, Season 2 (Tanukarr's example).
    fn franchise() -> Vec<AnimeNode> {
        vec![
            node(1, "TV", &[("SEQUEL", 2, "MOVIE")]),
            node(2, "MOVIE", &[("PREQUEL", 1, "TV"), ("SEQUEL", 3, "TV")]),
            node(
                3,
                "TV",
                &[("PREQUEL", 2, "MOVIE"), ("SIDE_STORY", 9, "OVA")],
            ),
        ]
    }

    #[tokio::test]
    async fn chains_number_seasons_and_films_from_the_root() {
        let g = |r: Resolution| match r {
            Resolution::Resolved(g) => (g.id, g.member_kind, g.ordinal),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            g(run(franchise(), 3).await),
            ("anilist:1".into(), MemberKind::Season, 2)
        );
        assert_eq!(
            g(run(franchise(), 2).await),
            ("anilist:1".into(), MemberKind::Movie, 1)
        );
        assert_eq!(
            g(run(franchise(), 1).await),
            ("anilist:1".into(), MemberKind::Season, 1)
        );
    }

    #[tokio::test]
    async fn standalone_unsupported_and_ambiguous_titles_stay_ungrouped() {
        assert_eq!(
            run(vec![node(5, "TV", &[])], 5).await,
            Resolution::Unrelated
        );
        assert_eq!(
            run(vec![node(6, "OVA", &[("PREQUEL", 1, "TV")])], 6).await,
            Resolution::Unsupported
        );
        // Two prequels that are both main-line entries.
        let two = vec![
            node(10, "TV", &[("SEQUEL", 12, "TV")]),
            node(11, "TV", &[("SEQUEL", 12, "TV")]),
            node(12, "TV", &[("PREQUEL", 10, "TV"), ("PREQUEL", 11, "TV")]),
        ];
        assert!(matches!(run(two, 12).await, Resolution::Ambiguous(_)));
        // Not reciprocal: the prequel names another sequel.
        let one_way = vec![
            node(20, "TV", &[("SEQUEL", 99, "TV")]),
            node(21, "TV", &[("PREQUEL", 20, "TV")]),
        ];
        assert!(matches!(run(one_way, 21).await, Resolution::Ambiguous(_)));
        // A side story listed as a second prequel is dropped.
        let side = vec![
            node(30, "TV", &[("SEQUEL", 32, "TV"), ("SIDE_STORY", 31, "ONA")]),
            node(31, "ONA", &[("PARENT", 30, "TV"), ("SEQUEL", 32, "TV")]),
            node(32, "TV", &[("PREQUEL", 30, "TV"), ("PREQUEL", 31, "ONA")]),
        ];
        assert!(
            matches!(run(side, 32).await, Resolution::Resolved(g) if g.ordinal == 2 && g.root == 30)
        );
    }
}
