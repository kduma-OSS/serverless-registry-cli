//! `sreg auth` with a static token and with GitHub Actions OIDC.

mod common;

use common::{sreg, sreg_with, stdout_json, TOKEN};
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn host(server: &MockServer) -> String {
    server.uri().trim_start_matches("http://").to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn prints_composer_auth_export() {
    let server = MockServer::start().await;

    sreg(&server, &["auth", "composer"])
        .await
        .success()
        .stdout(format!(
            "export COMPOSER_AUTH='{{\"bearer\":{{\"{}\":\"{TOKEN}\"}}}}'\n",
            host(&server)
        ));

    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn json_output_names_the_variable() {
    let server = MockServer::start().await;

    let assert = sreg(
        &server,
        &["auth", "cargo", "--registry-name", "acme", "--json"],
    )
    .await
    .success();

    assert_eq!(
        stdout_json(&assert),
        json!({ "eco": "cargo", "name": "CARGO_REGISTRIES_ACME_TOKEN", "value": TOKEN })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_into_an_existing_npmrc() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let npmrc = dir.path().join(".npmrc");
    std::fs::write(&npmrc, "@acme:registry=https://example.com/npm/\n").unwrap();

    sreg(
        &server,
        &["auth", "npm", "--write", npmrc.to_str().unwrap()],
    )
    .await
    .success()
    .stdout(predicate::str::contains("Wrote the token of `root`"))
    .stdout(predicate::str::contains(TOKEN).not());

    assert_eq!(
        std::fs::read_to_string(&npmrc).unwrap(),
        format!(
            "@acme:registry=https://example.com/npm/\n//{}/npm/:_authToken={TOKEN}\n",
            host(&server)
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn creates_credentials_toml_readable_by_owner_only() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cargo").join("credentials.toml");

    sreg(
        &server,
        &["auth", "cargo", "--write", file.to_str().unwrap()],
    )
    .await
    .success();

    let content: toml::Table = std::fs::read_to_string(&file).unwrap().parse().unwrap();
    assert_eq!(content["registries"]["sreg"]["token"].as_str(), Some(TOKEN));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn oidc_builds_a_gh_token_for_the_project() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/oidc"))
        .and(query_param("audience", server.uri().as_str()))
        .and(header("authorization", "bearer request-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": "eyJ.oidc.sig" })))
        .expect(1)
        .mount(&server)
        .await;
    let request_url = format!("{}/oidc?api-version=2.0", server.uri());

    let assert = sreg_with(
        &server,
        &["auth", "npm", "--oidc", "--project", "ci-acme", "--json"],
        &[
            ("ACTIONS_ID_TOKEN_REQUEST_URL", request_url.as_str()),
            ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "request-token"),
        ],
    )
    .await
    .success();

    assert_eq!(stdout_json(&assert)["value"], "ci-acme:gh.eyJ.oidc.sig");
}

#[tokio::test(flavor = "multi_thread")]
async fn oidc_outside_github_actions_fails() {
    let server = MockServer::start().await;

    sreg(
        &server,
        &["auth", "composer", "--oidc", "--project", "ci-acme"],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("ACTIONS_ID_TOKEN_REQUEST_URL"));
}
