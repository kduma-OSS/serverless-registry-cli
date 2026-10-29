# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- The `yank` and `delete` roles of the registry: `sreg yank` and `sreg delete` work with a
  token (a `key`, a GitHub Actions `gh` secret or a browser login) of a credential that holds
  the role and whose grants cover the version, not only with an admin. `sreg whoami` shows
  the `yank` and `delete` grants, and the API models carry `yank_grants` / `delete_grants`.
  Needs a Worker with the roles; docs/spec.md and docs/api.md are synced with the Worker.

## [0.2.3] - 2026-10-29

### Added

- `sreg yank <eco> <name> <version>` and `sreg delete <eco> <name> (<version> | --all)` are
  back, for `composer`, `npm`, `cargo`, `nuget`, `generic`, `swift` and `maven`. They
  authenticate with the token, with GitHub Actions OIDC (`--oidc`, `--oidc-project`; `_`
  lets the registry find the project, an `admin` credential included) or by signing in
  through the browser (`--login`).
- `sreg login --yank`, `--delete` and `--delete-package` ask the registry for a short-lived
  admin token that may yank or delete exactly the listed versions or packages (needs a
  registry that supports the `yank` and `delete` login purposes, `docs/api.md` 6.20).
- The client can replace the `scope` of a secret: `Client::update_secret` sends
  `PATCH …/secrets/<id>` with a description and/or a scope (`UpdateSecretRequest`);
  `describe_secret` keeps working and sends only the description.
- `sreg whoami` lists the operations of a yank or delete login token (`ops` in the registry's
  answer, also in `--json`) instead of reporting that it has no grants, and the package
  versions a publish login token may publish (`scopes`).

## [0.2.2] - 2026-10-28

### Changed

- A credential may hold `admin` together with `read` and `publish` (registry API, `docs/api.md`
  2.4): the client models document it, and `sreg whoami` shows the roles and the grants of a token
  that both administers and reads or publishes. The legacy `kind` of such a credential is `admin`.
- Documentation: `docs/spec.md` and `docs/api.md` synced with the registry.

## [0.2.1] - 2026-10-28

### Changed

- `sreg login --publish` no longer stops at 20 versions on its own: the registry decides, with
  its `MAX_PUBLISH_SCOPES` setting (20 by default), and its error names the limit.
- Documentation: `docs/spec.md` and `docs/api.md` synced with the registry (the scope of an
  existing secret can be edited with `PATCH …/secrets/<id>`).

## [0.2.0] - 2026-10-28

### Added

- Credentials with roles (registry API, `docs/api.md` 2.4): `sreg whoami` prints `Roles:` (the
  roles the token may use) and, for a token with both `read` and `publish`, a list of grants
  for each role. `--json` output gains `roles` and `access`. The client models know `roles`,
  `read_grants`, `publish_grants` and the `scope` of a secret, and `update_credential` sends
  `PATCH` with roles and per-role grants. Registries without roles keep working: `kind` and
  `grants` are used when `roles` and `access` are missing.

### Changed

- `sreg whoami` prints `Roles:` instead of `Kind:` (`--json` still has `kind`).

## [0.1.5] - 2026-10-28

### Fixed

- The "Bump and release" workflow pushes the release commit and tag as the release GitHub App
  (`SERVERLESS_REGISTRY_RELEASE_APP_ID` and `SERVERLESS_REGISTRY_RELEASE_APP_PRIVATE_KEY`) when
  they are set. A ruleset that requires status checks on `main` rejects the push made with
  `GITHUB_TOKEN` (`GH013`, "Required status check is expected"); the App can be put on the
  ruleset's bypass list. `RELEASE_TOKEN` still takes precedence, and without any of them the
  workflow uses `GITHUB_TOKEN`.

## [0.1.4] - 2026-10-28

### Changed

- The copies of `docs/api.md` and `docs/spec.md` describe the registry's `503 bucket_mismatch`
  (an R2 bucket that is not the installation's own) and `POST /api/v1/bucket/accept`. The CLI
  shows the registry's message for the error, like for any other.

### Added

- **Sign in through the browser.** `--login` on `sreg auth <eco>`, `sreg generic get` and every
  `publish` command, and the new `sreg login [--publish <eco>:<package>@<version>]...`, open a
  page of the registry where you sign in with a `login` secret (a password and an optional
  one-time code) or just approve, and receive a short-lived token: a pull token, or a publish
  token for exactly the package versions being published. `--no-browser` (or
  `SREG_NO_BROWSER`) prints the page instead of opening it. Needs a registry with login
  secrets (`docs/api.md` 2.6, 6.20, 6.21).
- A `401` now suggests signing in again with `sreg login`.
- The credential models know the `login` secret type and the new `description`, `totp`,
  `password` and `replaces` fields, the one-time `login` reveal of a created secret, and
  `Client::describe_secret` (`PATCH …/secrets/<id>`).

### Security

- Updated `sha2` (0.11.1, which brings `block-buffer` 0.12.1, GHSA-qwgh-2vcv-g2f7), `time` (0.3.55, RUSTSEC-2026-0009) and `zip` (9.1.3, RUSTSEC-2026-0367, -0368 and
  -0369). The CLI only reads and writes archives and never extracts them.

### Changed

- The minimum supported Rust version is now 1.88, which the fixed `time` and `zip` need.

## [0.1.3] - 2026-10-28

### Added

- `--archive <file>` for `sreg npm|composer|cargo|nuget publish`: publish an archive built
  earlier (`.tgz`, `.zip`, `.crate`, `.nupkg`) byte for byte. The package, version and
  dependencies are read from the manifest inside the archive; no git checkout and no
  native packaging tool is needed, and tags in the current directory are ignored.
  `--version` is a consistency check (for a Composer archive without a `version`, the
  version to publish). The archive is validated completely before the first request;
  `--tag-prefix`, project and workspace selectors and arguments after `--` are rejected
  together with it. Authentication, `--dry-run`, `--skip-existing` and `--json` work as
  before.
- `sreg cargo publish --archive … --source-registry-index <url>`: dependencies that refer
  to the given (retired) registry index are described in the sparse index as
  dependencies from this registry (`registry: null`), without changing the archive;
  dependencies from other registries keep their identity.

## [0.1.2] - 2026-10-28

### Changed

- The GitHub Action repository is renamed to
  [`kduma-OSS/serverless-registry-action`](https://github.com/kduma-OSS/serverless-registry-action),
  which now also has `setup`, `auth` and `get` actions next to `publish`
  (`uses: kduma-OSS/serverless-registry-action/publish@v1`); the release job dispatches
  its Release workflow there.

## [0.1.1] - 2026-10-28

### Added

- The registry URL may carry the token in its userinfo, Composer style:
  `https://<login>:<password>@packages.example.com/` (both parts percent-decoded). The
  credentials need a login and a password, are refused over plain `http` except for
  `localhost`/`127.0.0.1`, and are stripped from the URL before it is used or printed.
  `--oidc` still wins; an explicit `--token`/`REGISTRY_TOKEN` that differs from the URL's
  credentials is an error. `sreg auth` writes the URL's credentials, and the GitHub
  Action skips OIDC when `registry` carries them.
- `sreg generic get <name> <constraint>`: a version that is not an exact semver version
  (`^1.2`, `">=1 <2"`, `1.x`) is a constraint resolved by the registry
  (`GET /generic/<name>/latest?version=<constraint>`, with `--prerelease` and the label
  filters); the resolved version is printed on stderr and `--json` prints the feed
  document.
- `sreg generic get --sole`: fail unless the label filter selects exactly one file. The
  registry's `GET /generic/<name>/<version|latest>/-/download` redirects to that file and
  names it in `X-Resolved-Version` and `X-Resolved-File`; the file is still verified
  against the size and SHA-256 of its feed entry. `409 ambiguous` lists the candidate
  files and exits with `1`; `--out` may name a file path instead of a directory.
- **Bump and release** workflow (`.github/workflows/bump-and-release.yml`, started by hand
  on `main`): computes the next version (`prerelease`, `patch`, `minor`, `release`,
  `custom`, …) with the dependency-free `scripts/bump-version.mjs`, sets it in
  `Cargo.toml` and `Cargo.lock`, rolls `CHANGELOG.md`, commits and tags `v<version>` and
  starts the cargo-dist `Release` workflow for the tag.
- cargo-dist post-announce job (`.github/workflows/release-action.yml`): after a stable
  release it starts the Release workflow of
  `kduma-OSS/serverless-registry-publish-action` with the new version as the action's
  default `sreg-version`, authenticated as a GitHub App (repository secrets
  `SERVERLESS_REGISTRY_RELEASE_APP_ID` and `SERVERLESS_REGISTRY_RELEASE_APP_PRIVATE_KEY`).
  Without them it only prints a notice; the action repository's hourly sync then picks
  the release up.

### Changed

- The GitHub Action moved to
  [`kduma-OSS/serverless-registry-publish-action`](https://github.com/kduma-OSS/serverless-registry-publish-action)
  (`uses: kduma-OSS/serverless-registry-publish-action@v1`), with its history; it is
  versioned and released there, and the `action/` directory and `action/VERSION` are gone.

## [0.1.0] - 2026-10-28

### Added

- Cargo workspace with the `sreg-core`, `sreg-eco-composer`, `sreg-eco-npm`,
  `sreg-eco-cargo`, `sreg-eco-nuget`, `sreg-eco-generic` and `sreg-cli` crates.
- Shared error type, `Ecosystem` enum (composer, npm, cargo, nuget, generic), `Packager`
  trait and SHA-256 file digest helper.
- Copies of the specification (`docs/spec.md`) and the API contract (`docs/api.md`).
- API models for every `/api/v1` request and response, and the grant shorthand parser
  `eco:package[:versions[:transitive]]` (maven packages keep their `groupId:artifactId`).
  `sreg whoami` shows whether each grant of a `read` credential is transitive.
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
- Swift and Maven ecosystems (`Ecosystem::Swift`, `Ecosystem::Maven`), the crates
  `sreg-eco-swift` and `sreg-eco-maven` and the Cargo features `swift` and `maven`.
- Swift packaging (`sreg swift publish --scope <scope>`): `swift package dump-package`,
  then `swift package archive-source -o <tmp>/<name>-<version>.zip`; published as
  `<scope>.<name>` with the zip as `archive` and `Package.swift` plus every
  `Package@swift-*.swift` as `manifest` files; the version from `--version` or the release
  tag (required, Package.swift has none); the manifest holds the tools version, the
  repository URL (`--repository-url` or the `origin` remote without credentials) and the
  dump; arguments after `--` go to `archive-source` (`-o`, `--output` and
  `--package-path` are reserved).
- Maven publishing (`sreg maven publish --from-dir <dir> [--coords <group:artifact>]`):
  one artifact version from a local Maven-2 file repository written by `mvn deploy
  -DaltDeploymentRepository=local::file:./out` or Gradle's `publish` to a `file://`
  repository; `--coords` selects the artifact, `--version` or the release tag the version
  directory (otherwise the only one); effective POMs only (`<parent>` and `${…}`
  properties rejected), no `-SNAPSHOT` versions; the files of the version directory
  without checksums and `maven-metadata*.xml`, with the roles `pom`, `archive`,
  `sources`, `javadoc`, `module` and `extra` (`.asc` signatures included); the coordinates, packaging,
  dependencies and the POM itself as the manifest.
- Release tag selection shared by all packagers (`sreg_core::git`): exactly one tag
  `v<semver>` on `HEAD`, or `<prefix><semver>` with `--tag-prefix <prefix>`.
- `--skip-existing` on every publish command: an already published version (or appended
  file) exits with `0` and reports `{ "skipped": true, ... }` with `--json`.
- Arguments after `--` on `sreg npm|cargo|nuget|swift publish` are passed verbatim to the
  packaging tool; composer, maven and generic publishing reject them.
- `CommandRunner` abstraction for packagers that run external tools, and temporary work
  directories owned by the packages they belong to.
- `sreg generic publish` and `sreg generic get`.
- `sreg whoami`, `sreg version` and `sreg auth <composer|npm|cargo|nuget|swift|maven>`
  (see `docs/cli-auth.md`), with text and `--json` output; `sreg auth swift` prints a
  `swift package-registry login` command or writes a `.netrc` entry, `sreg auth maven`
  prints `settings.xml` and Gradle snippets or merges a `<server>` into `settings.xml`.
- Exit codes `0` success, `1` error, `2` already published, `3` client too old (with the
  minimum version and a link to the releases).
- Cargo features `composer`, `npm`, `cargo`, `nuget`, `generic`, `swift` and `maven` (all on
  by default).
- CI workflow running fmt, clippy, tests, an MSRV check, a build and the tests of every
  single ecosystem feature (and of none), and release builds.
- Releases through cargo-dist (`dist-workspace.toml`, `.github/workflows/release.yml`):
  `sreg` archives with SHA-256 checksums for macOS (arm64, x86_64), Linux (x86_64 glibc
  and musl, arm64) and Windows (x86_64), and shell and PowerShell installers.
- Composite GitHub Action (`action/`, `kduma-OSS/serverless-registry-cli/action@v1`):
  installs the checksum-verified `sreg` release named in `action/VERSION` (or
  `sreg-version`) on Linux, macOS and Windows runners and runs
  `sreg <ecosystem> publish` with a token or, without one, with OIDC; inputs for every
  ecosystem's options, `--skip-existing` by default, and the outputs `name`, `version`,
  `url` and `skipped`.
- Installation, GitHub Action and compatibility documentation, and the release process
  (`docs/releasing.md`).

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

[Unreleased]: https://github.com/kduma-OSS/serverless-registry-cli/compare/v0.2.3...HEAD
[0.2.3]: https://github.com/kduma-OSS/serverless-registry-cli/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/kduma-OSS/serverless-registry-cli/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/kduma-OSS/serverless-registry-cli/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/kduma-OSS/serverless-registry-cli/compare/v0.1.5...v0.2.0
[0.1.5]: https://github.com/kduma-OSS/serverless-registry-cli/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/kduma-OSS/serverless-registry-cli/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/kduma-OSS/serverless-registry-cli/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/kduma-OSS/serverless-registry-cli/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/kduma-OSS/serverless-registry-cli/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/kduma-OSS/serverless-registry-cli/releases/tag/v0.1.0
