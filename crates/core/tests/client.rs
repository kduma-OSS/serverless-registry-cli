//! Tests of the registry API client against a local mock server.

use std::time::Duration;

use serde_json::json;
use sreg_core::models::{AddSecretRequest, CreateCredentialRequest, Grant, Kind};
use sreg_core::retry::RetryPolicy;
use sreg_core::{Client, Ecosystem, Error};
use url::Url;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "shop-frontend:k7Qx2mN9pLw4vR8sT1bYc3dF6hJ0aZ5e";

fn client(server: &MockServer, token: Option<&str>) -> Client {
    Client::new(
        Url::parse(&server.uri()).unwrap(),
        token.map(str::to_owned),
        "0.3.1",
    )
    .unwrap()
    .with_retry_policy(RetryPolicy {
        attempts: 3,
        base_delay: Duration::from_millis(1),
    })
}

fn ok(body: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("x-min-client-version", "0.1.0")
        .set_body_json(body)
}

fn error(status: u16, code: &str, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .insert_header("x-min-client-version", "0.2.0")
        .set_body_json(json!({ "error": code, "message": message }))
}

#[tokio::test]
async fn whoami_sends_identity_headers_and_parses_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(header("x-client-version", "0.3.1"))
        .and(header("user-agent", "sreg/0.3.1"))
        .and(header("accept", "application/json"))
        .respond_with(ok(json!({
            "project": "shop-frontend",
            "kind": "read",
            "grants": [{ "eco": "npm", "package": "@acme/sdk", "versions": "^2.0" }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client(&server, Some(TOKEN));
    let whoami = client.whoami().await.unwrap();

    assert_eq!(whoami.project, "shop-frontend");
    assert_eq!(whoami.kind, Kind::Read);
    assert_eq!(whoami.grants[0].package, "@acme/sdk");
    assert_eq!(client.min_client_version().as_deref(), Some("0.1.0"));
}

#[tokio::test]
async fn requests_without_token_carry_no_authorization() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/version"))
        .respond_with(ok(
            json!({ "version": "0.4.0", "minClientVersion": "0.1.0", "schemaVersion": 1 }),
        ))
        .mount(&server)
        .await;

    let info = client(&server, None).version().await.unwrap();

    assert_eq!(info.version, "0.4.0");
    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].headers.get("authorization").is_none());
    assert_eq!(requests[0].headers["x-client-version"], "0.3.1");
}

#[tokio::test]
async fn registry_url_path_prefix_is_kept() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/registry/api/v1/whoami"))
        .respond_with(ok(
            json!({ "project": "root", "kind": "admin", "grants": [] }),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let url = Url::parse(&format!("{}/registry", server.uri())).unwrap();
    let client = Client::new(url, Some(TOKEN.into()), "0.3.1").unwrap();

    assert_eq!(client.whoami().await.unwrap().kind, Kind::Admin);
}

#[tokio::test]
async fn package_names_are_encoded_as_one_segment() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/packages/npm/@acme%2Fsdk/2.3.1/yank"))
        .respond_with(ok(json!({
            "eco": "npm", "name": "@acme/sdk", "version": "2.3.1",
            "published": "2026-10-28T10:12:00Z", "yanked": true,
            "files": [{ "name": "sdk-2.3.1.tgz", "size": 48213, "sha256": "9f2c", "role": "archive" }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let response = client(&server, Some(TOKEN))
        .yank(Ecosystem::Npm, "@acme/sdk", "2.3.1")
        .await
        .unwrap();

    assert!(response.version.yanked);
    assert_eq!(response.name, "@acme/sdk");
}

#[tokio::test]
async fn error_envelope_maps_to_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/credentials"))
        .respond_with(error(403, "forbidden", "admin credential required"))
        .mount(&server)
        .await;

    let err = client(&server, Some(TOKEN))
        .credentials()
        .await
        .unwrap_err();

    match err {
        Error::Api {
            status,
            code,
            message,
        } => {
            assert_eq!(status, 403);
            assert_eq!(code, "forbidden");
            assert_eq!(message, "admin credential required");
        }
        other => panic!("unexpected error {other:?}"),
    }
}

#[tokio::test]
async fn non_json_error_body_still_maps_to_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>bad gateway</html>"))
        .mount(&server)
        .await;

    let err = client(&server, Some(TOKEN)).whoami().await.unwrap_err();

    assert!(
        matches!(err, Error::Api { status: 502, ref code, .. } if code == "unknown"),
        "{err:?}"
    );
}

#[tokio::test]
async fn upgrade_required_maps_to_client_too_old() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .respond_with(error(426, "client_too_old", "upgrade sreg"))
        .mount(&server)
        .await;

    let err = client(&server, Some(TOKEN)).whoami().await.unwrap_err();

    assert!(
        matches!(err, Error::ClientTooOld { ref min } if min == "0.2.0"),
        "{err:?}"
    );
}

#[tokio::test]
async fn version_exists_conflict_maps_to_its_own_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .and(body_json(
            json!({ "uploadId": "01JBX9T6Q4ZK3M8N2P5R7S9V0W" }),
        ))
        .respond_with(error(409, "version_exists", "already published"))
        .mount(&server)
        .await;

    let err = client(&server, Some(TOKEN))
        .publish_commit("01JBX9T6Q4ZK3M8N2P5R7S9V0W")
        .await
        .unwrap_err();

    assert!(matches!(err, Error::VersionExists), "{err:?}");
}

#[tokio::test]
async fn file_exists_conflict_maps_to_its_own_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(error(409, "file_exists", "cli-linux.tar.gz already exists"))
        .mount(&server)
        .await;

    let request: sreg_core::models::PublishInitRequest = serde_json::from_value(json!({
        "eco": "generic", "name": "cli", "version": "1.0.0", "files": [], "manifest": {}
    }))
    .unwrap();
    let err = client(&server, Some(TOKEN))
        .publish_init(&request)
        .await
        .unwrap_err();

    assert!(
        matches!(err, Error::FileExists { ref message } if message.contains("cli-linux")),
        "{err:?}"
    );
}

#[tokio::test]
async fn other_conflicts_stay_api_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .respond_with(error(409, "upload_mismatch", "size differs"))
        .mount(&server)
        .await;

    let err = client(&server, Some(TOKEN))
        .publish_commit("01JBX9T6Q4ZK3M8N2P5R7S9V0W")
        .await
        .unwrap_err();

    assert!(
        matches!(err, Error::Api { status: 409, ref code, .. } if code == "upload_mismatch"),
        "{err:?}"
    );
}

#[tokio::test]
async fn get_requests_are_retried_on_server_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .respond_with(error(503, "internal_error", "try again"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .respond_with(ok(
            json!({ "project": "root", "kind": "admin", "grants": [] }),
        ))
        .mount(&server)
        .await;

    let whoami = client(&server, Some(TOKEN)).whoami().await.unwrap();

    assert_eq!(whoami.project, "root");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn get_requests_honour_retry_after_on_rate_limit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .respond_with(error(429, "rate_limited", "slow down").insert_header("retry-after", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/whoami"))
        .respond_with(ok(
            json!({ "project": "root", "kind": "admin", "grants": [] }),
        ))
        .mount(&server)
        .await;

    assert!(client(&server, Some(TOKEN)).whoami().await.is_ok());
}

#[tokio::test]
async fn non_idempotent_requests_are_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/credentials"))
        .respond_with(error(500, "internal_error", "boom"))
        .expect(1)
        .mount(&server)
        .await;

    let request = CreateCredentialRequest::single(
        "shop-frontend",
        Kind::Read,
        vec![Grant::parse_shorthand("composer:acme/*").unwrap()],
    );
    let err = client(&server, Some(TOKEN))
        .create_credential(&request)
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Api { status: 500, .. }));
}

#[tokio::test]
async fn credential_endpoints_use_documented_paths_and_bodies() {
    let server = MockServer::start().await;
    let credential = json!({
        "project": "shop-frontend",
        "kind": "read",
        "auth": [{ "id": "3f9a1c0b7d2e", "type": "key", "created": "2026-10-28T09:00:00Z", "expires": null }],
        "grants": []
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/credentials/shop-frontend"))
        .respond_with(ok(credential.clone()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/credentials/shop-frontend"))
        .and(body_json(json!({ "grants": [] })))
        .respond_with(ok(credential))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/credentials/shop-frontend/secrets"))
        .and(body_json(json!({ "type": "key" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "secret": { "id": "7c21de4490ab", "type": "key", "created": "2026-10-28T09:10:00Z", "expires": null },
            "token": "shop-frontend:Q2w9Er7Ty5Ui3Op1As8Df6Gh4Jk2Lz0X"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/credentials/shop-frontend/secrets/3f9a1c0b7d2e",
        ))
        .and(body_json(json!({ "description": "CI of the shop" })))
        .respond_with(ok(json!({
            "secret": { "id": "3f9a1c0b7d2e", "type": "key", "created": "2026-10-28T09:00:00Z",
                        "expires": null, "description": "CI of the shop" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/credentials/shop-frontend/secrets/3f9a1c0b7d2e",
        ))
        .and(body_json(json!({ "expires": "2026-11-11T00:00:00Z" })))
        .respond_with(ok(json!({
            "secret": { "id": "3f9a1c0b7d2e", "type": "key", "created": "2026-10-28T09:00:00Z",
                        "expires": "2026-11-11T00:00:00Z" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/credentials/shop-frontend"))
        .respond_with(ok(json!({ "project": "shop-frontend", "deleted": true })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client(&server, Some(TOKEN));
    assert_eq!(
        client.credential("shop-frontend").await.unwrap().auth.len(),
        1
    );
    assert!(client
        .update_grants("shop-frontend", &[])
        .await
        .unwrap()
        .grants
        .is_empty());
    let added = client
        .add_secret("shop-frontend", &AddSecretRequest::key(None))
        .await
        .unwrap();
    assert!(added.token.is_some());
    let described = client
        .describe_secret(
            "shop-frontend",
            "3f9a1c0b7d2e",
            Some("CI of the shop".into()),
        )
        .await
        .unwrap();
    assert_eq!(
        described.secret.description.as_deref(),
        Some("CI of the shop")
    );
    let expired = client
        .expire_secret(
            "shop-frontend",
            "3f9a1c0b7d2e",
            Some("2026-11-11T00:00:00Z".into()),
        )
        .await
        .unwrap();
    assert_eq!(
        expired.secret.expires.as_deref(),
        Some("2026-11-11T00:00:00Z")
    );
    assert!(
        client
            .delete_credential("shop-frontend")
            .await
            .unwrap()
            .deleted
    );
}

#[tokio::test]
async fn package_endpoints_use_documented_paths() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/packages"))
        .respond_with(ok(json!({ "packages": [] })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/packages/composer/acme%2Fsdk"))
        .respond_with(ok(
            json!({ "eco": "composer", "name": "acme/sdk", "versions": [] }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/packages/composer/acme%2Fsdk/1.2.0"))
        .respond_with(ok(
            json!({ "eco": "composer", "name": "acme/sdk", "version": "1.2.0", "deleted": true }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/packages/composer/acme%2Fsdk"))
        .respond_with(ok(
            json!({ "eco": "composer", "name": "acme/sdk", "deletedVersions": ["1.1.0"] }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/packages/cargo/acme-core/reindex"))
        .respond_with(ok(
            json!({ "eco": "cargo", "name": "acme-core", "manifests": 2 }),
        ))
        .mount(&server)
        .await;

    let client = client(&server, Some(TOKEN));
    assert!(client.packages().await.unwrap().packages.is_empty());
    assert_eq!(
        client
            .package(Ecosystem::Composer, "acme/sdk")
            .await
            .unwrap()
            .name,
        "acme/sdk"
    );
    assert!(
        client
            .delete_version(Ecosystem::Composer, "acme/sdk", "1.2.0")
            .await
            .unwrap()
            .deleted
    );
    assert_eq!(
        client
            .delete_package(Ecosystem::Composer, "acme/sdk")
            .await
            .unwrap()
            .deleted_versions,
        ["1.1.0"]
    );
    assert_eq!(
        client
            .reindex(Ecosystem::Cargo, "acme-core")
            .await
            .unwrap()
            .manifests,
        2
    );
}

#[test]
fn rejects_non_http_registry_urls() {
    let err = Client::new(Url::parse("ftp://example.com").unwrap(), None, "0.3.1").unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)));
}

#[test]
fn debug_output_hides_the_token() {
    let client = Client::new(
        Url::parse("https://packages.example.com").unwrap(),
        Some(TOKEN.into()),
        "0.3.1",
    )
    .unwrap();
    assert!(!format!("{client:?}").contains("k7Qx2mN9"));
}
