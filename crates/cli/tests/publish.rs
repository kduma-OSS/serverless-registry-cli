//! `sreg <eco> publish` for the packagers that are not implemented yet.

mod common;

use common::sreg;
use predicates::prelude::*;
use wiremock::MockServer;

#[tokio::test(flavor = "multi_thread")]
async fn packaging_failures_stop_before_contacting_the_registry() {
    let server = MockServer::start().await;

    let ecosystems = [
        #[cfg(feature = "composer")]
        "composer",
        #[cfg(feature = "npm")]
        "npm",
        #[cfg(feature = "cargo")]
        "cargo",
        #[cfg(feature = "nuget")]
        "nuget",
    ];
    for eco in ecosystems {
        sreg(
            &server,
            &[eco, "publish", "--version", "1.0.0", "--dry-run"],
        )
        .await
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(format!(
            "failed to package the {eco} project"
        )));
    }

    assert!(server.received_requests().await.unwrap().is_empty());
}
