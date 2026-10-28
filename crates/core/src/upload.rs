//! Upload of an archive to the presigned `PUT` URL returned by `publish/init`.
//!
//! The archive is streamed from disk with a fixed `Content-Length` (never chunked) and
//! exactly the headers returned by `init`. Transport errors and `5xx` answers are
//! retried (`docs/api.md` 7.2); `403` is reported as [`Error::UploadExpired`] so the
//! caller can start again from `init`; any other `4xx` is fatal.
//!
//! The presigned URL carries credentials in its query string, so it is never logged
//! and transport errors are stripped of it.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_LENGTH};
use reqwest::StatusCode;
use url::Url;

use crate::error::{Error, Result};
use crate::retry::RetryPolicy;

/// Margin before the URL's expiry below which no new attempt is started.
pub const EXPIRY_MARGIN: Duration = Duration::from_secs(30);

/// Time allowed to establish a connection to the storage.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Time allowed between two reads of the storage's response.
const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Uploads archives to presigned URLs.
#[derive(Debug, Clone)]
pub struct Uploader {
    http: reqwest::Client,
    retry: RetryPolicy,
}

impl Uploader {
    /// Creates an uploader using `retry` for transport errors and `5xx` answers.
    pub fn new(retry: RetryPolicy) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()?;
        Ok(Uploader { http, retry })
    }

    /// Uploads the file at `path` (`size` bytes) to `put_url` with exactly `headers`.
    ///
    /// When `not_after` is set and less than [`EXPIRY_MARGIN`] away before an attempt,
    /// the upload stops with [`Error::UploadExpired`] instead of starting the attempt.
    pub async fn upload(
        &self,
        put_url: &str,
        headers: &BTreeMap<String, String>,
        path: &Path,
        size: u64,
        not_after: Option<SystemTime>,
    ) -> Result<()> {
        let url = Url::parse(put_url).map_err(|_| {
            Error::InvalidInput("the registry returned an invalid upload URL".into())
        })?;
        let header_map = build_headers(headers, size)?;
        let target = redacted(&url);
        let attempts = self.retry.attempts.max(1);

        let mut attempt = 1;
        loop {
            if not_after.is_some_and(expires_soon) {
                tracing::debug!(target = %target, "upload URL expires soon");
                return Err(Error::UploadExpired);
            }

            let err = match self.attempt(&url, &header_map, path, size, &target).await {
                Ok(()) => return Ok(()),
                Err(err) => err,
            };
            let retryable = match &err {
                Error::Http(_) => true,
                Error::Upload { status, .. } => *status >= 500,
                _ => false,
            };
            if !retryable || attempt >= attempts {
                return Err(err);
            }
            tracing::debug!(target = %target, attempt, error = %err, "upload attempt failed");
            self.retry.wait(attempt).await;
            attempt += 1;
        }
    }

    async fn attempt(
        &self,
        url: &Url,
        headers: &HeaderMap,
        path: &Path,
        size: u64,
        target: &str,
    ) -> Result<()> {
        let file = tokio::fs::File::open(path).await?;
        let actual = file.metadata().await?.len();
        if actual != size {
            return Err(Error::InvalidInput(format!(
                "the archive changed while publishing: expected {size} bytes, found {actual}"
            )));
        }

        let response = self
            .http
            .put(url.clone())
            .headers(headers.clone())
            .body(reqwest::Body::from(file))
            .send()
            .await
            .map_err(|err| Error::Http(err.without_url()))?;
        let status = response.status();
        tracing::debug!(method = "PUT", target = %target, status = status.as_u16(), "storage response");

        if status.is_success() {
            return Ok(());
        }
        if status == StatusCode::FORBIDDEN {
            return Err(Error::UploadExpired);
        }
        let body = response.text().await.unwrap_or_default();
        Err(Error::Upload {
            status: status.as_u16(),
            code: storage_error_code(&body).unwrap_or_else(|| "unknown".to_owned()),
        })
    }
}

/// Uploads the file at `path` (`size` bytes) to `put_url` with exactly `headers`,
/// using the default retry policy (3 attempts, 1 s and 2 s backoff).
pub async fn upload_archive(
    put_url: &str,
    headers: &BTreeMap<String, String>,
    path: &Path,
    size: u64,
) -> Result<()> {
    Uploader::new(RetryPolicy::default())?
        .upload(put_url, headers, path, size, None)
        .await
}

fn build_headers(headers: &BTreeMap<String, String>, size: u64) -> Result<HeaderMap> {
    let invalid = || Error::InvalidInput("the registry returned invalid upload headers".into());

    let mut map = HeaderMap::new();
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| invalid())?;
        let value = HeaderValue::from_str(value).map_err(|_| invalid())?;
        map.insert(name, value);
    }

    let declared = map
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    match declared {
        Some(declared) if declared == size => Ok(map),
        Some(declared) => Err(Error::InvalidInput(format!(
            "the registry expects {declared} bytes but the archive has {size}"
        ))),
        None => {
            map.insert(CONTENT_LENGTH, HeaderValue::from(size));
            Ok(map)
        }
    }
}

fn expires_soon(not_after: SystemTime) -> bool {
    match not_after.duration_since(SystemTime::now()) {
        Ok(remaining) => remaining < EXPIRY_MARGIN,
        Err(_) => true,
    }
}

/// The URL without its query string, safe for logs.
fn redacted(url: &Url) -> String {
    format!(
        "{}://{}{}",
        url.scheme(),
        url.host_str().unwrap_or_default(),
        url.path()
    )
}

/// Extracts `<Code>…</Code>` from an S3-style XML error body.
///
/// Only the code is kept: S3 error bodies may echo parts of the signed request.
fn storage_error_code(body: &str) -> Option<String> {
    let start = body.find("<Code>")? + "<Code>".len();
    let end = start + body[start..].find("</Code>")?;
    let code = body[start..end].trim();
    let valid = !code.is_empty()
        && code.len() <= 64
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    valid.then(|| code.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_storage_error_code() {
        let body =
            "<?xml version=\"1.0\"?><Error><Code>BadDigest</Code><Message>x</Message></Error>";
        assert_eq!(storage_error_code(body).as_deref(), Some("BadDigest"));
        assert_eq!(storage_error_code("not xml"), None);
        assert_eq!(storage_error_code("<Code>a b</Code>"), None);
    }

    #[test]
    fn redacts_the_query_string() {
        let url = Url::parse("https://acc.r2.example/bucket/key.tgz?X-Amz-Signature=abc").unwrap();
        assert_eq!(redacted(&url), "https://acc.r2.example/bucket/key.tgz");
    }

    #[test]
    fn rejects_a_content_length_that_differs_from_the_archive() {
        let headers = BTreeMap::from([("content-length".to_owned(), "10".to_owned())]);
        assert!(matches!(
            build_headers(&headers, 11),
            Err(Error::InvalidInput(_))
        ));
        assert_eq!(build_headers(&headers, 10).unwrap()[CONTENT_LENGTH], "10");
    }

    #[test]
    fn deadline_margin_is_thirty_seconds() {
        assert!(expires_soon(SystemTime::now() + Duration::from_secs(29)));
        assert!(!expires_soon(SystemTime::now() + Duration::from_secs(60)));
        assert!(expires_soon(SystemTime::now() - Duration::from_secs(1)));
    }
}
