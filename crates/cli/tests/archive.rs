//! `sreg <eco> publish --archive` end to end: archives built beforehand are described
//! from their own manifests and uploaded byte for byte, from a directory that is neither
//! a git checkout nor a project. No packaging tool is needed.

#![cfg(all(
    feature = "composer",
    feature = "npm",
    feature = "cargo",
    feature = "nuget"
))]

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};

use common::{init_body, init_responder, json, sha256_hex, sreg_in, stdout_json};
use flate2::write::GzEncoder;
use flate2::Compression;
use predicates::prelude::*;
use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn write_tgz(path: &Path, files: &[(&str, &str)]) {
    let mut builder = tar::Builder::new(GzEncoder::new(
        std::fs::File::create(path).unwrap(),
        Compression::default(),
    ));
    for (name, data) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, name, data.as_bytes())
            .unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap();
}

fn write_zip(path: &Path, files: &[(&str, &str)]) {
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, data) in files {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(data.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
}

const NPM_MANIFEST: &str =
    r#"{"name":"@acme/demo","version":"1.2.3","dependencies":{"left-pad":"^1.0.0"}}"#;
const COMPOSER_MANIFEST: &str =
    r#"{"name":"acme/sdk","version":"1.2.3","require":{"php":">=8.1","psr/log":"^3.0"}}"#;
const CARGO_MANIFEST: &str = r#"[package]
name = "licensing-kit-example"
version = "1.2.3"

[dependencies]
licensing-kit-core = { version = "^2.1", registry-index = "sparse+https://old.example.com/cargo/index/" }
renamed = { package = "licensing-kit-util", version = "0.4", features = ["x"], registry-index = "sparse+https://old.example.com/cargo/index/" }
other = { version = "1", registry-index = "sparse+https://other.example.org/index/" }
serde = "1"

[target.'cfg(unix)'.dependencies]
libc = "0.2"
"#;
const NUSPEC: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://schemas.microsoft.com/packaging/2013/05/nuspec.xsd">
  <metadata>
    <id>Acme.Sdk</id>
    <version>1.2.3</version>
    <authors>Acme</authors>
    <description>Acme SDK</description>
    <dependencies>
      <group targetFramework="net8.0">
        <dependency id="Newtonsoft.Json" version="[13.0.1, )" />
      </group>
    </dependencies>
  </metadata>
</package>
"#;

/// The archives under their on-disk names, which differ from the names the registry
/// stores them under.
struct Archives {
    dir: tempfile::TempDir,
}

impl Archives {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        write_tgz(
            &dir.path().join("package.tgz"),
            &[
                ("package/package.json", NPM_MANIFEST),
                ("package/index.js", "module.exports = 1;\n"),
            ],
        );
        write_zip(
            &dir.path().join("package.zip"),
            &[
                ("composer.json", COMPOSER_MANIFEST),
                ("src/Client.php", "<?php\n"),
            ],
        );
        write_tgz(
            &dir.path().join("package.crate"),
            &[
                ("licensing-kit-example-1.2.3/Cargo.toml", CARGO_MANIFEST),
                ("licensing-kit-example-1.2.3/src/lib.rs", ""),
            ],
        );
        write_zip(
            &dir.path().join("Acme.Sdk.1.2.3.nupkg"),
            &[
                ("Acme.Sdk.nuspec", NUSPEC),
                ("lib/net8.0/Acme.Sdk.dll", "MZ"),
            ],
        );
        Archives { dir }
    }

    fn path(&self, name: &str) -> String {
        self.dir.path().join(name).to_str().unwrap().to_owned()
    }

    fn bytes(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.dir.path().join(name)).unwrap()
    }
}

/// An empty directory that is not a git checkout and has no project in it.
fn bare_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

async fn mount_publish(server: &MockServer, upload_id: &'static str, put: &str, commit: Value) {
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_responder(server, upload_id))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/bucket/{put}")))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/commit"))
        .and(body_json(json!({ "uploadId": upload_id })))
        .respond_with(json(201, commit))
        .expect(1)
        .mount(server)
        .await;
}

fn put_body(requests: &[wiremock::Request]) -> Vec<u8> {
    requests
        .iter()
        .find(|request| request.method.as_str() == "PUT")
        .expect("a PUT")
        .body
        .clone()
}

/// Asserts that the one declared file is the input file, byte for byte.
fn assert_uploaded_as_is(requests: &[wiremock::Request], original: &[u8], name: &str) {
    let init = init_body(requests);
    let files = init["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["name"], name);
    assert_eq!(files[0]["role"], "archive");
    assert_eq!(files[0]["size"], original.len());
    assert_eq!(files[0]["sha256"], sha256_hex(original));
    assert_eq!(put_body(requests), original);
}

#[tokio::test(flavor = "multi_thread")]
async fn npm_uploads_the_tarball_unchanged() {
    let archives = Archives::new();
    let server = MockServer::start().await;
    mount_publish(
        &server,
        "upload-npm-0001",
        "acme-demo-1.2.3.tgz",
        json!({ "eco": "npm", "name": "@acme/demo", "version": "1.2.3",
                "url": "https://packages.example.com/npm/@acme%2fdemo",
                "files": [{ "name": "acme-demo-1.2.3.tgz" }] }),
    )
    .await;
    let cwd = bare_dir();

    sreg_in(
        &server,
        cwd.path(),
        &[
            "npm",
            "publish",
            "--archive",
            &archives.path("package.tgz"),
            "--version",
            "1.2.3",
            "--tag",
            "beta",
        ],
    )
    .await
    .success()
    .stdout(predicate::str::contains("Published npm @acme/demo 1.2.3"));

    let requests = server.received_requests().await.unwrap();
    assert_uploaded_as_is(
        &requests,
        &archives.bytes("package.tgz"),
        "acme-demo-1.2.3.tgz",
    );
    let init = init_body(&requests);
    assert_eq!(init["eco"], "npm");
    assert_eq!(init["name"], "@acme/demo");
    assert_eq!(init["version"], "1.2.3");
    assert_eq!(init["tag"], "beta");
    assert_eq!(
        init["manifest"]["dependencies"],
        json!({ "left-pad": "^1.0.0" })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn composer_uploads_the_zip_unchanged() {
    let archives = Archives::new();
    let server = MockServer::start().await;
    mount_publish(
        &server,
        "upload-composer-0001",
        "sdk-1.2.3.zip",
        json!({ "eco": "composer", "name": "acme/sdk", "version": "1.2.3",
                "url": "https://packages.example.com/composer/acme/sdk",
                "files": [{ "name": "sdk-1.2.3.zip" }] }),
    )
    .await;
    let cwd = bare_dir();

    sreg_in(
        &server,
        cwd.path(),
        &[
            "composer",
            "publish",
            "--archive",
            &archives.path("package.zip"),
        ],
    )
    .await
    .success()
    .stdout(predicate::str::contains(
        "Published composer acme/sdk 1.2.3",
    ));

    let requests = server.received_requests().await.unwrap();
    assert_uploaded_as_is(&requests, &archives.bytes("package.zip"), "sdk-1.2.3.zip");
    let init = init_body(&requests);
    assert_eq!(init["manifest"]["require"]["psr/log"], "^3.0");
    assert_eq!(init["version"], "1.2.3");
}

#[tokio::test(flavor = "multi_thread")]
async fn composer_takes_the_version_from_the_flag_when_the_archive_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let zip = dir.path().join("dist.zip");
    write_zip(&zip, &[("composer.json", r#"{"name":"acme/sdk"}"#)]);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_responder(&server, "upload-composer-0002"))
        .expect(1)
        .mount(&server)
        .await;
    let cwd = bare_dir();

    // Without --version there is nothing to publish it as.
    sreg_in(
        &server,
        cwd.path(),
        &[
            "composer",
            "publish",
            "--archive",
            zip.to_str().unwrap(),
            "--dry-run",
        ],
    )
    .await
    .failure()
    .stderr(predicate::str::contains("--version"));

    sreg_in(
        &server,
        cwd.path(),
        &[
            "composer",
            "publish",
            "--archive",
            zip.to_str().unwrap(),
            "--version",
            "2.0.0-rc1",
            "--dry-run",
        ],
    )
    .await
    .success();
    let requests = server.received_requests().await.unwrap();
    let init = init_body(&requests);
    assert_eq!(init["version"], "2.0.0-rc1");
    assert!(
        init["manifest"].get("version").is_none(),
        "the archive is not rewritten"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cargo_uploads_the_crate_and_remaps_the_retired_registry() {
    let archives = Archives::new();
    let server = MockServer::start().await;
    mount_publish(
        &server,
        "upload-cargo-0001",
        "licensing-kit-example-1.2.3.crate",
        json!({ "eco": "cargo", "name": "licensing-kit-example", "version": "1.2.3",
                "url": "https://packages.example.com/cargo/api/v1/crates/licensing-kit-example/1.2.3/download",
                "files": [{ "name": "licensing-kit-example-1.2.3.crate" }] }),
    )
    .await;
    let cwd = bare_dir();

    sreg_in(
        &server,
        cwd.path(),
        &[
            "cargo",
            "publish",
            "--archive",
            &archives.path("package.crate"),
            "--version",
            "1.2.3",
            "--source-registry-index",
            "sparse+https://old.example.com/cargo/index/",
        ],
    )
    .await
    .success()
    .stdout(predicate::str::contains(
        "Published cargo licensing-kit-example 1.2.3",
    ));

    let requests = server.received_requests().await.unwrap();
    assert_uploaded_as_is(
        &requests,
        &archives.bytes("package.crate"),
        "licensing-kit-example-1.2.3.crate",
    );
    let manifest = &init_body(&requests)["manifest"];
    let deps: Vec<(&str, &Value, &Value)> = manifest["deps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|dep| {
            (
                dep["name"].as_str().unwrap(),
                &dep["registry"],
                &dep["package"],
            )
        })
        .collect();
    assert_eq!(
        deps,
        [
            ("licensing-kit-core", &json!(null), &json!(null)),
            ("renamed", &json!(null), &json!("licensing-kit-util")),
            (
                "other",
                &json!("sparse+https://other.example.org/index/"),
                &json!(null)
            ),
            (
                "serde",
                &json!("https://github.com/rust-lang/crates.io-index"),
                &json!(null)
            ),
            (
                "libc",
                &json!("https://github.com/rust-lang/crates.io-index"),
                &json!(null)
            ),
        ]
    );
    assert_eq!(manifest["deps"][4]["target"], "cfg(unix)");
    assert_eq!(manifest["deps"][1]["features"], json!(["x"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn cargo_without_the_option_keeps_the_registry_identities() {
    let archives = Archives::new();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_responder(&server, "upload-cargo-0002"))
        .expect(1)
        .mount(&server)
        .await;
    let cwd = bare_dir();

    sreg_in(
        &server,
        cwd.path(),
        &[
            "cargo",
            "publish",
            "--archive",
            &archives.path("package.crate"),
            "--dry-run",
        ],
    )
    .await
    .success();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        init_body(&requests)["manifest"]["deps"][0]["registry"],
        "sparse+https://old.example.com/cargo/index/"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn nuget_normalises_the_upload_name_but_not_the_file() {
    let archives = Archives::new();
    let server = MockServer::start().await;
    mount_publish(
        &server,
        "upload-nuget-0001",
        "acme.sdk.1.2.3.nupkg",
        json!({ "eco": "nuget", "name": "Acme.Sdk", "version": "1.2.3",
                "url": "https://packages.example.com/nuget/v3/index.json",
                "files": [{ "name": "acme.sdk.1.2.3.nupkg" }] }),
    )
    .await;
    let cwd = bare_dir();

    sreg_in(
        &server,
        cwd.path(),
        &[
            "nuget",
            "publish",
            "--archive",
            &archives.path("Acme.Sdk.1.2.3.nupkg"),
            "--version",
            "1.2.3",
        ],
    )
    .await
    .success()
    .stdout(predicate::str::contains("Published nuget Acme.Sdk 1.2.3"));

    let requests = server.received_requests().await.unwrap();
    assert_uploaded_as_is(
        &requests,
        &archives.bytes("Acme.Sdk.1.2.3.nupkg"),
        "acme.sdk.1.2.3.nupkg",
    );
    let init = init_body(&requests);
    assert_eq!(init["name"], "Acme.Sdk");
    assert_eq!(
        init["manifest"]["dependencyGroups"][0]["dependencies"][0]["id"],
        "Newtonsoft.Json"
    );
    assert!(archives.dir.path().join("Acme.Sdk.1.2.3.nupkg").is_file());
    assert!(!archives.dir.path().join("acme.sdk.1.2.3.nupkg").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn dry_run_json_describes_the_input_file() {
    let archives = Archives::new();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_responder(&server, "upload-dry-0001"))
        .expect(1)
        .mount(&server)
        .await;
    let cwd = bare_dir();

    let assert = sreg_in(
        &server,
        cwd.path(),
        &[
            "npm",
            "publish",
            "--archive",
            &archives.path("package.tgz"),
            "--dry-run",
            "--json",
        ],
    )
    .await
    .success();

    let report = stdout_json(&assert);
    assert_eq!(report["dryRun"], true);
    assert_eq!(report["files"][0]["name"], "acme-demo-1.2.3.tgz");
    assert_eq!(
        report["files"][0]["sha256"],
        sha256_hex(&archives.bytes("package.tgz"))
    );
    let requests = server.received_requests().await.unwrap();
    assert!(requests
        .iter()
        .all(|request| request.method.as_str() != "PUT"));
}

#[tokio::test(flavor = "multi_thread")]
async fn skip_existing_applies_to_archives() {
    let archives = Archives::new();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(common::error(409, "version_exists", "already exists"))
        .expect(2)
        .mount(&server)
        .await;
    let cwd = bare_dir();
    let file = archives.path("Acme.Sdk.1.2.3.nupkg");

    sreg_in(
        &server,
        cwd.path(),
        &["nuget", "publish", "--archive", &file],
    )
    .await
    .code(2);
    let assert = sreg_in(
        &server,
        cwd.path(),
        &[
            "nuget",
            "publish",
            "--archive",
            &file,
            "--skip-existing",
            "--json",
        ],
    )
    .await
    .success();
    assert_eq!(
        stdout_json(&assert),
        json!({ "skipped": true, "eco": "nuget", "name": "Acme.Sdk", "version": "1.2.3" })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mismatches_and_malformed_archives_fail_before_any_request() {
    let archives = Archives::new();
    let server = MockServer::start().await;
    let cwd = bare_dir();
    let garbage: PathBuf = archives.dir.path().join("garbage");
    std::fs::write(&garbage, b"definitely not an archive").unwrap();
    let garbage = garbage.to_str().unwrap();

    let cases: Vec<Vec<String>> = vec![
        vec![
            "npm",
            "--archive",
            &archives.path("package.tgz"),
            "--version",
            "9.9.9",
        ],
        vec![
            "composer",
            "--archive",
            &archives.path("package.zip"),
            "--version",
            "9.9.9",
        ],
        vec![
            "cargo",
            "--archive",
            &archives.path("package.crate"),
            "--version",
            "9.9.9",
        ],
        vec![
            "nuget",
            "--archive",
            &archives.path("Acme.Sdk.1.2.3.nupkg"),
            "--version",
            "9.9.9",
        ],
        vec!["npm", "--archive", garbage],
        vec!["composer", "--archive", garbage],
        vec!["cargo", "--archive", garbage],
        vec!["nuget", "--archive", garbage],
        vec!["npm", "--archive", "/nonexistent/package.tgz"],
    ]
    .into_iter()
    .map(|case| case.into_iter().map(str::to_owned).collect())
    .collect();
    for case in cases {
        let mut args = vec![case[0].as_str(), "publish"];
        args.extend(case[1..].iter().map(String::as_str));
        sreg_in(&server, cwd.path(), &args).await.failure();
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn incompatible_arguments_are_rejected() {
    let archives = Archives::new();
    let server = MockServer::start().await;
    let cwd = bare_dir();
    let tgz = archives.path("package.tgz");
    let zip = archives.path("package.zip");
    let krate = archives.path("package.crate");
    let nupkg = archives.path("Acme.Sdk.1.2.3.nupkg");

    let cases: Vec<(Vec<&str>, &str)> = vec![
        (
            vec!["npm", "publish", "--archive", &tgz, "--workspace", "w"],
            "--workspace",
        ),
        (
            vec!["npm", "publish", "--archive", &tgz, "--tag-prefix", "v"],
            "--tag-prefix",
        ),
        (
            vec![
                "npm",
                "publish",
                "--archive",
                &tgz,
                "--",
                "--ignore-scripts",
            ],
            "NPM_ARGS",
        ),
        (
            vec!["composer", "publish", "--archive", &zip, "--path", "."],
            "--path",
        ),
        (
            vec![
                "composer",
                "publish",
                "--archive",
                &zip,
                "--tag-prefix",
                "v",
            ],
            "--tag-prefix",
        ),
        (
            vec!["cargo", "publish", "--archive", &krate, "--package", "x"],
            "--package",
        ),
        (
            vec!["cargo", "publish", "--archive", &krate, "--", "--locked"],
            "CARGO_ARGS",
        ),
        (
            vec![
                "nuget",
                "publish",
                "--archive",
                &nupkg,
                "--project",
                "a.csproj",
            ],
            "--project",
        ),
        (
            vec!["nuget", "publish", "--archive", &nupkg, "--no-symbols"],
            "--no-symbols",
        ),
        (
            vec![
                "nuget",
                "publish",
                "--archive",
                &nupkg,
                "--",
                "-c",
                "Release",
            ],
            "DOTNET_ARGS",
        ),
        (
            vec![
                "cargo",
                "publish",
                "--source-registry-index",
                "sparse+https://x/",
            ],
            "--archive",
        ),
    ];
    for (args, needle) in cases {
        sreg_in(&server, cwd.path(), &args)
            .await
            .failure()
            .stderr(predicate::str::contains(needle));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn archive_mode_ignores_tags_of_the_current_checkout() {
    let archives = Archives::new();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/publish/init"))
        .respond_with(init_responder(&server, "upload-tag-0001"))
        .expect(1)
        .mount(&server)
        .await;
    let repo = tempfile::tempdir().unwrap();
    for args in [
        &["init", "-q"][..],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ],
        &["tag", "v9.9.9"],
    ] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .status();
        if !status.is_ok_and(|status| status.success()) {
            eprintln!("skipping: git is not usable");
            return;
        }
    }

    sreg_in(
        &server,
        repo.path(),
        &[
            "npm",
            "publish",
            "--archive",
            &archives.path("package.tgz"),
            "--dry-run",
        ],
    )
    .await
    .success();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(init_body(&requests)["version"], "1.2.3");
}
