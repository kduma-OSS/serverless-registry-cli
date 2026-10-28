//! Helpers for running the `sreg` binary against a local mock registry.

#![allow(dead_code)]

use assert_cmd::assert::Assert;
use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use wiremock::{MockServer, ResponseTemplate};

pub const TOKEN: &str = "root:k7Qx2mN9pLw4vR8sT1bYc3dF6hJ0aZ5e";

/// Runs `sreg` with `args` against `server` (as `REGISTRY_URL`) with [`TOKEN`].
pub async fn sreg(server: &MockServer, args: &[&str]) -> Assert {
    sreg_with(server, args, &[]).await
}

/// Like [`sreg`] with extra environment variables.
pub async fn sreg_with(server: &MockServer, args: &[&str], env: &[(&str, &str)]) -> Assert {
    let uri = server.uri();
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    tokio::task::spawn_blocking(move || {
        let mut command = cargo_bin_cmd!("sreg");
        command
            .args(&args)
            .env("REGISTRY_URL", uri)
            .env("REGISTRY_TOKEN", TOKEN)
            .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
            .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN");
        for (name, value) in env {
            command.env(name, value);
        }
        command.assert()
    })
    .await
    .unwrap()
}

/// A JSON response with the registry's version header.
pub fn json(status: u16, body: Value) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .insert_header("x-min-client-version", "0.1.0")
        .set_body_json(body)
}

/// An error envelope response.
pub fn error(status: u16, code: &str, message: &str) -> ResponseTemplate {
    json(
        status,
        serde_json::json!({ "error": code, "message": message }),
    )
}

/// Parses the stdout of a finished command as JSON.
pub fn stdout_json(assert: &Assert) -> Value {
    serde_json::from_slice(&assert.get_output().stdout).expect("stdout is a JSON document")
}
