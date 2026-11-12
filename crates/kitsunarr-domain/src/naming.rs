//! Folder and file name rendering (subset of Sonarr's naming tokens).
//!
//! Supported folder tokens: `{Series Title}`, `{Series TitleYear}`, `{Series CleanTitle}`,
//! `{Movie Title}`, `{Movie TitleYear}`, `{Year}`, `{Release Year}`. Unknown tokens are removed.

/// Sonarr's colon replacement formats: 0 delete, 1 dash, 2 space dash, 3 space dash space,
/// 4 smart (": " → " - ", ":" → "-"), 5 custom.
pub fn replace_colons(name: &str, format: i32, custom: &str) -> String {
    match format {
        0 => name.replace(':', ""),
        1 => name.replace(':', "-"),
        2 => name.replace(':', " -"),
        3 => name.replace(':', " - "),
        5 => name.replace(':', custom),
        _ => name.replace(": ", " - ").replace(':', "-"),
    }
}

/// Removes characters that are invalid on common filesystems and trims trailing dots/spaces.
pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | '"' | '/' | '\\' | '|' | '?' | '*') && !c.is_control())
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .trim_end_matches(['.', ' '])
        .trim_start_matches('.')
        .to_string()
}

/// Renders a series or movie folder name.
pub fn folder_name(
    format: &str,
    title: &str,
    year: Option<i32>,
    colon_format: i32,
    custom_colon: &str,
) -> String {
    let title_year = match year {
        Some(y) if !title.contains(&format!("({y})")) => format!("{title} ({y})"),
        _ => title.to_string(),
    };
    let year_text = year.map(|y| y.to_string()).unwrap_or_default();
    let mut out = format
        .replace("{Series TitleYear}", &title_year)
        .replace("{Movie TitleYear}", &title_year)
        .replace("{Series CleanTitle}", &crate::text::clean_title(title))
        .replace("{Series Title}", title)
        .replace("{Movie Title}", title)
        .replace("{Release Year}", &year_text)
        .replace("{Year}", &year_text);
    // Drop any token we do not support rather than leaving braces in a path.
    while let (Some(start), Some(end)) = (out.find('{'), out.find('}')) {
        if end < start {
            break;
        }
        out.replace_range(start..=end, "");
    }
    let out = out.replace("()", "");
    let name = sanitize(&replace_colons(&out, colon_format, custom_colon));
    if name.is_empty() {
        sanitize(&replace_colons(title, colon_format, custom_colon))
    } else {
        name
    }
}

/// Renders a film file name (without extension) from the movie file format: Tanukarr's
/// `MovieFormat` tokens `{Movie Title}`, `{Movie TitleYear}`, `{Movie CleanTitle}`,
/// `{Release Year}`, `{Year}`, `{Quality Full}`, `{Quality Title}`, `{Release Group}`.
pub fn movie_file_name(
    format: &str,
    title: &str,
    year: Option<i32>,
    quality_full: &str,
    release_group: Option<&str>,
    colon_format: i32,
    custom_colon: &str,
) -> String {
    let quality_title = quality_full
        .trim_end_matches(" Proper")
        .trim_end_matches(" REAL")
        .to_string();
    let with_quality = format
        .replace("{Quality Full}", quality_full)
        .replace("{Quality Title}", &quality_title)
        .replace("{Movie CleanTitle}", &crate::text::clean_title(title))
        .replace("{Release Group}", release_group.unwrap_or_default());
    let name = folder_name(&with_quality, title, year, colon_format, custom_colon);
    name.replace("[]", "").trim().to_string()
}

/// Joins a root folder and a child name with the root's separator style.
pub fn join(root: &str, child: &str) -> String {
    let sep = if root.contains('\\') && !root.contains('/') {
        '\\'
    } else {
        '/'
    };
    format!("{}{sep}{child}", root.trim_end_matches(['/', '\\']))
}

#[cfg(test)]
mod tests {
    #[test]
    fn movie_file_names() {
        assert_eq!(
            super::movie_file_name(
                "{Movie Title} ({Release Year}) {Quality Full}",
                "Blade Runner: 2049",
                Some(2017),
                "Bluray-1080p",
                None,
                4,
                ""
            ),
            "Blade Runner - 2049 (2017) Bluray-1080p"
        );
        assert_eq!(
            super::movie_file_name(
                "{Movie TitleYear} [{Quality Title}]-{Release Group}",
                "Up",
                Some(2009),
                "WEBDL-1080p Proper",
                Some("GRP"),
                4,
                ""
            ),
            "Up (2009) [WEBDL-1080p]-GRP"
        );
    }

    use super::*;

    #[test]
    fn renders_folder_names() {
        assert_eq!(
            folder_name("{Series Title}", "Breaking Bad", Some(2008), 4, ""),
            "Breaking Bad"
        );
        assert_eq!(
            folder_name("{Series TitleYear}", "Breaking Bad", Some(2008), 4, ""),
            "Breaking Bad (2008)"
        );
        assert_eq!(
            folder_name("{Series TitleYear}", "Cosmos (2014)", Some(2014), 4, ""),
            "Cosmos (2014)"
        );
        assert_eq!(
            folder_name("{Series Title}", "Re:Zero: Starting Life", None, 4, ""),
            "Re-Zero - Starting Life"
        );
        assert_eq!(
            folder_name("{Series Title} {tvdb-123}", "What If...?", None, 4, ""),
            "What If",
            "trailing dots are invalid on Windows"
        );
        assert_eq!(
            folder_name("{Movie Title} ({Release Year})", "Akira", None, 4, ""),
            "Akira"
        );
        assert_eq!(folder_name("{Series Title}", "A/B: C", None, 0, ""), "AB C");
    }

    #[test]
    fn joins_paths() {
        assert_eq!(join("/media/tv/", "X"), "/media/tv/X");
        assert_eq!(join("D:\\TV", "X"), "D:\\TV\\X");
    }
}
