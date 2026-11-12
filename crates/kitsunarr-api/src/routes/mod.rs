//! Route registration. Each handler carries a `#[utoipa::path]` annotation; registering it
//! through `routes!` keeps the router and the OpenAPI document identical.

pub mod events;
pub mod grab;
pub mod import_lists;
pub mod library;
pub mod library_tools;
pub mod metadata;
pub mod migration;
pub mod queue;
pub mod rules;
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
        .routes(routes!(library::rescan_series))
        .routes(routes!(
            library::rename_preview,
            library::rename_series_files
        ))
        .routes(routes!(library::delete_episode_file))
        .routes(routes!(library::list_episodes))
        .routes(routes!(library::set_episodes_monitored))
        .routes(routes!(library::list_movies))
        .routes(routes!(
            library::get_movie,
            library::update_movie,
            library::delete_movie
        ))
        .routes(routes!(library::bulk_edit_movies))
        .routes(routes!(
            library::list_collections,
            library::create_collection
        ))
        .routes(routes!(
            library::get_collection,
            library::update_collection,
            library::delete_collection
        ))
        .routes(routes!(library::add_collection_member))
        .routes(routes!(
            library::update_collection_member,
            library::remove_collection_member
        ))
        .routes(routes!(library::calendar))
        .routes(routes!(library::history))
        // metadata
        .routes(routes!(metadata::providers))
        .routes(routes!(metadata::search))
        .routes(routes!(metadata::add))
        .routes(routes!(metadata::folder_preview))
        .routes(routes!(metadata::refresh_series))
        .routes(routes!(metadata::refresh_movie))
        .routes(routes!(metadata::series_orderings))
        .routes(routes!(metadata::set_series_ordering))
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
            settings::update_quality_definitions,
            settings::create_custom_quality
        ))
        .routes(routes!(settings::retire_quality))
        .routes(routes!(settings::restore_quality))
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
        .routes(routes!(grab::notification_choices))
        .routes(routes!(import_lists::test_import_list))
        .routes(routes!(import_lists::sync_now))
        .routes(routes!(import_lists::status))
        .routes(routes!(import_lists::entries))
        .routes(routes!(import_lists::add_items))
        .routes(routes!(
            import_lists::list_exclusions,
            import_lists::create_exclusion
        ))
        .routes(routes!(import_lists::delete_exclusion))
        .routes(routes!(
            library_tools::get_settings,
            library_tools::put_settings
        ))
        .routes(routes!(library_tools::searches))
        .routes(routes!(library_tools::storage))
        .routes(routes!(library_tools::audit))
        .routes(routes!(library_tools::recycle))
        .routes(routes!(library_tools::restore))
        .routes(routes!(library_tools::list_keys, library_tools::create_key))
        .routes(routes!(rules::list_delay, rules::create_delay))
        .routes(routes!(rules::update_delay, rules::delete_delay))
        .routes(routes!(rules::list_release, rules::create_release))
        .routes(routes!(rules::update_release, rules::delete_release))
        .routes(routes!(rules::list_mappings, rules::create_mapping))
        .routes(routes!(rules::update_mapping, rules::delete_mapping))
        .routes(routes!(library_tools::versions))
        .routes(routes!(
            library_tools::manual_import_scan,
            library_tools::manual_import
        ))
        .routes(routes!(library_tools::activate_version))
        .routes(routes!(library_tools::delete_version))
        .routes(routes!(library_tools::delete_key))
        .routes(routes!(
            library_tools::get_series_naming,
            library_tools::put_series_naming
        ))
        .routes(routes!(
            import_lists::get_options,
            import_lists::put_options
        ))
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
