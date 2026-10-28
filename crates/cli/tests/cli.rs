//! End-to-end tests of the `sreg` binary.

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;

#[test]
fn help_lists_all_commands() {
    let mut assert = cargo_bin_cmd!("sreg").arg("--help").assert().success();
    for command in [
        "composer", "npm", "cargo", "whoami", "token", "yank", "delete", "auth",
    ] {
        assert = assert.stdout(predicate::str::contains(command));
    }
}

#[test]
fn token_help_lists_its_subcommands() {
    let mut assert = cargo_bin_cmd!("sreg")
        .args(["token", "--help"])
        .assert()
        .success();
    for command in ["create", "rotate", "list", "revoke"] {
        assert = assert.stdout(predicate::str::contains(command));
    }
}

#[test]
fn version_prints_package_version() {
    cargo_bin_cmd!("sreg")
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(concat!(
            "sreg ",
            env!("CARGO_PKG_VERSION")
        )));
}

#[test]
fn missing_registry_is_a_generic_error() {
    cargo_bin_cmd!("sreg")
        .arg("whoami")
        .env_remove("REGISTRY_URL")
        .env_remove("REGISTRY_TOKEN")
        .assert()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("REGISTRY_URL"));
}

#[test]
fn missing_token_is_a_generic_error() {
    cargo_bin_cmd!("sreg")
        .args(["whoami", "--registry", "http://127.0.0.1:9"])
        .env_remove("REGISTRY_TOKEN")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("REGISTRY_TOKEN"));
}

#[test]
fn version_subcommand_prints_client_version_offline() {
    cargo_bin_cmd!("sreg")
        .arg("version")
        .env_remove("REGISTRY_URL")
        .assert()
        .success()
        .stdout(predicate::str::contains(concat!(
            "sreg ",
            env!("CARGO_PKG_VERSION")
        )));
}

#[test]
fn usage_errors_do_not_use_the_version_exists_code() {
    cargo_bin_cmd!("sreg")
        .arg("no-such-command")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("unrecognized subcommand"));
}

#[test]
fn token_value_from_environment_is_not_shown_in_help() {
    cargo_bin_cmd!("sreg")
        .arg("--help")
        .env("REGISTRY_TOKEN", "shop-frontend:supersecretvalue")
        .assert()
        .success()
        .stdout(predicate::str::contains("supersecretvalue").not());
}
