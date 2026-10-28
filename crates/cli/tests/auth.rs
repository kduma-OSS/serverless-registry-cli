//! `sreg auth` with a static token and with GitHub Actions OIDC.

mod common;

use common::{sreg, sreg_with, stdout_json, TOKEN};
use predicates::prelude::*;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn host(server: &MockServer) -> String {
    server.uri().trim_start_matches("http://").to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn prints_composer_auth_export() {
    let server = MockServer::start().await;

    sreg(&server, &["auth", "composer"])
        .await
        .success()
        .stdout(format!(
            "export COMPOSER_AUTH='{{\"bearer\":{{\"{}\":\"{TOKEN}\"}}}}'\n",
            host(&server)
        ));

    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn json_output_names_the_variable() {
    let server = MockServer::start().await;

    let assert = sreg(
        &server,
        &["auth", "cargo", "--registry-name", "acme", "--json"],
    )
    .await
    .success();

    assert_eq!(
        stdout_json(&assert),
        json!({ "eco": "cargo", "name": "CARGO_REGISTRIES_ACME_TOKEN", "value": TOKEN })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_into_an_existing_npmrc() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let npmrc = dir.path().join(".npmrc");
    std::fs::write(&npmrc, "@acme:registry=https://example.com/npm/\n").unwrap();

    sreg(
        &server,
        &["auth", "npm", "--write", npmrc.to_str().unwrap()],
    )
    .await
    .success()
    .stdout(predicate::str::contains("Wrote the token of `root`"))
    .stdout(predicate::str::contains(TOKEN).not());

    assert_eq!(
        std::fs::read_to_string(&npmrc).unwrap(),
        format!(
            "@acme:registry=https://example.com/npm/\n//{}/npm/:_authToken={TOKEN}\n",
            host(&server)
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn creates_credentials_toml_readable_by_owner_only() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cargo").join("credentials.toml");

    sreg(
        &server,
        &["auth", "cargo", "--write", file.to_str().unwrap()],
    )
    .await
    .success();

    let content: toml::Table = std::fs::read_to_string(&file).unwrap().parse().unwrap();
    assert_eq!(content["registries"]["sreg"]["token"].as_str(), Some(TOKEN));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn oidc_builds_a_gh_token_for_the_project() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/oidc"))
        .and(query_param("audience", server.uri().as_str()))
        .and(header("authorization", "bearer request-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": "eyJ.oidc.sig" })))
        .expect(1)
        .mount(&server)
        .await;
    let request_url = format!("{}/oidc?api-version=2.0", server.uri());

    let assert = sreg_with(
        &server,
        &["auth", "npm", "--oidc", "--project", "ci-acme", "--json"],
        &[
            ("ACTIONS_ID_TOKEN_REQUEST_URL", request_url.as_str()),
            ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "request-token"),
        ],
    )
    .await
    .success();

    assert_eq!(stdout_json(&assert)["value"], "ci-acme:gh.eyJ.oidc.sig");
}

#[tokio::test(flavor = "multi_thread")]
async fn oidc_without_a_project_lets_the_registry_resolve_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/oidc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": "eyJ.oidc.sig" })))
        .expect(1)
        .mount(&server)
        .await;
    let request_url = format!("{}/oidc?api-version=2.0", server.uri());

    let assert = sreg_with(
        &server,
        &["auth", "npm", "--oidc", "--json"],
        &[
            ("ACTIONS_ID_TOKEN_REQUEST_URL", request_url.as_str()),
            ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "request-token"),
        ],
    )
    .await
    .success();

    assert_eq!(stdout_json(&assert)["value"], "_:gh.eyJ.oidc.sig");
}

#[tokio::test(flavor = "multi_thread")]
async fn oidc_outside_github_actions_fails() {
    let server = MockServer::start().await;

    sreg(
        &server,
        &["auth", "composer", "--oidc", "--project", "ci-acme"],
    )
    .await
    .code(1)
    .stderr(predicate::str::contains("ACTIONS_ID_TOKEN_REQUEST_URL"));
}

#[tokio::test(flavor = "multi_thread")]
async fn prints_the_dotnet_add_source_command_for_nuget() {
    let server = MockServer::start().await;

    sreg(&server, &["auth", "nuget", "--registry-name", "acme"])
        .await
        .success()
        .stdout(format!(
            "dotnet nuget add source '{}/nuget/index.json' -n 'acme' -u 'root' -p '{}' \
             --store-password-in-clear-text\n",
            server.uri(),
            TOKEN.split_once(':').unwrap().1
        ));
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_nuget_config() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("nuget.config");
    std::fs::write(
        &file,
        "<configuration>\n  <packageSources>\n    <add key=\"nuget.org\" value=\"https://api.nuget.org/v3/index.json\" />\n  </packageSources>\n</configuration>\n",
    )
    .unwrap();

    let assert = sreg(
        &server,
        &["auth", "nuget", "--write", file.to_str().unwrap(), "--json"],
    )
    .await
    .success();

    assert_eq!(stdout_json(&assert)["key"], "packageSourceCredentials.sreg");
    let content = std::fs::read_to_string(&file).unwrap();
    assert!(content.contains("nuget.org"));
    assert!(content.contains(&format!(
        "<add key=\"sreg\" value=\"{}/nuget/index.json\"/>",
        server.uri()
    )));
    assert!(content.contains("<add key=\"Username\" value=\"root\"/>"));
    assert!(content.contains(&format!(
        "<add key=\"ClearTextPassword\" value=\"{}\"/>",
        TOKEN.split_once(':').unwrap().1
    )));
}

#[tokio::test(flavor = "multi_thread")]
async fn prints_the_swift_registry_login_command() {
    let server = MockServer::start().await;

    sreg(&server, &["auth", "swift"])
        .await
        .success()
        .stdout(format!(
            "swift package-registry login '{}/swift' --username 'root' --password '{}'\n",
            server.uri(),
            TOKEN.split_once(':').unwrap().1
        ));

    let assert = sreg(&server, &["auth", "swift", "--json"]).await.success();
    let report = stdout_json(&assert);
    assert_eq!(report["eco"], "swift");
    assert_eq!(report["registry"], format!("{}/swift", server.uri()));
    assert_eq!(report["username"], "root");
    assert_eq!(report["password"], TOKEN.split_once(':').unwrap().1);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_a_netrc_entry_for_swift() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let netrc = dir.path().join(".netrc");
    std::fs::write(
        &netrc,
        "machine github.com login x password y\nmachine 127.0.0.1 login old password old\n",
    )
    .unwrap();

    let assert = sreg(
        &server,
        &[
            "auth",
            "swift",
            "--write",
            netrc.to_str().unwrap(),
            "--json",
        ],
    )
    .await
    .success();

    assert_eq!(stdout_json(&assert)["key"], "machine 127.0.0.1");
    assert_eq!(
        std::fs::read_to_string(&netrc).unwrap(),
        format!(
            "machine github.com login x password y\nmachine 127.0.0.1 login root password {}\n",
            TOKEN.split_once(':').unwrap().1
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn prints_maven_and_gradle_snippets() {
    let server = MockServer::start().await;
    let password = TOKEN.split_once(':').unwrap().1;

    sreg(&server, &["auth", "maven"])
        .await
        .success()
        .stdout(predicate::str::contains(format!(
            "<server>\n  <id>sreg</id>\n  <username>root</username>\n  \
             <password>{password}</password>\n</server>"
        )))
        .stdout(predicate::str::contains(format!(
            "credentials {{\n    username = \"root\"\n    password = \"{password}\"\n}}"
        )));

    let assert = sreg(
        &server,
        &["auth", "maven", "--registry-name", "acme", "--json"],
    )
    .await
    .success();
    let report = stdout_json(&assert);
    assert_eq!(report["eco"], "maven");
    assert_eq!(report["id"], "acme");
    assert_eq!(report["username"], "root");
    assert_eq!(report["password"], password);
    assert!(report["settingsXml"]
        .as_str()
        .unwrap()
        .contains("<id>acme</id>"));
    assert!(report["gradle"]
        .as_str()
        .unwrap()
        .starts_with("credentials {"));
}

#[tokio::test(flavor = "multi_thread")]
async fn merges_the_server_into_maven_settings() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.xml");
    std::fs::write(
        &settings,
        "<settings>\n  <servers>\n    <server>\n      <id>github</id>\n    </server>\n  </servers>\n</settings>\n",
    )
    .unwrap();

    let assert = sreg(
        &server,
        &[
            "auth",
            "maven",
            "--write",
            settings.to_str().unwrap(),
            "--json",
        ],
    )
    .await
    .success();

    assert_eq!(stdout_json(&assert)["key"], "servers.server.sreg");
    assert_eq!(
        std::fs::read_to_string(&settings).unwrap(),
        format!(
            "<settings>\n  <servers>\n    <server>\n      <id>github</id>\n    </server>\n    \
             <server>\n      <id>sreg</id>\n      <username>root</username>\n      \
             <password>{}</password>\n    </server>\n  </servers>\n</settings>\n",
            TOKEN.split_once(':').unwrap().1
        )
    );
}
