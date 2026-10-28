//! End-to-end tests of the publishing sequence against a local mock server that
//! plays both the registry and the storage.

use std::io::Write;
use std::time::Duration;

use serde_json::json;
use sreg_core::models::format_timestamp;
use sreg_core::publish::{publish, PublishOptions, PublishOutcome};
use sreg_core::retry::RetryPolicy;
use sreg_core::{Client, Ecosystem, Error, Package, PackageFile};
use time::OffsetDateTime;
use url::Url;
use wiremock::matchers::{body_bytes, body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CONTENT: &[u8] = b"pretend this is a gzipped npm tarball";
const UPLOAD_PATH: &str = "/bucket/_staging/upload/sdk-2.3.1.tgz";

struct Fixture {
    server: MockServer,
    client: Client,
    package: Package,
    _file: tempfile::NamedTempFile,
}

async fn fixture() -> Fixture {
    let server = MockServer::start().await;
    let client = Client::new(
        Url::parse(&server.uri()).unwrap(),
        Some("ci-acme-sdk:secret".into()),
        "0.3.1",
    )
    .unwrap()
    .with_retry_policy(RetryPolicy {
        attempts: 3,
        base_delay: Duration::from_millis(1),
    });

    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(CONTENT).unwrap();
    let package = Package {
        eco: Ecosystem::Npm,
        name: "@acme/sdk".into(),
        version: "2.3.1".into(),
        files: vec![PackageFile::archive(file.path(), "sdk-2.3.1.tgz".into()).unwrap()],
        manifest: json!({ "name": "@acme/sdk", "version": "2.3.1" }),
    };

    Fixture {
        server,
        client,
        package,
        _file: file,
    }
}

fn init_response(server: &MockServer, upload_id: &str, valid_for: Duration) -> ResponseTemplate {
    let expires = format_timestamp(OffsetDateTime::now_utc() + valid_for).unwrap();
    ResponseTemplate::new(200).set_body_json(json!({
        "uploadId": upload_id,
        "expires": expires,
        "uploads": [{
            "name": "sdk-2.3.1.tgz",
            "putUrl": format!("{}{UPLOAD_PATH}?X-Amz-Signature={upload_id}", server.uri()),
            "headers": {
                "content-length": CONTENT.len().to_string(),
                "x-amz-checksum-sha256": "c2hhMjU2"
            }
        }]
    }))
}

fn commit_response() -> ResponseTemplate {
    ResponseTemplate::new(201).set_body_json(json!({
        "eco": "npm",
        "name": "@acme/sdk",
        "version": "2.3.1",
        "url": "https://packages.example.com/npm/@acme/sdk/-/sdk-2.3.1.tgz",
        "files": [{
            "name": "sdk-2.3.1.tgz",
            "url": "https://packages.example.com/npm/@acme/sdk/-/sdk-2.3.1.tgz"
        }]
    }))
}

fn error(status: u16, code: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({ "error": code, "message": code }))
}

const TEN_MINUTES: Duration = Duration::from_secs(600);

async fn requests_to(server: &MockServer, method_name: &str, request_path: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| {
            request.method.as_str() == method_name && request.url.path() == request_path
        })
        .count()
}

#[tokio::test]
async fn publishes_through_init_put_and_commit() {
    let f = fixture().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .and(body_json(json!({
            "eco": "npm",
            "name": "@acme/sdk",
            "version": "2.3.1",
            "files": [{
                "name": "sdk-2.3.1.tgz",
                "size": CONTENT.len(),
                "sha256": f.package.files[0].sha256,
                "role": "archive"
            }],
            "manifest": { "name": "@acme/sdk", "version": "2.3.1" },
            "tag": "next"
        })))
        .respond_with(init_response(&f.server, "upload-1-abcdefghij", TEN_MINUTES))
        .expect(1)
        .mount(&f.server)
        .await;
    Mock::given(method("PUT"))
        .and(path(UPLOAD_PATH))
        .and(body_bytes(CONTENT))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&f.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .and(body_json(json!({ "uploadId": "upload-1-abcdefghij" })))
        .respond_with(commit_response())
        .expect(1)
        .mount(&f.server)
        .await;

    let outcome = publish(
        &f.client,
        &f.package,
        PublishOptions {
            tag: Some("next".into()),
            ..PublishOptions::default()
        },
    )
    .await
    .unwrap();

    let PublishOutcome::Published { response } = outcome else {
        panic!("expected a published outcome");
    };
    assert_eq!(response.version, "2.3.1");
    assert!(response.url.ends_with("/sdk-2.3.1.tgz"));
}

#[tokio::test]
async fn dry_run_stops_after_init() {
    let f = fixture().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_response(&f.server, "upload-1-abcdefghij", TEN_MINUTES))
        .expect(1)
        .mount(&f.server)
        .await;

    let outcome = publish(
        &f.client,
        &f.package,
        PublishOptions {
            dry_run: true,
            ..PublishOptions::default()
        },
    )
    .await
    .unwrap();

    let PublishOutcome::DryRun { init } = outcome else {
        panic!("expected a dry run");
    };
    assert_eq!(init.upload_id, "upload-1-abcdefghij");
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn existing_version_on_init_is_reported() {
    let f = fixture().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(error(409, "version_exists"))
        .mount(&f.server)
        .await;

    let err = publish(&f.client, &f.package, PublishOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(err, Error::VersionExists), "{err:?}");
}

#[tokio::test]
async fn expired_upload_url_triggers_a_second_init() {
    let f = fixture().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_response(&f.server, "upload-1-abcdefghij", TEN_MINUTES))
        .up_to_n_times(1)
        .mount(&f.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_response(&f.server, "upload-2-abcdefghij", TEN_MINUTES))
        .mount(&f.server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(403))
        .up_to_n_times(1)
        .mount(&f.server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&f.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .and(body_json(json!({ "uploadId": "upload-2-abcdefghij" })))
        .respond_with(commit_response())
        .expect(1)
        .mount(&f.server)
        .await;

    let outcome = publish(&f.client, &f.package, PublishOptions::default())
        .await
        .unwrap();

    assert!(matches!(outcome, PublishOutcome::Published { .. }));
    assert_eq!(
        requests_to(&f.server, "POST", "/api/v1/publish/init").await,
        2
    );
}

#[tokio::test]
async fn init_is_called_at_most_twice() {
    let f = fixture().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_response(&f.server, "upload-1-abcdefghij", TEN_MINUTES))
        .mount(&f.server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&f.server)
        .await;

    let err = publish(&f.client, &f.package, PublishOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(err, Error::UploadExpired), "{err:?}");
    assert_eq!(
        requests_to(&f.server, "POST", "/api/v1/publish/init").await,
        2
    );
    assert_eq!(requests_to(&f.server, "PUT", UPLOAD_PATH).await, 2);
}

#[tokio::test]
async fn url_about_to_expire_is_renewed_before_uploading() {
    let f = fixture().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_response(
            &f.server,
            "upload-1-abcdefghij",
            Duration::from_secs(5),
        ))
        .up_to_n_times(1)
        .mount(&f.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_response(&f.server, "upload-2-abcdefghij", TEN_MINUTES))
        .mount(&f.server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&f.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .and(body_json(json!({ "uploadId": "upload-2-abcdefghij" })))
        .respond_with(commit_response())
        .expect(1)
        .mount(&f.server)
        .await;

    let outcome = publish(&f.client, &f.package, PublishOptions::default())
        .await
        .unwrap();

    assert!(matches!(outcome, PublishOutcome::Published { .. }));
}

#[tokio::test]
async fn commit_is_retried_on_server_errors() {
    let f = fixture().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_response(&f.server, "upload-1-abcdefghij", TEN_MINUTES))
        .mount(&f.server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&f.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .respond_with(error(503, "internal_error"))
        .up_to_n_times(1)
        .mount(&f.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .respond_with(commit_response())
        .mount(&f.server)
        .await;

    let outcome = publish(&f.client, &f.package, PublishOptions::default())
        .await
        .unwrap();

    assert!(matches!(outcome, PublishOutcome::Published { .. }));
    assert_eq!(
        requests_to(&f.server, "POST", "/api/v1/publish/commit").await,
        2
    );
    assert_eq!(
        requests_to(&f.server, "POST", "/api/v1/publish/init").await,
        1
    );
}

#[tokio::test]
async fn upload_mismatch_restarts_from_init_once() {
    let f = fixture().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_response(&f.server, "upload-1-abcdefghij", TEN_MINUTES))
        .mount(&f.server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&f.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .respond_with(error(409, "upload_mismatch"))
        .mount(&f.server)
        .await;

    let err = publish(&f.client, &f.package, PublishOptions::default())
        .await
        .unwrap_err();

    assert!(
        matches!(err, Error::Api { status: 409, ref code, .. } if code == "upload_mismatch"),
        "{err:?}"
    );
    assert_eq!(
        requests_to(&f.server, "POST", "/api/v1/publish/init").await,
        2
    );
    assert_eq!(
        requests_to(&f.server, "POST", "/api/v1/publish/commit").await,
        2
    );
}

#[tokio::test]
async fn incomplete_upload_restarts_from_init_once() {
    let f = fixture().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_response(&f.server, "upload-1-abcdefghij", TEN_MINUTES))
        .mount(&f.server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&f.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .respond_with(error(400, "upload_incomplete"))
        .up_to_n_times(1)
        .mount(&f.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .respond_with(commit_response())
        .mount(&f.server)
        .await;

    let outcome = publish(&f.client, &f.package, PublishOptions::default())
        .await
        .unwrap();

    assert!(matches!(outcome, PublishOutcome::Published { .. }));
    assert_eq!(
        requests_to(&f.server, "POST", "/api/v1/publish/init").await,
        2
    );
    assert_eq!(requests_to(&f.server, "PUT", UPLOAD_PATH).await, 2);
}

#[tokio::test]
async fn uploads_every_file_of_a_generic_version() {
    let f = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let mut files = Vec::new();
    for (name, os) in [("cli-linux.tar.gz", "linux"), ("cli-macos.tar.gz", "macos")] {
        let path = dir.path().join(name);
        std::fs::write(&path, name.as_bytes()).unwrap();
        files.push(
            PackageFile::from_path(&path, None, None, [("os".to_owned(), os.to_owned())].into())
                .unwrap(),
        );
    }
    let package = Package {
        eco: Ecosystem::Generic,
        name: "tools/cli".into(),
        version: "1.0.0".into(),
        files,
        manifest: json!({ "channel": "stable" }),
    };
    let expires = format_timestamp(OffsetDateTime::now_utc() + TEN_MINUTES).unwrap();
    let uploads: Vec<_> = ["cli-macos.tar.gz", "cli-linux.tar.gz"]
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "putUrl": format!("{}/bucket/{name}?X-Amz-Signature=x", f.server.uri()),
                "headers": { "content-length": name.len().to_string(), "x-amz-checksum-sha256": "x" }
            })
        })
        .collect();
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .and(body_json(json!({
            "eco": "generic",
            "name": "tools/cli",
            "version": "1.0.0",
            "files": [
                { "name": "cli-linux.tar.gz", "size": 16, "sha256": package.files[0].sha256, "labels": { "os": "linux" } },
                { "name": "cli-macos.tar.gz", "size": 16, "sha256": package.files[1].sha256, "labels": { "os": "macos" } }
            ],
            "manifest": { "channel": "stable" },
            "append": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "uploadId": "upload-generic-0001",
            "expires": expires,
            "uploads": uploads
        })))
        .expect(1)
        .mount(&f.server)
        .await;
    for name in ["cli-linux.tar.gz", "cli-macos.tar.gz"] {
        Mock::given(method("PUT"))
            .and(path(format!("/bucket/{name}")))
            .and(body_bytes(name.as_bytes()))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&f.server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "eco": "generic", "name": "tools/cli", "version": "1.0.0",
            "url": "https://packages.example.com/generic/tools/cli/1.0.0",
            "files": [
                { "name": "cli-linux.tar.gz", "url": "https://packages.example.com/generic/tools/cli/1.0.0/cli-linux.tar.gz" },
                { "name": "cli-macos.tar.gz", "url": "https://packages.example.com/generic/tools/cli/1.0.0/cli-macos.tar.gz" }
            ]
        })))
        .expect(1)
        .mount(&f.server)
        .await;

    let outcome = publish(
        &f.client,
        &package,
        PublishOptions {
            append: Some(true),
            ..PublishOptions::default()
        },
    )
    .await
    .unwrap();

    let PublishOutcome::Published { response } = outcome else {
        panic!("expected a published outcome");
    };
    assert_eq!(response.files.len(), 2);
}

#[tokio::test]
async fn upload_instructions_must_match_the_declared_files() {
    let f = fixture().await;
    let expires = format_timestamp(OffsetDateTime::now_utc() + TEN_MINUTES).unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "uploadId": "upload-1-abcdefghij",
            "expires": expires,
            "uploads": [{ "name": "other.tgz", "putUrl": "http://127.0.0.1:9/x", "headers": {} }]
        })))
        .mount(&f.server)
        .await;

    let err = publish(&f.client, &f.package, PublishOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(err, Error::InvalidInput(_)), "{err:?}");
}

#[tokio::test]
async fn duplicate_file_names_are_rejected_before_init() {
    let f = fixture().await;
    let mut package = f.package.clone();
    package.files.push(package.files[0].clone());

    let err = publish(&f.client, &package, PublishOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(err, Error::InvalidInput(_)), "{err:?}");
    assert!(f.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn version_published_meanwhile_is_reported_on_commit() {
    let f = fixture().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_response(&f.server, "upload-1-abcdefghij", TEN_MINUTES))
        .mount(&f.server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&f.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .respond_with(error(409, "version_exists"))
        .expect(1)
        .mount(&f.server)
        .await;

    let err = publish(&f.client, &f.package, PublishOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(err, Error::VersionExists), "{err:?}");
}
