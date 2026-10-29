//! `sreg yank` and `sreg delete` against a mock registry.

mod common;

use common::{error, json, sreg, sreg_with, stdout_json};
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer};

#[tokio::test(flavor = "multi_thread")]
async fn yank_posts_to_the_encoded_version_path() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/packages/npm/@acme%2Fsdk/2.3.1/yank"))
        .respond_with(json(
            200,
            json!({
                "eco": "npm", "name": "@acme/sdk", "version": "2.3.1",
                "published": "2026-10-28T10:12:00Z", "yanked": true, "files": []
            }),
        ))
        .expect(1)
        .mount(&server)
        .await;

    sreg(&server, &["yank", "npm", "@acme/sdk", "2.3.1"])
        .await
        .success()
        .stdout(predicate::str::contains("Yanked npm @acme/sdk 2.3.1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn yank_json_prints_the_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/packages/composer/acme%2Fsdk/1.2.0/yank"))
        .respond_with(json(
            200,
            json!({
                "eco": "composer", "name": "acme/sdk", "version": "1.2.0",
                "published": "2026-10-28T10:12:00Z", "yanked": true, "files": []
            }),
        ))
        .mount(&server)
        .await;

    let assert = sreg(
        &server,
        &["yank", "composer", "acme/sdk", "1.2.0", "--json"],
    )
    .await
    .success();

    let body = stdout_json(&assert);
    assert_eq!(body["yanked"], true);
    assert_eq!(body["version"], "1.2.0");
}

#[tokio::test(flavor = "multi_thread")]
async fn yank_of_unknown_version_fails() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(error(404, "not_found", "version not found"))
        .mount(&server)
        .await;

    sreg(&server, &["yank", "cargo", "acme-core", "9.9.9"])
        .await
        .code(1)
        .stderr(predicate::str::contains("version not found"));
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_requires_confirmation_without_a_terminal() {
    let server = MockServer::start().await;

    for args in [
        &["delete", "cargo", "acme-core", "1.4.2"][..],
        &["delete", "cargo", "acme-core", "--all"][..],
    ] {
        sreg(&server, args)
            .await
            .code(1)
            .stderr(predicate::str::contains("--yes"));
    }

    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_version_with_yes() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/packages/cargo/acme-core/1.4.2"))
        .respond_with(json(
            200,
            json!({ "eco": "cargo", "name": "acme-core", "version": "1.4.2", "deleted": true }),
        ))
        .expect(1)
        .mount(&server)
        .await;

    sreg(&server, &["delete", "cargo", "acme-core", "1.4.2", "--yes"])
        .await
        .success()
        .stdout(predicate::str::contains("Deleted cargo acme-core 1.4.2"));
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_all_removes_the_whole_package() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/packages/composer/acme%2Fsdk"))
        .respond_with(json(
            200,
            json!({ "eco": "composer", "name": "acme/sdk", "deletedVersions": ["1.2.0", "1.1.0"] }),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let assert = sreg(
        &server,
        &["delete", "composer", "acme/sdk", "--all", "--yes", "--json"],
    )
    .await
    .success();

    assert_eq!(
        stdout_json(&assert)["deletedVersions"],
        json!(["1.2.0", "1.1.0"])
    );
}

fn yanked(version: &str) -> wiremock::ResponseTemplate {
    json(
        200,
        json!({
            "eco": "npm", "name": "@acme/sdk", "version": version,
            "published": "2026-10-28T10:12:00Z", "yanked": true, "files": []
        }),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn yank_authenticates_with_a_github_token_for_a_named_project() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/oidc"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(json!({ "value": "eyJ.oidc.sig" })),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/packages/npm/@acme%2Fsdk/2.3.1/yank"))
        .and(header("authorization", "Bearer ci-admin:gh.eyJ.oidc.sig"))
        .respond_with(yanked("2.3.1"))
        .expect(1)
        .mount(&server)
        .await;
    let request_url = format!("{}/oidc?api-version=2.0", server.uri());

    sreg_with(
        &server,
        &[
            "yank",
            "npm",
            "@acme/sdk",
            "2.3.1",
            "--oidc",
            "--oidc-project",
            "ci-admin",
        ],
        &[
            ("ACTIONS_ID_TOKEN_REQUEST_URL", request_url.as_str()),
            ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "request-token"),
        ],
    )
    .await
    .success();
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_with_oidc_lets_the_registry_resolve_the_project() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/oidc"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(json!({ "value": "eyJ.oidc.sig" })),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/packages/cargo/acme-core/1.4.2"))
        .and(header("authorization", "Bearer _:gh.eyJ.oidc.sig"))
        .respond_with(json(
            200,
            json!({ "eco": "cargo", "name": "acme-core", "version": "1.4.2", "deleted": true }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let request_url = format!("{}/oidc?api-version=2.0", server.uri());

    sreg_with(
        &server,
        &["delete", "cargo", "acme-core", "1.4.2", "--yes", "--oidc"],
        &[
            ("ACTIONS_ID_TOKEN_REQUEST_URL", request_url.as_str()),
            ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "request-token"),
        ],
    )
    .await
    .success();
}
