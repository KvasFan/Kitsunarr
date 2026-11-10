//! Library repositories: series (TV/anime), episodes, files, movies and collections.

pub mod collections;
pub mod movies;
pub mod series;
pub mod stats;
pub mod wanted;

/// Escapes `%`, `_` and `\` for use in a `LIKE ... ESCAPE '\'` pattern.
pub(crate) fn like_pattern(term: &str) -> String {
    let mut out = String::with_capacity(term.len() + 2);
    out.push('%');
    for c in term.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// Parses a JSON array of integers produced by `json_group_array` (NULL-safe).
pub(crate) fn int_list(raw: Option<&str>) -> Vec<i64> {
    raw.and_then(|r| serde_json::from_str::<Vec<Option<i64>>>(r).ok())
        .map(|v| v.into_iter().flatten().collect())
        .unwrap_or_default()
}

/// First poster URL from a stored image list.
pub(crate) fn poster_url(images_json: &str) -> Option<String> {
    let images: Vec<kitsunarr_domain::MediaImage> = serde_json::from_str(images_json).ok()?;
    images
        .into_iter()
        .find(|i| i.cover_type.eq_ignore_ascii_case("poster"))
        .map(|i| i.remote_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_escaping() {
        assert_eq!(like_pattern("50%_off\\"), "%50\\%\\_off\\\\%");
    }

    #[test]
    fn int_lists() {
        assert_eq!(int_list(Some("[1,null,3]")), vec![1, 3]);
        assert!(int_list(None).is_empty());
    }
}
