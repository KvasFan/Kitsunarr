//! Title normalisation helpers shared by the library, search and migration code.

/// Lower-case title with articles kept, used for sorting ("The Office" → "office, the" is
/// intentionally *not* done; Sonarr sorts on the lower-cased title without leading articles).
pub fn sort_title(title: &str) -> String {
    let lower = title.trim().to_lowercase();
    for article in ["the ", "a ", "an "] {
        if let Some(rest) = lower.strip_prefix(article)
            && !rest.is_empty()
        {
            return rest.to_string();
        }
    }
    lower
}

/// Alphanumeric-only lower-case form used for title matching ("Marvel's Agents of S.H.I.E.L.D."
/// → "marvelsagentsshield"). Mirrors the intent of Sonarr's `CleanSeriesTitle`: punctuation,
/// whitespace and the words "a", "an", "the", "and", "or", "of" are dropped.
pub fn clean_title(title: &str) -> String {
    let normalized: String = title
        .chars()
        .map(|c| if c == '&' { ' ' } else { c })
        .collect::<String>()
        .to_lowercase();
    let mut out = String::with_capacity(normalized.len());
    for word in normalized.split(|c: char| !c.is_alphanumeric()) {
        if word.is_empty() || matches!(word, "a" | "an" | "the" | "and" | "or" | "of") {
            continue;
        }
        out.push_str(word);
    }
    if out.is_empty() {
        // Titles made only of stop words or symbols still need a stable key.
        normalized.chars().filter(|c| c.is_alphanumeric()).collect()
    } else {
        out
    }
}

/// URL slug: lower-case words joined by dashes.
pub fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut dash = false;
    for c in title.to_lowercase().chars() {
        if c.is_alphanumeric() {
            slug.push(c);
            dash = false;
        } else if !dash && !slug.is_empty() {
            slug.push('-');
            dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_titles() {
        assert_eq!(sort_title("The Office"), "office");
        assert_eq!(sort_title("A"), "a");
        assert_eq!(sort_title("Breaking Bad"), "breaking bad");
    }

    #[test]
    fn clean_titles() {
        assert_eq!(
            clean_title("Marvel's Agents of S.H.I.E.L.D."),
            "marvelsagentsshield"
        );
        assert_eq!(clean_title("The Office (US)"), "officeus");
        assert_eq!(clean_title("Law & Order"), "laworder");
        assert_eq!(clean_title("The"), "the");
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("Cowboy Bebop: The Movie"), "cowboy-bebop-the-movie");
        assert_eq!(slugify("  --Hello  World-- "), "hello-world");
    }
}
