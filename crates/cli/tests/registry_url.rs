//! Registry URLs carrying the token in their userinfo (`https://<login>:<password>@host/`).
//!
//! The mock registry speaks plain http, which credentials are only allowed over for
//! `localhost` and `127.0.0.1`.

mod common;

use common::{json, sreg_with, stdout_json, TOKEN};
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer};

const SECRET: &str = "s3cr3t-Pw";

/// The registry URL of `server` with `userinfo` (`login:password`, already encoded).
fn url_with(server: &MockServer, userinfo: &str) -> String {
    server
        .uri()
        .replacen("http://", &format!("http://{userinfo}@"), 1)
        + "/"
}

/// `REGISTRY_TOKEN` cleared, so that only the URL carries the token.
const NO_TOKEN: (&str, &str) = ("REGISTRY_TOKEN", "");

async fn whoami_server(token: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .and(header("authorization", format!("Bearer {token}").as_str()))
        .respond_with(json(
            200,
            json!({ "project": "shop-frontend", "kind": "read", "grants": [] }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    server
}

fn stdout(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
}

fn stderr(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stderr).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_authenticates_with_the_url_credentials() {
    let server = whoami_server(&format!("shop-frontend:{SECRET}")).await;
    let registry = url_with(&server, &format!("shop-frontend:{SECRET}"));

    let assert = sreg_with(
        &server,
        &["whoami", "--registry", &registry, "-vv"],
        &[NO_TOKEN],
    )
    .await
    .success()
    .stdout(predicate::str::contains("Project: shop-frontend"));

    assert!(!stdout(&assert).contains(SECRET));
    assert!(!stderr(&assert).contains(SECRET), "{}", stderr(&assert));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_json_never_contains_the_secret() {
    let server = whoami_server(&format!("shop-frontend:{SECRET}")).await;
    let registry = url_with(&server, &format!("shop-frontend:{SECRET}"));

    let assert = sreg_with(
        &server,
        &["whoami", "--json"],
        &[NO_TOKEN, ("REGISTRY_URL", &registry)],
    )
    .await
    .success();

    assert_eq!(stdout_json(&assert)["project"], "shop-frontend");
    assert!(!stdout(&assert).contains(SECRET));
}

#[tokio::test(flavor = "multi_thread")]
async fn url_credentials_are_percent_decoded() {
    let server = whoami_server("okta.shop:p@ss:word").await;
    let registry = url_with(&server, "okta.shop:p%40ss%3Aword");

    sreg_with(&server, &["whoami", "--registry", &registry], &[NO_TOKEN])
        .await
        .success();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_differing_explicit_token_is_a_conflict() {
    let server = MockServer::start().await;
    let registry = url_with(&server, &format!("shop-frontend:{SECRET}"));

    // `sreg_with` sets `REGISTRY_TOKEN` to `TOKEN`, which differs from the URL's.
    let assert = sreg_with(&server, &["whoami", "--registry", &registry], &[])
        .await
        .code(1)
        .stderr(predicate::str::contains("differ"));

    assert!(!stderr(&assert).contains(SECRET));
    assert!(!stderr(&assert).contains(TOKEN));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_token_twice_is_accepted() {
    let server = whoami_server(TOKEN).await;
    let registry = url_with(&server, TOKEN);

    sreg_with(&server, &["whoami", "--registry", &registry], &[])
        .await
        .success();
}

#[tokio::test(flavor = "multi_thread")]
async fn credentials_need_a_password() {
    let server = MockServer::start().await;
    let registry = url_with(&server, "shop-frontend");

    sreg_with(&server, &["whoami", "--registry", &registry], &[NO_TOKEN])
        .await
        .code(1)
        .stderr(predicate::str::contains(
            "registry URL credentials need both a login and a password",
        ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn credentials_over_plain_http_to_a_remote_host_are_refused() {
    let server = MockServer::start().await;
    let registry = format!("http://shop-frontend:{SECRET}@packages.example.invalid/");

    let assert = sreg_with(&server, &["whoami", "--registry", &registry], &[NO_TOKEN])
        .await
        .code(1)
        .stderr(predicate::str::contains("clear text"));

    assert!(!stderr(&assert).contains(SECRET));
}

#[tokio::test(flavor = "multi_thread")]
async fn transport_errors_show_the_url_without_credentials() {
    let server = MockServer::start().await;
    let registry = format!("http://shop-frontend:{SECRET}@127.0.0.1:1/");

    let assert = sreg_with(
        &server,
        &["whoami", "--registry", &registry, "-vv"],
        &[NO_TOKEN],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("127.0.0.1:1"));

    assert!(!stderr(&assert).contains(SECRET), "{}", stderr(&assert));
}

#[tokio::test(flavor = "multi_thread")]
async fn version_never_sends_the_url_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/version"))
        .respond_with(json(
            200,
            json!({ "version": "0.4.0", "minClientVersion": "0.0.1", "schemaVersion": 1 }),
        ))
        .mount(&server)
        .await;
    let registry = url_with(&server, &format!("shop-frontend:{SECRET}"));

    let assert = sreg_with(
        &server,
        &["version", "--json", "--registry", &registry],
        &[],
    )
    .await
    .success();

    assert_eq!(stdout_json(&assert)["registry"]["version"], "0.4.0");
    assert!(!stdout(&assert).contains(SECRET));
    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].headers.get("authorization").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_writes_the_url_credentials_for_the_bare_host() {
    let server = MockServer::start().await;
    let registry = url_with(&server, &format!("shop-frontend:{SECRET}"));
    let host = server.uri().trim_start_matches("http://").to_owned();

    sreg_with(
        &server,
        &["auth", "composer", "--registry", &registry],
        &[NO_TOKEN],
    )
    .await
    .success()
    .stdout(format!(
        "export COMPOSER_AUTH='{{\"bearer\":{{\"{host}\":\"shop-frontend:{SECRET}\"}}}}'\n"
    ));

    let assert = sreg_with(
        &server,
        &["auth", "swift", "--json", "--registry", &registry],
        &[NO_TOKEN],
    )
    .await
    .success();
    let body = stdout_json(&assert);
    assert_eq!(body["registry"], format!("{}/swift", server.uri()));
    assert_eq!(body["username"], "shop-frontend");
    assert_eq!(body["password"], SECRET);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[cfg(feature = "generic")]
#[tokio::test(flavor = "multi_thread")]
async fn generic_publish_authenticates_with_the_url_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .and(header(
            "authorization",
            format!("Bearer shop-frontend:{SECRET}").as_str(),
        ))
        .respond_with(common::init_responder(&server, "upload-0001"))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cli.tar.gz");
    std::fs::write(&file, b"hello").unwrap();
    let registry = url_with(&server, &format!("shop-frontend:{SECRET}"));

    let assert = sreg_with(
        &server,
        &[
            "generic",
            "publish",
            "tools/cli",
            "1.0.0",
            file.to_str().unwrap(),
            "--dry-run",
            "--json",
        ],
        &[NO_TOKEN, ("REGISTRY_URL", &registry)],
    )
    .await
    .success();

    assert_eq!(stdout_json(&assert)["dryRun"], true);
    assert!(!stdout(&assert).contains(SECRET));
}
