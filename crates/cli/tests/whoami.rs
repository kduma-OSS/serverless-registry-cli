//! `sreg whoami` and `sreg version` against a mock registry.

mod common;

use common::{error, json, sreg, stdout_json, TOKEN};
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer};

async fn whoami_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(header("x-client-version", env!("CARGO_PKG_VERSION")))
        .respond_with(json(
            200,
            json!({
                "project": "shop-frontend",
                "kind": "read",
                "grants": [
                    { "eco": "npm", "package": "@acme/sdk", "versions": "^2.0" },
                    { "eco": "composer", "package": "acme/*", "versions": "*" }
                ]
            }),
        ))
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_prints_project_kind_and_grants() {
    let server = whoami_server().await;

    sreg(&server, &["whoami"])
        .await
        .success()
        .stdout(predicate::str::contains("Project: shop-frontend"))
        .stdout(predicate::str::contains("Kind:    read"))
        .stdout(predicate::str::contains("npm       @acme/sdk  ^2.0"))
        .stdout(predicate::str::contains("k7Qx2mN9").not());
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_json_is_the_only_stdout() {
    let server = whoami_server().await;

    let assert = sreg(&server, &["whoami", "--json"]).await.success();

    let body = stdout_json(&assert);
    assert_eq!(body["project"], "shop-frontend");
    assert_eq!(body["grants"][1]["package"], "acme/*");
}

#[tokio::test(flavor = "multi_thread")]
async fn api_errors_exit_with_one_and_report_the_message() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .respond_with(error(401, "unauthorized", "invalid credential"))
        .mount(&server)
        .await;

    sreg(&server, &["whoami", "--json"])
        .await
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("invalid credential"));
}

#[tokio::test(flavor = "multi_thread")]
async fn client_too_old_exits_with_three_and_links_the_releases() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .respond_with(
            error(426, "client_too_old", "upgrade").insert_header("x-min-client-version", "9.1.0"),
        )
        .mount(&server)
        .await;

    sreg(&server, &["whoami"])
        .await
        .code(3)
        .stderr(predicate::str::contains("9.1.0"))
        .stderr(predicate::str::contains(
            "https://github.com/kduma-OSS/serverless-registry-cli/releases",
        ));
}

#[tokio::test(flavor = "multi_thread")]
async fn version_reports_the_registry_without_a_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/version"))
        .respond_with(json(
            200,
            json!({ "version": "0.4.0", "minClientVersion": "0.0.1", "schemaVersion": 1 }),
        ))
        .mount(&server)
        .await;

    let assert = sreg(&server, &["version", "--json"]).await.success();

    let body = stdout_json(&assert);
    assert_eq!(body["clientVersion"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["registry"]["version"], "0.4.0");
    assert_eq!(body["compatible"], true);
    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].headers.get("authorization").is_none());
}
