//! Sonarr's "Set permissions" media management options (`SetPermissionsLinux`, `ChmodFolder`,
//! `ChownGroup`): `DiskProvider.SetPermissions` gives folders the mode and files the mode
//! without its execute bits, then changes the group.

use std::path::Path;

use kitsunarr_domain::api::settings::MediaManagementSettings;

/// `MediaFileAttributeService.SetFilePermissions` / `SetFolderPermissions`: nothing unless
/// "set permissions" is on (Linux and other Unix systems).
pub fn apply(path: &Path, media: &MediaManagementSettings) {
    if !media.set_permissions {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(mode) = u32::from_str_radix(media.chmod_folder.trim(), 8) {
            let mode = if path.is_dir() { mode } else { mode & !0o111 };
            if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
                tracing::warn!(path = %path.display(), error = %e, "could not set the permissions");
            }
        }
        let group = media.chown_group.trim();
        if !group.is_empty() {
            match group_id(
                group,
                &std::fs::read_to_string("/etc/group").unwrap_or_default(),
            ) {
                Some(gid) => {
                    if let Err(e) = std::os::unix::fs::chown(path, None, Some(gid)) {
                        tracing::warn!(path = %path.display(), group, error = %e, "could not change the group");
                    }
                }
                None => tracing::warn!(group, "unknown group, the owner was not changed"),
            }
        }
    }
}

/// A group id given as a number, or a group name from an `/etc/group` listing.
pub(crate) fn group_id(group: &str, listing: &str) -> Option<u32> {
    if let Ok(gid) = group.parse() {
        return Some(gid);
    }
    listing.lines().find_map(|line| {
        let mut parts = line.split(':');
        let name = parts.next()?;
        let _password = parts.next()?;
        let gid = parts.next()?;
        (name == group).then(|| gid.trim().parse().ok()).flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_by_number_or_name() {
        let listing = "root:x:0:\nmedia:x:1001:alice,bob\n# comment\nusers:x:100:";
        assert_eq!(group_id("1001", ""), Some(1001));
        assert_eq!(group_id("media", listing), Some(1001));
        assert_eq!(group_id("users", listing), Some(100));
        assert_eq!(group_id("nobody-here", listing), None);
    }

    #[cfg(unix)]
    #[test]
    fn folders_get_the_mode_and_files_lose_the_execute_bits() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Season 01");
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("a.mkv");
        std::fs::write(&file, b"x").unwrap();
        let mut media = MediaManagementSettings {
            chmod_folder: "750".into(),
            ..Default::default()
        };
        apply(&file, &media);
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_ne!(mode(&file), 0o640, "off by default");
        media.set_permissions = true;
        // The current group, by number: changing to it is always allowed.
        media.chown_group =
            std::os::unix::fs::MetadataExt::gid(&std::fs::metadata(&file).unwrap()).to_string();
        apply(&file, &media);
        apply(&folder, &media);
        assert_eq!((mode(&file), mode(&folder)), (0o640, 0o750));
    }
}
