//! Route registration. Each handler carries a `#[utoipa::path]` annotation; registering it
//! through `routes!` keeps the router and the OpenAPI document identical.

pub mod events;
pub mod grab;
pub mod library;
pub mod metadata;
pub mod migration;
pub mod queue;
pub mod schema;
pub mod search;
pub mod settings;
pub mod system;
pub mod wanted;

use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::{AppState, auth};

/// Routes reachable without authentication.
pub fn public_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(auth::status))
        .routes(routes!(auth::setup))
        .routes(routes!(auth::login))
        .routes(routes!(system::ping))
}

/// Routes requiring a session or API key.
pub fn protected_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        // auth
        .routes(routes!(auth::logout))
        .routes(routes!(auth::change_password))
        .routes(routes!(auth::get_api_key))
        .routes(routes!(auth::regenerate_api_key))
        // system
        .routes(routes!(system::status))
        .routes(routes!(system::health))
        .routes(routes!(system::tasks))
        .routes(routes!(system::run_task))
        .routes(routes!(system::task_runs))
        .routes(routes!(system::backups, system::create_backup))
        .routes(routes!(system::logs))
        .routes(routes!(events::stream))
        // library
        .routes(routes!(library::stats))
        .routes(routes!(library::list_series))
        .routes(routes!(
            library::get_series,
            library::update_series,
            library::delete_series
        ))
        .routes(routes!(library::bulk_edit_series))
        .routes(routes!(library::list_episodes))
        .routes(routes!(library::set_episodes_monitored))
        .routes(routes!(library::list_movies))
        .routes(routes!(
            library::get_movie,
            library::update_movie,
            library::delete_movie
        ))
        .routes(routes!(library::bulk_edit_movies))
        .routes(routes!(library::list_collections))
        .routes(routes!(library::calendar))
        .routes(routes!(library::history))
        // metadata
        .routes(routes!(metadata::providers))
        .routes(routes!(metadata::search))
        .routes(routes!(metadata::add))
        .routes(routes!(metadata::folder_preview))
        .routes(routes!(metadata::refresh_series))
        .routes(routes!(metadata::refresh_movie))
        // settings
        .routes(routes!(settings::list_tags, settings::create_tag))
        .routes(routes!(settings::delete_tag))
        .routes(routes!(
            settings::list_root_folders,
            settings::create_root_folder
        ))
        .routes(routes!(settings::delete_root_folder))
        .routes(routes!(
            settings::list_quality_definitions,
            settings::update_quality_definitions
        ))
        .routes(routes!(
            settings::list_quality_profiles,
            settings::create_quality_profile
        ))
        .routes(routes!(
            settings::get_quality_profile,
            settings::update_quality_profile,
            settings::delete_quality_profile
        ))
        .routes(routes!(
            settings::list_custom_formats,
            settings::create_custom_format
        ))
        .routes(routes!(
            settings::update_custom_format,
            settings::delete_custom_format
        ))
        .routes(routes!(settings::list_providers, settings::create_provider))
        .routes(routes!(schema::provider_schema))
        .routes(routes!(
            settings::get_provider,
            settings::update_provider,
            settings::delete_provider
        ))
        .routes(routes!(settings::get_naming, settings::put_naming))
        .routes(routes!(
            settings::get_media_management,
            settings::put_media_management
        ))
        .routes(routes!(
            settings::get_indexer_options,
            settings::put_indexer_options
        ))
        .routes(routes!(settings::get_general, settings::put_general))
        .routes(routes!(
            settings::get_metadata_sources,
            settings::put_metadata_sources
        ))
        .routes(routes!(
            settings::get_download_client_options,
            settings::put_download_client_options
        ))
        .routes(routes!(settings::catalogues))
        // search
        .routes(routes!(search::test_indexer))
        .routes(routes!(search::search_releases))
        .routes(routes!(grab::grab_release))
        .routes(routes!(search::automatic_search))
        .routes(routes!(grab::test_download_client))
        .routes(routes!(grab::test_notification))
        .routes(routes!(queue::get_queue))
        .routes(routes!(queue::refresh_queue))
        .routes(routes!(queue::remove_from_queue))
        .routes(routes!(queue::list_blocklist))
        .routes(routes!(queue::delete_blocklist))
        .routes(routes!(queue::remove_blocklist))
        .routes(routes!(queue::mark_history_failed))
        .routes(routes!(wanted::missing))
        .routes(routes!(wanted::cutoff_unmet))
        .routes(routes!(wanted::search_wanted))
        // migration
        .routes(routes!(migration::create_api_session))
        .routes(routes!(migration::upload_backup))
        .routes(routes!(migration::get_session, migration::discard_session))
        .routes(routes!(migration::plan))
        .routes(routes!(migration::apply))
        .routes(routes!(migration::runs))
        .routes(routes!(migration::run_report))
}
