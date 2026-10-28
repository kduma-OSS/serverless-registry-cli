# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Cargo workspace with the `sreg-core`, `sreg-eco-composer`, `sreg-eco-npm`,
  `sreg-eco-cargo`, `sreg-eco-nuget`, `sreg-eco-generic` and `sreg-cli` crates.
- Shared error type, `Ecosystem` enum (composer, npm, cargo, nuget, generic), `Packager`
  trait and SHA-256 file digest helper.
- Copies of the specification (`docs/spec.md`) and the API contract (`docs/api.md`).
- API models for every `/api/v1` request and response, and the grant shorthand parser
  `eco:package[:versions]`.
- HTTP client (reqwest with rustls) sending `User-Agent`, `X-Client-Version` and the
  bearer token, reading `X-Min-Client-Version`, mapping `426`, `409 version_exists`,
  `409 file_exists` and the error envelope to typed errors, and retrying `GET` requests
  on transport errors, `5xx` and `429`.
- Publishing of versions as lists of files: `init`, a streamed fixed-length `PUT` per file
  with retries, a new `init` when the presigned URLs expire, and an idempotent `commit`
  with retries.
- GitHub Actions OIDC tokens (`<project>:gh.<jwt>` with the registry URL as audience) and
  shared token resolution.
- Generic packages: `GenericPackager`, the generic feed and verified file downloads that
  never send the token to the storage.
- `sreg composer|npm|cargo|nuget publish` wired to the publish flow with `--version`,
  `--path`, `--dry-run`, `--oidc` and `--project` (`--tag` for npm); packaging itself is
  not implemented yet.
- `sreg generic publish` and `sreg generic get`.
- `sreg whoami`, `sreg version` and `sreg auth <composer|npm|cargo|nuget>` (see
  `docs/cli-auth.md`), with text and `--json` output.
- Exit codes `0` success, `1` error, `2` already published, `3` client too old (with the
  minimum version and a link to the releases).
- Cargo features `composer`, `npm`, `cargo`, `nuget` and `generic` (all on by default).
- CI workflow running fmt, clippy, tests, an MSRV check, a single-ecosystem build and
  release builds.

[Unreleased]: https://github.com/kduma-OSS/serverless-registry-cli/commits/main
