# Guidelines for contributors and AI agents

This repository contains `sreg`, the command-line tool of **Serverless Registry**: a
private, serverless package registry for Composer, npm, cargo, NuGet, Swift, Maven and
generic files hosted on Cloudflare. `sreg` is the only way to publish a version; it
packages a project, uploads the archive straight to R2 through a presigned URL and
commits the version through the Worker's API.
The Worker lives in the `serverless-registry-worker` repository.

Read these before changing anything:

- `docs/spec.md` – the full product specification (copy of the one in the Worker
  repository; the CLI section is the relevant part).
- `docs/api.md` – the HTTP contract of the Worker (`/api/v1`, plus the generic, NuGet,
  Swift and Maven feeds). The CLI must follow it exactly; if something is missing from the
  contract, fix the contract in the Worker repository first. Both documents are identical
  copies of the Worker's; keep them in sync.
- `docs/releasing.md` – how releases are cut and how CLI and Worker versions relate;
  `docs/cli-auth.md` – the design of `sreg auth`.

## Language

Everything in this repository is written in **English**: code, comments, identifiers,
commit messages, documentation, test names, error messages and CLI output. Do not add
text in any other language.

## Workspace layout

```
crates/
  core/          # HTTP client (reqwest + rustls), API models, presigned upload with retry, version checks
  eco-composer/  # packaging and manifest extraction for Composer
  eco-npm/       # packaging through `npm pack` and manifest extraction
  eco-cargo/     # packaging through `cargo package` and manifest extraction
  eco-nuget/     # packaging through `dotnet pack` and `.nuspec` extraction
  eco-generic/   # arbitrary files as generic package versions
  eco-swift/     # packaging through `swift package archive-source` and `dump-package`
  eco-maven/     # Maven artifacts from a local file repository, effective POM parsing
  cli/           # clap command tree, text and `--json` output, exit codes; one Cargo
                 # feature per ecosystem (all enabled by default)
scripts/         # bump-version.mjs (release version and changelog) and its tests
dist-workspace.toml  # cargo-dist release configuration
docs/            # specification, API contract, release process, `sreg auth` design
.github/workflows/   # CI (fmt, clippy, tests, feature matrix, MSRV, release scripts),
                     # Bump and release, the generated cargo-dist Release workflow and
                     # its post-announce job release-action.yml
```

The composite GitHub Actions (`publish`, `setup`, `auth`, `get`) are not part of this repository: they live in
[`kduma-OSS/serverless-registry-action`](https://github.com/kduma-OSS/serverless-registry-action)
and are released there (see `docs/releasing.md`).

Design rules that must hold:

- Every ecosystem crate implements the shared `Packager` trait from `core`.
- Configuration comes only from flags and environment variables (`REGISTRY_URL`,
  `REGISTRY_TOKEN`); there is no configuration file.
- Exit codes: `0` success, `1` generic error, `2` version already exists (`409`),
  `3` client too old for the Worker.
- Secrets never appear in output or logs.
- `--dry-run` stops after `init`; `--json` prints machine-readable output only.

## Tooling

```
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Before every commit run fmt, clippy and the tests. A commit must leave the tree green. CI
also builds every ecosystem feature on its own and none at all
(`cargo clippy -p sreg-cli --all-targets --locked --no-default-features --features <eco>`
and the same `cargo test`), and checks the minimum supported Rust version
(`cargo +1.85 check --workspace --all-targets --locked`); run these when a change touches
feature gating or uses newer language features.

## Testing

- Unit-test parsers and packagers with real fixtures under `crates/*/tests/fixtures`.
- Test the HTTP client against a local mock server (`wiremock`); no network access in
  tests.
- Commands that shell out (`npm pack`, `cargo package`, `dotnet pack`, `swift package`,
  `git`) are tested behind the `CommandRunner` trait so the shell-out can be faked; the CLI
  integration tests in `crates/cli/tests` run the binary against a `wiremock` registry and
  storage.

## Git conventions

- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):
  `type(scope)?: description`. Allowed types: `feat`, `fix`, `docs`, `style`, `refactor`,
  `test`, `chore`, `build`, `ci`, `perf`, `revert`. Use the lower-case imperative mood and
  keep the subject under 72 characters.
- Scopes used here: `core`, `cli`, `composer`, `npm`, `cargo`, `nuget`, `generic`, `swift`,
  `maven`, `dist`, `deps`, `ci`, `docs`.
- Make small, focused commits. One logical change per commit; a feature plus its tests is
  one commit, two unrelated features are two commits.
- **Never** add `Co-Authored-By` or `Claude-Session` trailers, or any other trailer. The
  `commit-msg` hook rejects both.
- Enable the hooks once per clone:

  ```
  git config core.hooksPath .githooks
  ```

- Do not rewrite published history. Do not force-push.

## Coding style

- Rust 2021 edition, minimum Rust 1.85, `#![deny(unsafe_code)]` in every crate, no
  `unwrap()` outside tests.
- Errors use `thiserror` in library crates and `anyhow` only in the binary.
- Keep crates small and dependency lists short; prefer `rustls` over native TLS.
