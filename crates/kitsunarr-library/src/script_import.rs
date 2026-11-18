//! Sonarr's script import (`ImportScriptService`, media management's "Import using script"):
//! a user script receives the source and destination paths (and Sonarr's `Sonarr_*`
//! environment variables), then copies, moves or converts the file itself and reports what it
//! did on its output.

use std::path::{Path, PathBuf};

/// `ScriptImportDecision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptDecision {
    /// The script put the file in place.
    MoveComplete,
    /// The script put the file in place and asks Kitsunarr to rename it (after a conversion).
    RenameRequested,
    /// The script left the file alone: Kitsunarr transfers it as usual.
    DeferMove,
}

/// `ScriptImportInfo`: what the script's output says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptOutcome {
    pub decision: ScriptDecision,
    /// `[MediaFile] <path>`: where the imported file ended up (the destination otherwise).
    pub media_file: Option<PathBuf>,
    /// `[ExtraFile] <path>` lines.
    pub extra_files: Vec<PathBuf>,
    /// False after `[PreventExtraImport]`.
    pub import_extra_files: bool,
}

/// `ProcessOutput`: reads the script's output lines (`OutputRegex`). A media file must have a
/// video extension and exist; only one may be given.
pub fn parse_output<S: AsRef<str>>(lines: &[S]) -> Result<ScriptOutcome, String> {
    let mut out = ScriptOutcome {
        decision: ScriptDecision::MoveComplete,
        media_file: None,
        extra_files: Vec::new(),
        import_extra_files: true,
    };
    for line in lines {
        let line = line.as_ref();
        let file = |tag: &str| {
            line.strip_prefix(tag)
                .map(|rest| rest.strip_prefix(char::is_whitespace).unwrap_or(rest))
                .filter(|rest| !rest.is_empty())
        };
        if let Some(name) = file("[MediaFile]") {
            if out.media_file.is_some() {
                return Err(
                    "Script output contains multiple media files. Only one media file can be returned."
                        .into(),
                );
            }
            let path = PathBuf::from(name);
            if !crate::import::VIDEO_EXTENSIONS.contains(&crate::import::extension(&path).as_str())
            {
                return Err(format!("Script output contains invalid media file: {name}"));
            }
            if !path.is_file() {
                return Err(format!(
                    "Script output contains non-existent media file: {name}"
                ));
            }
            out.media_file = Some(path);
        } else if let Some(name) = file("[ExtraFile]") {
            if !Path::new(name).is_file() {
                tracing::warn!(
                    file = name,
                    "script output contains non-existent possible extra file"
                );
            }
            out.extra_files.push(PathBuf::from(name));
        } else if line == "[PreventExtraImport]" {
            out.import_extra_files = false;
        } else if let Some(status) = line.strip_prefix("[MoveStatus]") {
            match status.strip_prefix(char::is_whitespace).unwrap_or(status) {
                "MoveComplete" => out.decision = ScriptDecision::MoveComplete,
                "RenameRequested" => out.decision = ScriptDecision::RenameRequested,
                "DeferMove" => out.decision = ScriptDecision::DeferMove,
                _ => {}
            }
        }
    }
    Ok(out)
}

/// `TryImport`: runs the script with the source and destination as its arguments and the
/// given environment, and reads its output. A non-zero exit code fails the import.
pub async fn run(
    script: &Path,
    source: &Path,
    destination: &Path,
    env: &[(String, String)],
) -> Result<ScriptOutcome, String> {
    tracing::debug!(script = %script.display(), "executing the import script");
    let output = tokio::process::Command::new(script)
        .arg(source)
        .arg(destination)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("Unable to run the import script {}: {e}", script.display()))?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let (stdout, stderr) = (text(&output.stdout), text(&output.stderr));
    tracing::debug!(%stdout, %stderr, "import script output");
    if !output.status.success() {
        return Err(format!(
            "Script exited with non-zero exit code: {}",
            output.status.code().unwrap_or(-1)
        ));
    }
    let lines: Vec<&str> = stdout
        .lines()
        .chain(stderr.lines())
        .map(|l| l.trim_end_matches('\r'))
        .collect();
    parse_output(&lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_says_what_the_script_did() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("a.mp4");
        std::fs::write(&video, b"x").unwrap();
        let none: [&str; 0] = [];
        let out = parse_output(&none).unwrap();
        assert_eq!(
            (out.decision, out.media_file, out.import_extra_files),
            (ScriptDecision::MoveComplete, None, true)
        );
        let lines = [
            format!("[MediaFile] {}", video.display()),
            "[ExtraFile] /nowhere/a.srt".to_string(),
            "[PreventExtraImport]".to_string(),
            "[MoveStatus] RenameRequested".to_string(),
            "unrelated chatter".to_string(),
        ];
        let out = parse_output(&lines).unwrap();
        assert_eq!(out.decision, ScriptDecision::RenameRequested);
        assert_eq!(out.media_file.as_deref(), Some(video.as_path()));
        assert_eq!(out.extra_files, vec![PathBuf::from("/nowhere/a.srt")]);
        assert!(!out.import_extra_files);
        assert_eq!(
            parse_output(&["[MoveStatus]DeferMove"]).unwrap().decision,
            ScriptDecision::DeferMove
        );
    }

    #[test]
    fn bad_media_files_fail_the_import() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("a.txt");
        std::fs::write(&text, b"x").unwrap();
        assert!(
            parse_output(&[format!("[MediaFile] {}", text.display())])
                .unwrap_err()
                .starts_with("Script output contains invalid media file")
        );
        assert!(
            parse_output(&["[MediaFile] /nowhere/a.mkv"])
                .unwrap_err()
                .starts_with("Script output contains non-existent media file")
        );
        let video = dir.path().join("a.mkv");
        std::fs::write(&video, b"x").unwrap();
        let twice = [
            format!("[MediaFile] {}", video.display()),
            format!("[MediaFile] {}", video.display()),
        ];
        assert!(
            parse_output(&twice)
                .unwrap_err()
                .contains("multiple media files")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_script_gets_both_paths_and_its_exit_code_counts() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("import.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\ncp \"$1\" \"$2\"\necho \"[MoveStatus] MoveComplete\"\n[ \"$Sonarr_Series_Title\" = Fail ] && exit 3\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (src, dest) = (dir.path().join("in.mkv"), dir.path().join("out.mkv"));
        std::fs::write(&src, b"video").unwrap();
        let env = vec![("Sonarr_Series_Title".to_string(), "Show".to_string())];
        let out = run(&script, &src, &dest, &env).await.unwrap();
        assert_eq!(out.decision, ScriptDecision::MoveComplete);
        assert_eq!(std::fs::read(&dest).unwrap(), b"video");
        let env = vec![("Sonarr_Series_Title".to_string(), "Fail".to_string())];
        assert_eq!(
            run(&script, &src, &dest, &env).await.unwrap_err(),
            "Script exited with non-zero exit code: 3"
        );
    }
}
