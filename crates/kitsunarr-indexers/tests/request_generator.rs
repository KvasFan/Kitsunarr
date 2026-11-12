//! Sonarr's `NewznabRequestGeneratorFixture`, replayed against the port.

use kitsunarr_indexers::request::search_mode;
use kitsunarr_indexers::{Capabilities, Chain, Criteria, Generator, Search, SeriesIds, Settings};

fn settings() -> Settings {
    Settings {
        base_url: "http://127.0.0.1:1234/".into(),
        api_path: "/api".into(),
        api_key: Some("abcd".into()),
        categories: vec![1, 2],
        anime_categories: vec![3, 4],
        anime_standard_format_search: false,
        additional_parameters: None,
        minimum_seeders: None,
        movie_categories: vec![2000],
    }
}

fn criteria(titles: &[&str]) -> Criteria {
    Criteria {
        ids: SeriesIds {
            tvdb: 20,
            imdb: Some("t40".into()),
            tv_rage: 10,
            tv_maze: 30,
            tmdb: 50,
        },
        scene_titles: titles.iter().map(|t| t.to_string()).collect(),
        search_mode: search_mode::DEFAULT,
        anime: false,
    }
}

const SINGLE: Search = Search::SingleEpisode {
    season: 1,
    episode: 2,
};
const ANIME: Search = Search::AnimeEpisode {
    absolute: 100,
    season: 5,
    episode: 4,
};

fn generator<'a>(s: &'a Settings, c: &'a Capabilities) -> Generator<'a> {
    Generator {
        settings: s,
        caps: c,
        page_size: 100,
    }
}

fn first_urls(chain: &Chain) -> Vec<String> {
    chain
        .all()
        .map(|p| p.pages().first().cloned().unwrap_or_default())
        .collect()
}

fn caps_with(params: &[&str]) -> Capabilities {
    Capabilities {
        tv_search_params: Some(params.iter().map(|p| p.to_string()).collect()),
        ..Default::default()
    }
}

#[test]
fn feed_uses_all_distinct_categories() {
    let caps = Capabilities::default();
    let s = settings();
    let chain = generator(&s, &caps).recent();
    // TV and anime categories, then Tanukarr's generic search on the movie categories.
    assert_eq!(chain.all().count(), 2);
    assert!(first_urls(&chain)[0].contains("&cat=1,2,3,4&"));
    let film = first_urls(&chain)[1].clone();
    assert!(film.contains("?t=search&cat=2000&"), "{film}");
    let mut no_films = settings();
    no_films.movie_categories.clear();
    assert_eq!(generator(&no_films, &caps).recent().all().count(), 1);
    let mut dup = settings();
    dup.categories = vec![1, 2, 3];
    assert!(first_urls(&generator(&dup, &caps).recent())[0].contains("&cat=1,2,3,4&"));
}

#[test]
fn anime_search_uses_anime_categories_absolute_numbers_and_paging() {
    let caps = Capabilities::default();
    let s = settings();
    let chain = generator(&s, &caps).search(&criteria(&["Monkey+Island"]), &ANIME);
    let urls = first_urls(&chain);
    assert_eq!(urls.len(), 2);
    assert!(urls.iter().all(|u| u.contains("&cat=3,4&")));
    assert!(urls[0].contains("?t=tvsearch&"));
    assert!(urls[1].contains("?t=search&"));
    assert!(urls[0].contains("rid=10&q=100"));
    assert!(urls[1].contains("q=Monkey%20Island+100"));
    let pages = chain.all().next().unwrap().pages();
    assert!(pages[0].contains("&offset=0&"));
    assert!(pages[1].contains("&offset=100&"));
    assert!(pages[2].contains("&offset=200&"));
    assert!(pages.len() < 500);
}

#[test]
fn anime_standard_numbering() {
    let caps = Capabilities::default();
    let mut s = settings();
    s.anime_standard_format_search = true;
    let chain = generator(&s, &caps).search(&criteria(&["Monkey+Island"]), &ANIME);
    assert_eq!(chain.tiers[0].len(), 4);
    let urls = first_urls(&chain);
    assert!(urls[0].contains("rid=10&q=100"));
    assert!(urls[1].contains("rid=10&season=5&ep=4"));
    assert!(urls[2].contains("q=Monkey%20Island+100"));
    assert!(urls[3].contains("q=Monkey%20Island&season=5&ep=4"));

    let season = Search::AnimeSeason {
        season: 3,
        season_scene_titles: vec![],
    };
    let chain = generator(&s, &caps).search(&criteria(&["Monkey Island"]), &season);
    assert_eq!(chain.tiers[0].len(), 2);
    let urls = first_urls(&chain);
    assert!(urls[0].contains("rid=10&season=3"));
    assert!(urls[1].contains("q=Monkey%20Island&season=3"));
}

#[test]
fn anime_season_title_aliases() {
    let caps = Capabilities::default();
    let mut s = settings();
    s.anime_standard_format_search = true;
    let alias = Search::AnimeSeason {
        season: 3,
        season_scene_titles: vec!["Monkey Island".into()],
    };
    let urls = first_urls(&generator(&s, &caps).search(&criteria(&["Monkey Island"]), &alias));
    assert_eq!(urls.iter().filter(|u| u.contains("q=")).count(), 1);
    assert!(urls.iter().any(|u| u.contains("?t=search&")
        && u.contains("q=Monkey%20Island")
        && !u.contains("season=")));

    s.anime_standard_format_search = false;
    let urls = first_urls(&generator(&s, &caps).search(&criteria(&["Monkey Island"]), &alias));
    assert_eq!(urls.len(), 1);
    assert!(urls[0].contains("q=Monkey%20Island") && !urls[0].contains("season="));

    s.anime_standard_format_search = true;
    let both = Search::AnimeSeason {
        season: 3,
        season_scene_titles: vec!["Monkey Island Revenge".into()],
    };
    let urls = first_urls(&generator(&s, &caps).search(
        &criteria(&["Monkey Island", "Monkey Island Revenge"]),
        &both,
    ));
    assert!(
        urls.iter()
            .any(|u| u.contains("q=Monkey%20Island&season=3"))
    );
    assert!(
        urls.iter()
            .any(|u| u.contains("q=Monkey%20Island%20Revenge") && !u.contains("season="))
    );
}

#[test]
fn id_selection() {
    let s = settings();
    let c = criteria(&["Monkey Island"]);
    let only = |params: &[&str]| first_urls(&generator(&s, &caps_with(params)).search(&c, &SINGLE));

    let urls = only(&["q", "season", "ep"]);
    assert_eq!(urls.len(), 1, "no id support: title search only");
    assert!(!urls[0].contains("rid=10") && urls[0].contains("q=Monkey"));

    let default_caps = Capabilities::default();
    let chain = generator(&s, &default_caps).search(&c, &SINGLE);
    assert_eq!(chain.tiers[0].len(), 1);
    assert!(first_urls(&chain)[0].contains("rid=10"));

    for (param, expect) in [
        ("tvdbid", "tvdbid=20"),
        ("tvmazeid", "tvmazeid=30"),
        ("imdbid", "imdbid=t40"),
        ("tmdbid", "tmdbid=50"),
    ] {
        let caps = caps_with(&["q", param, "season", "ep"]);
        let chain = generator(&s, &caps).search(&c, &SINGLE);
        assert_eq!(chain.tiers[0].len(), 1, "{param}");
        assert!(first_urls(&chain)[0].contains(expect), "{param}");
    }

    let prefer = caps_with(&["q", "tvdbid", "rid", "season", "ep"]);
    let url = &first_urls(&generator(&s, &prefer).search(&c, &SINGLE))[0];
    assert!(
        url.contains("tvdbid=20") && !url.contains("rid=10"),
        "tvdbid preferred over rid"
    );

    let mut aggregate = prefer.clone();
    aggregate.supports_aggregate_id_search = true;
    let chain = generator(&s, &aggregate).search(&c, &SINGLE);
    assert_eq!(chain.tiers[0].len(), 1);
    let url = &first_urls(&chain)[0];
    assert!(
        url.contains("tvdbid=20") && url.contains("rid=10"),
        "aggregated ids"
    );
}

#[test]
fn aggregate_without_usable_ids_falls_back_to_q() {
    let s = settings();
    let mut caps = caps_with(&["q", "season", "ep"]);
    caps.supports_aggregate_id_search = true;
    let chain = generator(&s, &caps).search(&criteria(&["Monkey Island"]), &SINGLE);
    assert_eq!(chain.tiers.len(), 1);
    assert_eq!(chain.tiers[0].len(), 1);
    assert!(first_urls(&chain)[0].contains("q="));

    let mut caps = caps_with(&["q", "rid", "season", "ep"]);
    caps.supports_aggregate_id_search = true;
    let mut no_ids = criteria(&["Monkey Island"]);
    no_ids.ids = SeriesIds::default();
    assert!(first_urls(&generator(&s, &caps).search(&no_ids, &SINGLE))[0].contains("q="));
}

#[test]
fn title_and_q_fallback_tiers_with_exact_escaping() {
    let s = settings();
    let mut caps = caps_with(&["q", "title", "tvdbid", "rid", "season", "ep"]);
    caps.supports_aggregate_id_search = true;
    let chain = generator(&s, &caps).search(&criteria(&["Monkey Island"]), &SINGLE);
    assert_eq!(chain.tiers.len(), 2);
    let tier2 = chain.tiers[1][0].pages()[0].clone();
    assert!(!tier2.contains("tvdbid=20") && !tier2.contains("rid=10") && !tier2.contains("q="));
    assert!(tier2.contains("title=Monkey%20Island"));

    let chain = generator(&s, &caps).search(&criteria(&["Elith & Little"]), &SINGLE);
    assert!(chain.tiers[1][0].pages()[0].contains("title=Elith%20%26%20Little"));

    let mut q = caps_with(&["q", "tvdbid", "rid", "season", "ep"]);
    q.supports_aggregate_id_search = true;
    let chain = generator(&s, &q).search(&criteria(&["Monkey Island"]), &SINGLE);
    assert_eq!(chain.tiers.len(), 2);
    let tier2 = chain.tiers[1][0].pages()[0].clone();
    assert!(!tier2.contains("tvdbid=20") && !tier2.contains("rid=10") && tier2.contains("q="));

    let mut raw = caps_with(&["q", "season", "ep"]);
    raw.tv_text_search_engine = "raw".into();
    let chain = generator(&s, &raw).search(&criteria(&["Edith & Little"]), &SINGLE);
    assert_eq!(chain.tiers.len(), 1);
    let url = &chain.tiers[0][0].pages()[0];
    assert!(url.contains("q=Edith%20%26%20Little") && !url.contains(" & "));

    let sphinx = caps_with(&["q", "season", "ep"]);
    let chain = generator(&s, &sphinx).search(&criteria(&["Edith & Little"]), &SINGLE);
    assert_eq!(chain.tiers.len(), 1);
    let url = &chain.tiers[0][0].pages()[0];
    assert!(url.contains("q=Edith%20and%20Little") && !url.contains("%26"));
}

#[test]
fn season_search_needs_only_the_season_parameter() {
    let s = settings();
    let season = Search::Season { season: 1 };
    let allowed = caps_with(&["q", "season"]);
    assert!(
        generator(&s, &allowed)
            .search(&criteria(&["Monkey Island"]), &season)
            .all()
            .count()
            > 0
    );
    let denied = caps_with(&["q", "ep"]);
    assert_eq!(
        generator(&s, &denied)
            .search(&criteria(&["Monkey Island"]), &season)
            .all()
            .count(),
        0
    );
}

#[test]
fn daily_and_season_zero_formats() {
    let s = settings();
    let caps = Capabilities::default();
    let daily = Search::DailyEpisode {
        date: chrono::NaiveDate::from_ymd_opt(2024, 3, 7).unwrap(),
    };
    assert!(
        first_urls(&generator(&s, &caps).search(&criteria(&["Show"]), &daily))[0]
            .contains("rid=10&season=2024&ep=03/07")
    );
    let special = Search::SingleEpisode {
        season: 0,
        episode: 3,
    };
    assert!(
        first_urls(&generator(&s, &caps).search(&criteria(&["Show"]), &special))[0]
            .contains("&season=00&ep=3")
    );
}
