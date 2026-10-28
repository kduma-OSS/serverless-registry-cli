//! `sreg yank` and `sreg delete` against a mock registry.

mod common;

use common::{error, json, sreg, stdout_json};
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{method, path};
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
                "published": "2026-10-28T10:12:00Z", "yanked": true, "size": 48213, "sha256": "9f2c"
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
                "published": "2026-10-28T10:12:00Z", "yanked": true, "size": 1, "sha256": "ab"
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
