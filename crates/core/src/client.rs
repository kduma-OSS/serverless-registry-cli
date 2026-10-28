//! HTTP client of the registry API (`/api/v1`).
//!
//! Every request carries `User-Agent: sreg/<version>`, `X-Client-Version`,
//! `Accept: application/json` and, when a token is configured,
//! `Authorization: Bearer <token>`. Error responses are mapped to [`Error`]:
//! `426` to [`Error::ClientTooOld`], `409 version_exists` to [`Error::VersionExists`],
//! `409 file_exists` to [`Error::FileExists`] and everything else to [`Error::Api`].

use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, RETRY_AFTER};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use crate::error::{Error, Result};
use crate::models::{
    AddSecretRequest, AddSecretResponse, CreateCredentialRequest, CreateCredentialResponse,
    Credential, CredentialList, DeleteCredentialResponse, DeletePackageResponse,
    DeleteVersionResponse, ErrorBody, ExpireSecretRequest, ExpireSecretResponse,
    GenericAmbiguousBody, GenericFeedVersion, Grant, PackageEntry, PackageList,
    PublishCommitRequest, PublishCommitResponse, PublishInitRequest, PublishInitResponse,
    ReindexResponse, UpdateGrantsRequest, VersionInfo, Whoami, YankResponse,
};
use crate::packager::Ecosystem;
use crate::retry::{is_retryable, RetryPolicy};

/// Request header carrying the client's own version.
pub const CLIENT_VERSION_HEADER: &str = "x-client-version";

/// Response header carrying the oldest client version the registry supports.
pub const MIN_CLIENT_VERSION_HEADER: &str = "x-min-client-version";

/// Time allowed to establish a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Time allowed for a whole API request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Time allowed for `publish/commit`, which copies the archive inside the registry.
const COMMIT_TIMEOUT: Duration = Duration::from_secs(300);

/// Default wait for a `429` without `Retry-After`.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(10);

/// Longest wait honoured for `Retry-After`.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Filters of a generic feed request (`docs/api.md`, 8.1).
#[derive(Debug, Clone, Copy, Default)]
pub struct GenericQuery<'a> {
    /// `label=<key>:<value>` per label: only files carrying every label.
    pub labels: &'a [(String, String)],
    /// `channel=prerelease`: `latest` considers prerelease versions too.
    pub prerelease: bool,
    /// `version=<constraint>`: `latest` is the highest version satisfying this semver
    /// range (npm syntax, e.g. `^1.2`, `>=1 <2`, `1.x`).
    pub constraint: Option<&'a str>,
    /// `include=yanked`: an exact version is returned even when it is yanked.
    pub include_yanked: bool,
}

impl GenericQuery<'_> {
    /// Appends the query parameters to `url`, leaving it without `?` when there are none.
    pub fn apply(&self, url: &mut Url) {
        {
            let mut query = url.query_pairs_mut();
            for (key, value) in self.labels {
                query.append_pair("label", &format!("{key}:{value}"));
            }
            if let Some(constraint) = self.constraint {
                query.append_pair("version", constraint);
            }
            if self.prerelease {
                query.append_pair("channel", "prerelease");
            }
            if self.include_yanked {
                query.append_pair("include", "yanked");
            }
        }
        if url.query() == Some("") {
            url.set_query(None);
        }
    }
}

/// Client of the registry API.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: Url,
    token: Option<String>,
    client_version: String,
    retry: RetryPolicy,
    min_client_version: Arc<Mutex<Option<String>>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base.as_str())
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("client_version", &self.client_version)
            .finish()
    }
}

impl Client {
    /// Creates a client for the registry at `registry_url`.
    ///
    /// `registry_url` is the registry's `PUBLIC_URL`; it may contain a path prefix.
    pub fn new(registry_url: Url, token: Option<String>, client_version: &str) -> Result<Self> {
        if registry_url.cannot_be_a_base() || !matches!(registry_url.scheme(), "http" | "https") {
            return Err(Error::InvalidInput(format!(
                "invalid registry URL `{registry_url}`: expected an http(s) URL"
            )));
        }

        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            CLIENT_VERSION_HEADER,
            HeaderValue::from_str(client_version).map_err(|_| {
                Error::InvalidInput(format!("invalid client version `{client_version}`"))
            })?,
        );

        let http = reqwest::Client::builder()
            .user_agent(format!("sreg/{client_version}"))
            .default_headers(headers)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;

        let mut base = registry_url;
        base.set_query(None);
        base.set_fragment(None);

        Ok(Client {
            http,
            base,
            token,
            client_version: client_version.to_owned(),
            retry: RetryPolicy::default(),
            min_client_version: Arc::new(Mutex::new(None)),
        })
    }

    /// Replaces the retry policy used for idempotent requests and uploads.
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// The retry policy used for idempotent requests and uploads.
    pub fn retry_policy(&self) -> RetryPolicy {
        self.retry
    }

    /// The registry URL this client talks to.
    pub fn registry_url(&self) -> &Url {
        &self.base
    }

    /// The version this client reports in `X-Client-Version`.
    pub fn client_version(&self) -> &str {
        &self.client_version
    }

    /// The last `X-Min-Client-Version` received from the registry, if any.
    pub fn min_client_version(&self) -> Option<String> {
        self.min_client_version
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    /// The URL of the API endpoint made of `segments` under `/api/v1`.
    ///
    /// Each segment is percent-encoded on its own, so `/` in a package name becomes `%2F`.
    pub fn endpoint(&self, segments: &[&str]) -> Result<Url> {
        let mut all = vec!["api", "v1"];
        all.extend_from_slice(segments);
        self.url(&all)
    }

    /// The URL made of `segments` under the registry URL, each percent-encoded.
    pub fn url(&self, segments: &[&str]) -> Result<Url> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| Error::InvalidInput(format!("invalid registry URL `{}`", self.base)))?
            .pop_if_empty()
            .extend(segments);
        Ok(url)
    }

    /// The token this client authenticates with, if any. Never log it.
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// `GET <registry>/generic/<name>/<version>` (or `/latest`): one version of a
    /// generic package from the generic feed.
    ///
    /// The package name is one path segment, so the `/` of a grouped name is sent as
    /// `%2F` (`/generic/acme%2Fplugin-foo/latest`). `query` adds the filters; with
    /// `latest` and a constraint, the registry resolves the highest version satisfying it.
    pub async fn generic_version(
        &self,
        name: &str,
        version: &str,
        query: &GenericQuery<'_>,
    ) -> Result<GenericFeedVersion> {
        let mut url = self.url(&["generic", name, version])?;
        query.apply(&mut url);
        self.send_to(Method::GET, url, None, self.retry, None).await
    }

    /// The URL of `GET <registry>/generic/<name>/<version>/-/download` (`<version>` may be
    /// `latest`), which redirects to the single file selected by `query`'s labels.
    ///
    /// Fetch it with [`Downloader::resolve_sole`](crate::download::Downloader::resolve_sole),
    /// which reads the redirect instead of following it.
    pub fn generic_sole_url(
        &self,
        name: &str,
        version: &str,
        query: &GenericQuery<'_>,
    ) -> Result<Url> {
        let mut url = self.url(&["generic", name, version, "-", "download"])?;
        query.apply(&mut url);
        Ok(url)
    }

    /// `GET` an endpoint, retrying on transport errors, `5xx` and `429`.
    pub async fn get<T: DeserializeOwned>(&self, segments: &[&str]) -> Result<T> {
        self.send(Method::GET, segments, None, self.retry, None)
            .await
    }

    /// `POST` an endpoint with an optional JSON body; never retried.
    pub async fn post<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        segments: &[&str],
        body: Option<&B>,
    ) -> Result<T> {
        let body = body.map(serde_json::to_value).transpose()?;
        self.send(Method::POST, segments, body, RetryPolicy::none(), None)
            .await
    }

    /// `PATCH` an endpoint with a JSON body; never retried.
    pub async fn patch<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        segments: &[&str],
        body: &B,
    ) -> Result<T> {
        let body = serde_json::to_value(body)?;
        self.send(
            Method::PATCH,
            segments,
            Some(body),
            RetryPolicy::none(),
            None,
        )
        .await
    }

    /// `DELETE` an endpoint with an optional JSON body; never retried.
    pub async fn delete<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        segments: &[&str],
        body: Option<&B>,
    ) -> Result<T> {
        let body = body.map(serde_json::to_value).transpose()?;
        self.send(Method::DELETE, segments, body, RetryPolicy::none(), None)
            .await
    }

    /// `GET /api/v1/version`.
    pub async fn version(&self) -> Result<VersionInfo> {
        self.get(&["version"]).await
    }

    /// `GET /api/v1/whoami`.
    pub async fn whoami(&self) -> Result<Whoami> {
        self.get(&["whoami"]).await
    }

    /// `POST /api/v1/publish/init`.
    pub async fn publish_init(&self, request: &PublishInitRequest) -> Result<PublishInitResponse> {
        self.post(&["publish", "init"], Some(request)).await
    }

    /// `POST /api/v1/publish/commit`. A single attempt; the caller retries.
    pub async fn publish_commit(&self, upload_id: &str) -> Result<PublishCommitResponse> {
        let body = serde_json::to_value(PublishCommitRequest {
            upload_id: upload_id.to_owned(),
        })?;
        self.send(
            Method::POST,
            &["publish", "commit"],
            Some(body),
            RetryPolicy::none(),
            Some(COMMIT_TIMEOUT),
        )
        .await
    }

    /// `GET /api/v1/packages`.
    pub async fn packages(&self) -> Result<PackageList> {
        self.get(&["packages"]).await
    }

    /// `GET /api/v1/packages/<eco>/<name>`.
    pub async fn package(&self, eco: Ecosystem, name: &str) -> Result<PackageEntry> {
        self.get(&["packages", eco.as_str(), name]).await
    }

    /// `POST /api/v1/packages/<eco>/<name>/<version>/yank`.
    pub async fn yank(&self, eco: Ecosystem, name: &str, version: &str) -> Result<YankResponse> {
        self.post::<_, ()>(&["packages", eco.as_str(), name, version, "yank"], None)
            .await
    }

    /// `DELETE /api/v1/packages/<eco>/<name>/<version>`.
    pub async fn delete_version(
        &self,
        eco: Ecosystem,
        name: &str,
        version: &str,
    ) -> Result<DeleteVersionResponse> {
        self.delete::<_, ()>(&["packages", eco.as_str(), name, version], None)
            .await
    }

    /// `DELETE /api/v1/packages/<eco>/<name>`.
    pub async fn delete_package(
        &self,
        eco: Ecosystem,
        name: &str,
    ) -> Result<DeletePackageResponse> {
        self.delete::<_, ()>(&["packages", eco.as_str(), name], None)
            .await
    }

    /// `POST /api/v1/packages/<eco>/<name>/reindex`.
    pub async fn reindex(&self, eco: Ecosystem, name: &str) -> Result<ReindexResponse> {
        self.post::<_, ()>(&["packages", eco.as_str(), name, "reindex"], None)
            .await
    }

    /// `GET /api/v1/credentials`.
    pub async fn credentials(&self) -> Result<CredentialList> {
        self.get(&["credentials"]).await
    }

    /// `POST /api/v1/credentials`.
    pub async fn create_credential(
        &self,
        request: &CreateCredentialRequest,
    ) -> Result<CreateCredentialResponse> {
        self.post(&["credentials"], Some(request)).await
    }

    /// `GET /api/v1/credentials/<project>`.
    pub async fn credential(&self, project: &str) -> Result<Credential> {
        self.get(&["credentials", project]).await
    }

    /// `PATCH /api/v1/credentials/<project>`.
    pub async fn update_grants(&self, project: &str, grants: &[Grant]) -> Result<Credential> {
        let body = UpdateGrantsRequest {
            grants: grants.to_vec(),
        };
        self.patch(&["credentials", project], &body).await
    }

    /// `DELETE /api/v1/credentials/<project>`.
    pub async fn delete_credential(&self, project: &str) -> Result<DeleteCredentialResponse> {
        self.delete::<_, ()>(&["credentials", project], None).await
    }

    /// `POST /api/v1/credentials/<project>/secrets`.
    pub async fn add_secret(
        &self,
        project: &str,
        request: &AddSecretRequest,
    ) -> Result<AddSecretResponse> {
        self.post(&["credentials", project, "secrets"], Some(request))
            .await
    }

    /// `DELETE /api/v1/credentials/<project>/secrets/<hash-prefix>`.
    ///
    /// `expires` defaults to now on the registry side.
    pub async fn expire_secret(
        &self,
        project: &str,
        id: &str,
        expires: Option<String>,
    ) -> Result<ExpireSecretResponse> {
        let body = expires.map(|expires| ExpireSecretRequest {
            expires: Some(expires),
        });
        self.delete(&["credentials", project, "secrets", id], body.as_ref())
            .await
    }

    async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<serde_json::Value>,
        retry: RetryPolicy,
        timeout: Option<Duration>,
    ) -> Result<T> {
        let url = self.endpoint(segments)?;
        self.send_to(method, url, body, retry, timeout).await
    }

    async fn send_to<T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        body: Option<serde_json::Value>,
        retry: RetryPolicy,
        timeout: Option<Duration>,
    ) -> Result<T> {
        let attempts = retry.attempts.max(1);
        let mut attempt = 1;
        loop {
            let (result, retry_after) = self
                .send_once(method.clone(), &url, body.as_ref(), timeout)
                .await;
            let err = match result {
                Ok(value) => return Ok(value),
                Err(err) => err,
            };
            let wait = match retry_after {
                Some(wait) => wait,
                None if is_retryable(&err) => retry.delay(attempt),
                None => return Err(err),
            };
            if attempt >= attempts {
                return Err(err);
            }
            tracing::debug!(
                method = %method,
                path = url.path(),
                attempt,
                delay_ms = wait.as_millis() as u64,
                "retrying registry request"
            );
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    }

    /// Sends one request.
    ///
    /// Returns the decoded body or the mapped error, and for `429` the time to wait
    /// before retrying.
    async fn send_once<T: DeserializeOwned>(
        &self,
        method: Method,
        url: &Url,
        body: Option<&serde_json::Value>,
        timeout: Option<Duration>,
    ) -> (Result<T>, Option<Duration>) {
        let mut request = self.http.request(method.clone(), url.clone());
        if let Some(token) = &self.token {
            let Ok(mut value) = HeaderValue::from_str(&format!("Bearer {token}")) else {
                let err = Error::InvalidInput("the token contains invalid characters".into());
                return (Err(err), None);
            };
            value.set_sensitive(true);
            request = request.header(AUTHORIZATION, value);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(err) => {
                tracing::debug!(method = %method, path = url.path(), error = %err, "request failed");
                return (Err(Error::Http(err)), None);
            }
        };
        let status = response.status();
        let headers = response.headers().clone();
        tracing::debug!(
            method = %method,
            path = url.path(),
            status = status.as_u16(),
            "registry response"
        );

        let min = header_str(&headers, MIN_CLIENT_VERSION_HEADER);
        if let Some(min) = &min {
            self.record_min_client_version(min);
        }

        let bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(err) => return (Err(Error::Http(err)), None),
        };
        if status.is_success() {
            return (serde_json::from_slice(&bytes).map_err(Error::from), None);
        }

        let retry_after = (status == StatusCode::TOO_MANY_REQUESTS).then(|| retry_after(&headers));
        (
            Err(error_from_response(status, min.as_deref(), &bytes)),
            retry_after,
        )
    }

    fn record_min_client_version(&self, min: &str) {
        if let Ok(mut guard) = self.min_client_version.lock() {
            if guard.as_deref() != Some(min) {
                tracing::trace!(min, "registry reported minimum client version");
                if let (Ok(own), Ok(min_version)) = (
                    semver::Version::parse(&self.client_version),
                    semver::Version::parse(min),
                ) {
                    if own < min_version {
                        tracing::warn!(
                            min,
                            own = %self.client_version,
                            "this client is older than the registry's minimum version"
                        );
                    }
                }
                *guard = Some(min.to_owned());
            }
        }
    }
}

/// Maps a non-2xx response to an [`Error`].
pub fn error_from_response(
    status: StatusCode,
    min_client_version: Option<&str>,
    body: &[u8],
) -> Error {
    if status == StatusCode::UPGRADE_REQUIRED {
        return Error::ClientTooOld {
            min: min_client_version.unwrap_or("unknown").to_owned(),
        };
    }

    let (code, message) = match serde_json::from_slice::<ErrorBody>(body) {
        Ok(body) => (body.error, body.message),
        Err(_) => (
            "unknown".to_owned(),
            status
                .canonical_reason()
                .unwrap_or("unexpected response")
                .to_owned(),
        ),
    };

    if status == StatusCode::CONFLICT && code == "version_exists" {
        return Error::VersionExists;
    }
    if status == StatusCode::CONFLICT && code == "file_exists" {
        return Error::FileExists { message };
    }
    if status == StatusCode::CONFLICT && code == "ambiguous" {
        let body = serde_json::from_slice::<GenericAmbiguousBody>(body).ok();
        let (version, files) = body
            .map(|body| (body.version, body.files))
            .unwrap_or_default();
        return Error::Ambiguous {
            message,
            version,
            files,
        };
    }

    Error::Api {
        status: status.as_u16(),
        code,
        message,
    }
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn retry_after(headers: &HeaderMap) -> Duration {
    header_str(headers, RETRY_AFTER.as_str())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_RETRY_AFTER)
        .min(MAX_RETRY_AFTER)
}
