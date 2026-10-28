//! Error type shared by all `sreg` library crates.

/// Convenience alias for results carrying [`enum@Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors returned by the `sreg` library crates.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Transport-level failure while talking to the registry or to R2.
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// The registry answered with an error response.
    #[error("registry returned {status} ({code}): {message}")]
    Api {
        /// HTTP status code of the response.
        status: u16,
        /// Machine-readable error code from the response body.
        code: String,
        /// Human-readable error message from the response body.
        message: String,
    },

    /// Local filesystem or process I/O failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A JSON document could not be serialized or deserialized.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// The input (flags, project files, archive contents) is not valid.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// The registry requires a newer client version.
    #[error(
        "this version of sreg is too old for the registry; version {min} or newer is required"
    )]
    ClientTooOld {
        /// Minimum client version accepted by the registry.
        min: String,
    },

    /// The presigned upload URL was rejected with `403`, usually because it expired.
    ///
    /// The caller can request a new URL with `publish/init` and try again.
    #[error("the storage rejected the upload URL (403); it has probably expired")]
    UploadExpired,

    /// The storage rejected the archive upload.
    #[error("the storage rejected the upload with status {status} ({code})")]
    Upload {
        /// HTTP status code returned by the storage.
        status: u16,
        /// Error code reported by the storage, or `unknown`.
        code: String,
    },

    /// The version being published already exists in the registry (`409`).
    #[error("this version already exists in the registry")]
    VersionExists,
}
