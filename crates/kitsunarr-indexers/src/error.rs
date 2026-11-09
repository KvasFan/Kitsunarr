//! Indexer errors, mirroring the exceptions Sonarr distinguishes (they drive the status backoff).

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum IndexerError {
    /// The indexer rejected the API key or needs one (Newznab error codes 100–199).
    #[error("API key rejected: {0}")]
    ApiKey(String),
    /// The indexer's request limit was reached ("Request limit reached", HTTP 429).
    #[error("request limit reached")]
    RequestLimit { retry_after_secs: Option<u64> },
    /// An `<error>` element with any other code.
    #[error("indexer error: {0}")]
    Indexer(String),
    /// Name resolution or connection failure (no escalation in the backoff).
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("HTTP {status}")]
    Http { status: u16 },
    /// The response is not well-formed XML.
    #[error("invalid XML: {0}")]
    InvalidXml(String),
    /// Unparseable or unexpected response.
    #[error("unexpected response: {0}")]
    Unexpected(String),
}
