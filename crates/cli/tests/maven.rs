//! `sreg maven publish` end to end: the fixture repository of `sreg-eco-maven`, a mock
//! registry and a mock storage. No Maven or Gradle is needed.

#![cfg(feature = "maven")]

mod common;

use std::path::{Path, PathBuf};

use common::{init_body, init_responder, json, sreg_in, stdout_json};
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../eco-maven/tests/fixtures/repo")
}

fn repo_arg() -> String {
    repo().to_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn publishes_the_selected_artifact_with_all_its_files() {
    let server = MockServer::start().await;
    let cwd = tempfile::tempdir().unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_responder(&server, "upload-maven-0001"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .expect(7)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .and(body_json(json!({ "uploadId": "upload-maven-0001" })))
        .respond_with(json(
            201,
            json!({
                "eco": "maven", "name": "com.acme:sdk", "version": "1.4.0",
                "url": "https://packages.example.com/maven/com/acme/sdk/1.4.0/sdk-1.4.0.jar",
                "files": [
                    {
                        "name": "sdk-1.4.0.pom",
                        "url": "https://packages.example.com/maven/com/acme/sdk/1.4.0/sdk-1.4.0.pom"
                    },
                    {
                        "name": "sdk-1.4.0.jar",
                        "url": "https://packages.example.com/maven/com/acme/sdk/1.4.0/sdk-1.4.0.jar"
                    }
                ]
            }),
        ))
        .expect(1)
        .mount(&server)
        .await;

    sreg_in(
        &server,
        cwd.path(),
        &[
            "maven",
            "publish",
            "--from-dir",
            &repo_arg(),
            "--coords",
            "com.acme:sdk",
            "--version",
            "1.4.0",
        ],
    )
    .await
    .success()
    .stdout(predicate::str::contains(
        "Published maven com.acme:sdk 1.4.0",
    ))
    .stdout(predicate::str::contains("secret-signature").not());

    let requests = server.received_requests().await.unwrap();
    let init = init_body(&requests);
    assert_eq!(init["eco"], "maven");
    assert_eq!(init["name"], "com.acme:sdk");
    assert_eq!(init["version"], "1.4.0");
    let dir = repo().join("com/acme/sdk/1.4.0");
    let pom = std::fs::read_to_string(dir.join("sdk-1.4.0.pom")).unwrap();
    assert_eq!(init["manifest"]["groupId"], "com.acme");
    assert_eq!(init["manifest"]["artifactId"], "sdk");
    assert_eq!(init["manifest"]["version"], "1.4.0");
    assert_eq!(init["manifest"]["packaging"], "jar");
    assert_eq!(init["manifest"]["pom"], pom);
    assert_eq!(
        init["manifest"]["dependencies"][1],
        json!({
            "groupId": "com.fasterxml.jackson.core", "artifactId": "jackson-databind",
            "version": null, "scope": "runtime", "optional": false
        })
    );

    let files: Vec<(&str, &str)> = init["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["name"].as_str().unwrap(),
                file["role"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        files,
        [
            ("sdk-1.4.0.pom", "pom"),
            ("sdk-1.4.0.jar", "archive"),
            ("sdk-1.4.0.module", "module"),
            ("sdk-1.4.0-sources.jar", "sources"),
            ("sdk-1.4.0-javadoc.jar", "javadoc"),
            ("sdk-1.4.0-all.jar", "extra"),
            ("sdk-1.4.0.jar.asc", "extra"),
        ]
    );
    for file in init["files"].as_array().unwrap() {
        let name = file["name"].as_str().unwrap();
        let put = requests
            .iter()
            .find(|request| {
                request.method.as_str() == "PUT" && request.url.path() == format!("/bucket/{name}")
            })
            .unwrap_or_else(|| panic!("no upload of {name}"));
        assert_eq!(put.body, std::fs::read(dir.join(name)).unwrap(), "{name}");
        assert_eq!(file["sha256"], common::sha256_hex(&put.body), "{name}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn dry_run_of_the_only_version_of_an_aar() {
    let server = MockServer::start().await;
    let cwd = tempfile::tempdir().unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_responder(&server, "upload-maven-0002"))
        .expect(1)
        .mount(&server)
        .await;

    let assert = sreg_in(
        &server,
        cwd.path(),
        &[
            "maven",
            "publish",
            "--from-dir",
            &repo_arg(),
            "--coords",
            "com.acme.android:widgets",
            "--dry-run",
            "--json",
        ],
    )
    .await
    .success();

    let report = stdout_json(&assert);
    assert_eq!(report["dryRun"], true);
    assert_eq!(report["name"], "com.acme.android:widgets");
    assert_eq!(report["version"], "2.0.0");
    assert_eq!(report["files"][1]["name"], "widgets-2.0.0.aar");
    assert_eq!(report["files"][1]["role"], "archive");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(init_body(&requests)["manifest"]["packaging"], "aar");
    assert!(requests
        .iter()
        .all(|request| request.method.as_str() == "POST"));
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_selections_stop_before_contacting_the_registry() {
    let server = MockServer::start().await;
    let cwd = tempfile::tempdir().unwrap();
    let repo = repo_arg();
    let cases: [(&[&str], &str); 5] = [
        (
            &["--version", "1.4.0"],
            "several artifacts (com.acme.android:widgets, com.acme:sdk)",
        ),
        (
            &["--coords", "com.acme:sdk"],
            "several versions of com.acme:sdk",
        ),
        (
            &["--coords", "com.acme:sdk", "--version", "1.0.0-SNAPSHOT"],
            "is a snapshot",
        ),
        (
            &[
                "--coords",
                "com.acme:sdk",
                "--version",
                "1.4.0",
                "--",
                "-DskipTests",
            ],
            "arguments after `--`",
        ),
        (&["--coords", "acme"], "invalid --coords"),
    ];
    for (extra, message) in cases {
        let mut args = vec!["maven", "publish", "--from-dir", repo.as_str()];
        args.extend_from_slice(extra);
        sreg_in(&server, cwd.path(), &args)
            .await
            .code(1)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains(
                "failed to package the maven project",
            ))
            .stderr(predicate::str::contains(message));
    }
    let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../eco-maven/tests/fixtures/parent");
    sreg_in(
        &server,
        cwd.path(),
        &["maven", "publish", "--from-dir", parent.to_str().unwrap()],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("mvn help:effective-pom"));

    assert!(server.received_requests().await.unwrap().is_empty());
}
