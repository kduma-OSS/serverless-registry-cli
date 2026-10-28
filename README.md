# sreg

`sreg` is the command-line tool of **Serverless Registry**, a private, serverless
package registry for Composer, npm, cargo, NuGet and generic files hosted on Cloudflare
(Workers, R2 and KV).

`sreg` is the only way to publish a version. It packages the project (natively for
Composer, through `npm pack`, `cargo package` and `dotnet pack` for npm, cargo and
NuGet), uploads the files straight to R2 through presigned URLs and commits the version
through the registry's API. Published versions are immutable. It also downloads generic
files and writes credentials for package managers, including GitHub Actions OIDC
tokens. Administration (credentials, yank, delete) is done in the registry's web panel;
`sreg` has no administrative commands.

The registry itself lives in the
[`serverless-registry-worker`](https://github.com/kduma-OSS/serverless-registry-worker)
repository.

> **Work in progress.** The HTTP client, the publish flow, `whoami`, `auth`, `version`
> and the generic commands work; packaging for Composer, npm, cargo and NuGet is not
> implemented yet, so their `publish` commands exit with an error.

## Commands

| Command | What it does |
| --- | --- |
| `sreg composer publish [--version <v>] [--path .]` | Archives the directory (`git archive HEAD`, honouring `export-ignore`, falling back to a zip of the directory without `vendor/` and `.git/`) and reads `composer.json`. The version comes from `--version` or the git tag (`v1.2.3` → `1.2.3`); it must be semver, `dev-*` versions are rejected. |
| `sreg npm publish [--version <v>] [--tag <t>] [--path .]` | Runs `npm pack --json --pack-destination <tmp>` and reads the name, version and `package.json` from the tarball. `--version` is written into the manifest before packing. |
| `sreg cargo publish [--version <v>] [--path .] [-- <args>]` | Runs `cargo package --no-verify --allow-dirty` (extra arguments are passed through) and reads the `.crate` file and metadata from `cargo metadata`. `--version` is written into the manifest before packaging. |
| `sreg nuget publish [--version <v>] [--path .]` | Runs `dotnet pack` and reads the `.nuspec` from the `.nupkg`. `--version` is written into the manifest before packing. |
| `sreg generic publish <name> <version> <file>... [--label k=v]... [--meta k=v]... [--meta-file <json>] [--spec <files.json>] [--no-append]` | Publishes files as one version of a generic package. `--label` applies to every file; `--spec` lists files with their own labels instead. Files are appended to an already published version (within the registry's append window) unless `--no-append` is given. |
| `sreg generic get <name> [<version>\|latest] [--label k=v]... [--out <dir>] [--prerelease]` | Downloads the files of a version matching the labels into `--out` after verifying size and SHA-256 and prints their paths; `--json` prints the feed entry without downloading. |
| `sreg whoami` | Shows the identity and grants of the current token (`GET /api/v1/whoami`). |
| `sreg auth <composer\|npm\|cargo\|nuget> [--oidc --project <p>] [--write <file>] [--registry-name <n>]` | Writes credentials to `auth.json`, `.npmrc`, `credentials.toml` or `nuget.config`, or prints them for the environment (e.g. as `COMPOSER_AUTH`, or a `dotnet nuget add source` command). With `--oidc`, inside a GitHub Actions job with `id-token: write`, it fetches a JWT for the registry and builds a `<project>:gh.<jwt>` token; without it, it uses `REGISTRY_TOKEN`. See [`docs/cli-auth.md`](docs/cli-auth.md). |
| `sreg version` | Prints the version of `sreg` and, with `--registry`, the registry's version and the minimum client version it accepts. |

Every publish command computes the SHA-256 digest and size of each file, calls `init`,
uploads every file with `PUT` (3 attempts with backoff; a new `init` when the presigned
URLs expire) and calls `commit`. All of them accept `--dry-run` (stop after `init`),
`--oidc` and `--project <p>` (authenticate with a GitHub Actions OIDC token for the
project instead of `REGISTRY_TOKEN`).

Global options: `--registry <url>`, `--token <token>`, `--json` (machine-readable
output only on stdout) and `-v`/`--verbose` (logs of `sreg` itself to stderr; tokens
and presigned URLs are never logged).

## Configuration

There is no configuration file. Configuration comes only from flags and environment
variables:

| Flag | Environment variable | Description |
| --- | --- | --- |
| `--registry <url>` | `REGISTRY_URL` | Base URL of the registry, e.g. `https://packages.example.com`. |
| `--token <token>` | `REGISTRY_TOKEN` | Token in the form `[<source>.]<project>:[<type>.]<secret>`. |

In CI, pass both as environment variables from repository secrets.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success. |
| `1` | Generic error (including invalid usage). |
| `2` | Already published: the version exists (`409 version_exists`), or a file being appended exists (`409 file_exists`). |
| `3` | This client is too old for the registry (`426`); the message names the minimum version and links to the releases. |

## Building a smaller binary

Each ecosystem is a Cargo feature of the `sreg-cli` crate: `composer`, `npm`, `cargo`,
`nuget` and `generic`, all enabled by default. A binary for a single ecosystem:

```
cargo build --release -p sreg-cli --no-default-features --features composer
```

## Development

```
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

See [`AGENTS.md`](AGENTS.md) for the workspace layout and contribution rules.

## License

MIT, see [`LICENSE`](LICENSE).
