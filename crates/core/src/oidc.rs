//! GitHub Actions OIDC tokens.
//!
//! Inside a job with `permissions: id-token: write`, GitHub exposes
//! `ACTIONS_ID_TOKEN_REQUEST_URL` and `ACTIONS_ID_TOKEN_REQUEST_TOKEN`. The JWT obtained
//! from them, with the registry's `PUBLIC_URL` as audience, becomes the registry token
//! `<project>:gh.<jwt>`.

use std::time::Duration;

use reqwest::header::{ACCEPT, AUTHORIZATION};
use serde::Deserialize;
use url::Url;

use crate::error::{Error, Result};

/// Environment variable holding the token request URL.
pub const REQUEST_URL_ENV: &str = "ACTIONS_ID_TOKEN_REQUEST_URL";

/// Environment variable holding the bearer token for the token request.
pub const REQUEST_TOKEN_ENV: &str = "ACTIONS_ID_TOKEN_REQUEST_TOKEN";

const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct TokenResponse {
    value: String,
}

/// Requests a GitHub Actions OIDC token (JWT) for `audience`.
///
/// Fails with [`Error::Oidc`] outside a GitHub Actions job or without the
/// `id-token: write` permission.
pub async fn github_oidc_token(audience: &str) -> Result<String> {
    let missing = |name: &str| {
        Error::Oidc(format!(
            "{name} is not set; run inside a GitHub Actions job with `permissions: id-token: write`"
        ))
    };
    let url = non_empty_env(REQUEST_URL_ENV).ok_or_else(|| missing(REQUEST_URL_ENV))?;
    let token = non_empty_env(REQUEST_TOKEN_ENV).ok_or_else(|| missing(REQUEST_TOKEN_ENV))?;
    request_oidc_token(&url, &token, audience).await
}

/// Requests an OIDC token from `request_url` (`GET <url>&audience=<aud>`) with
/// `Authorization: bearer <request_token>` and returns its `value`.
pub async fn request_oidc_token(
    request_url: &str,
    request_token: &str,
    audience: &str,
) -> Result<String> {
    let mut url = Url::parse(request_url)
        .map_err(|_| Error::Oidc(format!("{REQUEST_URL_ENV} is not a valid URL")))?;
    url.query_pairs_mut().append_pair("audience", audience);

    let http = reqwest::Client::builder()
        .user_agent(concat!("sreg/", env!("CARGO_PKG_VERSION")))
        .timeout(TIMEOUT)
        .build()?;
    tracing::debug!(audience, "requesting GitHub Actions OIDC token");
    let response = http
        .get(url)
        .header(AUTHORIZATION, format!("bearer {request_token}"))
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(|err| Error::Http(err.without_url()))?;

    let status = response.status();
    if !status.is_success() {
        return Err(Error::Oidc(format!(
            "the token request failed with status {}; does the job have `permissions: id-token: write`?",
            status.as_u16()
        )));
    }
    let body = response
        .bytes()
        .await
        .map_err(|err| Error::Http(err.without_url()))?;
    let token: TokenResponse = serde_json::from_slice(&body)
        .map_err(|_| Error::Oidc("the token response is not valid JSON".into()))?;
    if token.value.is_empty() {
        return Err(Error::Oidc("the token response is empty".into()));
    }
    Ok(token.value)
}

/// Builds the registry token `<project>:gh.<jwt>`.
pub fn compose_token(project: &str, jwt: &str) -> String {
    format!("{project}:gh.{jwt}")
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::test_env::EnvGuard;

    const AUDIENCE: &str = "https://packages.example.com";

    async fn token_server(status: u16, body: serde_json::Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/token"))
            .and(query_param("api-version", "2.0"))
            .and(query_param("audience", AUDIENCE))
            .and(header("authorization", "bearer request-token"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&server)
            .await;
        server
    }

    #[test]
    fn composes_gh_token() {
        assert_eq!(
            compose_token("ci-acme-sdk", "eyJ.eyJ.sig"),
            "ci-acme-sdk:gh.eyJ.eyJ.sig"
        );
    }

    #[tokio::test]
    async fn requests_token_with_audience_appended_to_the_query() {
        let server = token_server(200, json!({ "count": 1, "value": "eyJ.eyJ.sig" })).await;
        let url = format!("{}/token?api-version=2.0", server.uri());

        let jwt = request_oidc_token(&url, "request-token", AUDIENCE)
            .await
            .unwrap();

        assert_eq!(jwt, "eyJ.eyJ.sig");
    }

    #[tokio::test]
    async fn refused_request_is_an_oidc_error() {
        let server = token_server(403, json!({ "message": "no" })).await;
        let url = format!("{}/token?api-version=2.0", server.uri());

        let err = request_oidc_token(&url, "request-token", AUDIENCE)
            .await
            .unwrap_err();

        assert!(matches!(err, Error::Oidc(ref message) if message.contains("403")));
    }

    #[test]
    fn reads_request_url_and_token_from_the_environment() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let server = runtime.block_on(token_server(200, json!({ "value": "eyJ.env.sig" })));
        let url = format!("{}/token?api-version=2.0", server.uri());
        let _env = EnvGuard::set(&[
            (REQUEST_URL_ENV, Some(url.as_str())),
            (REQUEST_TOKEN_ENV, Some("request-token")),
        ]);

        let jwt = runtime.block_on(github_oidc_token(AUDIENCE)).unwrap();

        assert_eq!(jwt, "eyJ.env.sig");
    }

    #[test]
    fn missing_environment_is_an_oidc_error() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _env = EnvGuard::set(&[(REQUEST_URL_ENV, None), (REQUEST_TOKEN_ENV, None)]);

        let err = runtime.block_on(github_oidc_token(AUDIENCE)).unwrap_err();

        assert!(
            matches!(err, Error::Oidc(ref message) if message.contains(REQUEST_URL_ENV)),
            "{err:?}"
        );
    }
}
