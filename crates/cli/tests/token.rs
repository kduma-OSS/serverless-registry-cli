//! `sreg token …` against a mock registry.

mod common;

use common::{error, json, sreg, stdout_json};
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, Request};

const NEW_TOKEN: &str = "shop-frontend:Q2w9Er7Ty5Ui3Op1As8Df6Gh4Jk2Lz0X";

fn credential(auth: serde_json::Value) -> serde_json::Value {
    json!({
        "project": "shop-frontend",
        "kind": "read",
        "auth": auth,
        "grants": [{ "eco": "composer", "package": "acme/*", "versions": "*" }]
    })
}

async fn create_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/credentials"))
        .and(body_json(json!({
            "project": "shop-frontend",
            "kind": "read",
            "grants": [
                { "eco": "composer", "package": "acme/*", "versions": "*" },
                { "eco": "cargo", "package": "acme-core", "versions": ">=1.4, <2" }
            ]
        })))
        .respond_with(json(
            201,
            json!({
                "credential": credential(json!([
                    { "id": "3f9a1c0b7d2e", "type": "key", "created": "2026-10-28T09:00:00Z", "expires": null }
                ])),
                "token": NEW_TOKEN
            }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    server
}

const CREATE_ARGS: &[&str] = &[
    "token",
    "create",
    "--project",
    "shop-frontend",
    "--kind",
    "read",
    "--grant",
    "composer:acme/*",
    "--grant",
    "cargo:acme-core:>=1.4, <2",
];

#[tokio::test(flavor = "multi_thread")]
async fn token_create_prints_the_token_once() {
    let server = create_server().await;

    let assert = sreg(&server, CREATE_ARGS).await.success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("Created read credential `shop-frontend`."));
    assert!(stdout.contains("store it now"));
    assert_eq!(stdout.matches(NEW_TOKEN).count(), 1);
    assert!(stdout.lines().any(|line| line == NEW_TOKEN));
}

#[tokio::test(flavor = "multi_thread")]
async fn token_create_json_prints_the_full_response() {
    let server = create_server().await;
    let mut args = CREATE_ARGS.to_vec();
    args.push("--json");

    let assert = sreg(&server, &args).await.success();

    let body = stdout_json(&assert);
    assert_eq!(body["token"], NEW_TOKEN);
    assert_eq!(body["credential"]["auth"][0]["id"], "3f9a1c0b7d2e");
}

#[tokio::test(flavor = "multi_thread")]
async fn token_create_sends_the_first_secret_expiry() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/credentials"))
        .and(body_json(json!({
            "project": "ci-acme",
            "kind": "publish",
            "grants": [{ "eco": "npm", "package": "@acme/*", "versions": "*" }],
            "secret": { "type": "key", "expires": "2027-01-31" }
        })))
        .respond_with(json(
            201,
            json!({
                "credential": {
                    "project": "ci-acme", "kind": "publish",
                    "auth": [{ "id": "3f9a1c0b7d2e", "type": "key", "created": "2026-10-28T09:00:00Z",
                               "expires": "2027-01-31T00:00:00Z" }],
                    "grants": [{ "eco": "npm", "package": "@acme/*", "versions": "*" }]
                },
                "token": "ci-acme:secret"
            }),
        ))
        .expect(1)
        .mount(&server)
        .await;

    sreg(
        &server,
        &[
            "token",
            "create",
            "--project",
            "ci-acme",
            "--kind",
            "publish",
            "--grant",
            "npm:@acme/*",
            "--expires",
            "2027-01-31",
        ],
    )
    .await
    .success()
    .stdout(predicate::str::contains("expires at 2027-01-31T00:00:00Z"));
}

#[tokio::test(flavor = "multi_thread")]
async fn token_create_rejects_invalid_grants_before_calling_the_registry() {
    let server = MockServer::start().await;

    sreg(
        &server,
        &[
            "token",
            "create",
            "--project",
            "p1",
            "--kind",
            "read",
            "--grant",
            "pypi:x",
        ],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("invalid grant"));

    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn token_create_conflict_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/credentials"))
        .respond_with(error(409, "credential_exists", "project already exists"))
        .mount(&server)
        .await;

    sreg(
        &server,
        &["token", "create", "--project", "p1", "--kind", "admin"],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("project already exists"));
}

#[tokio::test(flavor = "multi_thread")]
async fn token_rotate_issues_a_key_and_expires_the_old_ones() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/credentials/shop-frontend"))
        .respond_with(json(
            200,
            credential(json!([
                { "id": "3f9a1c0b7d2e", "type": "key", "created": "2026-06-01T00:00:00Z", "expires": null },
                { "id": "a41d07e9c2b5", "type": "key", "created": "2026-06-01T00:00:00Z", "expires": "2020-01-01T00:00:00Z" },
                { "id": "b52e18fad3c6", "type": "key", "created": "2026-06-01T00:00:00Z", "expires": "2099-01-01T00:00:00Z" },
                { "id": "c08e5b1f9a27", "type": "gh", "repository": "acme/shop", "ref": null, "workflow": null,
                  "created": "2026-06-01T00:00:00Z", "expires": null }
            ])),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/credentials/shop-frontend/secrets"))
        .and(body_json(json!({ "type": "key" })))
        .respond_with(json(
            201,
            json!({
                "secret": { "id": "7c21de4490ab", "type": "key", "created": "2026-10-28T09:10:00Z", "expires": null },
                "token": NEW_TOKEN
            }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    for id in ["3f9a1c0b7d2e", "b52e18fad3c6"] {
        Mock::given(method("DELETE"))
            .and(path(format!(
                "/api/v1/credentials/shop-frontend/secrets/{id}"
            )))
            .respond_with(move |request: &Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                json(
                    200,
                    json!({ "secret": { "id": id, "type": "key", "created": "2026-06-01T00:00:00Z",
                                        "expires": body["expires"] } }),
                )
            })
            .expect(1)
            .mount(&server)
            .await;
    }

    let assert = sreg(
        &server,
        &[
            "token",
            "rotate",
            "--project",
            "shop-frontend",
            "--expire-old",
            "7d",
            "--json",
        ],
    )
    .await
    .success();

    let body = stdout_json(&assert);
    assert_eq!(body["token"], NEW_TOKEN);
    assert_eq!(body["oldSecrets"].as_array().unwrap().len(), 2);
    assert_eq!(body["oldSecrets"][0]["status"], "expiring");

    let requests = server.received_requests().await.unwrap();
    let deletes: Vec<_> = requests
        .iter()
        .filter(|request| request.method.as_str() == "DELETE")
        .collect();
    assert_eq!(deletes.len(), 2);
    let expires: serde_json::Value = serde_json::from_slice(&deletes[0].body).unwrap();
    let expires = expires["expires"].as_str().unwrap();
    let parsed =
        time::OffsetDateTime::parse(expires, &time::format_description::well_known::Rfc3339)
            .unwrap();
    let days = (parsed - time::OffsetDateTime::now_utc()).whole_hours();
    assert!((7 * 24 - 1..=7 * 24).contains(&days), "{expires}");
    assert!(
        expires.ends_with('Z') && !expires.contains('.'),
        "{expires}"
    );
}

async fn rotate_server(old_expires: Option<String>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/credentials/shop-frontend"))
        .respond_with(json(
            200,
            credential(json!([
                { "id": "3f9a1c0b7d2e", "type": "key", "created": "2026-06-01T00:00:00Z",
                  "expires": old_expires }
            ])),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/credentials/shop-frontend/secrets"))
        .respond_with(json(
            201,
            json!({
                "secret": { "id": "7c21de4490ab", "type": "key", "created": "2026-10-28T09:10:00Z", "expires": null },
                "token": NEW_TOKEN
            }),
        ))
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread")]
async fn token_rotate_does_not_extend_earlier_expiries() {
    let soon = time::OffsetDateTime::now_utc() + time::Duration::days(2);
    let soon = soon
        .replace_nanosecond(0)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let server = rotate_server(Some(soon.clone())).await;

    sreg(&server, &["token", "rotate", "--project", "shop-frontend"])
        .await
        .success()
        .stdout(predicate::str::contains(NEW_TOKEN))
        .stdout(predicate::str::contains(format!(
            "3f9a1c0b7d2e  unchanged, already expires {soon}"
        )));

    let requests = server.received_requests().await.unwrap();
    assert!(requests
        .iter()
        .all(|request| request.method.as_str() != "DELETE"));
}

#[tokio::test(flavor = "multi_thread")]
async fn token_rotate_prints_the_new_token_even_when_expiring_fails() {
    let server = rotate_server(None).await;
    Mock::given(method("DELETE"))
        .respond_with(error(500, "internal_error", "kv unavailable"))
        .mount(&server)
        .await;

    sreg(&server, &["token", "rotate", "--project", "shop-frontend"])
        .await
        .code(1)
        .stdout(predicate::str::contains(NEW_TOKEN))
        .stdout(predicate::str::contains("FAILED to set expiry"))
        .stderr(predicate::str::contains("the new token was issued"));
}

#[tokio::test(flavor = "multi_thread")]
async fn token_rotate_rejects_invalid_durations_before_calling_the_registry() {
    let server = MockServer::start().await;

    sreg(
        &server,
        &[
            "token",
            "rotate",
            "--project",
            "shop-frontend",
            "--expire-old",
            "14",
        ],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("invalid duration"));

    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn token_list_prints_a_table() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/credentials"))
        .respond_with(json(
            200,
            json!({ "credentials": [
                credential(json!([
                    { "id": "3f9a1c0b7d2e", "type": "key", "created": "2026-06-01T00:00:00Z", "expires": null },
                    { "id": "a41d07e9c2b5", "type": "key", "created": "2026-06-01T00:00:00Z", "expires": "2099-11-11T00:00:00Z" },
                    { "id": "b52e18fad3c6", "type": "key", "created": "2026-06-01T00:00:00Z", "expires": "2020-01-01T00:00:00Z" }
                ])),
                { "project": "root", "kind": "admin", "auth": [
                    { "id": "d19f2a3b4c5d", "type": "key", "created": "2026-06-01T00:00:00Z", "expires": null }
                ], "grants": [] }
            ] }),
        ))
        .mount(&server)
        .await;

    sreg(&server, &["token", "list"])
        .await
        .success()
        .stdout(predicate::str::contains(
            "PROJECT        KIND   ACTIVE SECRETS  NEAREST EXPIRY",
        ))
        .stdout(predicate::str::contains(
            "shop-frontend  read   2               2099-11-11T00:00:00Z",
        ))
        .stdout(predicate::str::contains(
            "root           admin  1               never",
        ));
}

#[tokio::test(flavor = "multi_thread")]
async fn token_revoke_requires_confirmation_without_a_terminal() {
    let server = MockServer::start().await;

    sreg(&server, &["token", "revoke", "--project", "shop-frontend"])
        .await
        .code(1)
        .stderr(predicate::str::contains("--yes"));

    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn token_revoke_with_yes_deletes_the_credential() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/credentials/shop-frontend"))
        .respond_with(json(
            200,
            json!({ "project": "shop-frontend", "deleted": true }),
        ))
        .expect(1)
        .mount(&server)
        .await;

    sreg(
        &server,
        &["token", "revoke", "--project", "shop-frontend", "--yes"],
    )
    .await
    .success()
    .stdout(predicate::str::contains(
        "Revoked credential `shop-frontend`.",
    ));
}
