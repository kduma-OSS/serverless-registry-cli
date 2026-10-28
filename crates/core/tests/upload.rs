//! Tests of the presigned archive upload against a local mock server.

use std::collections::BTreeMap;
use std::io::Write;
use std::time::{Duration, SystemTime};

use sreg_core::retry::RetryPolicy;
use sreg_core::upload::Uploader;
use sreg_core::Error;
use wiremock::matchers::{body_bytes, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CONTENT: &[u8] = b"archive bytes for the upload test";

fn archive() -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(CONTENT).unwrap();
    file
}

fn headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("content-length".to_owned(), CONTENT.len().to_string()),
        ("x-amz-checksum-sha256".to_owned(), "c2hhMjU2".to_owned()),
    ])
}

fn uploader() -> Uploader {
    Uploader::new(RetryPolicy {
        attempts: 3,
        base_delay: Duration::from_millis(1),
    })
    .unwrap()
}

fn put_url(server: &MockServer) -> String {
    format!(
        "{}/bucket/_staging/01JBX9T6Q4ZK3M8N2P5R7S9V0W/sdk-2.3.1.tgz?X-Amz-Signature=sig",
        server.uri()
    )
}

const UPLOAD_PATH: &str = "/bucket/_staging/01JBX9T6Q4ZK3M8N2P5R7S9V0W/sdk-2.3.1.tgz";

#[tokio::test]
async fn uploads_with_exact_headers_and_fixed_length() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(UPLOAD_PATH))
        .and(query_param("X-Amz-Signature", "sig"))
        .and(header("content-length", CONTENT.len().to_string().as_str()))
        .and(header("x-amz-checksum-sha256", "c2hhMjU2"))
        .and(body_bytes(CONTENT))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let file = archive();

    uploader()
        .upload(
            &put_url(&server),
            &headers(),
            file.path(),
            CONTENT.len() as u64,
            None,
        )
        .await
        .unwrap();

    let request = &server.received_requests().await.unwrap()[0];
    for forbidden in [
        "transfer-encoding",
        "content-type",
        "authorization",
        "user-agent",
        "x-client-version",
    ] {
        assert!(
            request.headers.get(forbidden).is_none(),
            "unexpected header {forbidden}"
        );
    }
    let amz: Vec<_> = request
        .headers
        .keys()
        .filter(|name| name.as_str().starts_with("x-amz-"))
        .collect();
    assert_eq!(amz, ["x-amz-checksum-sha256"]);
}

#[tokio::test]
async fn retries_after_a_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(body_bytes(CONTENT))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let file = archive();

    uploader()
        .upload(
            &put_url(&server),
            &headers(),
            file.path(),
            CONTENT.len() as u64,
            None,
        )
        .await
        .unwrap();

    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn gives_up_after_three_attempts() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(500))
        .expect(3)
        .mount(&server)
        .await;
    let file = archive();

    let err = uploader()
        .upload(
            &put_url(&server),
            &headers(),
            file.path(),
            CONTENT.len() as u64,
            None,
        )
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Upload { status: 500, .. }), "{err:?}");
}

#[tokio::test]
async fn forbidden_is_reported_as_expiry_without_retry() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(403).set_body_string(
            "<Error><Code>AccessDenied</Code><Message>Request has expired</Message></Error>",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let file = archive();

    let err = uploader()
        .upload(
            &put_url(&server),
            &headers(),
            file.path(),
            CONTENT.len() as u64,
            None,
        )
        .await
        .unwrap_err();

    assert!(matches!(err, Error::UploadExpired), "{err:?}");
}

#[tokio::test]
async fn other_client_errors_are_fatal() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_string("<Error><Code>BadDigest</Code><Message>x</Message></Error>"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let file = archive();

    let err = uploader()
        .upload(
            &put_url(&server),
            &headers(),
            file.path(),
            CONTENT.len() as u64,
            None,
        )
        .await
        .unwrap_err();

    assert!(
        matches!(err, Error::Upload { status: 400, ref code } if code == "BadDigest"),
        "{err:?}"
    );
}

#[tokio::test]
async fn does_not_start_an_attempt_when_the_url_is_about_to_expire() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let file = archive();

    let err = uploader()
        .upload(
            &put_url(&server),
            &headers(),
            file.path(),
            CONTENT.len() as u64,
            Some(SystemTime::now() + Duration::from_secs(10)),
        )
        .await
        .unwrap_err();

    assert!(matches!(err, Error::UploadExpired), "{err:?}");
}

#[tokio::test]
async fn transport_errors_do_not_reveal_the_presigned_url() {
    // Nothing listens on a port that was bound and released again.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let uri = format!("http://127.0.0.1:{port}{UPLOAD_PATH}?X-Amz-Signature=sig");
    let file = archive();

    let err = uploader()
        .upload(&uri, &headers(), file.path(), CONTENT.len() as u64, None)
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Http(_)), "{err:?}");
    assert!(!err.to_string().contains("X-Amz-Signature"), "{err}");
}

#[tokio::test]
async fn archive_size_must_match_the_declaration() {
    let server = MockServer::start().await;
    let file = archive();

    let err = uploader()
        .upload(
            &put_url(&server),
            &headers(),
            file.path(),
            CONTENT.len() as u64 + 1,
            None,
        )
        .await
        .unwrap_err();

    assert!(matches!(err, Error::InvalidInput(_)), "{err:?}");
}
