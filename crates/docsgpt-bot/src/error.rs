//! Errors returned by this crate.

/// A boxed error from a backend or platform this crate doesn't know about.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Everything that can go wrong in the bot plumbing.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The configuration is invalid.
    #[error("{0}")]
    Config(String),
    /// The storage backend failed.
    #[error("storage: {0}")]
    Storage(#[source] BoxError),
    /// A DocsGPT call failed.
    #[error(transparent)]
    DocsGpt(#[from] docsgpt::Error),
    /// The chat platform failed (sending, editing, uploading, …).
    #[error("platform: {0}")]
    Platform(#[source] BoxError),
}

impl Error {
    /// Wrap an error from the chat platform.
    pub fn platform(e: impl Into<BoxError>) -> Self {
        Error::Platform(e.into())
    }

    pub(crate) fn storage(e: impl Into<BoxError>) -> Self {
        Error::Storage(e.into())
    }

    pub(crate) fn config(msg: impl Into<String>) -> Self {
        Error::Config(msg.into())
    }
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
