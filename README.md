# sreg

`sreg` is the command-line tool of **Serverless Registry**, a private, serverless
package registry for Composer, npm and cargo hosted on Cloudflare (Workers, R2 and KV).

`sreg` is the only way to publish a version. It packages the project (natively for
Composer, through `npm pack` and `cargo package` for npm and cargo), uploads the archive
straight to R2 through a presigned URL and commits the version through the registry's
API. Published versions are immutable. It also covers token management and yank/delete
for administrators, and writes credentials for package managers, including GitHub
Actions OIDC tokens.

The registry itself lives in the
[`serverless-registry-worker`](https://github.com/kduma-OSS/serverless-registry-worker)
repository.

> **Work in progress.** The command tree is in place, but the commands are not
> implemented yet and currently exit with an error.

## Commands

| Command | What it does |
| --- | --- |
| `sreg composer publish [--version <v>] [--path .]` | Archives the directory (`git archive HEAD`, honouring `export-ignore`, falling back to a zip of the directory without `vendor/` and `.git/`) and reads `composer.json`. The version comes from `--version` or the git tag (`v1.2.3` → `1.2.3`); it must be semver, `dev-*` versions are rejected. |
| `sreg npm publish [--tag <t>] [--path .]` | Runs `npm pack --json --pack-destination <tmp>` and reads the name, version and `package.json` from the tarball. |
| `sreg cargo publish [--path .] [-- <args>]` | Runs `cargo package --no-verify --allow-dirty` (extra arguments are passed through) and reads the `.crate` file and metadata from `cargo metadata`. |
| `sreg whoami` | Shows the identity and grants of the current token (`GET /api/v1/whoami`). |
| `sreg token create --project <p> --kind read --grant composer:acme/*:* …` | Admin. Creates a credential and prints its token once. |
| `sreg token rotate --project <p> [--expire-old 14d]` | Admin. Issues a new secret; the old one expires. |
| `sreg token list`, `sreg token revoke --project <p>` | Admin. Lists or revokes credentials. |
| `sreg yank <eco> <name> <version>`, `sreg delete <eco> <name> <version>` | Admin. Yanks or deletes a version. |
| `sreg auth <eco> [--oidc] [--project <p>] [--write <file>]` | Writes credentials to `auth.json`, `.npmrc` or `credentials.toml`, or prints them for export (e.g. as `COMPOSER_AUTH`). With `--oidc`, inside a GitHub Actions job with `id-token: write`, it fetches a JWT for the registry and builds a `<project>:gh.<jwt>` token; without it, it uses `REGISTRY_TOKEN`. |

Every publish command computes the SHA-256 digest and size of the archive, calls
`init`, uploads the archive with `PUT` (3 attempts with backoff) and calls `commit`.
`--dry-run` stops after `init`. With `--oidc`, publish commands authenticate with the
GitHub Actions OIDC token directly.

Global options: `--registry <url>`, `--token <token>`, `--json` (machine-readable
output only) and `-v`/`--verbose` (logs to stderr; secrets are never logged).

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
| `2` | The version already exists in the registry (`409`). |
| `3` | This client is too old for the registry. |

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
