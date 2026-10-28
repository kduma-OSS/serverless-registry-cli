//! The publishing sequence: `init`, one `PUT` per file to its presigned URL, `commit`.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::client::Client;
use crate::error::{Error, Result};
use crate::models::{
    FileUpload, PublishCommitResponse, PublishFile, PublishInitRequest, PublishInitResponse,
};
use crate::packager::{Package, PackageFile};
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
    /// Add the files to an already published version (generic only).
    pub append: bool,
}

/// Result of a publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishOutcome {
    /// `--dry-run`: the registry accepted the declaration; nothing was uploaded.
    DryRun {
        /// The `init` response. Its presigned URLs are secrets and must not be printed.
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
/// Calls `init`, uploads every file and calls `commit`. A new `init` is made (at most
/// [`MAX_INITS`] in total) when an upload URL expires or is about to expire, or when
/// `commit` reports `409 upload_mismatch` or `400 upload_incomplete`; every file is
/// uploaded again after a new `init`. `commit` is retried on transport errors and `5xx`
/// because it is idempotent.
pub async fn publish(
    client: &Client,
    package: &Package,
    opts: PublishOptions,
) -> Result<PublishOutcome> {
    let request = init_request(package, opts.tag, opts.append)?;
    let uploader = Uploader::new(client.retry_policy())?;

    let mut inits = 0;
    loop {
        let init = client.publish_init(&request).await?;
        inits += 1;
        tracing::debug!(
            upload_id = %init.upload_id,
            expires = %init.expires,
            files = init.uploads.len(),
            "publish initialised"
        );
        let uploads = match_uploads(package, &init)?;
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
        let mut expired = false;
        for (file, upload) in &uploads {
            tracing::debug!(file = %file.name, size = file.size, "uploading");
            match uploader
                .upload(
                    &upload.put_url,
                    &upload.headers,
                    &file.path,
                    file.size,
                    not_after,
                )
                .await
            {
                Ok(()) => {}
                Err(Error::UploadExpired) if inits < MAX_INITS => {
                    tracing::debug!("upload URL expired; requesting new ones");
                    expired = true;
                    break;
                }
                Err(err) => return Err(err),
            }
        }
        if expired {
            continue;
        }

        match commit(client, &init.upload_id).await {
            Ok(response) => return Ok(PublishOutcome::Published { response }),
            Err(Error::Api { code, .. })
                if (code == "upload_mismatch" || code == "upload_incomplete")
                    && inits < MAX_INITS =>
            {
                tracing::debug!(code = %code, "staged files rejected; starting again from init");
            }
            Err(err) => return Err(err),
        }
    }
}

fn init_request(
    package: &Package,
    tag: Option<String>,
    append: bool,
) -> Result<PublishInitRequest> {
    if package.files.is_empty() {
        return Err(Error::InvalidInput(
            "a version needs at least one file".into(),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for file in &package.files {
        if !seen.insert(file.name.as_str()) {
            return Err(Error::InvalidInput(format!(
                "duplicate file name `{}`",
                file.name
            )));
        }
    }
    Ok(PublishInitRequest {
        eco: package.eco,
        name: package.name.clone(),
        version: package.version.clone(),
        files: package
            .files
            .iter()
            .map(|file| PublishFile {
                name: file.name.clone(),
                size: file.size,
                sha256: file.sha256.clone(),
                role: file.role.clone(),
                labels: file.labels.clone(),
            })
            .collect(),
        manifest: package.manifest.clone(),
        tag,
        append,
    })
}

/// Pairs every file with its upload instructions; the registry must return exactly one
/// entry per declared file.
fn match_uploads<'a>(
    package: &'a Package,
    init: &'a PublishInitResponse,
) -> Result<Vec<(&'a PackageFile, &'a FileUpload)>> {
    let mismatch = || {
        Error::InvalidInput(
            "the registry returned upload instructions that do not match the declared files".into(),
        )
    };
    let by_name: BTreeMap<&str, &FileUpload> = init
        .uploads
        .iter()
        .map(|upload| (upload.name.as_str(), upload))
        .collect();
    if by_name.len() != init.uploads.len() || by_name.len() != package.files.len() {
        return Err(mismatch());
    }
    package
        .files
        .iter()
        .map(|file| {
            by_name
                .get(file.name.as_str())
                .map(|upload| (file, *upload))
                .ok_or_else(mismatch)
        })
        .collect()
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
