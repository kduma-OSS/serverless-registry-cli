//! Tests of the generic feed and of file downloads against local mock servers: one
//! plays the registry, the other the storage behind its redirects.

use std::time::Duration;

use serde_json::json;
use sreg_core::download::Downloader;
use sreg_core::models::GenericFeedFile;
use sreg_core::retry::RetryPolicy;
use sreg_core::{Client, Error, GenericQuery};
use url::Url;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "shop-frontend:k7Qx2mN9pLw4vR8sT1bYc3dF6hJ0aZ5e";
const CONTENT: &[u8] = b"hello";
const CONTENT_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

fn client(server: &MockServer) -> Client {
    Client::new(
        Url::parse(&server.uri()).unwrap(),
        Some(TOKEN.into()),
        "0.3.1",
    )
    .unwrap()
    .with_retry_policy(RetryPolicy {
        attempts: 2,
        base_delay: Duration::from_millis(1),
    })
}

fn feed_file(registry: &MockServer, sha256: &str) -> GenericFeedFile {
    GenericFeedFile {
        name: "cli-linux.tar.gz".into(),
        size: CONTENT.len() as u64,
        sha256: sha256.into(),
        digests: Default::default(),
        labels: [("os".to_owned(), "linux".to_owned())].into(),
        url: format!(
            "{}/generic/tools%2Fcli/1.0.0/cli-linux.tar.gz",
            registry.uri()
        ),
    }
}

async fn redirecting_servers(body: &'static [u8]) -> (MockServer, MockServer) {
    let registry = MockServer::start().await;
    let storage = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/generic/tools%2Fcli/1.0.0/cli-linux.tar.gz"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(
            ResponseTemplate::new(302).insert_header(
                "location",
                format!(
                    "{}/bucket/cli-linux.tar.gz?X-Amz-Signature=sig",
                    storage.uri()
                )
                .as_str(),
            ),
        )
        .mount(&registry)
        .await;
    Mock::given(method("GET"))
        .and(path("/bucket/cli-linux.tar.gz"))
        .and(query_param("X-Amz-Signature", "sig"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(&storage)
        .await;
    (registry, storage)
}

#[tokio::test]
async fn feed_request_carries_labels_and_channel() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/generic/tools%2Fcli/latest"))
        .and(query_param("label", "os:linux"))
        .and(query_param("label", "arch:x86_64"))
        .and(query_param("channel", "prerelease"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "tools/cli",
            "version": "1.1.0-rc.1",
            "published": "2026-10-28T10:12:00Z",
            "yanked": false,
            "meta": { "channel": "beta" },
            "files": [{
                "name": "cli-linux.tar.gz", "size": 5, "sha256": CONTENT_SHA256,
                "labels": { "os": "linux", "arch": "x86_64" },
                "url": "https://packages.example.com/generic/tools%2Fcli/1.1.0-rc.1/cli-linux.tar.gz"
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let labels = vec![
        ("os".to_owned(), "linux".to_owned()),
        ("arch".to_owned(), "x86_64".to_owned()),
    ];
    let version = client(&server)
        .generic_version(
            "tools/cli",
            "latest",
            &GenericQuery {
                labels: &labels,
                prerelease: true,
                ..GenericQuery::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(version.version, "1.1.0-rc.1");
    assert_eq!(version.meta["channel"], "beta");
    assert_eq!(version.files[0].labels["arch"], "x86_64");
}

#[tokio::test]
async fn feed_request_carries_the_version_constraint() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/generic/tools%2Fcli/latest"))
        .and(query_param("version", ">=1 <2"))
        .and(query_param("label", "os:linux"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "tools/cli", "version": "1.4.0", "published": "2026-10-28T10:12:00Z",
            "yanked": false, "meta": {}, "files": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let labels = vec![("os".to_owned(), "linux".to_owned())];
    let version = client(&server)
        .generic_version(
            "tools/cli",
            "latest",
            &GenericQuery {
                labels: &labels,
                constraint: Some(">=1 <2"),
                ..GenericQuery::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(version.version, "1.4.0");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0].url.query(),
        Some("label=os%3Alinux&version=%3E%3D1+%3C2")
    );
}

#[tokio::test]
async fn feed_errors_are_mapped() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/generic/cli/9.9.9"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({ "error": "not_found", "message": "no such version" })),
        )
        .mount(&server)
        .await;

    let err = client(&server)
        .generic_version("cli", "9.9.9", &GenericQuery::default())
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Api { status: 404, .. }), "{err:?}");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].url.query(), None);
}

#[tokio::test]
async fn downloads_through_the_redirect_without_leaking_the_token() {
    let (registry, storage) = redirecting_servers(CONTENT).await;
    let dir = tempfile::tempdir().unwrap();

    let path = Downloader::new(&client(&registry))
        .unwrap()
        .download(&feed_file(&registry, CONTENT_SHA256), dir.path())
        .await
        .unwrap();

    assert_eq!(path, dir.path().join("cli-linux.tar.gz"));
    assert_eq!(std::fs::read(&path).unwrap(), CONTENT);
    let storage_requests = storage.received_requests().await.unwrap();
    assert_eq!(storage_requests.len(), 1);
    assert!(storage_requests[0].headers.get("authorization").is_none());
    assert!(storage_requests[0]
        .headers
        .get("x-client-version")
        .is_none());
}

#[tokio::test]
async fn checksum_mismatch_is_an_integrity_error_and_leaves_nothing() {
    let (registry, _storage) = redirecting_servers(b"HELLO").await;
    let dir = tempfile::tempdir().unwrap();

    let err = Downloader::new(&client(&registry))
        .unwrap()
        .download(&feed_file(&registry, CONTENT_SHA256), dir.path())
        .await
        .unwrap_err();

    assert!(
        matches!(err, Error::Integrity { ref file, .. } if file == "cli-linux.tar.gz"),
        "{err:?}"
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn size_mismatch_is_an_integrity_error() {
    let (registry, _storage) = redirecting_servers(b"hello world").await;
    let dir = tempfile::tempdir().unwrap();

    let err = Downloader::new(&client(&registry))
        .unwrap()
        .download(&feed_file(&registry, CONTENT_SHA256), dir.path())
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Integrity { .. }), "{err:?}");
}

#[tokio::test]
async fn token_is_not_sent_to_file_urls_on_other_origins() {
    let registry = MockServer::start().await;
    let elsewhere = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(CONTENT))
        .mount(&elsewhere)
        .await;
    let mut file = feed_file(&registry, CONTENT_SHA256);
    file.url = format!("{}/cli-linux.tar.gz", elsewhere.uri());
    let dir = tempfile::tempdir().unwrap();

    Downloader::new(&client(&registry))
        .unwrap()
        .download(&file, dir.path())
        .await
        .unwrap();

    let requests = elsewhere.received_requests().await.unwrap();
    assert!(requests[0].headers.get("authorization").is_none());
}

#[tokio::test]
async fn storage_server_errors_are_retried() {
    let registry = MockServer::start().await;
    let storage = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "location",
            format!("{}/bucket/cli-linux.tar.gz", storage.uri()).as_str(),
        ))
        .mount(&registry)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&storage)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(CONTENT))
        .mount(&storage)
        .await;
    let dir = tempfile::tempdir().unwrap();

    let path = Downloader::new(&client(&registry))
        .unwrap()
        .download(&feed_file(&registry, CONTENT_SHA256), dir.path())
        .await
        .unwrap();

    assert_eq!(std::fs::read(path).unwrap(), CONTENT);
}

/// A registry whose download endpoint redirects to `storage`, which answers `403` (an
/// expired presigned URL) the first `forbidden` times and then serves the content.
async fn expiring_servers(forbidden: u64) -> (MockServer, MockServer) {
    let registry = MockServer::start().await;
    let storage = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "location",
            format!("{}/bucket/cli-linux.tar.gz", storage.uri()).as_str(),
        ))
        .mount(&registry)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403))
        .up_to_n_times(forbidden)
        .mount(&storage)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(CONTENT))
        .mount(&storage)
        .await;
    (registry, storage)
}

#[tokio::test]
async fn an_expired_download_url_is_requested_again_once() {
    let (registry, _storage) = expiring_servers(1).await;
    let dir = tempfile::tempdir().unwrap();

    let path = Downloader::new(&client(&registry))
        .unwrap()
        .download(&feed_file(&registry, CONTENT_SHA256), dir.path())
        .await
        .unwrap();

    assert_eq!(std::fs::read(path).unwrap(), CONTENT);
    assert_eq!(registry.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_second_storage_refusal_is_fatal() {
    let (registry, _storage) = expiring_servers(2).await;
    let dir = tempfile::tempdir().unwrap();

    let err = Downloader::new(&client(&registry))
        .unwrap()
        .download(&feed_file(&registry, CONTENT_SHA256), dir.path())
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Api { status: 403, .. }), "{err:?}");
    assert_eq!(registry.received_requests().await.unwrap().len(), 2);
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn sole_file_is_resolved_from_the_redirect() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/generic/tools%2Fcli/latest/-/download"))
        .and(query_param("label", "os:linux"))
        .and(query_param("version", "^1"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header(
                    "location",
                    "https://storage.example/b/cli.tgz?X-Amz-Signature=s",
                )
                .insert_header("x-resolved-version", "1.4.0")
                .insert_header("x-resolved-file", "cli-linux.tar.gz"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = client(&server);
    let labels = vec![("os".to_owned(), "linux".to_owned())];
    let url = client
        .generic_sole_url(
            "tools/cli",
            "latest",
            &GenericQuery {
                labels: &labels,
                constraint: Some("^1"),
                ..GenericQuery::default()
            },
        )
        .unwrap();

    let sole = Downloader::new(&client)
        .unwrap()
        .resolve_sole(url)
        .await
        .unwrap();

    assert_eq!(sole.version, "1.4.0");
    assert_eq!(sole.name, "cli-linux.tar.gz");
    assert_eq!(sole.location.host_str(), Some("storage.example"));
    assert!(!format!("{sole:?}").contains("Signature"));
}

#[tokio::test]
async fn sole_file_rejects_unsafe_names_and_missing_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/generic/cli/1.0.0/-/download"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", "https://storage.example/x")
                .insert_header("x-resolved-version", "1.0.0")
                .insert_header("x-resolved-file", "../evil"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/generic/cli/2.0.0/-/download"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "https://s.example/x"))
        .mount(&server)
        .await;
    let client = client(&server);
    let downloader = Downloader::new(&client).unwrap();

    for version in ["1.0.0", "2.0.0"] {
        let url = client
            .generic_sole_url("cli", version, &GenericQuery::default())
            .unwrap();
        let err = downloader.resolve_sole(url).await.unwrap_err();
        assert!(matches!(err, Error::InvalidInput(_)), "{version}: {err:?}");
    }
}

#[tokio::test]
async fn ambiguous_labels_list_the_candidates() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/generic/cli/latest/-/download"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "error": "ambiguous",
            "message": "2 files match the labels",
            "version": "1.4.0",
            "files": [
                { "name": "cli-linux-x64.tar.gz", "labels": { "os": "linux", "arch": "x64" } },
                "cli-linux-arm64.tar.gz"
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/generic/cli/9.9.9/-/download"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({ "error": "not_found", "message": "no file matches" })),
        )
        .mount(&server)
        .await;
    let client = client(&server);
    let downloader = Downloader::new(&client).unwrap();

    let url = client
        .generic_sole_url("cli", "latest", &GenericQuery::default())
        .unwrap();
    let err = downloader.resolve_sole(url).await.unwrap_err();
    let Error::Ambiguous {
        message,
        version,
        files,
    } = err
    else {
        panic!("expected Ambiguous, got {err:?}");
    };
    assert_eq!(message, "2 files match the labels");
    assert_eq!(version.as_deref(), Some("1.4.0"));
    assert_eq!(files[0].name, "cli-linux-x64.tar.gz");
    assert_eq!(files[0].labels["arch"], "x64");
    assert_eq!(files[1].name, "cli-linux-arm64.tar.gz");
    assert!(files[1].labels.is_empty());

    let url = client
        .generic_sole_url("cli", "9.9.9", &GenericQuery::default())
        .unwrap();
    let err = downloader.resolve_sole(url).await.unwrap_err();
    assert!(matches!(err, Error::Api { status: 404, .. }), "{err:?}");
}

#[tokio::test]
async fn download_to_writes_the_given_path() {
    let (registry, _storage) = redirecting_servers(CONTENT).await;
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("cli.tgz");

    Downloader::new(&client(&registry))
        .unwrap()
        .download_to(&feed_file(&registry, CONTENT_SHA256), &destination)
        .await
        .unwrap();

    assert_eq!(std::fs::read(&destination).unwrap(), CONTENT);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}
