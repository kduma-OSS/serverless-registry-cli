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

/// Role of a credential.
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

/// Access to a set of packages and versions of one ecosystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// Ecosystem the grant applies to.
    pub eco: Ecosystem,
    /// Exact package name or a pattern with a trailing `*`.
    pub package: String,
    /// Version constraint in the ecosystem's syntax; `*` means all versions.
    pub versions: String,
    /// Whether a `read` grant also opens the packages its versions depend on. The registry
    /// always returns it; `None` omits it from a request, which the registry stores as
    /// `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transitive: Option<bool>,
}

impl Grant {
    /// Parses the CLI shorthand `eco:package[:versions[:transitive]]`.
    ///
    /// The value is split at the first two `:` (for maven, whose package names contain one
    /// `:`, at the first three), so the version constraint may itself contain colons or
    /// commas (`cargo:acme-core:>=1.4, <2`). `versions` defaults to `*`. A rest ending in
    /// `:true` or `:false` sets `transitive`, which is otherwise left to the registry's
    /// default (`true`).
    pub fn parse_shorthand(value: &str) -> Result<Self> {
        let invalid = |reason: &str| {
            Error::InvalidInput(format!(
                "invalid grant `{value}`: {reason}; expected \
                 <eco>:<package>[:<versions>[:<transitive>]]"
            ))
        };

        let (eco, rest) = value.split_once(':').unwrap_or((value, ""));
        let eco = Ecosystem::from_str(eco).map_err(|_| {
            invalid("unknown ecosystem, expected one of composer, npm, cargo, nuget, generic, swift, maven")
        })?;
        // A maven package is `groupId:artifactId`: its own `:` is not a separator.
        let separators = if eco == Ecosystem::Maven { 2 } else { 1 };
        let mut parts = rest.splitn(separators + 1, ':');
        let package_parts: Vec<&str> = parts.by_ref().take(separators).collect();
        if package_parts.len() < separators || package_parts.iter().any(|part| part.is_empty()) {
            return Err(invalid("missing package"));
        }
        let package = package_parts.join(":");
        let rest = parts.next().unwrap_or("*");
        let (versions, transitive) = match rest.rsplit_once(':') {
            Some((versions, "true")) => (versions, Some(true)),
            Some((versions, "false")) => (versions, Some(false)),
            _ => (rest, None),
        };

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
            package,
            versions: versions.to_owned(),
            transitive,
        })
    }
}

impl fmt::Display for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.eco, self.package, self.versions)?;
        match self.transitive {
            Some(transitive) => write!(f, ":{transitive}"),
            None => Ok(()),
        }
    }
}

/// Body of every non-2xx response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// Stable, machine-readable error code, e.g. `version_exists`.
    pub error: String,
    /// Human-readable description.
    pub message: String,
}

/// `GET /api/v1/version`.
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

/// The grants of each role a token may use (`access` of `GET /api/v1/whoami`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Access {
    /// Grants of the `read` role; absent when the token may not read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<Vec<Grant>>,
    /// Grants of the `publish` role; absent when the token may not publish.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<Vec<Grant>>,
}

/// `GET /api/v1/whoami`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Whoami {
    /// `[<source>.]<project>` of the credential.
    pub project: String,
    /// Legacy view of the roles: `admin` when the token may administer, otherwise `read` when it
    /// may read, otherwise `publish`.
    pub kind: Kind,
    /// Legacy view of the grants: those of `kind`; empty for `admin`.
    pub grants: Vec<Grant>,
    /// The roles this token may use. Absent from registries older than credential roles; see
    /// [`Whoami::token_roles`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<Kind>,
    /// The grants of each role in [`roles`](Self::roles); see [`Whoami::role_grants`].
    #[serde(default, skip_serializing_if = "Access::is_empty")]
    pub access: Access,
}

impl Access {
    /// Whether the registry sent no per-role grants.
    pub fn is_empty(&self) -> bool {
        self.read.is_none() && self.publish.is_none()
    }
}

impl Whoami {
    /// The roles the token may use; `[kind]` for a registry that does not send `roles`.
    pub fn token_roles(&self) -> Vec<Kind> {
        if self.roles.is_empty() {
            vec![self.kind]
        } else {
            self.roles.clone()
        }
    }

    /// The grants of each of the token's roles that carries grants (`admin` has none), in
    /// `read`, `publish` order. A registry that does not send `access` reports its legacy
    /// `grants` under `kind`.
    pub fn role_grants(&self) -> Vec<(Kind, &[Grant])> {
        if self.access.is_empty() {
            return match self.kind {
                Kind::Admin => Vec::new(),
                kind => vec![(kind, self.grants.as_slice())],
            };
        }
        let mut out = Vec::new();
        if let Some(grants) = &self.access.read {
            out.push((Kind::Read, grants.as_slice()));
        }
        if let Some(grants) = &self.access.publish {
            out.push((Kind::Publish, grants.as_slice()));
        }
        out
    }
}

/// A file declared in `POST /api/v1/publish/init`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishFile {
    /// File name in the registry.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Lower-case hex SHA-256.
    pub sha256: String,
    /// Role of the file; `archive` for composer, npm and cargo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Free-form labels (generic packages).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

/// `POST /api/v1/publish/init` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishInitRequest {
    /// Ecosystem of the package.
    pub eco: Ecosystem,
    /// Package name.
    pub name: String,
    /// Version being published.
    pub version: String,
    /// Files of the version.
    pub files: Vec<PublishFile>,
    /// Ecosystem-native manifest, or the free-form `meta` object for generic packages.
    pub manifest: serde_json::Value,
    /// npm dist-tag (npm only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Generic only: `true` adds the files to an already published version, `false`
    /// requires a new version. Omitted for the other ecosystems.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append: Option<bool>,
}

/// Upload instructions for one file in the `init` response.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileUpload {
    /// File name, as declared.
    pub name: String,
    /// Presigned `PUT` URL. Treat it as a secret: never print or log it.
    pub put_url: String,
    /// Exactly the headers the upload must carry (lower-case names).
    pub headers: BTreeMap<String, String>,
}

impl fmt::Debug for FileUpload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileUpload")
            .field("name", &self.name)
            .field("put_url", &"<redacted>")
            .field("headers", &self.headers)
            .finish()
    }
}

/// `POST /api/v1/publish/init` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishInitResponse {
    /// Identifier to pass to `commit`.
    pub upload_id: String,
    /// When the presigned URLs stop working (RFC 3339).
    pub expires: String,
    /// One entry per declared file.
    pub uploads: Vec<FileUpload>,
}

impl PublishInitResponse {
    /// Parses [`expires`](Self::expires).
    pub fn expires_at(&self) -> Result<OffsetDateTime> {
        parse_timestamp(&self.expires)
    }
}

/// `POST /api/v1/publish/commit` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishCommitRequest {
    /// Identifier returned by `init`.
    pub upload_id: String,
}

/// A published file in the `commit` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedFile {
    /// File name.
    pub name: String,
    /// Download URL; `None` (`null` on the wire) for a file without a download endpoint,
    /// such as NuGet symbols.
    pub url: Option<String>,
}

/// `POST /api/v1/publish/commit` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishCommitResponse {
    /// Ecosystem of the package.
    pub eco: Ecosystem,
    /// Package name.
    pub name: String,
    /// Published version.
    pub version: String,
    /// Download URL of the archive (composer, npm, cargo) or of the version (generic).
    pub url: String,
    /// Download URL of every file of the version.
    #[serde(default)]
    pub files: Vec<PublishedFile>,
}

/// A file of a published version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// File name.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Lower-case hex SHA-256.
    pub sha256: String,
    /// Extra digests computed by the registry (maven: `sha1`, `md5`, `sha512`), hex.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub digests: BTreeMap<String, String>,
    /// Role of the file, e.g. `archive`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Labels (generic packages).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

/// One version of a package (`PackageVersion`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionEntry {
    /// Version string.
    pub version: String,
    /// Publication time (RFC 3339).
    pub published: String,
    /// Whether the version is yanked.
    pub yanked: bool,
    /// Files of the version.
    pub files: Vec<FileEntry>,
}

/// A package with its versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageEntry {
    /// Ecosystem of the package.
    pub eco: Ecosystem,
    /// Decoded package name.
    pub name: String,
    /// Versions, highest first.
    pub versions: Vec<VersionEntry>,
    /// `read` credentials only: what the grant opens transitively (docs/api.md 6.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<PackageDependency>>,
}

/// A package a `read` grant opens transitively, with the versions it opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageDependency {
    /// Package name.
    pub name: String,
    /// The declared constraints that lead to it.
    pub constraints: Vec<String>,
    /// The versions opened, highest first.
    pub versions: Vec<String>,
}

/// `GET /api/v1/packages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageList {
    /// Packages sorted by ecosystem and name.
    pub packages: Vec<PackageEntry>,
}

/// `POST /api/v1/packages/<eco>/<name>/<version>/yank`.
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

/// `DELETE /api/v1/packages/<eco>/<name>/<version>`.
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

/// `DELETE /api/v1/packages/<eco>/<name>`.
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

/// `POST /api/v1/packages/<eco>/<name>/reindex`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReindexResponse {
    /// Ecosystem of the package.
    pub eco: Ecosystem,
    /// Package name.
    pub name: String,
    /// Number of version manifests the index was built from.
    pub manifests: u64,
}

/// A file in the generic feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenericFeedFile {
    /// File name.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Lower-case hex SHA-256.
    pub sha256: String,
    /// Extra digests computed by the registry, hex; only when the version has them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub digests: BTreeMap<String, String>,
    /// Labels of the file.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Download URL on the registry; answers with a redirect to the storage.
    pub url: String,
}

/// `GET <registry>/generic/<name>/<version>` and `GET <registry>/generic/<name>/latest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenericFeedVersion {
    /// Package name.
    pub name: String,
    /// Version.
    pub version: String,
    /// Publication time (RFC 3339).
    pub published: String,
    /// Whether the version is yanked.
    pub yanked: bool,
    /// Free-form metadata of the version.
    #[serde(default)]
    pub meta: serde_json::Value,
    /// Files matching the requested labels.
    pub files: Vec<GenericFeedFile>,
}

/// A file the label filter of `GET /generic/<name>/<version>/-/download` left among
/// several (`409 ambiguous`). The registry may list bare names or file entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "GenericCandidateRepr")]
pub struct GenericCandidate {
    /// File name.
    pub name: String,
    /// Labels of the file, when the registry sent them.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum GenericCandidateRepr {
    Name(String),
    Entry {
        name: String,
        #[serde(default)]
        labels: BTreeMap<String, String>,
    },
}

impl From<GenericCandidateRepr> for GenericCandidate {
    fn from(repr: GenericCandidateRepr) -> Self {
        match repr {
            GenericCandidateRepr::Name(name) => GenericCandidate {
                name,
                labels: BTreeMap::new(),
            },
            GenericCandidateRepr::Entry { name, labels } => GenericCandidate { name, labels },
        }
    }
}

/// Body of a `409 ambiguous` answer of the generic download endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GenericAmbiguousBody {
    /// The version the registry resolved, if it says.
    #[serde(default)]
    pub version: Option<String>,
    /// The files the labels select.
    #[serde(default)]
    pub files: Vec<GenericCandidate>,
}

/// Type of an auth entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretType {
    /// A static key generated by the registry.
    Key,
    /// A GitHub Actions OIDC token.
    Gh,
    /// A password, with an optional one-time code, used in a browser (never sent as a token).
    Login,
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
            SecretType::Login => "login",
            SecretType::Other => "other",
        }
    }
}

impl fmt::Display for SecretType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An auth entry of a credential.
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
    /// The roles this entry may use, within the credential's roles. Empty when the registry
    /// predates credential roles (the entry then has the credential's single role).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<Kind>,
    /// A note about the entry (at most 200 characters); `None` when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `login` only: whether a one-time code is required besides the password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp: Option<bool>,
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

/// A credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    /// Local project slug.
    pub project: String,
    /// Legacy view of the roles: `admin` when the credential holds it, otherwise `read` when it
    /// holds that, otherwise `publish`.
    pub kind: Kind,
    /// The roles of the credential: any non-empty combination of `read`, `publish` and `admin`.
    /// Empty when the
    /// registry predates credential roles; see [`Credential::held_roles`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<Kind>,
    /// Auth entries, including expired ones until the cron removes them.
    pub auth: Vec<AuthEntry>,
    /// Legacy view of the grants: those of `kind`; always empty for `admin`.
    pub grants: Vec<Grant>,
    /// Grants of the `read` role; empty without it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_grants: Vec<Grant>,
    /// Grants of the `publish` role; empty without it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publish_grants: Vec<Grant>,
}

impl Credential {
    /// The roles of the credential; `[kind]` for a registry that does not send `roles`.
    pub fn held_roles(&self) -> Vec<Kind> {
        if self.roles.is_empty() {
            vec![self.kind]
        } else {
            self.roles.clone()
        }
    }
}

/// `GET /api/v1/credentials`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialList {
    /// Credentials sorted by project.
    pub credentials: Vec<Credential>,
}

/// `POST /api/v1/credentials/<project>/secrets` request; also the
/// `secret` field of [`CreateCredentialRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddSecretRequest {
    /// Type of the new entry.
    #[serde(rename = "type")]
    pub secret_type: SecretType,
    /// Expiry (RFC 3339 or `YYYY-MM-DD`); omitted means never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    /// The roles the secret may use. Required for a credential with both `read` and
    /// `publish`; omitted otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<Kind>,
    /// A note about the entry (at most 200 characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `login` only: the password; the registry generates one when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// `login` only: also require a one-time code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp: Option<bool>,
    /// `gh` only: the `id` of a `gh` entry that expires now while this one is added (an edit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
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
            scope: Vec::new(),
            description: None,
            password: None,
            totp: None,
            replaces: None,
            repository: None,
            git_ref: None,
            workflow: None,
        }
    }
}

/// What the registry shows once when a `login` entry is created.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginReveal {
    /// The password, chosen or generated; never returned again.
    pub password: String,
    /// The one-time code enrolment, when the entry has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp: Option<TotpReveal>,
}

/// The shared secret of a one-time code, as shown once.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TotpReveal {
    /// The base32 shared secret.
    pub secret: String,
    /// The `otpauth://` link authenticator apps import.
    pub otpauth_uri: String,
}

impl fmt::Debug for LoginReveal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginReveal")
            .field("password", &"<redacted>")
            .field("totp", &self.totp.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// `POST /api/v1/credentials/<project>/secrets` response.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddSecretResponse {
    /// The new entry.
    pub secret: AuthEntry,
    /// The full token; present only for `key` entries and never returned again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The password and one-time code enrolment; present only for `login` entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<LoginReveal>,
}

impl fmt::Debug for AddSecretResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AddSecretResponse")
            .field("secret", &self.secret)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("login", &self.login)
            .finish()
    }
}

/// `POST /api/v1/credentials` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCredentialRequest {
    /// Project slug.
    pub project: String,
    /// Legacy single-role form: the credential's only role. Not together with
    /// [`roles`](Self::roles).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    /// The roles of the credential: any non-empty combination of `read`, `publish` and `admin`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<Kind>,
    /// Legacy form: the grants of a credential with one role; must be empty for `admin`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<Grant>,
    /// Grants of the `read` role.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_grants: Vec<Grant>,
    /// Grants of the `publish` role.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publish_grants: Vec<Grant>,
    /// The first auth entry; the registry defaults to a `key` without expiry, which it
    /// refuses for a credential with both roles (the entry needs a `scope`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<AddSecretRequest>,
}

impl CreateCredentialRequest {
    /// A credential with a single role and its grants (the legacy form).
    pub fn single(project: impl Into<String>, kind: Kind, grants: Vec<Grant>) -> Self {
        CreateCredentialRequest {
            project: project.into(),
            kind: Some(kind),
            roles: Vec::new(),
            grants,
            read_grants: Vec::new(),
            publish_grants: Vec::new(),
            secret: None,
        }
    }

    /// A credential with both roles, separate grants and the scope of its first secret.
    pub fn both(
        project: impl Into<String>,
        read_grants: Vec<Grant>,
        publish_grants: Vec<Grant>,
        secret: AddSecretRequest,
    ) -> Self {
        CreateCredentialRequest {
            project: project.into(),
            kind: None,
            roles: vec![Kind::Read, Kind::Publish],
            grants: Vec::new(),
            read_grants,
            publish_grants,
            secret: Some(secret),
        }
    }
}

/// `POST /api/v1/credentials` response.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCredentialResponse {
    /// The created credential.
    pub credential: Credential,
    /// The full token; present only when the first secret is a `key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The password and one-time code enrolment; present only when the first secret is a
    /// `login`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<LoginReveal>,
}

impl fmt::Debug for CreateCredentialResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateCredentialResponse")
            .field("credential", &self.credential)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("login", &self.login)
            .finish()
    }
}

/// `PATCH /api/v1/credentials/<project>/secrets/<hash-prefix>` request: the description of the
/// entry and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescribeSecretRequest {
    /// The new description; `None` (sent as `null`) clears it.
    pub description: Option<String>,
}

/// `PATCH /api/v1/credentials/<project>` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateGrantsRequest {
    /// The new, complete list of grants.
    pub grants: Vec<Grant>,
}

/// `PATCH /api/v1/credentials/<project>` request for credentials with roles: every field is
/// optional, at least one must be set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCredentialRequest {
    /// The new roles (any combination of `read`, `publish` and `admin`; the last admin
    /// credential keeps `admin`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<Kind>>,
    /// The new, complete list of `read` grants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_grants: Option<Vec<Grant>>,
    /// The new, complete list of `publish` grants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_grants: Option<Vec<Grant>>,
}

/// `DELETE /api/v1/credentials/<project>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteCredentialResponse {
    /// The deleted project.
    pub project: String,
    /// Always `true`.
    pub deleted: bool,
}

/// `DELETE /api/v1/credentials/<project>/secrets/<hash-prefix>` request
///.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpireSecretRequest {
    /// When the entry expires (RFC 3339 or `YYYY-MM-DD`); omitted means now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
}

/// `DELETE /api/v1/credentials/<project>/secrets/<hash-prefix>` response
///.
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
        for value in [
            "composer:acme/*:*",
            "npm:@acme/sdk:^2.0:false",
            "maven:com.acme:sdk:[1.0,2.0):true",
        ] {
            assert_eq!(Grant::parse_shorthand(value).unwrap().to_string(), value);
        }
    }

    #[test]
    fn grant_shorthand_reads_the_transitive_suffix() {
        let grant = Grant::parse_shorthand("npm:@acme/sdk:^2.0:false").unwrap();
        assert_eq!(grant.versions, "^2.0");
        assert_eq!(grant.transitive, Some(false));
        let grant = Grant::parse_shorthand("cargo:acme-core:>=1.4, <2:true").unwrap();
        assert_eq!(grant.versions, ">=1.4, <2");
        assert_eq!(grant.transitive, Some(true));
        assert_eq!(
            Grant::parse_shorthand("npm:@acme/sdk").unwrap().transitive,
            None
        );
    }

    #[test]
    fn grant_shorthand_keeps_the_colon_of_maven_packages() {
        let grant = Grant::parse_shorthand("maven:com.acme:sdk:[1.0,2.0)").unwrap();
        assert_eq!(grant.eco, Ecosystem::Maven);
        assert_eq!(grant.package, "com.acme:sdk");
        assert_eq!(grant.versions, "[1.0,2.0)");
        let grant = Grant::parse_shorthand("maven:com.acme:*").unwrap();
        assert_eq!(grant.package, "com.acme:*");
        assert_eq!(grant.versions, "*");
        assert!(Grant::parse_shorthand("maven:com.acme").is_err());
    }

    #[test]
    fn grant_transitive_is_omitted_from_requests_unless_set() {
        let grant = Grant::parse_shorthand("composer:acme/*").unwrap();
        let value = serde_json::to_value(&grant).unwrap();
        assert_eq!(
            value,
            json!({ "eco": "composer", "package": "acme/*", "versions": "*" })
        );
        let read: Grant = serde_json::from_value(json!({
            "eco": "npm", "package": "@acme/sdk", "versions": "^2.0", "transitive": false
        }))
        .unwrap();
        assert_eq!(read.transitive, Some(false));
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
            files: vec![PublishFile {
                name: "sdk-2.3.1.tgz".into(),
                size: 48213,
                sha256: "9f".repeat(32),
                role: Some("archive".into()),
                labels: BTreeMap::new(),
            }],
            manifest: json!({ "name": "@acme/sdk", "version": "2.3.1" }),
            tag: None,
            append: None,
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["eco"], "npm");
        assert_eq!(
            value["files"],
            json!([{ "name": "sdk-2.3.1.tgz", "size": 48213, "sha256": "9f".repeat(32), "role": "archive" }])
        );
        assert!(value.get("tag").is_none());
        assert!(value.get("append").is_none());
    }

    #[test]
    fn generic_init_request_carries_labels_and_append() {
        let request = PublishInitRequest {
            eco: Ecosystem::Npm,
            name: "tools/cli".into(),
            version: "1.0.0".into(),
            files: vec![PublishFile {
                name: "cli-linux.tar.gz".into(),
                size: 1,
                sha256: "ab".repeat(32),
                role: None,
                labels: BTreeMap::from([("os".to_owned(), "linux".to_owned())]),
            }],
            manifest: json!({}),
            tag: None,
            append: Some(true),
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["files"][0]["labels"], json!({ "os": "linux" }));
        assert!(value["files"][0].get("role").is_none());
        assert_eq!(value["append"], true);
    }

    #[test]
    fn publish_init_response_parses_and_hides_urls_in_debug() {
        let response: PublishInitResponse = serde_json::from_value(json!({
            "uploadId": "01JBX9T6Q4ZK3M8N2P5R7S9V0W",
            "expires": "2026-10-28T10:22:00Z",
            "uploads": [{
                "name": "sdk-2.3.1.tgz",
                "putUrl": "https://r2.example/_staging/x?X-Amz-Signature=secret",
                "headers": { "content-length": "48213", "x-amz-checksum-sha256": "nyx=" }
            }]
        }))
        .unwrap();
        assert_eq!(response.upload_id, "01JBX9T6Q4ZK3M8N2P5R7S9V0W");
        assert_eq!(response.uploads[0].headers["content-length"], "48213");
        assert_eq!(
            response.expires_at().unwrap().unix_timestamp(),
            1_793_182_920
        );
        assert!(!format!("{response:?}").contains("secret"));
    }

    #[test]
    fn commit_response_lists_files() {
        let response: PublishCommitResponse = serde_json::from_value(json!({
            "eco": "npm", "name": "@acme/sdk", "version": "2.3.1",
            "url": "https://packages.example.com/npm/@acme/sdk/-/sdk-2.3.1.tgz",
            "files": [{ "name": "sdk-2.3.1.tgz", "url": "https://packages.example.com/npm/@acme/sdk/-/sdk-2.3.1.tgz" }]
        }))
        .unwrap();
        assert_eq!(response.files[0].name, "sdk-2.3.1.tgz");
    }

    #[test]
    fn commit_response_accepts_files_without_url() {
        let body = json!({
            "eco": "nuget", "name": "acme.sdk", "version": "1.2.0",
            "url": "https://packages.example.com/nuget/flat/acme.sdk/1.2.0/acme.sdk.1.2.0.nupkg",
            "files": [
                { "name": "acme.sdk.1.2.0.nupkg", "url": "https://packages.example.com/nuget/flat/acme.sdk/1.2.0/acme.sdk.1.2.0.nupkg" },
                { "name": "acme.sdk.1.2.0.snupkg", "url": null }
            ]
        });
        let response: PublishCommitResponse = serde_json::from_value(body.clone()).unwrap();
        assert!(response.files[0].url.is_some());
        assert_eq!(response.files[1].url, None);
        // `--json` prints the body as received.
        assert_eq!(serde_json::to_value(&response).unwrap(), body);
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
    fn credential_with_both_roles_parses_scopes_and_per_role_grants() {
        let credential: Credential = serde_json::from_value(json!({
            "project": "acme-sdk",
            "kind": "read",
            "roles": ["read", "publish"],
            "auth": [
                { "id": "3f9a1c0b7d2e", "type": "key", "scope": ["read"],
                  "created": "2026-10-28T09:00:00Z", "expires": null },
                { "id": "c08e5b1f9a27", "type": "gh", "scope": ["publish"],
                  "repository": "acme/acme-sdk", "ref": "refs/tags/v*", "workflow": null,
                  "created": "2026-10-28T09:05:00Z", "expires": null }
            ],
            "grants": [{ "eco": "cargo", "package": "acme-core", "versions": "*" }],
            "read_grants": [{ "eco": "cargo", "package": "acme-core", "versions": "*" }],
            "publish_grants": [{ "eco": "cargo", "package": "acme-sdk", "versions": "*" }]
        }))
        .unwrap();
        assert_eq!(credential.held_roles(), vec![Kind::Read, Kind::Publish]);
        assert_eq!(credential.auth[0].scope, vec![Kind::Read]);
        assert_eq!(credential.auth[1].scope, vec![Kind::Publish]);
        assert_eq!(credential.read_grants[0].package, "acme-core");
        assert_eq!(credential.publish_grants[0].package, "acme-sdk");
    }

    #[test]
    fn credential_from_a_registry_without_roles_has_its_kind_as_only_role() {
        let credential: Credential = serde_json::from_value(json!({
            "project": "ci", "kind": "publish",
            "auth": [{ "id": "3f9a1c0b7d2e", "type": "key",
                       "created": "2026-10-28T09:00:00Z", "expires": null }],
            "grants": []
        }))
        .unwrap();
        assert_eq!(credential.held_roles(), vec![Kind::Publish]);
        assert!(credential.auth[0].scope.is_empty());
        assert!(credential.read_grants.is_empty());
    }

    #[test]
    fn whoami_reports_roles_and_the_grants_of_each() {
        let both: Whoami = serde_json::from_value(json!({
            "project": "acme-sdk", "kind": "read", "grants": [],
            "roles": ["read", "publish"],
            "access": {
                "read": [{ "eco": "npm", "package": "@acme/sdk", "versions": "*" }],
                "publish": []
            }
        }))
        .unwrap();
        assert_eq!(both.token_roles(), vec![Kind::Read, Kind::Publish]);
        let sections = both.role_grants();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, Kind::Read);
        assert_eq!(sections[0].1.len(), 1);
        assert_eq!(sections[1].0, Kind::Publish);
        assert!(sections[1].1.is_empty());
    }

    #[test]
    fn whoami_from_a_registry_without_roles_falls_back_to_kind_and_grants() {
        let legacy: Whoami = serde_json::from_value(json!({
            "project": "ci", "kind": "publish",
            "grants": [{ "eco": "npm", "package": "@acme/sdk", "versions": "*" }]
        }))
        .unwrap();
        assert_eq!(legacy.token_roles(), vec![Kind::Publish]);
        let sections = legacy.role_grants();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, Kind::Publish);
        let admin: Whoami =
            serde_json::from_value(json!({ "project": "root", "kind": "admin", "grants": [] }))
                .unwrap();
        assert!(admin.role_grants().is_empty());
    }

    #[test]
    fn whoami_of_an_admin_that_also_reads_reports_both_roles_and_the_read_grants() {
        let whoami: Whoami = serde_json::from_value(json!({
            "project": "ops", "kind": "admin", "grants": [],
            "roles": ["read", "admin"],
            "access": { "read": [{ "eco": "npm", "package": "@acme/sdk", "versions": "*" }] }
        }))
        .unwrap();
        assert_eq!(whoami.token_roles(), vec![Kind::Read, Kind::Admin]);
        let sections = whoami.role_grants();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, Kind::Read);
    }

    #[test]
    fn create_credential_with_both_roles_serializes_roles_grants_and_scope() {
        let mut secret = AddSecretRequest::key(None);
        secret.scope = vec![Kind::Read];
        let request = CreateCredentialRequest::both(
            "acme-sdk",
            vec![Grant::parse_shorthand("cargo:acme-core").unwrap()],
            vec![Grant::parse_shorthand("cargo:acme-sdk").unwrap()],
            secret,
        );
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "project": "acme-sdk",
                "roles": ["read", "publish"],
                "read_grants": [{ "eco": "cargo", "package": "acme-core", "versions": "*" }],
                "publish_grants": [{ "eco": "cargo", "package": "acme-sdk", "versions": "*" }],
                "secret": { "type": "key", "scope": ["read"] }
            })
        );
    }

    #[test]
    fn update_credential_request_sends_only_what_is_set() {
        let request = UpdateCredentialRequest {
            roles: Some(vec![Kind::Read, Kind::Publish]),
            publish_grants: Some(vec![Grant::parse_shorthand("cargo:acme-sdk").unwrap()]),
            ..UpdateCredentialRequest::default()
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "roles": ["read", "publish"],
                "publish_grants": [{ "eco": "cargo", "package": "acme-sdk", "versions": "*" }]
            })
        );
    }

    #[test]
    fn auth_entries_carry_a_description_and_the_login_type() {
        let list: CredentialList = serde_json::from_value(json!({
            "credentials": [{
                "project": "ops",
                "kind": "admin",
                "auth": [
                    { "id": "5d3b8e21c7f0", "type": "login", "totp": true, "description": "Ops lead",
                      "created": "2026-10-28T09:00:00Z", "expires": null },
                    { "id": "3f9a1c0b7d2e", "type": "key", "description": null,
                      "created": "2026-10-28T09:00:00Z", "expires": null },
                    { "id": "c08e5b1f9a27", "type": "gh", "repository": "acme/shop",
                      "created": "2026-10-28T09:05:00Z", "expires": null }
                ],
                "grants": []
            }]
        }))
        .unwrap();
        let auth = &list.credentials[0].auth;
        assert_eq!(auth[0].secret_type, SecretType::Login);
        assert_eq!(auth[0].totp, Some(true));
        assert_eq!(auth[0].description.as_deref(), Some("Ops lead"));
        assert_eq!(auth[1].description, None);
        assert_eq!(auth[2].description, None);
        assert_eq!(SecretType::Login.as_str(), "login");
    }

    #[test]
    fn add_secret_request_serializes_the_new_fields_only_when_set() {
        let mut request = AddSecretRequest::key(None);
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({ "type": "key" })
        );
        request.secret_type = SecretType::Login;
        request.password = Some("correct-horse-battery".into());
        request.totp = Some(true);
        request.description = Some("Jane".into());
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "type": "login",
                "description": "Jane",
                "password": "correct-horse-battery",
                "totp": true
            })
        );
        let mut edit = AddSecretRequest::key(None);
        edit.secret_type = SecretType::Gh;
        edit.repository = Some("acme/shop".into());
        edit.replaces = Some("c08e5b1f9a27".into());
        assert_eq!(
            serde_json::to_value(&edit).unwrap(),
            json!({ "type": "gh", "replaces": "c08e5b1f9a27", "repository": "acme/shop" })
        );
    }

    #[test]
    fn a_login_reveal_is_parsed_and_never_printed() {
        let response: AddSecretResponse = serde_json::from_value(json!({
            "secret": { "id": "5d3b8e21c7f0", "type": "login", "totp": true,
                        "created": "2026-10-28T09:10:00Z", "expires": null },
            "login": {
                "password": "Hq3vN8xZk1Lp6RtY0wUe5mCa",
                "totp": { "secret": "JBSWY3DPEHPK3PXP",
                          "otpauth_uri": "otpauth://totp/registry:ops?secret=JBSWY3DPEHPK3PXP" }
            }
        }))
        .unwrap();
        let login = response.login.as_ref().unwrap();
        assert_eq!(login.password, "Hq3vN8xZk1Lp6RtY0wUe5mCa");
        assert_eq!(login.totp.as_ref().unwrap().secret, "JBSWY3DPEHPK3PXP");
        let shown = format!("{response:?}");
        for secret in ["Hq3vN8xZk1Lp6RtY0wUe5mCa", "JBSWY3DPEHPK3PXP"] {
            assert!(!shown.contains(secret), "{shown}");
        }
    }

    #[test]
    fn describe_secret_request_sends_null_to_clear() {
        let set = DescribeSecretRequest {
            description: Some("note".into()),
        };
        assert_eq!(
            serde_json::to_value(&set).unwrap(),
            json!({ "description": "note" })
        );
        let clear = DescribeSecretRequest { description: None };
        assert_eq!(
            serde_json::to_value(&clear).unwrap(),
            json!({ "description": null })
        );
    }

    #[test]
    fn auth_entry_activity_follows_expiry() {
        let now = parse_timestamp("2026-10-28T10:00:00Z").unwrap();
        let mut entry = AuthEntry {
            id: "3f9a1c0b7d2e".into(),
            secret_type: SecretType::Key,
            created: "2026-10-28T09:00:00Z".into(),
            expires: None,
            scope: Vec::new(),
            description: None,
            totp: None,
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
        let request = CreateCredentialRequest::single(
            "shop-frontend",
            Kind::Read,
            vec![Grant::parse_shorthand("composer:acme/*:*").unwrap()],
        );
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
            "published": "2026-10-28T10:12:00Z", "yanked": true,
            "files": [{ "name": "sdk-2.3.1.tgz", "size": 48213, "sha256": "9f2c", "role": "archive" }]
        }))
        .unwrap();
        assert!(response.version.yanked);
        assert_eq!(response.version.version, "2.3.1");
        assert_eq!(response.version.files[0].role.as_deref(), Some("archive"));
    }

    #[test]
    fn package_listing_keeps_digests_and_dependencies() {
        let body = json!({
            "eco": "maven", "name": "com.acme:sdk",
            "versions": [{
                "version": "1.4.0", "published": "2026-10-28T10:12:00Z", "yanked": false,
                "files": [{
                    "name": "sdk-1.4.0.jar", "size": 3, "sha256": "ab", "role": "archive",
                    "digests": { "md5": "cd", "sha1": "ef", "sha512": "01" }
                }]
            }],
            "dependencies": [
                { "name": "com.acme:core", "constraints": ["[1.2.0]"], "versions": ["1.2.0"] }
            ]
        });
        let entry: PackageEntry = serde_json::from_value(body.clone()).unwrap();
        assert_eq!(entry.versions[0].files[0].digests["sha1"], "ef");
        assert_eq!(
            entry.dependencies.as_ref().unwrap()[0].constraints,
            ["[1.2.0]"]
        );
        assert_eq!(serde_json::to_value(&entry).unwrap(), body);
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
