//! Download of generic package files listed by the generic feed.
//!
//! The file URL points at the registry, which answers with a redirect to a presigned
//! storage URL. Redirects are followed by hand: the `Authorization` header is sent only
//! to the registry (and only when the URL is on the registry's origin), never to the
//! storage. Every file is written to a temporary file next to its destination, checked
//! against the listed size and SHA-256, and renamed into place only when it matches.

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
        let url = Url::parse(&file.url).map_err(|_| {
            Error::InvalidInput(format!(
                "the registry returned an invalid URL for `{}`",
                file.name
            ))
        })?;
        let destination = dir.join(&file.name);
        let partial = dir.join(format!(".{}.part", file.name));

        let attempts = self.retry.attempts.max(1);
        let mut attempt = 1;
        loop {
            let result = self.attempt(&url, file, &partial).await;
            let err = match result {
                Ok(()) => break,
                Err(err) => err,
            };
            let _ = std::fs::remove_file(&partial);
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

        std::fs::rename(&partial, &destination)?;
        Ok(destination)
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
            let mut request = self.http.get(url.clone());
            if on_registry {
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

            let response = request
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
                let min = response
                    .headers()
                    .get(crate::client::MIN_CLIENT_VERSION_HEADER)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let body = response.bytes().await.unwrap_or_default();
                return Err(crate::client::error_from_response(
                    status,
                    min.as_deref(),
                    &body,
                ));
            }
            return Err(Error::Api {
                status: status.as_u16(),
                code: "storage_error".into(),
                message: format!("the storage answered {status} for the download"),
            });
        }
        Err(Error::InvalidInput("too many redirects".into()))
    }
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
