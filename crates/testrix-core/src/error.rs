use thiserror::Error;

/// Domain errors produced by `testrix-core` operations.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("project `{project_id}` was not found")]
    ProjectNotFound { project_id: String },

    #[error("folder `{folder_id}` was not found")]
    FolderNotFound { folder_id: String },

    #[error("request `{request_id}` was not found")]
    RequestNotFound { request_id: String },

    #[error("invalid URL `{url}`: {reason}")]
    InvalidUrl { url: String, reason: String },

    #[error("document validation failed: {reason}")]
    Validation { reason: String },
}
