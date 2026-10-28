//! Request and response models of the registry HTTP API (`/api/v1`).
//!
//! The models follow the API contract in `docs/api.md` exactly. Field names are
//! `camelCase` on the wire unless the contract says otherwise; timestamps are kept as
//! the RFC 3339 strings the registry sends. Unknown response fields are ignored.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::error::{Error, Result};
use crate::packager::Ecosystem;

/// Role of a credential (`docs/api.md` 2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// May install packages covered by its grants.
    Read,
    /// May publish packages covered by its grants.
    Publish,
    /// May administer the registry.
    Admin,
}

impl Kind {
    /// The identifier used in the API.
    pub const fn as_str(self) -> &'static str {
        match self {
            Kind::Read => "read",
            Kind::Publish => "publish",
            Kind::Admin => "admin",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Access to a set of packages and versions of one ecosystem (`docs/api.md` 5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// Ecosystem the grant applies to.
    pub eco: Ecosystem,
    /// Exact package name or a pattern with a trailing `*`.
    pub package: String,
    /// Version constraint in the ecosystem's syntax; `*` means all versions.
    pub versions: String,
}

impl Grant {
    /// Parses the CLI shorthand `eco:package[:versions]`.
    ///
    /// The value is split at the first two `:`, so the version constraint may itself
    /// contain colons or commas (`cargo:acme-core:>=1.4, <2`). `versions` defaults to `*`.
    pub fn parse_shorthand(value: &str) -> Result<Self> {
        let invalid = |reason: &str| {
            Error::InvalidInput(format!(
                "invalid grant `{value}`: {reason}; expected <eco>:<package>[:<versions>]"
            ))
        };

        let mut parts = value.splitn(3, ':');
        let eco = parts.next().unwrap_or_default();
        let package = parts.next().ok_or_else(|| invalid("missing package"))?;
        let versions = parts.next().unwrap_or("*");

        let eco = Ecosystem::from_str(eco)
            .map_err(|_| invalid("unknown ecosystem, expected one of composer, npm, cargo"))?;
        if package.is_empty() {
            return Err(invalid("empty package"));
        }
        if package.chars().any(char::is_whitespace) {
            return Err(invalid("the package must not contain whitespace"));
        }
        if package
            .find('*')
            .is_some_and(|pos| pos != package.len() - 1)
        {
            return Err(invalid("`*` may only be the last character of the package"));
        }
        if versions.trim().is_empty() {
            return Err(invalid("empty version constraint"));
        }

        Ok(Grant {
            eco,
            package: package.to_owned(),
            versions: versions.to_owned(),
        })
    }
}

impl fmt::Display for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.eco, self.package, self.versions)
    }
}

/// Body of every non-2xx response (`docs/api.md` 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// Stable, machine-readable error code, e.g. `version_exists`.
    pub error: String,
    /// Human-readable description.
    pub message: String,
}

/// `GET /api/v1/version` (`docs/api.md` 6.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    /// Version of the running Worker.
    pub version: String,
    /// Oldest supported client version.
    pub min_client_version: String,
    /// Data schema version used by the Worker.
    pub schema_version: u64,
}

/// `GET /api/v1/whoami` (`docs/api.md` 6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Whoami {
    /// `[<source>.]<project>` of the credential.
    pub project: String,
    /// Role of the credential.
    pub kind: Kind,
    /// Grants of the credential; empty for `admin`.
    pub grants: Vec<Grant>,
}

/// `POST /api/v1/publish/init` request (`docs/api.md` 6.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishInitRequest {
    /// Ecosystem of the package.
    pub eco: Ecosystem,
    /// Package name.
    pub name: String,
    /// Version being published.
    pub version: String,
    /// Archive file name.
    pub file: String,
    /// Archive size in bytes.
    pub size: u64,
    /// Lower-case hex SHA-256 of the archive.
    pub sha256: String,
    /// Ecosystem-native manifest.
    pub manifest: serde_json::Value,
    /// npm dist-tag (npm only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

/// `POST /api/v1/publish/init` response (`docs/api.md` 6.4).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishInitResponse {
    /// Identifier to pass to `commit`.
    pub upload_id: String,
    /// Presigned `PUT` URL. Treat it as a secret: never print or log it.
    pub put_url: String,
    /// When `put_url` stops working (RFC 3339).
    pub expires: String,
    /// Exactly the headers the upload must carry (lower-case names).
    pub headers: BTreeMap<String, String>,
}

impl PublishInitResponse {
    /// Parses [`expires`](Self::expires).
    pub fn expires_at(&self) -> Result<OffsetDateTime> {
        parse_timestamp(&self.expires)
    }
}

impl fmt::Debug for PublishInitResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublishInitResponse")
            .field("upload_id", &self.upload_id)
            .field("put_url", &"<redacted>")
            .field("expires", &self.expires)
            .field("headers", &self.headers)
            .finish()
    }
}

/// `POST /api/v1/publish/commit` request (`docs/api.md` 6.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishCommitRequest {
    /// Identifier returned by `init`.
    pub upload_id: String,
}

/// `POST /api/v1/publish/commit` response (`docs/api.md` 6.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishCommitResponse {
    /// Ecosystem of the package.
    pub eco: Ecosystem,
    /// Package name.
    pub name: String,
    /// Published version.
    pub version: String,
    /// Download URL of the archive in the ecosystem's native protocol.
    pub url: String,
}

/// One version of a package (`PackageVersion` in `docs/api.md` 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionEntry {
    /// Version string.
    pub version: String,
    /// Publication time (RFC 3339).
    pub published: String,
    /// Whether the version is yanked.
    pub yanked: bool,
    /// Archive size in bytes.
    pub size: u64,
    /// Lower-case hex SHA-256 of the archive.
    pub sha256: String,
}

/// A package with its versions (`Package` in `docs/api.md` 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageEntry {
    /// Ecosystem of the package.
    pub eco: Ecosystem,
    /// Decoded package name.
    pub name: String,
    /// Versions, highest first.
    pub versions: Vec<VersionEntry>,
}

/// `GET /api/v1/packages` (`docs/api.md` 6.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageList {
    /// Packages sorted by ecosystem and name.
    pub packages: Vec<PackageEntry>,
}

/// `POST /api/v1/packages/<eco>/<name>/<version>/yank` (`docs/api.md` 6.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YankResponse {
    /// Ecosystem of the package.
    pub eco: Ecosystem,
    /// Package name.
    pub name: String,
    /// The yanked version.
    #[serde(flatten)]
    pub version: VersionEntry,
}

/// `DELETE /api/v1/packages/<eco>/<name>/<version>` (`docs/api.md` 6.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteVersionResponse {
    /// Ecosystem of the package.
    pub eco: Ecosystem,
    /// Package name.
    pub name: String,
    /// Deleted version.
    pub version: String,
    /// Always `true`.
    pub deleted: bool,
}

/// `DELETE /api/v1/packages/<eco>/<name>` (`docs/api.md` 6.10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletePackageResponse {
    /// Ecosystem of the package.
    pub eco: Ecosystem,
    /// Package name.
    pub name: String,
    /// Every version that was deleted.
    pub deleted_versions: Vec<String>,
}

/// `POST /api/v1/packages/<eco>/<name>/reindex` (`docs/api.md` 6.11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReindexResponse {
    /// Ecosystem of the package.
    pub eco: Ecosystem,
    /// Package name.
    pub name: String,
    /// Number of version manifests the index was built from.
    pub manifests: u64,
}

/// Type of an auth entry (`docs/api.md` 2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretType {
    /// A static key generated by the registry.
    Key,
    /// A GitHub Actions OIDC token.
    Gh,
    /// A type introduced by a newer registry.
    #[serde(other)]
    Other,
}

impl SecretType {
    /// The identifier used in the API.
    pub const fn as_str(self) -> &'static str {
        match self {
            SecretType::Key => "key",
            SecretType::Gh => "gh",
            SecretType::Other => "other",
        }
    }
}

impl fmt::Display for SecretType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An auth entry of a credential (`KeyAuthEntry` / `GhAuthEntry` in `docs/api.md` 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthEntry {
    /// 12 lower-case hex characters identifying the entry.
    pub id: String,
    /// Type of the entry.
    #[serde(rename = "type")]
    pub secret_type: SecretType,
    /// Creation time (RFC 3339).
    pub created: String,
    /// Expiry time (RFC 3339); `None` means never.
    pub expires: Option<String>,
    /// `gh` only: `owner/repo` matched against the JWT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// `gh` only: required ref; `None` means any ref.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// `gh` only: required workflow; `None` means any workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
}

impl AuthEntry {
    /// Parses [`expires`](Self::expires); `None` means the entry never expires.
    pub fn expires_at(&self) -> Result<Option<OffsetDateTime>> {
        self.expires.as_deref().map(parse_timestamp).transpose()
    }

    /// Whether the entry is active at `now` (`expires` is `null` or in the future).
    ///
    /// An unparsable expiry is treated as active, so it is never silently ignored.
    pub fn is_active(&self, now: OffsetDateTime) -> bool {
        match self.expires_at() {
            Ok(Some(expires)) => expires > now,
            Ok(None) | Err(_) => true,
        }
    }
}

/// A credential (`Credential` in `docs/api.md` 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    /// Local project slug.
    pub project: String,
    /// Role of the credential.
    pub kind: Kind,
    /// Auth entries, including expired ones until the cron removes them.
    pub auth: Vec<AuthEntry>,
    /// Grants; always empty for `admin`.
    pub grants: Vec<Grant>,
}

/// `GET /api/v1/credentials` (`docs/api.md` 6.12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialList {
    /// Credentials sorted by project.
    pub credentials: Vec<Credential>,
}

/// `POST /api/v1/credentials/<project>/secrets` request (`docs/api.md` 6.17); also the
/// `secret` field of [`CreateCredentialRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddSecretRequest {
    /// Type of the new entry.
    #[serde(rename = "type")]
    pub secret_type: SecretType,
    /// Expiry (RFC 3339 or `YYYY-MM-DD`); omitted means never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    /// `gh` only: `owner/repo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// `gh` only: required ref.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// `gh` only: required workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
}

impl AddSecretRequest {
    /// A `key` entry with an optional expiry.
    pub fn key(expires: Option<String>) -> Self {
        AddSecretRequest {
            secret_type: SecretType::Key,
            expires,
            repository: None,
            git_ref: None,
            workflow: None,
        }
    }
}

/// `POST /api/v1/credentials/<project>/secrets` response (`docs/api.md` 6.17).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddSecretResponse {
    /// The new entry.
    pub secret: AuthEntry,
    /// The full token; present only for `key` entries and never returned again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl fmt::Debug for AddSecretResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AddSecretResponse")
            .field("secret", &self.secret)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// `POST /api/v1/credentials` request (`docs/api.md` 6.13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCredentialRequest {
    /// Project slug.
    pub project: String,
    /// Role of the credential.
    pub kind: Kind,
    /// Grants; must be empty for `admin`.
    pub grants: Vec<Grant>,
    /// The first auth entry; the registry defaults to a `key` without expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<AddSecretRequest>,
}

/// `POST /api/v1/credentials` response (`docs/api.md` 6.13).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCredentialResponse {
    /// The created credential.
    pub credential: Credential,
    /// The full token; present only when the first secret is a `key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl fmt::Debug for CreateCredentialResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateCredentialResponse")
            .field("credential", &self.credential)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// `PATCH /api/v1/credentials/<project>` request (`docs/api.md` 6.15).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateGrantsRequest {
    /// The new, complete list of grants.
    pub grants: Vec<Grant>,
}

/// `DELETE /api/v1/credentials/<project>` (`docs/api.md` 6.16).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteCredentialResponse {
    /// The deleted project.
    pub project: String,
    /// Always `true`.
    pub deleted: bool,
}

/// `DELETE /api/v1/credentials/<project>/secrets/<hash-prefix>` request
/// (`docs/api.md` 6.18).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpireSecretRequest {
    /// When the entry expires (RFC 3339 or `YYYY-MM-DD`); omitted means now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
}

/// `DELETE /api/v1/credentials/<project>/secrets/<hash-prefix>` response
/// (`docs/api.md` 6.18).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpireSecretResponse {
    /// The updated entry.
    pub secret: AuthEntry,
}

/// Parses an RFC 3339 timestamp, or a `YYYY-MM-DD` date meaning midnight UTC.
pub fn parse_timestamp(value: &str) -> Result<OffsetDateTime> {
    if let Ok(parsed) = OffsetDateTime::parse(value, &Rfc3339) {
        return Ok(parsed);
    }
    let date_only = format!("{value}T00:00:00Z");
    OffsetDateTime::parse(&date_only, &Rfc3339)
        .map_err(|_| Error::InvalidInput(format!("invalid timestamp `{value}`")))
}

/// Formats a timestamp the way the API does: RFC 3339, UTC, second precision.
pub fn format_timestamp(value: OffsetDateTime) -> Result<String> {
    let utc = value
        .to_offset(time::UtcOffset::UTC)
        .replace_nanosecond(0)
        .map_err(|err| Error::InvalidInput(format!("invalid timestamp: {err}")))?;
    utc.format(&Rfc3339)
        .map_err(|err| Error::InvalidInput(format!("invalid timestamp: {err}")))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn grant_shorthand_with_versions() {
        let grant = Grant::parse_shorthand("npm:@acme/sdk:^2.0").unwrap();
        assert_eq!(grant.eco, Ecosystem::Npm);
        assert_eq!(grant.package, "@acme/sdk");
        assert_eq!(grant.versions, "^2.0");
    }

    #[test]
    fn grant_shorthand_defaults_versions_to_all() {
        let grant = Grant::parse_shorthand("composer:acme/*").unwrap();
        assert_eq!(grant.eco, Ecosystem::Composer);
        assert_eq!(grant.package, "acme/*");
        assert_eq!(grant.versions, "*");
    }

    #[test]
    fn grant_shorthand_splits_at_first_two_colons_only() {
        let grant = Grant::parse_shorthand("cargo:acme-core:>=1.4, <2").unwrap();
        assert_eq!(grant.package, "acme-core");
        assert_eq!(grant.versions, ">=1.4, <2");

        let grant = Grant::parse_shorthand("npm:@acme/sdk:a:b").unwrap();
        assert_eq!(grant.versions, "a:b");
    }

    #[test]
    fn grant_shorthand_round_trips_through_display() {
        let grant = Grant::parse_shorthand("composer:acme/*:*").unwrap();
        assert_eq!(grant.to_string(), "composer:acme/*:*");
    }

    #[test]
    fn grant_shorthand_rejects_invalid_values() {
        for value in [
            "",
            "composer",
            "pypi:requests:*",
            "composer::*",
            "composer:acme/*x:*",
            "composer:acme/sdk:",
            "npm:@acme/ sdk",
        ] {
            assert!(
                matches!(Grant::parse_shorthand(value), Err(Error::InvalidInput(_))),
                "{value} should be rejected"
            );
        }
    }

    #[test]
    fn version_info_uses_camel_case() {
        let info: VersionInfo = serde_json::from_value(
            json!({ "version": "0.4.0", "minClientVersion": "0.1.0", "schemaVersion": 1 }),
        )
        .unwrap();
        assert_eq!(info.min_client_version, "0.1.0");
        assert_eq!(info.schema_version, 1);
    }

    #[test]
    fn publish_init_request_matches_contract() {
        let request = PublishInitRequest {
            eco: Ecosystem::Npm,
            name: "@acme/sdk".into(),
            version: "2.3.1".into(),
            file: "sdk-2.3.1.tgz".into(),
            size: 48213,
            sha256: "9f".repeat(32),
            manifest: json!({ "name": "@acme/sdk", "version": "2.3.1" }),
            tag: None,
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["eco"], "npm");
        assert_eq!(value["size"], 48213);
        assert!(value.get("tag").is_none());
    }

    #[test]
    fn publish_init_response_parses_and_hides_url_in_debug() {
        let response: PublishInitResponse = serde_json::from_value(json!({
            "uploadId": "01JBX9T6Q4ZK3M8N2P5R7S9V0W",
            "putUrl": "https://r2.example/_staging/x?X-Amz-Signature=secret",
            "expires": "2026-10-28T10:22:00Z",
            "headers": { "content-length": "48213", "x-amz-checksum-sha256": "nyx=" }
        }))
        .unwrap();
        assert_eq!(response.upload_id, "01JBX9T6Q4ZK3M8N2P5R7S9V0W");
        assert_eq!(response.headers["content-length"], "48213");
        assert_eq!(
            response.expires_at().unwrap().unix_timestamp(),
            1_793_182_920
        );
        assert!(!format!("{response:?}").contains("secret"));
    }

    #[test]
    fn commit_request_uses_camel_case() {
        let value = serde_json::to_value(PublishCommitRequest {
            upload_id: "abc".into(),
        })
        .unwrap();
        assert_eq!(value, json!({ "uploadId": "abc" }));
    }

    #[test]
    fn credential_list_parses_key_and_gh_entries() {
        let list: CredentialList = serde_json::from_value(json!({
            "credentials": [{
                "project": "shop-frontend",
                "kind": "read",
                "auth": [
                    { "id": "3f9a1c0b7d2e", "type": "key", "created": "2026-10-28T09:00:00Z", "expires": null },
                    { "id": "c08e5b1f9a27", "type": "gh", "repository": "acme/shop-frontend",
                      "ref": "refs/heads/main", "workflow": null,
                      "created": "2026-10-28T09:05:00Z", "expires": "2026-11-11T00:00:00Z" },
                    { "id": "000000000000", "type": "future", "created": "2026-10-28T09:05:00Z", "expires": null }
                ],
                "grants": [{ "eco": "cargo", "package": "acme-core", "versions": ">=1.4, <2" }]
            }]
        }))
        .unwrap();
        let credential = &list.credentials[0];
        assert_eq!(credential.kind, Kind::Read);
        assert_eq!(credential.auth[0].secret_type, SecretType::Key);
        assert_eq!(
            credential.auth[1].git_ref.as_deref(),
            Some("refs/heads/main")
        );
        assert_eq!(credential.auth[2].secret_type, SecretType::Other);
        assert_eq!(credential.grants[0].eco, Ecosystem::Cargo);
    }

    #[test]
    fn auth_entry_activity_follows_expiry() {
        let now = parse_timestamp("2026-10-28T10:00:00Z").unwrap();
        let mut entry = AuthEntry {
            id: "3f9a1c0b7d2e".into(),
            secret_type: SecretType::Key,
            created: "2026-10-28T09:00:00Z".into(),
            expires: None,
            repository: None,
            git_ref: None,
            workflow: None,
        };
        assert!(entry.is_active(now));
        entry.expires = Some("2026-10-28T09:59:59Z".into());
        assert!(!entry.is_active(now));
        entry.expires = Some("2026-11-11".into());
        assert!(entry.is_active(now));
    }

    #[test]
    fn create_credential_request_omits_default_secret() {
        let request = CreateCredentialRequest {
            project: "shop-frontend".into(),
            kind: Kind::Read,
            grants: vec![Grant::parse_shorthand("composer:acme/*:*").unwrap()],
            secret: None,
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "project": "shop-frontend",
                "kind": "read",
                "grants": [{ "eco": "composer", "package": "acme/*", "versions": "*" }]
            })
        );
    }

    #[test]
    fn add_secret_request_serializes_type_and_expiry() {
        let request = AddSecretRequest::key(Some("2026-11-11".into()));
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({ "type": "key", "expires": "2026-11-11" })
        );
    }

    #[test]
    fn responses_with_tokens_hide_them_in_debug() {
        let response: CreateCredentialResponse = serde_json::from_value(json!({
            "credential": { "project": "p1", "kind": "admin", "auth": [], "grants": [] },
            "token": "p1:supersecret"
        }))
        .unwrap();
        assert_eq!(response.token.as_deref(), Some("p1:supersecret"));
        assert!(!format!("{response:?}").contains("supersecret"));
    }

    #[test]
    fn yank_response_flattens_version_fields() {
        let response: YankResponse = serde_json::from_value(json!({
            "eco": "npm", "name": "@acme/sdk", "version": "2.3.1",
            "published": "2026-10-28T10:12:00Z", "yanked": true, "size": 48213, "sha256": "9f2c"
        }))
        .unwrap();
        assert!(response.version.yanked);
        assert_eq!(response.version.version, "2.3.1");
    }

    #[test]
    fn delete_package_response_uses_camel_case() {
        let response: DeletePackageResponse = serde_json::from_value(json!({
            "eco": "composer", "name": "acme/sdk", "deletedVersions": ["1.2.0", "1.1.0"]
        }))
        .unwrap();
        assert_eq!(response.deleted_versions, ["1.2.0", "1.1.0"]);
    }

    #[test]
    fn timestamps_parse_dates_and_format_without_fraction() {
        let parsed = parse_timestamp("2026-11-11").unwrap();
        assert_eq!(format_timestamp(parsed).unwrap(), "2026-11-11T00:00:00Z");
        let precise = parse_timestamp("2026-10-28T10:12:00.123Z").unwrap();
        assert_eq!(format_timestamp(precise).unwrap(), "2026-10-28T10:12:00Z");
        assert!(parse_timestamp("tomorrow").is_err());
    }
}
