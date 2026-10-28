//! `sreg generic publish` and `sreg generic get` against a mock registry.

#![cfg(feature = "generic")]

mod common;

use std::path::Path;

use common::{error, json, sreg, stdout_json, TOKEN};
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{body_bytes, body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
const WORLD_SHA256: &str = "486ea46224d1bb4fb680f34f7c9ad96a8f24ec88be73ea8e5a6c65260e9cb8a7";

fn files(dir: &Path) -> (String, String) {
    let linux = dir.join("cli-linux.tar.gz");
    let macos = dir.join("cli-macos.tar.gz");
    std::fs::write(&linux, b"hello").unwrap();
    std::fs::write(&macos, b"world").unwrap();
    (
        linux.to_str().unwrap().to_owned(),
        macos.to_str().unwrap().to_owned(),
    )
}

fn init_response(server: &MockServer) -> ResponseTemplate {
    let expires = time::OffsetDateTime::now_utc() + time::Duration::minutes(10);
    let expires = expires
        .replace_nanosecond(0)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let uploads: Vec<serde_json::Value> = ["cli-linux.tar.gz", "cli-macos.tar.gz"]
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "putUrl": format!("{}/bucket/{name}?X-Amz-Signature=secret-signature", server.uri()),
                "headers": { "content-length": "5", "x-amz-checksum-sha256": "x" }
            })
        })
        .collect();
    json(
        200,
        json!({ "uploadId": "upload-generic-0001", "expires": expires, "uploads": uploads }),
    )
}

fn commit_response(server: &MockServer) -> ResponseTemplate {
    let base = format!("{}/generic/tools%2Fcli/1.0.0", server.uri());
    json(
        201,
        json!({
            "eco": "generic", "name": "tools/cli", "version": "1.0.0", "url": base,
            "files": [
                { "name": "cli-linux.tar.gz", "url": format!("{base}/cli-linux.tar.gz") },
                { "name": "cli-macos.tar.gz", "url": format!("{base}/cli-macos.tar.gz") }
            ]
        }),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn publishes_files_with_labels_and_meta() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (linux, macos) = files(dir.path());
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(body_json(json!({
            "eco": "generic",
            "name": "tools/cli",
            "version": "1.0.0",
            "files": [
                { "name": "cli-linux.tar.gz", "size": 5, "sha256": HELLO_SHA256, "labels": { "channel": "stable" } },
                { "name": "cli-macos.tar.gz", "size": 5, "sha256": WORLD_SHA256, "labels": { "channel": "stable" } }
            ],
            "manifest": { "commit": "abc123", "notes": "first" },
            "append": true
        })))
        .respond_with(init_response(&server))
        .expect(1)
        .mount(&server)
        .await;
    for (name, body) in [
        ("cli-linux.tar.gz", b"hello"),
        ("cli-macos.tar.gz", b"world"),
    ] {
        Mock::given(method("PUT"))
            .and(path(format!("/bucket/{name}")))
            .and(body_bytes(body.to_vec()))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .and(body_json(json!({ "uploadId": "upload-generic-0001" })))
        .respond_with(commit_response(&server))
        .expect(1)
        .mount(&server)
        .await;
    let meta = dir.path().join("meta.json");
    std::fs::write(&meta, r#"{ "notes": "first", "commit": "overridden" }"#).unwrap();

    sreg(
        &server,
        &[
            "generic",
            "publish",
            "tools/cli",
            "1.0.0",
            &linux,
            &macos,
            "--label",
            "channel=stable",
            "--meta-file",
            meta.to_str().unwrap(),
            "--meta",
            "commit=abc123",
        ],
    )
    .await
    .success()
    .stdout(predicate::str::contains(
        "Published generic tools/cli 1.0.0",
    ))
    .stdout(predicate::str::contains("cli-macos.tar.gz"))
    .stdout(predicate::str::contains("secret-signature").not());
}

#[tokio::test(flavor = "multi_thread")]
async fn dry_run_json_lists_files_without_presigned_urls() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (linux, macos) = files(dir.path());
    let spec = dir.path().join("files.json");
    std::fs::write(
        &spec,
        serde_json::to_string(&json!([
            { "path": linux, "labels": { "os": "linux" } },
            { "path": macos, "labels": { "os": "macos" } }
        ]))
        .unwrap(),
    )
    .unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_response(&server))
        .expect(1)
        .mount(&server)
        .await;

    let assert = sreg(
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
            "--json",
        ],
    )
    .await
    .success();

    let body = stdout_json(&assert);
    assert_eq!(body["dryRun"], true);
    assert_eq!(body["uploadId"], "upload-generic-0001");
    assert_eq!(body["files"][1]["labels"]["os"], "macos");
    assert!(!String::from_utf8_lossy(&assert.get_output().stdout).contains("secret-signature"));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let init: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(init["append"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn existing_version_exits_with_two() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (linux, _) = files(dir.path());
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(error(
            409,
            "version_exists",
            "tools/cli 1.0.0 already exists",
        ))
        .mount(&server)
        .await;

    sreg(
        &server,
        &["generic", "publish", "tools/cli", "1.0.0", &linux],
    )
    .await
    .code(2)
    .stdout(predicate::str::is_empty())
    .stderr(predicate::str::contains("already exists"));
}

#[tokio::test(flavor = "multi_thread")]
async fn existing_file_exits_with_two() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (linux, _) = files(dir.path());
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(error(
            409,
            "file_exists",
            "cli-linux.tar.gz already exists in 1.0.0",
        ))
        .mount(&server)
        .await;

    sreg(
        &server,
        &["generic", "publish", "tools/cli", "1.0.0", &linux],
    )
    .await
    .code(2)
    .stderr(predicate::str::contains("cli-linux.tar.gz already exists"));
}

#[tokio::test(flavor = "multi_thread")]
async fn skip_existing_turns_an_existing_file_into_success() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (linux, _) = files(dir.path());
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(error(
            409,
            "file_exists",
            "cli-linux.tar.gz already exists in 1.0.0",
        ))
        .expect(2)
        .mount(&server)
        .await;

    sreg(
        &server,
        &[
            "generic",
            "publish",
            "tools/cli",
            "1.0.0",
            &linux,
            "--skip-existing",
        ],
    )
    .await
    .success()
    .stdout(predicate::str::contains(
        "generic tools/cli 1.0.0: version already published, skipping",
    ));

    let assert = sreg(
        &server,
        &[
            "generic",
            "publish",
            "tools/cli",
            "1.0.0",
            &linux,
            "--skip-existing",
            "--json",
        ],
    )
    .await
    .success();
    assert_eq!(
        stdout_json(&assert),
        json!({ "skipped": true, "eco": "generic", "name": "tools/cli", "version": "1.0.0" })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn skip_existing_does_not_hide_other_errors() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (linux, _) = files(dir.path());
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(error(409, "version_closed", "the append window has closed"))
        .mount(&server)
        .await;

    sreg(
        &server,
        &[
            "generic",
            "publish",
            "tools/cli",
            "1.0.0",
            &linux,
            "--skip-existing",
        ],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("append window"));
}

#[tokio::test(flavor = "multi_thread")]
async fn client_too_old_on_publish_exits_with_three() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (linux, _) = files(dir.path());
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(
            error(426, "client_too_old", "upgrade").insert_header("x-min-client-version", "7.0.0"),
        )
        .mount(&server)
        .await;

    sreg(
        &server,
        &["generic", "publish", "tools/cli", "1.0.0", &linux],
    )
    .await
    .code(3)
    .stderr(predicate::str::contains("7.0.0"))
    .stderr(predicate::str::contains(
        "https://github.com/kduma-OSS/serverless-registry-cli/releases",
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_input_fails_before_contacting_the_registry() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (linux, _) = files(dir.path());
    std::fs::create_dir(dir.path().join("copy")).unwrap();
    let copy = dir.path().join("copy").join("cli-linux.tar.gz");
    std::fs::write(&copy, b"again").unwrap();

    for args in [
        vec![
            "generic",
            "publish",
            "tools/cli",
            "1.0.0",
            &linux,
            copy.to_str().unwrap(),
        ],
        vec!["generic", "publish", "Tools", "1.0.0", &linux],
        vec!["generic", "publish", "tools/cli", "v1.0.0", &linux],
        vec![
            "generic",
            "publish",
            "tools/cli",
            "1.0.0",
            &linux,
            "--label",
            "os",
        ],
    ] {
        sreg(&server, &args).await.code(1);
    }

    assert!(server.received_requests().await.unwrap().is_empty());
}

async fn feed_server(storage_body: &'static [u8]) -> (MockServer, MockServer) {
    let registry = MockServer::start().await;
    let storage = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/generic/tools%2Fcli/latest"))
        .and(query_param("label", "os:linux"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(json(
            200,
            json!({
                "name": "tools/cli", "version": "1.0.0", "published": "2026-10-28T10:12:00Z",
                "yanked": false, "meta": {},
                "files": [{
                    "name": "cli-linux.tar.gz", "size": 5, "sha256": HELLO_SHA256,
                    "labels": { "os": "linux" },
                    "url": format!("{}/generic/tools%2Fcli/1.0.0/cli-linux.tar.gz", registry.uri())
                }]
            }),
        ))
        .mount(&registry)
        .await;
    Mock::given(method("GET"))
        .and(path("/generic/tools%2Fcli/1.0.0/cli-linux.tar.gz"))
        .respond_with(
            ResponseTemplate::new(302).insert_header(
                "location",
                format!(
                    "{}/bucket/cli-linux.tar.gz?X-Amz-Signature=s",
                    storage.uri()
                )
                .as_str(),
            ),
        )
        .mount(&registry)
        .await;
    Mock::given(method("GET"))
        .and(path("/bucket/cli-linux.tar.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(storage_body))
        .mount(&storage)
        .await;
    (registry, storage)
}

#[tokio::test(flavor = "multi_thread")]
async fn get_downloads_and_verifies_the_files() {
    let (registry, storage) = feed_server(b"hello").await;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("downloads");

    sreg(
        &registry,
        &[
            "generic",
            "get",
            "tools/cli",
            "--label",
            "os=linux",
            "--out",
            out.to_str().unwrap(),
        ],
    )
    .await
    .success()
    .stdout(predicate::str::contains("cli-linux.tar.gz"));

    assert_eq!(
        std::fs::read(out.join("cli-linux.tar.gz")).unwrap(),
        b"hello"
    );
    let storage_requests = storage.received_requests().await.unwrap();
    assert!(storage_requests[0].headers.get("authorization").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn get_json_prints_the_feed_without_downloading() {
    let (registry, storage) = feed_server(b"hello").await;

    let assert = sreg(
        &registry,
        &[
            "generic",
            "get",
            "tools/cli",
            "latest",
            "--label",
            "os=linux",
            "--json",
        ],
    )
    .await
    .success();

    assert_eq!(stdout_json(&assert)["files"][0]["sha256"], HELLO_SHA256);
    assert!(storage.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn get_fails_on_checksum_mismatch() {
    let (registry, _storage) = feed_server(b"HELLO").await;
    let dir = tempfile::tempdir().unwrap();

    sreg(
        &registry,
        &[
            "generic",
            "get",
            "tools/cli",
            "--label",
            "os=linux",
            "--out",
            dir.path().to_str().unwrap(),
        ],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("corrupt"));

    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn get_rejects_prerelease_with_an_explicit_version() {
    let server = MockServer::start().await;

    sreg(
        &server,
        &["generic", "get", "tools/cli", "1.0.0", "--prerelease"],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("--prerelease"));
}

/// A registry resolving `^1.2` (with prereleases) to `1.4.0-rc.1`, and its storage.
async fn constraint_server() -> (MockServer, MockServer) {
    let registry = MockServer::start().await;
    let storage = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/generic/tools%2Fcli/latest"))
        .and(query_param("version", "^1.2"))
        .and(query_param("channel", "prerelease"))
        .and(query_param("label", "os:linux"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(json(
            200,
            json!({
                "name": "tools/cli", "version": "1.4.0-rc.1", "published": "2026-10-28T10:12:00Z",
                "yanked": false, "meta": {},
                "files": [{
                    "name": "cli-linux.tar.gz", "size": 5, "sha256": HELLO_SHA256,
                    "labels": { "os": "linux" },
                    "url": format!("{}/generic/tools%2Fcli/1.4.0-rc.1/cli-linux.tar.gz", registry.uri())
                }]
            }),
        ))
        .expect(1)
        .mount(&registry)
        .await;
    Mock::given(method("GET"))
        .and(path("/generic/tools%2Fcli/1.4.0-rc.1/cli-linux.tar.gz"))
        .respond_with(
            ResponseTemplate::new(302).insert_header(
                "location",
                format!(
                    "{}/bucket/cli-linux.tar.gz?X-Amz-Signature=s",
                    storage.uri()
                )
                .as_str(),
            ),
        )
        .mount(&registry)
        .await;
    Mock::given(method("GET"))
        .and(path("/bucket/cli-linux.tar.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(&b"hello"[..]))
        .mount(&storage)
        .await;
    (registry, storage)
}

#[tokio::test(flavor = "multi_thread")]
async fn get_resolves_a_version_constraint() {
    let (registry, _storage) = constraint_server().await;
    let dir = tempfile::tempdir().unwrap();

    sreg(
        &registry,
        &[
            "generic",
            "get",
            "tools/cli",
            "^1.2",
            "--prerelease",
            "--label",
            "os=linux",
            "--out",
            dir.path().to_str().unwrap(),
        ],
    )
    .await
    .success()
    .stdout(predicate::str::contains("cli-linux.tar.gz"))
    .stdout(predicate::str::contains("1.4.0-rc.1").not())
    .stderr(predicate::str::contains(
        "Resolved tools/cli ^1.2 to 1.4.0-rc.1",
    ));

    assert_eq!(
        std::fs::read(dir.path().join("cli-linux.tar.gz")).unwrap(),
        b"hello"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn get_json_prints_the_resolved_feed_document() {
    let (registry, storage) = constraint_server().await;

    let assert = sreg(
        &registry,
        &[
            "generic",
            "get",
            "tools/cli",
            "^1.2",
            "--prerelease",
            "--label",
            "os=linux",
            "--json",
        ],
    )
    .await
    .success();

    let body = stdout_json(&assert);
    assert_eq!(body["version"], "1.4.0-rc.1");
    assert_eq!(body["files"][0]["name"], "cli-linux.tar.gz");
    assert!(storage.received_requests().await.unwrap().is_empty());
}

/// A registry whose single-file endpoint resolves `^1` and `os=linux` to
/// `1.4.0`/`cli-linux.tar.gz` on `storage`, with the feed entry of that version.
async fn sole_server(storage_body: &'static [u8]) -> (MockServer, MockServer) {
    let registry = MockServer::start().await;
    let storage = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/generic/tools%2Fcli/latest/-/download"))
        .and(query_param("version", "^1"))
        .and(query_param("label", "os:linux"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header(
                    "location",
                    format!(
                        "{}/bucket/cli-linux.tar.gz?X-Amz-Signature=s",
                        storage.uri()
                    )
                    .as_str(),
                )
                .insert_header("x-resolved-version", "1.4.0")
                .insert_header("x-resolved-file", "cli-linux.tar.gz"),
        )
        .expect(1)
        .mount(&registry)
        .await;
    Mock::given(method("GET"))
        .and(path("/generic/tools%2Fcli/1.4.0"))
        .and(query_param("label", "os:linux"))
        .and(query_param("include", "yanked"))
        .respond_with(json(
            200,
            json!({
                "name": "tools/cli", "version": "1.4.0", "published": "2026-10-28T10:12:00Z",
                "yanked": false, "meta": {},
                "files": [{
                    "name": "cli-linux.tar.gz", "size": 5, "sha256": HELLO_SHA256,
                    "labels": { "os": "linux" },
                    "url": format!("{}/generic/tools%2Fcli/1.4.0/cli-linux.tar.gz", registry.uri())
                }]
            }),
        ))
        .mount(&registry)
        .await;
    Mock::given(method("GET"))
        .and(path("/generic/tools%2Fcli/1.4.0/cli-linux.tar.gz"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(
            ResponseTemplate::new(302).insert_header(
                "location",
                format!(
                    "{}/bucket/cli-linux.tar.gz?X-Amz-Signature=s",
                    storage.uri()
                )
                .as_str(),
            ),
        )
        .mount(&registry)
        .await;
    Mock::given(method("GET"))
        .and(path("/bucket/cli-linux.tar.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(storage_body))
        .mount(&storage)
        .await;
    (registry, storage)
}

#[tokio::test(flavor = "multi_thread")]
async fn sole_downloads_the_single_file_to_a_file_path() {
    let (registry, storage) = sole_server(b"hello").await;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("bin").join("cli.tgz");

    sreg(
        &registry,
        &[
            "generic",
            "get",
            "tools/cli",
            "^1",
            "--label",
            "os=linux",
            "--sole",
            "--out",
            out.to_str().unwrap(),
        ],
    )
    .await
    .success()
    .stdout(format!("{}\n", out.display()))
    .stderr(predicate::str::contains(
        "Resolved tools/cli ^1 to 1.4.0 cli-linux.tar.gz",
    ))
    .stderr(predicate::str::contains("X-Amz-Signature").not());

    assert_eq!(std::fs::read(&out).unwrap(), b"hello");
    let storage_requests = storage.received_requests().await.unwrap();
    assert_eq!(storage_requests.len(), 1);
    assert!(storage_requests[0].headers.get("authorization").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn sole_renews_an_expired_storage_url() {
    let (registry, storage) = sole_server(b"hello").await;
    Mock::given(method("GET"))
        .and(path("/bucket/cli-linux.tar.gz"))
        .respond_with(ResponseTemplate::new(403))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&storage)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("cli.tgz");

    sreg(
        &registry,
        &[
            "generic",
            "get",
            "tools/cli",
            "^1",
            "--label",
            "os=linux",
            "--sole",
            "--out",
            out.to_str().unwrap(),
        ],
    )
    .await
    .success();

    assert_eq!(std::fs::read(&out).unwrap(), b"hello");
    assert_eq!(storage.received_requests().await.unwrap().len(), 2);
    let file_requests = registry
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path().ends_with("/1.4.0/cli-linux.tar.gz"))
        .count();
    assert_eq!(file_requests, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn sole_names_the_file_after_the_registry_in_a_directory() {
    let (registry, _storage) = sole_server(b"hello").await;
    let dir = tempfile::tempdir().unwrap();
    let out = format!("{}/downloads/", dir.path().display());

    sreg(
        &registry,
        &[
            "generic",
            "get",
            "tools/cli",
            "^1",
            "--label",
            "os=linux",
            "--sole",
            "--out",
            &out,
        ],
    )
    .await
    .success();

    assert_eq!(
        std::fs::read(dir.path().join("downloads").join("cli-linux.tar.gz")).unwrap(),
        b"hello"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sole_verifies_the_file_against_the_feed() {
    let (registry, _storage) = sole_server(b"HELLO").await;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("cli.tgz");

    sreg(
        &registry,
        &[
            "generic",
            "get",
            "tools/cli",
            "^1",
            "--label",
            "os=linux",
            "--sole",
            "--out",
            out.to_str().unwrap(),
        ],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("corrupt"));

    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn sole_json_prints_the_feed_entry_of_the_file_without_downloading() {
    let (registry, storage) = sole_server(b"hello").await;

    let assert = sreg(
        &registry,
        &[
            "generic",
            "get",
            "tools/cli",
            "^1",
            "--label",
            "os=linux",
            "--sole",
            "--json",
        ],
    )
    .await
    .success();

    let body = stdout_json(&assert);
    assert_eq!(body["version"], "1.4.0");
    assert_eq!(body["files"].as_array().unwrap().len(), 1);
    assert_eq!(body["files"][0]["sha256"], HELLO_SHA256);
    assert!(storage.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn sole_lists_the_candidates_when_the_labels_are_ambiguous() {
    let registry = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/generic/tools%2Fcli/latest/-/download"))
        .and(query_param("label", "os:linux"))
        .respond_with(error(409, "ambiguous", "2 files match the labels").set_body_json(json!({
            "error": "ambiguous",
            "message": "2 files match the labels",
            "version": "1.4.0",
            "files": [
                { "name": "cli-linux-x64.tar.gz", "labels": { "os": "linux", "arch": "x64" } },
                { "name": "cli-linux-arm64.tar.gz", "labels": { "os": "linux", "arch": "arm64" } }
            ]
        })))
        .expect(1)
        .mount(&registry)
        .await;
    let dir = tempfile::tempdir().unwrap();

    sreg(
        &registry,
        &[
            "generic",
            "get",
            "tools/cli",
            "--label",
            "os=linux",
            "--sole",
            "--out",
            dir.path().to_str().unwrap(),
        ],
    )
    .await
    .code(1)
    .stdout(predicate::str::is_empty())
    .stderr(predicate::str::contains("2 files match the labels"))
    .stderr(predicate::str::contains("files of 1.4.0"))
    .stderr(predicate::str::contains(
        "cli-linux-x64.tar.gz (arch=x64, os=linux)",
    ))
    .stderr(predicate::str::contains(
        "cli-linux-arm64.tar.gz (arch=arm64, os=linux)",
    ));

    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn sole_fails_when_no_file_matches() {
    let registry = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/generic/tools%2Fcli/latest/-/download"))
        .respond_with(error(404, "not_found", "no file matches the labels"))
        .mount(&registry)
        .await;

    sreg(
        &registry,
        &[
            "generic",
            "get",
            "tools/cli",
            "--label",
            "os=plan9",
            "--sole",
        ],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("no file matches the labels"));
}
