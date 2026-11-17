use std::path::PathBuf;
use testrix_core::CoreError;
use thiserror::Error;

/// Storage and YAML serialization errors.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("filesystem error at `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("YAML parsing error in `{path}`: {source}")]
    YamlRead {
        path: PathBuf,
        #[source]
        source: yaml_serde::Error,
    },

    #[error("YAML serialization error: {0}")]
    YamlWrite(#[from] yaml_serde::Error),

    #[error(transparent)]
    Core(#[from] CoreError),

    #[error("invalid path `{path}`: {reason}")]
    InvalidPath { path: PathBuf, reason: String },
}
