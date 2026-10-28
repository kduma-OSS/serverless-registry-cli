//! `sreg <eco> publish` failures that happen before the registry is contacted.

mod common;

#[cfg(any(feature = "composer", feature = "generic"))]
use common::sreg;
use common::sreg_in;
use predicates::prelude::*;
use wiremock::MockServer;

#[tokio::test(flavor = "multi_thread")]
async fn packaging_failures_stop_before_contacting_the_registry() {
    let server = MockServer::start().await;
    // An empty directory: there is no project to package for any ecosystem.
    let empty = tempfile::tempdir().unwrap();

    let ecosystems = [
        #[cfg(feature = "composer")]
        "composer",
        #[cfg(feature = "npm")]
        "npm",
        #[cfg(feature = "cargo")]
        "cargo",
        #[cfg(feature = "nuget")]
        "nuget",
        #[cfg(feature = "swift")]
        "swift",
        #[cfg(feature = "maven")]
        "maven",
    ];
    for eco in ecosystems {
        let mut args = vec![eco, "publish", "--version", "1.0.0", "--dry-run"];
        match eco {
            "maven" => args.extend(["--from-dir", "."]),
            "swift" => args.extend(["--scope", "acme"]),
            _ => {}
        }
        sreg_in(&server, empty.path(), &args)
            .await
            .code(1)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains(format!(
                "failed to package the {eco} project"
            )));
    }

    assert!(server.received_requests().await.unwrap().is_empty());
}

#[cfg(feature = "composer")]
#[tokio::test(flavor = "multi_thread")]
async fn composer_publish_rejects_passthrough_arguments() {
    let server = MockServer::start().await;
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../eco-composer/tests/fixtures/library"
    );

    sreg(
        &server,
        &[
            "composer",
            "publish",
            "--path",
            fixture,
            "--version",
            "1.0.0",
            "--",
            "--foo",
        ],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("arguments after `--`"));

    assert!(server.received_requests().await.unwrap().is_empty());
}

#[cfg(feature = "generic")]
#[tokio::test(flavor = "multi_thread")]
async fn generic_publish_rejects_passthrough_arguments() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cli.zip");
    std::fs::write(&file, b"hello").unwrap();

    sreg(
        &server,
        &[
            "generic",
            "publish",
            "cli",
            "1.0.0",
            file.to_str().unwrap(),
            "--",
            "--foo",
        ],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("no arguments after `--`"));

    assert!(server.received_requests().await.unwrap().is_empty());
}
