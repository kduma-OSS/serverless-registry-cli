//! `sreg swift publish` argument validation. Every case fails before `swift` runs, so
//! no Swift toolchain is needed and the registry is never contacted.

#![cfg(feature = "swift")]

mod common;

use std::path::{Path, PathBuf};

use common::{copy_dir, sreg_in};
use predicates::prelude::*;
use wiremock::MockServer;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../eco-swift/tests/fixtures/DemoKit")
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_arguments_stop_before_packaging() {
    let server = MockServer::start().await;
    // A copy outside any git repository, so no release tag is found.
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), dir.path());
    let empty = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    let empty_path = empty.path().to_str().unwrap();

    let cases: [(&[&str], &str); 6] = [
        (&["--version", "1.0.0"], "--scope <SCOPE>"),
        (
            &["--scope", "acme_corp", "--version", "1.0.0", "--path", path],
            "invalid Swift registry scope `acme_corp`",
        ),
        (
            &[
                "--scope",
                "acme",
                "--version",
                "1.0.0",
                "--path",
                empty_path,
            ],
            "Package.swift` does not exist",
        ),
        (
            &["--scope", "acme", "--path", path],
            "Package.swift declares no version",
        ),
        (
            &["--scope", "acme", "--version", "v1.0.0", "--path", path],
            "without a leading `v`",
        ),
        (
            &[
                "--scope",
                "acme",
                "--version",
                "1.0.0",
                "--path",
                path,
                "--",
                "-o",
                "x.zip",
            ],
            "`-o` cannot be passed",
        ),
    ];
    for (extra, message) in cases {
        let mut args = vec!["swift", "publish"];
        args.extend_from_slice(extra);
        sreg_in(&server, empty.path(), &args)
            .await
            .code(1)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains(message));
    }

    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_toolchain_is_explained() {
    let installed = std::process::Command::new("swift")
        .arg("--version")
        .output()
        .is_ok();
    if installed {
        eprintln!("skipping: swift is installed");
        return;
    }
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), dir.path());

    sreg_in(
        &server,
        dir.path(),
        &["swift", "publish", "--scope", "acme", "--version", "1.0.0"],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains(
        "failed to package the swift project",
    ))
    .stderr(predicate::str::contains("`swift` was not found"));

    assert!(server.received_requests().await.unwrap().is_empty());
}
