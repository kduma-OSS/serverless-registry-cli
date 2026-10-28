//! `sreg composer publish` end to end: a real git repository with the fixture library,
//! a mock registry and a mock storage.

#![cfg(feature = "composer")]

mod common;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{json, sreg, sreg_with, stdout_json, TOKEN};
use predicates::prelude::*;
use serde_json::{json, Value};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../eco-composer/tests/fixtures/library")
}

fn fixture_manifest() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture().join("composer.json")).unwrap())
        .unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=sreg tests",
            "-c",
            "user.email=tests@example.com",
        ])
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The fixture library committed to a fresh repository, tagged with `tag` if given.
fn repository(tag: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), dir.path());
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "release"]);
    if let Some(tag) = tag {
        git(dir.path(), &["tag", tag]);
    }
    dir
}

/// Answers `init` with one upload per declared file, pointing at `/bucket/<name>`.
fn init_responder(server: &MockServer) -> impl Fn(&Request) -> ResponseTemplate {
    let uri = server.uri();
    move |request: &Request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let uploads: Vec<Value> = body["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| {
                let name = file["name"].as_str().unwrap();
                json!({
                    "name": name,
                    "putUrl": format!("{uri}/bucket/{name}?X-Amz-Signature=secret-signature"),
                    "headers": {
                        "content-length": file["size"].to_string(),
                        "x-amz-checksum-sha256": "x"
                    }
                })
            })
            .collect();
        let expires = time::OffsetDateTime::now_utc() + time::Duration::minutes(10);
        let expires = expires
            .replace_nanosecond(0)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        json(
            200,
            json!({ "uploadId": "upload-composer-0001", "expires": expires, "uploads": uploads }),
        )
    }
}

fn init_body(requests: &[Request]) -> Value {
    let init = requests
        .iter()
        .find(|request| request.url.path() == "/api/v1/publish/init")
        .expect("an init request");
    serde_json::from_slice(&init.body).unwrap()
}

fn zip_names(bytes: &[u8]) -> Vec<String> {
    let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    archive.file_names().map(str::to_owned).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn publishes_the_tagged_library() {
    let server = MockServer::start().await;
    let repo = repository(Some("v1.2.3"));
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(init_responder(&server))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/bucket/sdk-1.2.3.zip"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .and(body_json(json!({ "uploadId": "upload-composer-0001" })))
        .respond_with(json(
            201,
            json!({
                "eco": "composer", "name": "acme/sdk", "version": "1.2.3",
                "url": "https://packages.example.com/composer/dist/acme/sdk/1.2.3.zip",
                "files": [{
                    "name": "sdk-1.2.3.zip",
                    "url": "https://packages.example.com/composer/dist/acme/sdk/1.2.3.zip"
                }]
            }),
        ))
        .expect(1)
        .mount(&server)
        .await;

    sreg(
        &server,
        &[
            "composer",
            "publish",
            "--path",
            repo.path().to_str().unwrap(),
        ],
    )
    .await
    .success()
    .stdout(predicate::str::contains(
        "Published composer acme/sdk 1.2.3",
    ))
    .stdout(predicate::str::contains("secret-signature").not());

    let requests = server.received_requests().await.unwrap();
    let init = init_body(&requests);
    assert_eq!(init["eco"], "composer");
    assert_eq!(init["name"], "acme/sdk");
    assert_eq!(init["version"], "1.2.3");
    assert_eq!(init["manifest"], fixture_manifest());
    assert!(init.get("tag").is_none() && init.get("append").is_none());
    let files = init["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["name"], "sdk-1.2.3.zip");
    assert_eq!(files[0]["role"], "archive");
    assert!(files[0].get("labels").is_none());

    // The uploaded bytes are the declared archive.
    let put = requests
        .iter()
        .find(|request| request.method.as_str() == "PUT")
        .unwrap();
    assert_eq!(files[0]["size"], put.body.len());
    let scratch = tempfile::tempdir().unwrap();
    let uploaded = scratch.path().join("uploaded.zip");
    std::fs::write(&uploaded, &put.body).unwrap();
    let (sha256, _) = sreg_core::digest::sha256_file(&uploaded).unwrap();
    assert_eq!(files[0]["sha256"], sha256);
    let names = zip_names(&put.body);
    assert!(
        names.iter().any(|name| name == "src/Client.php"),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|name| name.starts_with("tests")),
        "{names:?}"
    );
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&put.body)).unwrap();
    let mut composer_json = String::new();
    archive
        .by_name("composer.json")
        .unwrap()
        .read_to_string(&mut composer_json)
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&composer_json).unwrap(),
        fixture_manifest()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dry_run_with_an_explicit_version_stops_after_init() {
    let server = MockServer::start().await;
    let repo = repository(None);
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_responder(&server))
        .expect(1)
        .mount(&server)
        .await;

    let assert = sreg(
        &server,
        &[
            "composer",
            "publish",
            "--path",
            repo.path().to_str().unwrap(),
            "--version",
            "2.0.0-rc1",
            "--dry-run",
            "--json",
        ],
    )
    .await
    .success();

    let report = stdout_json(&assert);
    assert_eq!(report["dryRun"], true);
    assert_eq!(report["eco"], "composer");
    assert_eq!(report["version"], "2.0.0-rc1");
    assert_eq!(report["files"][0]["name"], "sdk-2.0.0-rc1.zip");
    assert_eq!(report["files"][0]["role"], "archive");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(init_body(&requests)["version"], "2.0.0-rc1");
}

#[tokio::test(flavor = "multi_thread")]
async fn oidc_publishes_with_a_github_token() {
    let server = MockServer::start().await;
    let repo = repository(Some("v1.2.3"));
    Mock::given(method("GET"))
        .and(path("/oidc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": "eyJ.oidc.sig" })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .and(header("authorization", "Bearer ci-acme:gh.eyJ.oidc.sig"))
        .respond_with(init_responder(&server))
        .expect(1)
        .mount(&server)
        .await;
    let request_url = format!("{}/oidc?api-version=2.0", server.uri());

    sreg_with(
        &server,
        &[
            "composer",
            "publish",
            "--path",
            repo.path().to_str().unwrap(),
            "--oidc",
            "--oidc-project",
            "ci-acme",
            "--dry-run",
        ],
        &[
            ("ACTIONS_ID_TOKEN_REQUEST_URL", request_url.as_str()),
            ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "request-token"),
        ],
    )
    .await
    .success()
    .stdout(predicate::str::contains("Dry run"))
    .stdout(predicate::str::contains("sdk-1.2.3.zip"));
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_versions_stop_before_contacting_the_registry() {
    let server = MockServer::start().await;
    let repo = repository(None);
    let dir = repo.path().to_str().unwrap();

    sreg(&server, &["composer", "publish", "--path", dir])
        .await
        .code(1)
        .stderr(predicate::str::contains("HEAD has no tag"));
    sreg(
        &server,
        &[
            "composer",
            "publish",
            "--path",
            dir,
            "--version",
            "dev-main",
        ],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("development version"));
    sreg(
        &server,
        &[
            "composer",
            "publish",
            "--path",
            dir,
            "--version",
            "1.0.0-preview",
        ],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("invalid version"));

    assert!(server.received_requests().await.unwrap().is_empty());
}
