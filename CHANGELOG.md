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
  `--path`, `--dry-run`, `--oidc` and `--project` (`--tag` for npm); packaging for npm,
  cargo and nuget is not implemented yet.
- Composer packaging (`sreg composer publish`): the name from `composer.json`, the
  version from `--version` or the tag of `HEAD` (`v1.2.3` → `1.2.3`; releases and
  `alpha`/`beta`/`rc` prereleases only, `dev-*` rejected), the archive from
  `git archive HEAD` honouring `export-ignore` (the committed `composer.json` must match
  the one on disk) or, outside git, a deterministic zip of the directory without
  `vendor/` and `.git`; uploaded as `<package>-<version>.zip` with `composer.json` as the
  manifest.
- `CommandRunner` abstraction for packagers that run external tools, and temporary work
  directories owned by the packages they belong to.
- `sreg generic publish` and `sreg generic get`.
- `sreg whoami`, `sreg version` and `sreg auth <composer|npm|cargo|nuget>` (see
  `docs/cli-auth.md`), with text and `--json` output.
- Exit codes `0` success, `1` error, `2` already published, `3` client too old (with the
  minimum version and a link to the releases).
- Cargo features `composer`, `npm`, `cargo`, `nuget` and `generic` (all on by default).
- CI workflow running fmt, clippy, tests, an MSRV check, a single-ecosystem build and
  release builds.

### Fixed

- Grouped generic package names are sent to the generic feed as one path segment with
  the `/` encoded as `%2F` (`/generic/acme%2Fplugin-foo/latest`), as the API contract
  requires.

[Unreleased]: https://github.com/kduma-OSS/serverless-registry-cli/commits/main
