//! `sreg cargo publish` end to end: the real `cargo package` on fixture crates without
//! dependencies, a mock registry and a mock storage. Skipped when cargo is missing.

#![cfg(feature = "cargo")]

mod common;

use std::path::{Path, PathBuf};

use common::{copy_dir, init_body, init_responder, json, sreg_in, stdout_json, tool_available};
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../eco-cargo/tests/fixtures/demo")
}

/// A workspace with `acme-a` (version inherited from the workspace) and `acme-b`.
fn workspace() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let write = |file: &str, text: &str| {
        let path = root.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "Cargo.toml",
        "[workspace]\nresolver = \"2\"\nmembers = [\"a\", \"b\"]\n\n\
         [workspace.package]\nversion = \"0.3.0\"\n",
    );
    write(
        "a/Cargo.toml",
        "[package]\nname = \"acme-a\"\nversion.workspace = true\nedition = \"2021\"\n\
         description = \"a\"\nlicense = \"MIT\"\n",
    );
    write("a/src/lib.rs", "");
    write(
        "b/Cargo.toml",
        "[package]\nname = \"acme-b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         description = \"b\"\nlicense = \"MIT\"\n",
    );
    write("b/src/lib.rs", "");
    root
}

#[tokio::test(flavor = "multi_thread")]
async fn publishes_the_crate_with_the_requested_version() {
    if !tool_available("cargo") {
        return;
    }
    let server = MockServer::start().await;
    let project = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), project.path());
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_responder(&server, "upload-cargo-0001"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/bucket/acme-demo-1.2.3.crate"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .and(body_json(json!({ "uploadId": "upload-cargo-0001" })))
        .respond_with(json(
            201,
            json!({
                "eco": "cargo", "name": "acme-demo", "version": "1.2.3",
                "url": "https://packages.example.com/cargo/api/v1/crates/acme-demo/1.2.3/download",
                "files": [{
                    "name": "acme-demo-1.2.3.crate",
                    "url": "https://packages.example.com/cargo/api/v1/crates/acme-demo/1.2.3/download"
                }]
            }),
        ))
        .expect(1)
        .mount(&server)
        .await;

    sreg_in(
        &server,
        &project.path().join("src"),
        &["cargo", "publish", "--version", "1.2.3", "--", "--quiet"],
    )
    .await
    .success()
    .stdout(predicate::str::contains("Published cargo acme-demo 1.2.3"))
    .stderr(predicate::str::contains("note: set the version in"));

    let manifest = std::fs::read_to_string(project.path().join("Cargo.toml")).unwrap();
    assert!(
        manifest.contains("version = \"1.2.3\" # bumped by the release workflow"),
        "{manifest}"
    );

    let requests = server.received_requests().await.unwrap();
    let init = init_body(&requests);
    assert_eq!(init["eco"], "cargo");
    assert_eq!(init["name"], "acme-demo");
    assert_eq!(init["version"], "1.2.3");
    assert!(init.get("tag").is_none());
    assert_eq!(
        init["manifest"],
        json!({
            "name": "acme-demo",
            "vers": "1.2.3",
            "deps": [],
            "features": { "default": ["std"], "extra": ["std"], "std": [] },
            "links": null,
            "rust_version": "1.70"
        })
    );
    let files = init["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["name"], "acme-demo-1.2.3.crate");
    assert_eq!(files[0]["role"], "archive");
    let put = requests
        .iter()
        .find(|request| request.method.as_str() == "PUT")
        .unwrap();
    assert_eq!(files[0]["size"], put.body.len());
    assert_eq!(files[0]["sha256"], common::sha256_hex(&put.body));
}

#[tokio::test(flavor = "multi_thread")]
async fn package_selects_a_workspace_member() {
    if !tool_available("cargo") {
        return;
    }
    let server = MockServer::start().await;
    let root = workspace();
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_responder(&server, "upload-cargo-0002"))
        .expect(1)
        .mount(&server)
        .await;

    let assert = sreg_in(
        &server,
        root.path(),
        &[
            "cargo",
            "publish",
            "--package",
            "acme-b",
            "--version",
            "0.2.0",
            "--dry-run",
            "--json",
        ],
    )
    .await
    .success();

    let report = stdout_json(&assert);
    assert_eq!(report["name"], "acme-b");
    assert_eq!(report["version"], "0.2.0");
    assert_eq!(report["files"][0]["name"], "acme-b-0.2.0.crate");
    let init = init_body(&server.received_requests().await.unwrap());
    assert_eq!(init["manifest"]["vers"], "0.2.0");
    let manifest = std::fs::read_to_string(root.path().join("b/Cargo.toml")).unwrap();
    assert!(manifest.contains("version = \"0.2.0\""), "{manifest}");
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_selection_and_inherited_versions_are_checked() {
    if !tool_available("cargo") {
        return;
    }
    let server = MockServer::start().await;
    let root = workspace();

    sreg_in(
        &server,
        root.path(),
        &["cargo", "publish", "--version", "1.0.0"],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("select one with --package"));

    sreg_in(
        &server,
        root.path(),
        &[
            "cargo",
            "publish",
            "--package",
            "acme-a",
            "--version",
            "0.4.0",
        ],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("version.workspace = true"))
    .stderr(predicate::str::contains("[workspace.package]"));

    sreg_in(
        &server,
        root.path(),
        &[
            "cargo",
            "publish",
            "--package",
            "acme-a",
            "--",
            "-p",
            "acme-b",
        ],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains(
        "cannot be passed to `cargo package`",
    ));

    assert!(server.received_requests().await.unwrap().is_empty());
}
