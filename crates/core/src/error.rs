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

    /// An external program (`git`, `npm`, `cargo`, `dotnet`) ran and failed.
    #[error("`{command}` failed: {message}")]
    Command {
        /// The command line, without secrets.
        command: String,
        /// What the program reported on standard error, or its exit status.
        message: String,
    },

    /// A JSON document could not be serialized or deserialized.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// The input (flags, project files, archive contents) is not valid.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// A GitHub Actions OIDC token could not be obtained.
    #[error("GitHub Actions OIDC: {0}")]
    Oidc(String),

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
    #[error("the upload URL expired or was rejected by the storage (403)")]
    UploadExpired,

    /// The storage rejected the archive upload.
    #[error("the storage rejected the upload with status {status} ({code})")]
    Upload {
        /// HTTP status code returned by the storage.
        status: u16,
        /// Error code reported by the storage, or `unknown`.
        code: String,
    },

    /// A downloaded file does not match the size or digest listed by the registry.
    #[error("downloaded file `{file}` is corrupt: {detail}")]
    Integrity {
        /// File name.
        file: String,
        /// What differs.
        detail: String,
    },

    /// The version being published already exists in the registry (`409`).
    #[error("this version already exists in the registry")]
    VersionExists,

    /// A file being appended already exists in the published version
    /// (`409 file_exists`).
    #[error("a file with this name already exists in the published version: {message}")]
    FileExists {
        /// Human-readable message from the registry.
        message: String,
    },
}
