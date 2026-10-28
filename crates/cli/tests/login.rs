//! `sreg login` and `--login` against a mock registry: the session, the poll and what is
//! done with the token that comes back.

mod common;

use common::{error, json, sreg, sreg_with, stdout_json, TOKEN};
use predicates::prelude::*;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer};

const ISSUED: &str =
    "reader:st.eyJ0eXAiOiJzdCJ9.c2lnbmF0dXJlLXNpZ25hdHVyZS1zaWduYXR1cmUtc2lnbmF0dXJlLTEyMw";
const POLL_TOKEN: &str = "poll-token-for-the-cli-only-0123456789abcdef";

fn session_response(server: &MockServer) -> wiremock::ResponseTemplate {
    json(
        201,
        json!({
            "session": "signed-session-id",
            "url": format!("{}/authorize/signed-session-id", server.uri()),
            "poll_token": POLL_TOKEN,
            "interval": 0,
            "expires_at": "2026-10-28T10:10:00.000Z"
        }),
    )
}

fn approved(kind: &str) -> wiremock::ResponseTemplate {
    json(
        200,
        json!({
            "status": "approved",
            "token": ISSUED,
            "expires_at": "2026-10-28T11:00:00.000Z",
            "project": "reader",
            "kind": kind
        }),
    )
}

/// A registry that creates one session and answers the first poll with "pending".
async fn login_server(kind: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/sessions"))
        .respond_with(session_response(&server))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/sessions/poll"))
        .respond_with(json(202, json!({ "status": "pending" })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/sessions/poll"))
        .respond_with(approved(kind))
        .with_priority(2)
        .mount(&server)
        .await;
    server
}

fn bodies(requests: &[wiremock::Request], endpoint: &str) -> Vec<Value> {
    requests
        .iter()
        .filter(|request| request.url.path() == endpoint)
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn login_prints_an_export_line_and_keeps_the_rest_on_stderr() {
    let server = login_server("read").await;

    sreg(&server, &["login", "--no-browser"])
        .await
        .success()
        .stdout(predicate::eq(format!("export REGISTRY_TOKEN='{ISSUED}'\n")))
        .stderr(predicate::str::contains(
            "Requesting a token to download packages.",
        ))
        .stderr(predicate::str::contains(format!(
            "{}/authorize/signed-session-id",
            server.uri()
        )))
        .stderr(predicate::str::contains("signed in as reader (read)"))
        .stderr(predicate::str::contains("st.").not())
        .stderr(predicate::str::contains(POLL_TOKEN).not());

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        bodies(&requests, "/api/v1/auth/sessions"),
        vec![json!({ "purpose": "pull" })]
    );
    let polls = bodies(&requests, "/api/v1/auth/sessions/poll");
    assert_eq!(polls.len(), 2);
    assert_eq!(
        polls[0],
        json!({ "session": "signed-session-id", "poll_token": POLL_TOKEN })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_environment_can_turn_the_browser_off_with_the_usual_spellings() {
    for value in ["1", "true", "yes"] {
        let server = login_server("read").await;

        sreg_with(&server, &["login"], &[("SREG_NO_BROWSER", value)])
            .await
            .success()
            .stdout(predicate::str::starts_with("export REGISTRY_TOKEN="));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn login_never_sends_the_static_token() {
    let server = login_server("read").await;

    sreg(&server, &["login", "--no-browser"]).await.success();

    for request in server.received_requests().await.unwrap() {
        assert!(
            !request.headers.contains_key("authorization"),
            "{} carried a credential",
            request.url.path()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn login_json_is_the_only_stdout() {
    let server = login_server("read").await;

    let assert = sreg(&server, &["login", "--no-browser", "--json"])
        .await
        .success();

    let body = stdout_json(&assert);
    assert_eq!(body["token"], ISSUED);
    assert_eq!(body["expiresAt"], "2026-10-28T11:00:00.000Z");
    assert_eq!(body["project"], "reader");
    assert_eq!(body["kind"], "read");
}

#[tokio::test(flavor = "multi_thread")]
async fn login_asks_to_publish_the_listed_versions() {
    let server = login_server("publish").await;

    sreg(
        &server,
        &[
            "login",
            "--no-browser",
            "--publish",
            "composer:acme/sdk@1.4.0",
            "--publish",
            "npm:@acme/sdk@2.0.0",
        ],
    )
    .await
    .success()
    .stderr(predicate::str::contains("composer acme/sdk 1.4.0"))
    .stderr(predicate::str::contains("npm @acme/sdk 2.0.0"));

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        bodies(&requests, "/api/v1/auth/sessions"),
        vec![json!({
            "purpose": "publish",
            "scopes": [
                { "eco": "composer", "package": "acme/sdk", "version": "1.4.0" },
                { "eco": "npm", "package": "@acme/sdk", "version": "2.0.0" }
            ]
        })]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn login_rejects_a_malformed_publish_spec_before_calling_the_registry() {
    let server = MockServer::start().await;

    sreg(&server, &["login", "--no-browser", "--publish", "acme/sdk"])
        .await
        .code(1)
        .stderr(predicate::str::contains(
            "expected <ecosystem>:<package>@<version>",
        ));

    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_login_fails() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/sessions"))
        .respond_with(session_response(&server))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/sessions/poll"))
        .respond_with(error(403, "forbidden", "The login was refused."))
        .mount(&server)
        .await;

    sreg(&server, &["login", "--no-browser"])
        .await
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("The login was refused."));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_session_fails() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/sessions"))
        .respond_with(session_response(&server))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/sessions/poll"))
        .respond_with(error(404, "not_found", "The login session has expired."))
        .mount(&server)
        .await;

    sreg(&server, &["login", "--no-browser"])
        .await
        .code(1)
        .stderr(predicate::str::contains("The login session has expired."));
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_login_writes_the_issued_token_instead_of_the_static_one() {
    let server = login_server("read").await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("auth.json");

    sreg(
        &server,
        &[
            "auth",
            "composer",
            "--login",
            "--no-browser",
            "--write",
            file.to_str().unwrap(),
        ],
    )
    .await
    .success()
    .stdout(predicate::str::contains(ISSUED).not())
    .stderr(predicate::str::contains(
        "Requesting a token to download packages.",
    ));

    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains(ISSUED), "{written}");
    assert!(!written.contains(TOKEN));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        bodies(&requests, "/api/v1/auth/sessions"),
        vec![json!({ "purpose": "pull", "eco": "composer" })]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn login_conflicts_with_oidc() {
    let server = MockServer::start().await;

    sreg(&server, &["auth", "composer", "--login", "--oidc"])
        .await
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[tokio::test(flavor = "multi_thread")]
async fn generic_get_login_downloads_with_the_issued_token() {
    let server = login_server("read").await;
    Mock::given(method("GET"))
        .and(path("/generic/tools%2Fcli/latest"))
        .and(header("authorization", format!("Bearer {ISSUED}").as_str()))
        .respond_with(json(
            200,
            json!({
                "name": "tools/cli", "version": "1.0.0", "published": "2026-10-28T10:12:00Z",
                "yanked": false, "meta": {}, "files": []
            }),
        ))
        .expect(1)
        .mount(&server)
        .await;

    sreg(
        &server,
        &["generic", "get", "tools/cli", "--login", "--no-browser"],
    )
    .await
    // The version has no files: the command fails after it used the token.
    .code(1)
    .stderr(predicate::str::contains("has no files"));

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        bodies(&requests, "/api/v1/auth/sessions"),
        vec![json!({ "purpose": "pull", "eco": "generic" })]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn generic_publish_login_asks_for_exactly_the_published_version() {
    let server = login_server("publish").await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cli-linux.tar.gz");
    std::fs::write(&file, b"hello").unwrap();
    let spec = dir.path().join("files.json");
    std::fs::write(
        &spec,
        serde_json::to_string(&json!([{ "path": file }])).unwrap(),
    )
    .unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .and(header("authorization", format!("Bearer {ISSUED}").as_str()))
        .respond_with(common::init_responder(&server, "upload-login-0001"))
        .expect(1)
        .mount(&server)
        .await;

    sreg_with(
        &server,
        &[
            "generic",
            "publish",
            "tools/cli",
            "1.0.0",
            "--spec",
            spec.to_str().unwrap(),
            "--no-append",
            "--dry-run",
            "--login",
            "--no-browser",
        ],
        &[],
    )
    .await
    .success();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        bodies(&requests, "/api/v1/auth/sessions"),
        vec![json!({
            "purpose": "publish",
            "scopes": [{ "eco": "generic", "package": "tools/cli", "version": "1.0.0" }]
        })]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unauthorized_answer_suggests_signing_in_again() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .respond_with(error(401, "unauthorized", "Authentication required."))
        .mount(&server)
        .await;

    sreg(&server, &["whoami"])
        .await
        .code(1)
        .stderr(predicate::str::contains("sreg login"));
}
