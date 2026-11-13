//! Custom Script (`CustomScript`): runs a program with Sonarr's `Sonarr_*` environment
//! variables, so scripts written for Sonarr keep working. The variables are read from the
//! Sonarr-compatible webhook payload of the event.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::{Event, Instance, NotifyError, NotifyResult};

fn s(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn join(list: &Value, key: &str, sep: &str) -> String {
    list.as_array()
        .map(|a| a.iter().map(|e| s(&e[key])).collect::<Vec<_>>().join(sep))
        .unwrap_or_default()
}

/// Sonarr's environment variables for an event.
pub fn environment(event: &Event, instance: &Instance) -> BTreeMap<String, String> {
    let p = crate::webhook::payload(event, instance);
    let mut env = BTreeMap::new();
    let mut put = |k: &str, v: String| {
        env.insert(format!("Sonarr_{k}"), v);
    };
    let event_type = s(&p["eventType"]);
    put("EventType", event_type.clone());
    put("InstanceName", s(&p["instanceName"]));
    put("ApplicationUrl", s(&p["applicationUrl"]));
    let series = &p["series"];
    if series.is_object() {
        put("Series_Id", s(&series["id"]));
        put("Series_Title", s(&series["title"]));
        put("Series_TitleSlug", s(&series["titleSlug"]));
        put("Series_Path", s(&series["path"]));
        put("Series_TvdbId", s(&series["tvdbId"]));
        put("Series_TvMazeId", s(&series["tvMazeId"]));
        put("Series_TmdbId", s(&series["tmdbId"]));
        put("Series_ImdbId", s(&series["imdbId"]));
        put("Series_Type", s(&series["type"]));
        put("Series_Year", s(&series["year"]));
        put("Series_OriginalCountry", s(&series["originalCountry"]));
        put(
            "Series_Genres",
            series["genres"]
                .as_array()
                .map(|g| g.iter().map(s).collect::<Vec<_>>().join("|"))
                .unwrap_or_default(),
        );
        put(
            "Series_Tags",
            series["tags"]
                .as_array()
                .map(|g| g.iter().map(s).collect::<Vec<_>>().join("|"))
                .unwrap_or_default(),
        );
    }
    let episodes = &p["episodes"];
    let episode_vars = |prefix: &str, put: &mut dyn FnMut(&str, String)| {
        put(
            &format!("{prefix}_EpisodeCount"),
            episodes.as_array().map_or(0, Vec::len).to_string(),
        );
        put(
            &format!("{prefix}_SeasonNumber"),
            s(&episodes[0]["seasonNumber"]),
        );
        put(
            &format!("{prefix}_EpisodeNumbers"),
            join(episodes, "episodeNumber", ","),
        );
        put(
            &format!("{prefix}_EpisodeAirDates"),
            join(episodes, "airDate", ","),
        );
        put(
            &format!("{prefix}_EpisodeAirDatesUtc"),
            join(episodes, "airDateUtc", ","),
        );
        put(
            &format!("{prefix}_EpisodeTitles"),
            join(episodes, "title", "|"),
        );
        put(
            &format!("{prefix}_EpisodeOverviews"),
            join(episodes, "overview", "|"),
        );
    };
    match event_type.as_str() {
        "Grab" => {
            episode_vars("Release", &mut put);
            let r = &p["release"];
            put("Release_Title", s(&r["releaseTitle"]));
            put("Release_Indexer", s(&r["indexer"]));
            put("Release_Size", s(&r["size"]));
            put("Release_Quality", s(&r["quality"]));
            put("Release_QualityVersion", s(&r["qualityVersion"]));
            put("Release_ReleaseGroup", s(&r["releaseGroup"]));
            put("Release_CustomFormatScore", s(&r["customFormatScore"]));
            put("Download_Client", s(&p["downloadClient"]));
            put("Download_Client_Type", s(&p["downloadClientType"]));
            put("Download_Id", s(&p["downloadId"]));
        }
        "Download" => {
            episode_vars("EpisodeFile", &mut put);
            let f = &p["episodeFile"];
            put(
                "IsUpgrade",
                if p["isUpgrade"] == Value::Bool(true) {
                    "True"
                } else {
                    "False"
                }
                .into(),
            );
            put("EpisodeFile_Id", s(&f["id"]));
            put("EpisodeFile_RelativePath", s(&f["relativePath"]));
            put("EpisodeFile_Path", s(&f["path"]));
            put("EpisodeFile_Quality", s(&f["quality"]));
            put("EpisodeFile_QualityVersion", s(&f["qualityVersion"]));
            put("EpisodeFile_ReleaseGroup", s(&f["releaseGroup"]));
            put("EpisodeFile_SceneName", s(&f["sceneName"]));
            put("EpisodeFile_SourcePath", s(&f["sourcePath"]));
            put("EpisodeFile_EpisodeIds", join(episodes, "id", ","));
            put("Download_Client", s(&p["downloadClient"]));
            put("Download_Client_Type", s(&p["downloadClientType"]));
            put("Download_Id", s(&p["downloadId"]));
            if let Some(d) = p["deletedFiles"].as_array() {
                put(
                    "DeletedRelativePaths",
                    d.iter()
                        .map(|x| s(&x["relativePath"]))
                        .collect::<Vec<_>>()
                        .join("|"),
                );
                put(
                    "DeletedPaths",
                    d.iter()
                        .map(|x| s(&x["path"]))
                        .collect::<Vec<_>>()
                        .join("|"),
                );
            }
        }
        "ManualInteractionRequired" => {
            episode_vars("Download", &mut put);
            let d = &p["downloadInfo"];
            put("Download_Client", s(&p["downloadClient"]));
            put("Download_Client_Type", s(&p["downloadClientType"]));
            put("Download_Id", s(&p["downloadId"]));
            put("Download_Size", s(&d["size"]));
            put("Download_Title", s(&d["title"]));
            put("Download_Quality", s(&d["quality"]));
            put("Download_Quality_Version", s(&d["qualityVersion"]));
        }
        "ApplicationUpdate" => {
            put("Update_Message", s(&p["message"]));
            put("Update_NewVersion", s(&p["newVersion"]));
            put("Update_PreviousVersion", s(&p["previousVersion"]));
        }
        "Rename" => {
            let files = &p["renamedEpisodeFiles"];
            put("EpisodeFile_Ids", join(files, "id", ","));
            put(
                "EpisodeFile_RelativePaths",
                join(files, "relativePath", "|"),
            );
            put("EpisodeFile_Paths", join(files, "path", "|"));
            put(
                "EpisodeFile_PreviousRelativePaths",
                join(files, "previousRelativePath", "|"),
            );
            put(
                "EpisodeFile_PreviousPaths",
                join(files, "previousPath", "|"),
            );
        }
        "SeriesDelete" => put(
            "Series_DeletedFiles",
            if p["deletedFiles"] == Value::Bool(true) {
                "True"
            } else {
                "False"
            }
            .into(),
        ),
        "EpisodeFileDelete" => {
            episode_vars("EpisodeFile", &mut put);
            let f = &p["episodeFile"];
            put("EpisodeFile_DeleteReason", s(&p["deleteReason"]));
            put("EpisodeFile_Id", s(&f["id"]));
            put("EpisodeFile_RelativePath", s(&f["relativePath"]));
            put("EpisodeFile_Path", s(&f["path"]));
            put("EpisodeFile_Quality", s(&f["quality"]));
            put("EpisodeFile_ReleaseGroup", s(&f["releaseGroup"]));
        }
        "Health" | "HealthRestored" => {
            let prefix = if event_type == "Health" {
                "Health_Issue"
            } else {
                "Health_Restored"
            };
            put(&format!("{prefix}_Level"), s(&p["level"]));
            put(&format!("{prefix}_Message"), s(&p["message"]));
            put(&format!("{prefix}_Type"), s(&p["type"]));
            put(&format!("{prefix}_Wiki"), s(&p["wikiUrl"]));
        }
        _ => {}
    }
    env
}

/// Runs the script (at most two minutes); a non-zero exit is a failure.
pub async fn run(
    settings: &Map<String, Value>,
    event: &Event,
    instance: &Instance,
) -> NotifyResult<()> {
    let path = crate::required(settings, "path", "The script path")?;
    let script = std::path::Path::new(&path);
    if !script.is_absolute() || !script.is_file() {
        return Err(NotifyError::Settings(format!(
            "{path} is not an existing file"
        )));
    }
    let args: Vec<String> = crate::text(settings, "arguments")
        .map(|a| a.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let mut cmd = tokio::process::Command::new(script);
    cmd.args(args)
        .envs(environment(event, instance))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let child = cmd
        .spawn()
        .map_err(|e| NotifyError::Settings(format!("the script could not start: {e}")))?;
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| NotifyError::Settings("the script did not finish within two minutes".into()))?
    .map_err(|e| NotifyError::Settings(e.to_string()))?;
    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(NotifyError::Settings(format!(
            "the script exited with {}: {}",
            out.status,
            stderr.lines().last().unwrap_or_default()
        )))
    }
}
