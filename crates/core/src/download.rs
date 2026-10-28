//! Download of generic package files listed by the generic feed.
//!
//! The file URL points at the registry, which answers with a redirect to a presigned
//! storage URL. Redirects are followed by hand: the `Authorization` header is sent only
//! to the registry (and only when the URL is on the registry's origin), never to the
//! storage. Every file is written to a temporary file next to its destination, checked
//! against the listed size and SHA-256, and renamed into place only when it matches.
//!
//! [`Downloader::resolve_sole`] reads the redirect of the single-file endpoint
//! (`/generic/<name>/<version>/-/download`) instead of following it, so that the caller
//! learns which version and file the registry picked before downloading.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::header::{HeaderValue, AUTHORIZATION, LOCATION};
use reqwest::redirect::Policy;
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use url::Url;

use crate::client::{Client, CLIENT_VERSION_HEADER};
use crate::error::{Error, Result};
use crate::models::GenericFeedFile;
use crate::retry::RetryPolicy;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Redirects followed per file before giving up.
const MAX_REDIRECTS: usize = 5;

/// Response header of the single-file endpoint naming the resolved version.
pub const RESOLVED_VERSION_HEADER: &str = "x-resolved-version";

/// Response header of the single-file endpoint naming the resolved file.
pub const RESOLVED_FILE_HEADER: &str = "x-resolved-file";

/// The file the single-file endpoint redirected to.
#[derive(Clone, PartialEq, Eq)]
pub struct SoleFile {
    /// The resolved version (`X-Resolved-Version`).
    pub version: String,
    /// The file name (`X-Resolved-File`); safe to use as a file name.
    pub name: String,
    /// The redirect target. Usually a presigned storage URL: never print or log it.
    pub location: Url,
}

impl std::fmt::Debug for SoleFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoleFile")
            .field("version", &self.version)
            .field("name", &self.name)
            .field("location", &"<redacted>")
            .finish()
    }
}

/// Downloads files of generic packages.
#[derive(Debug, Clone)]
pub struct Downloader {
    http: reqwest::Client,
    registry: Url,
    token: Option<String>,
    client_version: String,
    retry: RetryPolicy,
}

impl Downloader {
    /// A downloader authenticating like `client` towards the registry.
    pub fn new(client: &Client) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!("sreg/{}", client.client_version()))
            .redirect(Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()?;
        Ok(Downloader {
            http,
            registry: client.registry_url().clone(),
            token: client.token().map(str::to_owned),
            client_version: client.client_version().to_owned(),
            retry: client.retry_policy(),
        })
    }

    /// Downloads `file` into `dir` and returns its path.
    ///
    /// Fails with [`Error::Integrity`] when the size or digest differs; nothing is left
    /// behind in that case.
    pub async fn download(&self, file: &GenericFeedFile, dir: &Path) -> Result<PathBuf> {
        validate_file_name(&file.name)?;
        let destination = dir.join(&file.name);
        self.download_to(file, &destination).await?;
        Ok(destination)
    }

    /// Downloads `file` to the path `destination`, verified like [`Self::download`].
    pub async fn download_to(&self, file: &GenericFeedFile, destination: &Path) -> Result<()> {
        let url = Url::parse(&file.url).map_err(|_| {
            Error::InvalidInput(format!(
                "the registry returned an invalid URL for `{}`",
                file.name
            ))
        })?;
        let file_name = destination
            .file_name()
            .ok_or_else(|| {
                Error::InvalidInput(format!("`{}` is not a file path", destination.display()))
            })?
            .to_string_lossy();
        let partial = destination.with_file_name(format!(".{file_name}.part"));

        let attempts = self.retry.attempts.max(1);
        let mut attempt = 1;
        let mut renewed = false;
        loop {
            let result = self.attempt(&url, file, &partial).await;
            let err = match result {
                Ok(()) => break,
                Err(err) => err,
            };
            let _ = std::fs::remove_file(&partial);
            // A presigned URL that expired before the download started answers `403`:
            // ask the registry for a new one, once per file (docs/api.md 7.4).
            if is_storage_forbidden(&err) && !renewed {
                renewed = true;
                tracing::debug!(file = %file.name, "storage refused the download URL; requesting a new one");
                continue;
            }
            let retryable = match &err {
                Error::Http(_) => true,
                Error::Api { status, .. } => *status >= 500,
                _ => false,
            };
            if !retryable || attempt >= attempts {
                return Err(err);
            }
            tracing::debug!(file = %file.name, attempt, error = %err, "download failed; retrying");
            self.retry.wait(attempt).await;
            attempt += 1;
        }

        std::fs::rename(&partial, destination)?;
        Ok(())
    }

    /// Requests the single-file endpoint `url` (see
    /// [`Client::generic_sole_url`](crate::Client::generic_sole_url)) and returns the
    /// version and file it redirects to, without downloading the file.
    ///
    /// `409 ambiguous` becomes [`Error::Ambiguous`] with the candidate files; `404` and
    /// the other errors are mapped like every registry response. Transport errors and
    /// `5xx` are retried.
    pub async fn resolve_sole(&self, url: Url) -> Result<SoleFile> {
        let attempts = self.retry.attempts.max(1);
        let mut attempt = 1;
        loop {
            let err = match self.resolve_once(&url).await {
                Ok(sole) => return Ok(sole),
                Err(err) => err,
            };
            let retryable = match &err {
                Error::Http(_) => true,
                Error::Api { status, .. } => *status >= 500,
                _ => false,
            };
            if !retryable || attempt >= attempts {
                return Err(err);
            }
            tracing::debug!(attempt, error = %err, "resolving the file failed; retrying");
            self.retry.wait(attempt).await;
            attempt += 1;
        }
    }

    async fn resolve_once(&self, url: &Url) -> Result<SoleFile> {
        let response = self
            .request(url)?
            .send()
            .await
            .map_err(|err| Error::Http(err.without_url()))?;
        let status = response.status();
        tracing::debug!(
            method = "GET",
            path = url.path(),
            status = status.as_u16(),
            "registry response"
        );
        if !status.is_redirection() {
            if status.is_success() {
                return Err(Error::InvalidInput(format!(
                    "the registry answered {status} instead of redirecting to the file"
                )));
            }
            return Err(registry_error(response).await);
        }

        let headers = response.headers();
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    Error::InvalidInput(format!("the registry's redirect has no `{name}` header"))
                })
        };
        let location = url
            .join(&header(LOCATION.as_str())?)
            .map_err(|_| Error::InvalidInput("redirect to an invalid location".into()))?;
        let version = header(RESOLVED_VERSION_HEADER)?;
        let name = header(RESOLVED_FILE_HEADER)?;
        validate_file_name(&name)?;
        Ok(SoleFile {
            version,
            name,
            location,
        })
    }

    /// A `GET` of `url`, with the client version and the token only when `url` is on the
    /// registry's origin.
    fn request(&self, url: &Url) -> Result<reqwest::RequestBuilder> {
        let mut request = self.http.get(url.clone());
        if url.origin() == self.registry.origin() {
            request = request.header(CLIENT_VERSION_HEADER, self.client_version.as_str());
            if let Some(token) = &self.token {
                let mut value =
                    HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                        Error::InvalidInput("the token contains invalid characters".into())
                    })?;
                value.set_sensitive(true);
                request = request.header(AUTHORIZATION, value);
            }
        }
        Ok(request)
    }

    async fn attempt(&self, url: &Url, file: &GenericFeedFile, partial: &Path) -> Result<()> {
        let mut response = self.follow(url.clone()).await?;

        let mut output = File::create(partial)?;
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| Error::Http(err.without_url()))?
        {
            size += chunk.len() as u64;
            if size > file.size {
                return Err(integrity(
                    file,
                    format!("more than the listed {} bytes", file.size),
                ));
            }
            hasher.update(&chunk);
            output.write_all(&chunk)?;
        }
        output.flush()?;
        drop(output);

        if size != file.size {
            return Err(integrity(
                file,
                format!("expected {} bytes, received {size}", file.size),
            ));
        }
        let digest = hex::encode(hasher.finalize());
        if digest != file.sha256.to_ascii_lowercase() {
            return Err(integrity(
                file,
                format!("expected SHA-256 {}, got {digest}", file.sha256),
            ));
        }
        Ok(())
    }

    /// Requests `url`, following redirects by hand, and returns the final `200` response.
    async fn follow(&self, mut url: Url) -> Result<reqwest::Response> {
        for _ in 0..=MAX_REDIRECTS {
            let on_registry = url.origin() == self.registry.origin();
            let response = self
                .request(&url)?
                .send()
                .await
                .map_err(|err| Error::Http(err.without_url()))?;
            let status = response.status();
            // Only the registry's paths are logged; storage URLs carry signatures.
            if on_registry {
                tracing::debug!(
                    method = "GET",
                    path = url.path(),
                    status = status.as_u16(),
                    "registry response"
                );
            } else {
                tracing::debug!(
                    method = "GET",
                    host = url.host_str().unwrap_or_default(),
                    status = status.as_u16(),
                    "storage response"
                );
            }

            if status.is_redirection() {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| Error::InvalidInput("redirect without a location".into()))?;
                url = url
                    .join(location)
                    .map_err(|_| Error::InvalidInput("redirect to an invalid location".into()))?;
                continue;
            }
            if status == StatusCode::OK {
                return Ok(response);
            }
            if on_registry {
                return Err(registry_error(response).await);
            }
            return Err(Error::Api {
                status: status.as_u16(),
                code: STORAGE_ERROR.into(),
                message: format!("the storage answered {status} for the download"),
            });
        }
        Err(Error::InvalidInput("too many redirects".into()))
    }
}

/// Code of the error for a non-`200` answer of the storage host.
const STORAGE_ERROR: &str = "storage_error";

/// Whether the storage host answered `403`, e.g. for an expired presigned URL.
fn is_storage_forbidden(err: &Error) -> bool {
    matches!(err, Error::Api { status: 403, code, .. } if code == STORAGE_ERROR)
}

/// Maps an error response of the registry to an [`Error`].
async fn registry_error(response: reqwest::Response) -> Error {
    let status = response.status();
    let min = response
        .headers()
        .get(crate::client::MIN_CLIENT_VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response.bytes().await.unwrap_or_default();
    crate::client::error_from_response(status, min.as_deref(), &body)
}

fn integrity(file: &GenericFeedFile, detail: String) -> Error {
    Error::Integrity {
        file: file.name.clone(),
        detail,
    }
}

/// File names come from the registry; never let them escape the output directory.
fn validate_file_name(name: &str) -> Result<()> {
    let valid =
        !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', ':', '\0']);
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "the registry listed an unsafe file name `{name}`"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_file_names() {
        for name in ["", "../x", "a/b", ".hidden", "a\\b", "c:x"] {
            assert!(validate_file_name(name).is_err(), "{name}");
        }
        assert!(validate_file_name("cli-linux.tar.gz").is_ok());
    }
}
