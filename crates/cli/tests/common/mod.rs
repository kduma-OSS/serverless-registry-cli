//! Helpers for running the `sreg` binary against a local mock registry.

#![allow(dead_code)]

use std::path::Path;

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
    run(server, None, args, env).await
}

/// Like [`sreg`], run in the directory `dir`.
pub async fn sreg_in(server: &MockServer, dir: &Path, args: &[&str]) -> Assert {
    run(server, Some(dir), args, &[]).await
}

async fn run(
    server: &MockServer,
    dir: Option<&Path>,
    args: &[&str],
    env: &[(&str, &str)],
) -> Assert {
    let uri = server.uri();
    let dir = dir.map(Path::to_path_buf);
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
        if let Some(dir) = dir {
            command.current_dir(dir);
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

/// Answers `init` with one upload per declared file, pointing at `/bucket/<name>` on
/// `server`, under the upload id `upload_id`.
pub fn init_responder(
    server: &MockServer,
    upload_id: &'static str,
) -> impl Fn(&wiremock::Request) -> ResponseTemplate {
    let uri = server.uri();
    move |request: &wiremock::Request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let uploads: Vec<Value> = body["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| {
                let name = file["name"].as_str().unwrap();
                serde_json::json!({
                    "name": name,
                    "putUrl": format!("{uri}/bucket/{name}?X-Amz-Signature=secret-signature"),
                    "headers": {
                        "content-length": file["size"].to_string(),
                        "x-amz-checksum-sha256": "x"
                    }
                })
            })
            .collect();
        let expires = time::OffsetDateTime::now_utc() + time::Duration::minutes(10);
        let expires = expires
            .replace_nanosecond(0)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        json(
            200,
            serde_json::json!({ "uploadId": upload_id, "expires": expires, "uploads": uploads }),
        )
    }
}

/// The body of the first `init` request among `requests`.
pub fn init_body(requests: &[wiremock::Request]) -> Value {
    let init = requests
        .iter()
        .find(|request| request.url.path() == "/api/v1/publish/init")
        .expect("an init request");
    serde_json::from_slice(&init.body).unwrap()
}

/// Copies the directory `from` to `to`, recursively.
pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Whether `program --version` runs; tests that need a real packaging tool skip
/// themselves when it is missing.
pub fn tool_available(program: &str) -> bool {
    let available = std::process::Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !available {
        eprintln!("skipping: {program} is not installed");
    }
    available
}

/// The SHA-256 of `bytes` as lower-case hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    std::fs::write(&path, bytes).unwrap();
    sreg_core::digest::sha256_file(&path).unwrap().0
}
