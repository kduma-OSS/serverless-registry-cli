//! The publishing sequence: `init`, `PUT` to the presigned URL, `commit`
//! (`docs/api.md` 7.2).

use std::time::SystemTime;

use crate::client::Client;
use crate::error::{Error, Result};
use crate::models::{PublishCommitResponse, PublishInitRequest, PublishInitResponse};
use crate::packager::Package;
use crate::retry::is_retryable;
use crate::upload::Uploader;

/// `init` is called at most this many times per publish.
pub const MAX_INITS: u32 = 2;

/// Options of a publish.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublishOptions {
    /// Stop after the registry accepted the declaration (`init`).
    pub dry_run: bool,
    /// npm dist-tag to point at the version.
    pub tag: Option<String>,
}

/// Result of a publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishOutcome {
    /// `--dry-run`: the registry accepted the declaration; nothing was uploaded.
    DryRun {
        /// The `init` response. Its `put_url` is a secret and must not be printed.
        init: PublishInitResponse,
    },
    /// The version was published.
    Published {
        /// The `commit` response.
        response: PublishCommitResponse,
    },
}

/// Publishes `package` through `client`.
///
/// Calls `init`, uploads the archive and calls `commit`. A new `init` is made (at most
/// [`MAX_INITS`] in total) when the upload URL expires or is about to expire, or when
/// `commit` reports `409 upload_mismatch`. `commit` is retried on transport errors and
/// `5xx` because it is idempotent.
pub async fn publish(
    client: &Client,
    package: &Package,
    opts: PublishOptions,
) -> Result<PublishOutcome> {
    let request = PublishInitRequest {
        eco: package.eco,
        name: package.name.clone(),
        version: package.version.clone(),
        file: package.file_name.clone(),
        size: package.size,
        sha256: package.sha256.clone(),
        manifest: package.manifest.clone(),
        tag: opts.tag,
    };
    let uploader = Uploader::new(client.retry_policy())?;

    let mut inits = 0;
    loop {
        let init = client.publish_init(&request).await?;
        inits += 1;
        tracing::debug!(upload_id = %init.upload_id, expires = %init.expires, "publish initialised");
        if opts.dry_run {
            return Ok(PublishOutcome::DryRun { init });
        }

        let not_after = match init.expires_at() {
            Ok(expires) => Some(SystemTime::from(expires)),
            Err(_) => {
                tracing::warn!(expires = %init.expires, "registry returned an invalid expiry");
                None
            }
        };
        let uploaded = uploader
            .upload(
                &init.put_url,
                &init.headers,
                &package.archive,
                package.size,
                not_after,
            )
            .await;
        match uploaded {
            Ok(()) => {}
            Err(Error::UploadExpired) if inits < MAX_INITS => {
                tracing::debug!("upload URL expired; requesting a new one");
                continue;
            }
            Err(err) => return Err(err),
        }

        match commit(client, &init.upload_id).await {
            Ok(response) => return Ok(PublishOutcome::Published { response }),
            Err(Error::Api {
                status: 409, code, ..
            }) if code == "upload_mismatch" && inits < MAX_INITS => {
                tracing::debug!("staged upload does not match; starting again from init");
            }
            Err(err) => return Err(err),
        }
    }
}

async fn commit(client: &Client, upload_id: &str) -> Result<PublishCommitResponse> {
    let retry = client.retry_policy();
    let mut attempt = 1;
    loop {
        match client.publish_commit(upload_id).await {
            Err(err) if is_retryable(&err) && attempt < retry.attempts => {
                tracing::debug!(attempt, error = %err, "commit failed; retrying");
                retry.wait(attempt).await;
                attempt += 1;
            }
            result => return result,
        }
    }
}
