use std::path::PathBuf;
use thiserror::Error;

/// Typed errors returned by the HTTP execution engine.
#[derive(Debug, Error)]
pub enum HttpError {
    #[error("request was cancelled")]
    Cancelled,

    #[error("invalid header `{name}`: {reason}")]
    InvalidHeader { name: String, reason: String },

    #[error("failed reading request body file `{path}`: {source}")]
    BodyFileRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("HTTP transport error: {0}")]
    Transport(#[from] reqwest::Error),

    #[error("failed initializing HTTP client/runtime: {0}")]
    Runtime(String),
}
