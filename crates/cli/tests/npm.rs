//! `sreg npm publish` end to end: the real `npm pack` on the fixture package, a mock
//! registry and a mock storage. Skipped when npm is not installed.

#![cfg(feature = "npm")]

mod common;

use std::io::Read;
use std::path::{Path, PathBuf};

use common::{copy_dir, init_body, init_responder, json, sreg_in, stdout_json, tool_available};
use predicates::prelude::*;
use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../eco-npm/tests/fixtures/demo")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// `package/package.json` of a gzipped npm tarball.
fn tarball_manifest(bytes: &[u8]) -> Value {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap().to_str() == Some("package/package.json") {
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            return serde_json::from_str(&text).unwrap();
        }
    }
    panic!("the tarball has no package/package.json");
}

#[tokio::test(flavor = "multi_thread")]
async fn publishes_the_package_with_the_requested_version_and_tag() {
    if !tool_available("npm") {
        return;
    }
    let server = MockServer::start().await;
    let project = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), project.path());
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_responder(&server, "upload-npm-0001"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/bucket/acme-demo-1.2.3.tgz"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .and(body_json(json!({ "uploadId": "upload-npm-0001" })))
        .respond_with(json(
            201,
            json!({
                "eco": "npm", "name": "@acme/demo", "version": "1.2.3",
                "url": "https://packages.example.com/npm/@acme/demo/-/demo-1.2.3.tgz",
                "files": [{
                    "name": "acme-demo-1.2.3.tgz",
                    "url": "https://packages.example.com/npm/@acme/demo/-/demo-1.2.3.tgz"
                }]
            }),
        ))
        .expect(1)
        .mount(&server)
        .await;

    sreg_in(
        &server,
        project.path(),
        &["npm", "publish", "--version", "1.2.3", "--tag", "beta"],
    )
    .await
    .success()
    .stdout(predicate::str::contains("Published npm @acme/demo 1.2.3"))
    .stdout(predicate::str::contains("secret-signature").not())
    .stderr(predicate::str::contains("note: set the version in"));

    let mut expected = read_json(&fixture().join("package.json"));
    expected["version"] = json!("1.2.3");
    assert_eq!(read_json(&project.path().join("package.json")), expected);

    let requests = server.received_requests().await.unwrap();
    let init = init_body(&requests);
    assert_eq!(init["eco"], "npm");
    assert_eq!(init["name"], "@acme/demo");
    assert_eq!(init["version"], "1.2.3");
    assert_eq!(init["tag"], "beta");
    assert_eq!(init["manifest"], expected);
    let files = init["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["name"], "acme-demo-1.2.3.tgz");
    assert_eq!(files[0]["role"], "archive");

    let put = requests
        .iter()
        .find(|request| request.method.as_str() == "PUT")
        .unwrap();
    assert_eq!(files[0]["size"], put.body.len());
    assert_eq!(files[0]["sha256"], common::sha256_hex(&put.body));
    assert_eq!(tarball_manifest(&put.body), expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn publishes_a_workspace_from_the_repository_root() {
    if !tool_available("npm") {
        return;
    }
    let server = MockServer::start().await;
    let root = tempfile::tempdir().unwrap();
    let root_manifest = json!({
        "name": "acme-monorepo",
        "private": true,
        "workspaces": ["packages/*"]
    });
    std::fs::write(
        root.path().join("package.json"),
        serde_json::to_string_pretty(&root_manifest).unwrap(),
    )
    .unwrap();
    copy_dir(&fixture(), &root.path().join("packages/demo"));
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_responder(&server, "upload-npm-0002"))
        .expect(1)
        .mount(&server)
        .await;

    let assert = sreg_in(
        &server,
        root.path(),
        &[
            "npm",
            "publish",
            "--workspace",
            "packages/demo",
            "--version",
            "2.0.0-rc.1",
            "--dry-run",
            "--json",
        ],
    )
    .await
    .success()
    .stderr(predicate::str::contains("workspace `packages/demo`"));

    let report = stdout_json(&assert);
    assert_eq!(report["dryRun"], true);
    assert_eq!(report["name"], "@acme/demo");
    assert_eq!(report["version"], "2.0.0-rc.1");
    assert_eq!(report["files"][0]["name"], "acme-demo-2.0.0-rc.1.tgz");

    let init = init_body(&server.received_requests().await.unwrap());
    assert_eq!(init["version"], "2.0.0-rc.1");
    // Without --tag the registry picks the dist-tag (`next` for a prerelease).
    assert!(init.get("tag").is_none());
    assert_eq!(
        read_json(&root.path().join("packages/demo/package.json"))["version"],
        "2.0.0-rc.1"
    );
    assert_eq!(read_json(&root.path().join("package.json")), root_manifest);
}

#[tokio::test(flavor = "multi_thread")]
async fn reserved_npm_flags_are_rejected() {
    let server = MockServer::start().await;
    let project = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), project.path());

    sreg_in(
        &server,
        project.path(),
        &[
            "npm",
            "publish",
            "--version",
            "1.2.3",
            "--",
            "--pack-destination",
            "/tmp",
        ],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains(
        "`--pack-destination` cannot be passed to `npm pack`",
    ));

    assert_eq!(
        read_json(&project.path().join("package.json")),
        read_json(&fixture().join("package.json"))
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}
