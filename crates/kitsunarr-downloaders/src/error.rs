//! Download client errors (Sonarr's `DownloadClientException` family).

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DownloadError {
    #[error("authentication failed; check the username and password (or API key)")]
    Authentication,
    #[error("could not connect: {0}")]
    Connection(String),
    #[error("the download client answered HTTP {status}")]
    Http { status: u16 },
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Client(String),
}

/// The innermost cause of a request error, without the URL.
pub(crate) fn cause(e: &reqwest::Error) -> String {
    use std::error::Error as _;
    if e.is_timeout() {
        return "the request timed out".into();
    }
    let mut innermost = None;
    let mut source = e.source();
    while let Some(s) = source {
        innermost = Some(s.to_string());
        source = s.source();
    }
    match (innermost, e.url()) {
        (Some(m), _) if !m.is_empty() => m,
        (_, Some(url)) => e.to_string().replace(url.as_str(), "<redacted>"),
        _ => e.to_string(),
    }
}
