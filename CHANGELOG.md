# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Cargo workspace with the `sreg-core`, `sreg-eco-composer`, `sreg-eco-npm`,
  `sreg-eco-cargo` and `sreg-cli` crates.
- Shared error type, `Ecosystem` enum, `Packager` trait and SHA-256 file digest helper.
- `sreg` command tree (`composer`, `npm` and `cargo publish`, `whoami`, `token`, `yank`,
  `delete`, `auth`) and exit codes; commands are not implemented yet.
- CI workflow running fmt, clippy, tests, an MSRV check and release builds.

[Unreleased]: https://github.com/kduma-OSS/serverless-registry-cli/commits/main
