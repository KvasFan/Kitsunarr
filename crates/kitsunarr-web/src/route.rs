//! Client-side routes.

use dioxus::prelude::*;

use crate::components::shell::AppShell;
use crate::pages::activity::{Activity, Blocklist, Queue};
use crate::pages::add::AddMedia;
use crate::pages::auth::{Login, Setup};
use crate::pages::calendar::Calendar;
use crate::pages::collections::Collections;
use crate::pages::dashboard::Dashboard;
use crate::pages::detail::{MovieDetailPage, SeriesDetailPage};
use crate::pages::import::ImportWizard;
use crate::pages::library::{AnimeLibrary, MoviesLibrary, SeriesLibrary};
use crate::pages::placeholders::NotFound;
use crate::pages::search::InteractiveSearch;
use crate::pages::settings::{
    SettingsDownloadClients, SettingsFormats, SettingsGeneral, SettingsImportLists,
    SettingsIndexers, SettingsMedia, SettingsMetadata, SettingsNotifications, SettingsProfiles,
    SettingsTags,
};
use crate::pages::system::{
    SystemBackups, SystemLibraryTools, SystemLogs, SystemStatusPage, SystemTasks,
};
use crate::pages::wanted::{WantedCutoff, WantedMissing};

#[derive(Routable, Clone, PartialEq, Debug)]
#[rustfmt::skip]
pub enum Route {
    #[route("/login")]
    Login {},
    #[route("/setup")]
    Setup {},
    #[layout(AppShell)]
        #[route("/")]
        Dashboard {},
        #[route("/series")]
        SeriesLibrary {},
        #[route("/anime")]
        AnimeLibrary {},
        #[route("/series/:id")]
        SeriesDetailPage { id: i64 },
        #[route("/movies")]
        MoviesLibrary {},
        #[route("/movies/:id")]
        MovieDetailPage { id: i64 },
        #[route("/collections")]
        Collections {},
        #[route("/calendar")]
        Calendar {},
        #[route("/activity/queue")]
        Queue {},
        #[route("/activity/history")]
        Activity {},
        #[route("/activity/blocklist")]
        Blocklist {},
        #[route("/wanted/missing")]
        WantedMissing {},
        #[route("/wanted/cutoffunmet")]
        WantedCutoff {},
        #[route("/search")]
        InteractiveSearch {},
        #[route("/add")]
        AddMedia {},
        #[route("/import")]
        ImportWizard {},
        #[route("/settings")]
        SettingsGeneral {},
        #[route("/settings/media")]
        SettingsMedia {},
        #[route("/settings/profiles")]
        SettingsProfiles {},
        #[route("/settings/customformats")]
        SettingsFormats {},
        #[route("/settings/indexers")]
        SettingsIndexers {},
        #[route("/settings/downloadclients")]
        SettingsDownloadClients {},
        #[route("/settings/notifications")]
        SettingsNotifications {},
        #[route("/settings/importlists")]
        SettingsImportLists {},
        #[route("/settings/metadata")]
        SettingsMetadata {},
        #[route("/settings/tags")]
        SettingsTags {},
        #[route("/system")]
        SystemStatusPage {},
        #[route("/system/tasks")]
        SystemTasks {},
        #[route("/system/library")]
        SystemLibraryTools {},
        #[route("/system/backups")]
        SystemBackups {},
        #[route("/system/logs")]
        SystemLogs {},
        #[route("/:..segments")]
        NotFound { segments: Vec<String> },
}
