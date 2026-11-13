//! Scene numbering from TheXEM (Sonarr's `XemService`). After a TheTVDB series in the official
//! order is added or refreshed, its episodes take TheXEM's scene numbers (with the numbering of
//! episodes TheXEM does not list yet extrapolated and flagged unverified), the series switches
//! to scene numbering when TheXEM maps it, and TheXEM's scene names become alternate titles
//! for their scene season.

use kitsunarr_db::Database;
use kitsunarr_domain::MetadataProvider;
use kitsunarr_metadata::Metadata;
use kitsunarr_metadata::xem::{XemMapping, XemName};

/// An episode's numbering, as `PerformUpdate` reads and writes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SceneNumbering {
    pub season_number: i32,
    pub episode_number: i32,
    pub scene_season_number: Option<i32>,
    pub scene_episode_number: Option<i32>,
    pub scene_absolute_episode_number: Option<i32>,
    pub unverified_scene_numbering: bool,
}

impl SceneNumbering {
    pub fn new(season_number: i32, episode_number: i32) -> Self {
        Self {
            season_number,
            episode_number,
            ..Self::default()
        }
    }
}

/// `PerformUpdate` once the mappings are known: clears every scene number, applies TheXEM's
/// (skipping entries without any scene number), then extrapolates the episodes TheXEM does not
/// list yet.
pub fn apply_mappings(episodes: &mut [SceneNumbering], mappings: &[XemMapping]) {
    for e in episodes.iter_mut() {
        e.scene_absolute_episode_number = None;
        e.scene_season_number = None;
        e.scene_episode_number = None;
        e.unverified_scene_numbering = false;
    }
    for m in mappings {
        // TheTVDB has not listed the episode yet.
        let Some(e) = episodes
            .iter_mut()
            .find(|e| e.season_number == m.tvdb.season && e.episode_number == m.tvdb.episode)
        else {
            continue;
        };
        if m.scene.absolute == 0 && m.scene.season == 0 && m.scene.episode == 0 {
            continue;
        }
        e.scene_absolute_episode_number = Some(m.scene.absolute);
        e.scene_season_number = Some(m.scene.season);
        e.scene_episode_number = Some(m.scene.episode);
    }
    if episodes
        .iter()
        .any(|e| e.scene_episode_number.is_some() && e.scene_season_number != Some(0))
    {
        extrapolate(episodes, mappings);
    }
}

/// `ExtrapolateMappings`.
fn extrapolate(episodes: &mut [SceneNumbering], mappings: &[XemMapping]) {
    use std::collections::{HashMap, HashSet};

    let mapped_seasons: HashSet<i32> = episodes
        .iter()
        .filter(|e| e.season_number != 0 && e.scene_episode_number.is_some())
        .map(|e| e.season_number)
        .collect();
    let mut scene_episodes: HashMap<i32, HashSet<i32>> = HashMap::new();
    let mut first_tvdb_episode: HashMap<i32, i32> = HashMap::new();
    for m in mappings {
        scene_episodes
            .entry(m.scene.season)
            .or_default()
            .insert(m.scene.episode);
        let first = first_tvdb_episode.entry(m.tvdb.season).or_insert(i32::MAX);
        *first = (*first).min(m.tvdb.episode);
    }
    let last_scene_season = mappings.iter().map(|m| m.scene.season).max().unwrap_or(0);
    let last_tvdb_season = mappings.iter().map(|m| m.tvdb.season).max().unwrap_or(0);

    // Every episode TheXEM does not list is unverified when an earlier episode of its season
    // is mapped, a mapping points scene numbers at it, or later seasons are shifted.
    for e in episodes.iter_mut() {
        if e.season_number == 0 || e.scene_episode_number.is_some() {
            continue;
        }
        if mapped_seasons.contains(&e.season_number) {
            if first_tvdb_episode
                .get(&e.season_number)
                .is_some_and(|first| *first <= e.episode_number)
            {
                e.unverified_scene_numbering = true;
                continue;
            }
            if scene_episodes
                .get(&e.season_number)
                .is_some_and(|s| s.contains(&e.episode_number))
            {
                e.unverified_scene_numbering = true;
                continue;
            }
        } else if last_scene_season != last_tvdb_season && e.season_number > last_tvdb_season {
            e.unverified_scene_numbering = true;
        }
    }

    for e in episodes.iter_mut() {
        if e.season_number == 0
            || e.scene_episode_number.is_some()
            || e.season_number < last_tvdb_season
            || !e.unverified_scene_numbering
        {
            continue;
        }
        let season: Vec<&XemMapping> = mappings
            .iter()
            .filter(|m| m.tvdb.season == e.season_number)
            .collect();
        if season.iter().any(|m| m.tvdb.episode >= e.episode_number) {
            continue;
        }
        if let Some(last_episode) = season.iter().max_by_key(|m| m.tvdb.episode) {
            // The last scene episode of that scene season (the last listed, on ties).
            let last_of_scene_season = mappings
                .iter()
                .filter(|m| m.scene.season == last_episode.scene.season)
                .fold(None::<&XemMapping>, |best, m| match best {
                    Some(b) if b.scene.episode > m.scene.episode => Some(b),
                    _ => Some(m),
                });
            if last_of_scene_season.is_some_and(|m| m.tvdb.season == 0) {
                continue;
            }
            let offset = e.episode_number - last_episode.tvdb.episode;
            e.scene_season_number = Some(last_episode.scene.season);
            e.scene_episode_number = Some(last_episode.scene.episode + offset);
            e.scene_absolute_episode_number = Some(last_episode.scene.absolute + offset);
        } else if last_tvdb_season != last_scene_season {
            let offset = e.season_number - last_tvdb_season;
            e.scene_season_number = Some(last_scene_season + offset);
            e.scene_episode_number = Some(e.episode_number);
        }
    }
}

/// What an update did, for logs and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XemOutcome {
    /// Not a TheTVDB series in the official order.
    NotApplicable,
    /// TheXEM could not be asked; nothing was changed.
    Unavailable,
    /// TheXEM has nothing for the series.
    NotMapped,
    /// The scene numbering was written; `true` when the series uses it.
    Updated(bool),
}

/// `XemService.Handle(SeriesUpdatedEvent)` for one series. Failures are logged and leave the
/// numbering as it was.
pub async fn update_series(db: &Database, metadata: &Metadata, id: i64) -> XemOutcome {
    match try_update(db, metadata, id).await {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::warn!(series = id, error = %e, "scene numbering from TheXEM was not updated");
            XemOutcome::Unavailable
        }
    }
}

async fn try_update(db: &Database, metadata: &Metadata, id: i64) -> Result<XemOutcome, String> {
    let row: Option<(String, String, Option<String>, bool, String)> = sqlx::query_as(
        "SELECT metadata_provider, metadata_id, episode_ordering, use_scene_numbering, title FROM series WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db.pool())
    .await
    .map_err(|e| e.to_string())?;
    let Some((provider, metadata_id, ordering, use_scene, title)) = row else {
        return Ok(XemOutcome::NotApplicable);
    };
    // TheXEM is keyed by TheTVDB id and maps TheTVDB's official (aired) order.
    let tvdb_id = metadata_id.trim().parse::<i64>().unwrap_or(0);
    if provider != MetadataProvider::Tvdb.as_str()
        || tvdb_id <= 0
        || ordering.as_deref().is_some_and(|o| o != "official")
    {
        return Ok(XemOutcome::NotApplicable);
    }
    let mapped = match metadata.xem.mapped_series().await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(error = %e, "failed to update the list of series TheXEM maps");
            return Ok(XemOutcome::Unavailable);
        }
    };
    if mapped.is_empty() {
        tracing::warn!("failed to update the list of series TheXEM maps");
        return Ok(XemOutcome::Unavailable);
    }
    // Scene names are applied whenever they can be read; numbering failures do not stop them.
    if let Ok(names) = metadata.xem.names().await {
        let names = names.get(&tvdb_id).map(Vec::as_slice).unwrap_or_default();
        write_names(db, id, &title, names)
            .await
            .map_err(|e| e.to_string())?;
    }
    if !mapped.contains(&tvdb_id) && !use_scene {
        return Ok(XemOutcome::NotMapped);
    }
    let mappings = metadata.xem.mappings(tvdb_id).await?;
    if mappings.is_empty() && !use_scene {
        return Ok(XemOutcome::NotMapped);
    }
    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT id, season_number, episode_number FROM episodes WHERE series_id = ? ORDER BY season_number, episode_number",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await
    .map_err(|e| e.to_string())?;
    let mut episodes: Vec<SceneNumbering> = rows
        .iter()
        .map(|(_, s, e)| SceneNumbering::new(*s as i32, *e as i32))
        .collect();
    apply_mappings(&mut episodes, &mappings);
    let use_scene = !mappings.is_empty();
    let mut tx = db.pool().begin().await.map_err(|e| e.to_string())?;
    for ((episode_id, _, _), e) in rows.iter().zip(&episodes) {
        sqlx::query(
            "UPDATE episodes SET scene_season_number = ?, scene_episode_number = ?, scene_absolute_episode_number = ?, \
             unverified_scene_numbering = ? WHERE id = ?",
        )
        .bind(e.scene_season_number)
        .bind(e.scene_episode_number)
        .bind(e.scene_absolute_episode_number)
        .bind(e.unverified_scene_numbering)
        .bind(episode_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }
    sqlx::query("UPDATE series SET use_scene_numbering = ? WHERE id = ?")
        .bind(use_scene)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    tracing::debug!(
        series = id,
        mappings = mappings.len(),
        "TheXEM mapping updated"
    );
    Ok(XemOutcome::Updated(use_scene))
}

/// Replaces the series' TheXEM names. A name equal to the series title for every season adds
/// nothing; a name already known from the catalogue takes TheXEM's scene season.
async fn write_names(
    db: &Database,
    id: i64,
    title: &str,
    names: &[XemName],
) -> Result<(), sqlx::Error> {
    let clean_series = kitsunarr_domain::text::clean_title(title);
    let mut tx = db.pool().begin().await?;
    sqlx::query("DELETE FROM series_alternate_titles WHERE series_id = ? AND source = 'xem'")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for n in names {
        let clean = kitsunarr_domain::text::clean_title(&n.title);
        if clean.is_empty() || (n.season.is_none() && clean == clean_series) {
            continue;
        }
        sqlx::query(
            "INSERT INTO series_alternate_titles (series_id, title, clean_title, season_number, source) \
             VALUES (?, ?, ?, ?, 'xem') ON CONFLICT (series_id, title) DO UPDATE SET \
             season_number = COALESCE(excluded.season_number, season_number)",
        )
        .bind(id)
        .bind(&n.title)
        .bind(&clean)
        .bind(n.season)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Sonarr's `XemServiceFixture`.
    use super::*;
    use kitsunarr_metadata::xem::XemValues;

    fn episodes() -> Vec<SceneNumbering> {
        [
            (1, 1),
            (1, 2),
            (2, 1),
            (2, 2),
            (2, 3),
            (2, 4),
            (2, 5),
            (3, 1),
            (3, 2),
        ]
        .into_iter()
        .map(|(s, e)| SceneNumbering::new(s, e))
        .collect()
    }

    fn mapping(sa: i32, ss: i32, se: i32, ta: i32, ts: i32, te: i32) -> XemMapping {
        XemMapping {
            scene: XemValues {
                absolute: sa,
                season: ss,
                episode: se,
            },
            tvdb: XemValues {
                absolute: ta,
                season: ts,
                episode: te,
            },
        }
    }

    /// `GivenTvdbMappings`.
    fn tvdb_mappings() -> Vec<XemMapping> {
        vec![
            mapping(1, 1, 1, 1, 1, 1), // 1x01 -> 1x01
            mapping(2, 1, 2, 2, 1, 2), // 1x02 -> 1x02
            mapping(3, 2, 1, 3, 2, 1), // 2x01 -> 2x01
            mapping(4, 2, 2, 4, 2, 2), // 2x02 -> 2x02
            mapping(5, 2, 3, 5, 2, 3), // 2x03 -> 2x03
            mapping(6, 3, 1, 6, 2, 4), // 3x01 -> 2x04
            mapping(7, 3, 2, 7, 2, 5), // 3x02 -> 2x05
        ]
    }

    fn run(mappings: &[XemMapping]) -> Vec<SceneNumbering> {
        let mut e = episodes();
        apply_mappings(&mut e, mappings);
        e
    }

    fn ep(e: &[SceneNumbering], s: i32, n: i32) -> SceneNumbering {
        e.iter()
            .find(|x| x.season_number == s && x.episode_number == n)
            .unwrap()
            .clone()
    }

    #[test]
    fn should_fetch_scenenumbering() {
        let e = run(&tvdb_mappings());
        let x = ep(&e, 2, 4);
        assert_eq!(
            (x.scene_season_number, x.scene_episode_number),
            (Some(3), Some(1))
        );
        assert_eq!(x.scene_absolute_episode_number, Some(6));
    }

    #[test]
    fn should_clear_scenenumbering_if_removed_from_thexem() {
        let mut e = episodes();
        for x in &mut e {
            x.scene_season_number = Some(x.season_number);
            x.scene_episode_number = Some(x.episode_number);
            x.unverified_scene_numbering = true;
        }
        apply_mappings(&mut e, &[]);
        assert!(e.iter().all(|x| x.scene_season_number.is_none()
            && x.scene_episode_number.is_none()
            && !x.unverified_scene_numbering));
    }

    #[test]
    fn should_flag_unknown_future_episodes_if_existing_season_is_mapped() {
        let mut m = tvdb_mappings();
        m.retain(|v| !(v.tvdb.season == 2 && v.tvdb.episode == 5));
        assert!(ep(&run(&m), 2, 5).unverified_scene_numbering);
    }

    #[test]
    fn should_flag_unknown_future_season_if_future_season_is_shifted() {
        assert!(ep(&run(&tvdb_mappings()), 3, 1).unverified_scene_numbering);
    }

    #[test]
    fn should_not_flag_unknown_future_season_if_future_season_is_not_shifted() {
        let mut m = tvdb_mappings();
        m.retain(|v| v.scene.season != 3);
        assert!(!ep(&run(&m), 3, 1).unverified_scene_numbering);
    }

    #[test]
    fn should_not_flag_past_episodes_if_not_causing_overlaps() {
        let mut m = tvdb_mappings();
        m.retain(|v| v.scene.season != 2);
        assert!(!ep(&run(&m), 2, 1).unverified_scene_numbering);
    }

    #[test]
    fn should_flag_past_episodes_if_causing_overlap() {
        let mut m = tvdb_mappings();
        m.retain(|v| !(v.scene.season == 2 && v.tvdb.episode <= 1));
        m.iter_mut()
            .find(|v| v.scene.season == 2 && v.scene.episode == 2)
            .unwrap()
            .scene
            .episode = 1;
        assert!(ep(&run(&m), 2, 1).unverified_scene_numbering);
    }

    #[test]
    fn should_not_extrapolate_season_with_specials() {
        let mut m = tvdb_mappings();
        let special = m
            .iter_mut()
            .find(|v| v.tvdb.season == 2 && v.tvdb.episode == 5)
            .unwrap();
        special.tvdb.season = 0;
        special.tvdb.episode = 1;
        let x = ep(&run(&m), 2, 5);
        assert!(x.unverified_scene_numbering);
        assert_eq!(x.scene_season_number, None);
        assert_eq!(x.scene_episode_number, None);
    }

    #[test]
    fn should_extrapolate_season_with_future_episodes() {
        let mut m = tvdb_mappings();
        m.retain(|v| !(v.tvdb.season == 2 && v.tvdb.episode == 5));
        let x = ep(&run(&m), 2, 5);
        assert!(x.unverified_scene_numbering);
        assert_eq!(x.scene_season_number, Some(3));
        assert_eq!(x.scene_episode_number, Some(2));
    }

    #[test]
    fn should_extrapolate_season_with_shifted_episodes() {
        let mut m = tvdb_mappings();
        m.retain(|v| !(v.tvdb.season == 2 && v.tvdb.episode == 5));
        let dual = m
            .iter_mut()
            .find(|v| v.tvdb.season == 2 && v.tvdb.episode == 4)
            .unwrap();
        dual.scene.season = 2;
        dual.scene.episode = 3;
        let x = ep(&run(&m), 2, 5);
        assert!(x.unverified_scene_numbering);
        assert_eq!(x.scene_season_number, Some(2));
        assert_eq!(x.scene_episode_number, Some(4));
    }

    #[test]
    fn should_extrapolate_shifted_future_seasons() {
        let x = ep(&run(&tvdb_mappings()), 3, 2);
        assert!(x.unverified_scene_numbering);
        assert_eq!(x.scene_season_number, Some(4));
        assert_eq!(x.scene_episode_number, Some(2));
    }

    #[test]
    fn should_not_extrapolate_matching_future_seasons() {
        let mut m = tvdb_mappings();
        m.retain(|v| v.scene.season == 1);
        let x = ep(&run(&m), 3, 2);
        assert!(!x.unverified_scene_numbering);
        assert_eq!(x.scene_season_number, None);
        assert_eq!(x.scene_episode_number, None);
    }

    #[test]
    fn should_skip_mapping_when_scene_information_is_all_zero() {
        let mut m = tvdb_mappings();
        m.push(mapping(0, 0, 0, 8, 3, 1));
        m.push(mapping(0, 0, 0, 9, 3, 2));
        let e = run(&m);
        assert!(!e.iter().any(|x| x.scene_absolute_episode_number == Some(0)
            && x.scene_season_number == Some(0)
            && x.scene_episode_number == Some(0)));
    }
}
