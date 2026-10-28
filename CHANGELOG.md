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
  `--tag-prefix`, `--path`, `--dry-run`, `--skip-existing`, `--oidc` and
  `--oidc-project` (`--tag` for npm).
- Composer packaging (`sreg composer publish`): the name from `composer.json`, the
  version from `--version` or the release tag on `HEAD` (`v1.2.3` → `1.2.3`; releases
  and `alpha`/`beta`/`rc` prereleases only, `dev-*` rejected), the archive from
  `git archive HEAD:<path>` honouring `export-ignore`, so a package anywhere in a
  repository becomes the archive root (the committed `composer.json` must match the one
  on disk), or, outside git, a deterministic zip of the directory without `vendor/` and
  `.git`; uploaded as `<package>-<version>.zip` with `composer.json` as the manifest.
- npm packaging (`sreg npm publish`): `npm pkg set version=<v>` for a requested
  version, `npm pack --json --pack-destination <tmp>`, `--workspace` for npm
  workspaces, scoped names and semver versions only, `package/package.json` from the
  tarball (without `_` fields) as the manifest and the tarball under the name `npm pack`
  gave it; `--json`, `--pack-destination` and the workspace flags are reserved after `--`.
- cargo packaging (`sreg cargo publish`): the crate found with `cargo metadata`
  (`--package`, the crate of the current directory or the only member), a
  formatting-preserving version rewrite of its `Cargo.toml` (`toml_edit`; refused for
  `version.workspace = true`), `cargo package --no-verify --allow-dirty -p <crate>
  --target-dir <tmp>`, and a cargo sparse-index line as the manifest (dependencies in
  index shape, `features`/`features2` split like cargo, `links`, `rust_version`).
- NuGet packaging (`sreg nuget publish`): `dotnet pack <project> -o <tmp>` with
  `-p:PackageVersion=<v>`, `--project` (solutions refused), the `.nuspec` read from the
  `.nupkg` into the manifest, the `.nupkg` as `archive` and the `.snupkg` as `symbols`
  (`--no-symbols` to leave it out), both named `<id>.<version>` in lower case.
- Without `--version` and `--tag-prefix`, npm, cargo and nuget use the `v<semver>` tag on
  `HEAD` when there is one and the version of the project's manifest otherwise; changes
  `sreg` makes to `package.json` or `Cargo.toml` are reported on stderr.
- Swift and Maven ecosystems (`Ecosystem::Swift`, `Ecosystem::Maven`), the stub crates
  `sreg-eco-swift` and `sreg-eco-maven`, the Cargo features `swift` and `maven`, and the
  commands `sreg swift publish` and `sreg maven publish --from-dir <dir>
  [--coords <group:artifact>]`, which are not implemented yet.
- Release tag selection shared by all packagers (`sreg_core::git`): exactly one tag
  `v<semver>` on `HEAD`, or `<prefix><semver>` with `--tag-prefix <prefix>`.
- `--skip-existing` on every publish command: an already published version (or appended
  file) exits with `0` and reports `{ "skipped": true, ... }` with `--json`.
- Arguments after `--` on `sreg npm|cargo|nuget publish` are passed verbatim to the
  packaging tool; composer and generic publishing reject them.
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

### Changed

- `--oidc` no longer needs a project: without `--project` (`sreg auth`) or
  `--oidc-project` (publish commands, `sreg generic get`) the token is `_:gh.<jwt>`, and
  the registry resolves the project from the JWT's `repository` claim.

- `--path` is accepted by `sreg composer publish` only; npm, cargo and nuget publish
  work in the current directory and select the package with `--workspace`, `--package`
  and `--project`.

- The OIDC project of the publish commands and of `sreg generic get` is now
  `--oidc-project` (`sreg nuget publish` will use `--project` for the `.csproj`);
  `sreg auth` keeps `--project`.

### Fixed

- Grouped generic package names are sent to the generic feed as one path segment with
  the `/` encoded as `%2F` (`/generic/acme%2Fplugin-foo/latest`), as the API contract
  requires.

[Unreleased]: https://github.com/kduma-OSS/serverless-registry-cli/commits/main
