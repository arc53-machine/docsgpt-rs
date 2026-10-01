//! Errors returned by [`crate::Client`].

/// Everything that can go wrong talking to DocsGPT.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The server answered with a non-success status.
    #[error("DocsGPT {endpoint} returned {status}: {body}")]
    Http {
        /// Endpoint path, e.g. `/stream`.
        endpoint: &'static str,
        /// HTTP status code.
        status: u16,
        /// Response body, truncated.
        body: String,
    },
    /// The server has this feature switched off (e.g. no speech provider configured).
    #[error("DocsGPT {endpoint} is disabled on this server: {message}")]
    FeatureDisabled {
        /// Endpoint path.
        endpoint: &'static str,
        /// The server's explanation.
        message: String,
    },
    /// The request timed out, or the stream sent nothing for longer than the idle timeout.
    #[error("DocsGPT {endpoint} timed out")]
    Timeout {
        /// Endpoint path.
        endpoint: &'static str,
    },
    /// The connection failed or broke.
    #[error("DocsGPT {endpoint}: {source}")]
    Transport {
        /// Endpoint path.
        endpoint: &'static str,
        /// Underlying HTTP client error.
        #[source]
        source: reqwest::Error,
    },
    /// The server's reply was not in the expected shape.
    #[error("DocsGPT {endpoint} sent an unexpected response: {message}")]
    Decode {
        /// Endpoint path.
        endpoint: &'static str,
        /// What was wrong.
        message: String,
    },
    /// A download was larger than the configured limit.
    #[error("download exceeds {limit} bytes")]
    TooLarge {
        /// The limit, in bytes.
        limit: usize,
    },
    /// Processing an uploaded attachment failed on the server.
    #[error("attachment processing failed: {detail}")]
    TaskFailed {
        /// Celery task id.
        task_id: String,
        /// Status payload from the server.
        detail: String,
    },
    /// The client could not be built (invalid base URL, TLS setup, …).
    #[error("invalid client configuration: {0}")]
    Config(String),
}

impl Error {
    /// HTTP status, for [`Error::Http`].
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Http { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub(crate) fn transport(endpoint: &'static str, source: reqwest::Error) -> Self {
        if source.is_timeout() {
            Error::Timeout { endpoint }
        } else {
            Error::Transport { endpoint, source }
        }
    }

    pub(crate) fn decode(endpoint: &'static str, message: impl Into<String>) -> Self {
        Error::Decode {
            endpoint,
            message: message.into(),
        }
    }
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
