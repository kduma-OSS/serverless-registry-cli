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
                    { "eco": "npm", "package": "@acme/sdk", "versions": "^2.0", "transitive": true },
                    { "eco": "composer", "package": "acme/*", "versions": "*", "transitive": false }
                ]
            }),
        ))
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_prints_project_roles_and_grants() {
    let server = whoami_server().await;

    sreg(&server, &["whoami"])
        .await
        .success()
        .stdout(predicate::str::contains("Project: shop-frontend"))
        .stdout(predicate::str::contains("Roles:   read"))
        .stdout(predicate::str::contains(
            "npm       @acme/sdk  ^2.0      yes",
        ))
        .stdout(predicate::str::contains(
            "composer  acme/*     *         no",
        ))
        .stdout(predicate::str::contains("k7Qx2mN9").not());
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_json_is_the_only_stdout() {
    let server = whoami_server().await;

    let assert = sreg(&server, &["whoami", "--json"]).await.success();

    let body = stdout_json(&assert);
    assert_eq!(body["project"], "shop-frontend");
    assert_eq!(body["grants"][1]["package"], "acme/*");
    assert_eq!(body["grants"][1]["transitive"], false);
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

#[tokio::test(flavor = "multi_thread")]
async fn whoami_lists_the_grants_of_each_role_of_a_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .respond_with(json(
            200,
            json!({
                "project": "acme-sdk",
                "kind": "read",
                "grants": [
                    { "eco": "composer", "package": "acme/*", "versions": "*", "transitive": true }
                ],
                "roles": ["read", "publish"],
                "access": {
                    "read": [
                        { "eco": "composer", "package": "acme/*", "versions": "*", "transitive": true }
                    ],
                    "publish": [
                        { "eco": "composer", "package": "acme/acme-sdk", "versions": "*", "transitive": true }
                    ]
                }
            }),
        ))
        .mount(&server)
        .await;

    let assert = sreg(&server, &["whoami"]).await.success();
    assert
        .stdout(predicate::str::contains("Roles:   read, publish"))
        .stdout(predicate::str::contains("Read grants:"))
        .stdout(predicate::str::contains("composer  acme/*   *         yes"))
        .stdout(predicate::str::contains("Publish grants:"))
        // `transitive` means nothing for publishing, so the publish list has no column.
        .stdout(predicate::str::contains("composer  acme/acme-sdk  *\n"));

    let json_out = sreg(&server, &["whoami", "--json"]).await.success();
    let body = stdout_json(&json_out);
    assert_eq!(body["roles"], json!(["read", "publish"]));
    assert_eq!(body["access"]["publish"][0]["package"], "acme/acme-sdk");
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_shows_a_token_that_may_only_publish() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .respond_with(json(
            200,
            json!({
                "project": "acme-sdk",
                "kind": "publish",
                "grants": [{ "eco": "npm", "package": "@acme/sdk", "versions": "*", "transitive": true }],
                "roles": ["publish"],
                "access": {
                    "publish": [{ "eco": "npm", "package": "@acme/sdk", "versions": "*", "transitive": true }]
                }
            }),
        ))
        .mount(&server)
        .await;

    sreg(&server, &["whoami"])
        .await
        .success()
        .stdout(predicate::str::contains("Roles:   publish"))
        .stdout(predicate::str::contains("Grants:"))
        .stdout(predicate::str::contains("Read grants").not())
        .stdout(predicate::str::contains("TRANSITIVE").not());
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_for_admin_has_no_grants() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .respond_with(json(
            200,
            json!({
                "project": "root", "kind": "admin", "grants": [],
                "roles": ["admin"], "access": {}
            }),
        ))
        .mount(&server)
        .await;

    sreg(&server, &["whoami"])
        .await
        .success()
        .stdout(predicate::str::contains("Roles:   admin"))
        .stdout(predicate::str::contains(
            "none (admin credentials have no grants)",
        ));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_for_an_admin_that_also_reads_shows_both_roles_and_its_grants() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .respond_with(json(
            200,
            json!({
                "project": "ops", "kind": "admin", "grants": [],
                "roles": ["read", "admin"],
                "access": { "read": [
                    { "eco": "npm", "package": "@acme/sdk", "versions": "*", "transitive": true }
                ] }
            }),
        ))
        .mount(&server)
        .await;

    sreg(&server, &["whoami"])
        .await
        .success()
        .stdout(predicate::str::contains("Roles:   read, admin"))
        .stdout(predicate::str::contains("@acme/sdk"))
        .stdout(predicate::str::contains("admin credentials have no grants").not());
}
