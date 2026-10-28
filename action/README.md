# Serverless Registry publish action

A composite GitHub Action that installs [`sreg`](../README.md) from its GitHub release
and publishes one package version to a
[Serverless Registry](https://github.com/kduma-OSS/serverless-registry-worker).

What it does:

1. Picks the release build for the runner (Linux x86_64 (musl) and arm64, macOS x86_64
   and arm64, Windows x86_64), downloads the archive and its `.sha256` file from the
   [releases](https://github.com/kduma-OSS/serverless-registry-cli/releases), verifies
   the checksum, unpacks it into `$RUNNER_TEMP/sreg` and adds it to `PATH`.
2. Runs `sreg <ecosystem> publish --json …` with `REGISTRY_URL` and `REGISTRY_TOKEN`
   (or `--oidc`) and turns the result into step outputs.

The action installs the `sreg` release it was published with (`action/VERSION`); pass
`sreg-version` to pin another one. It works on Linux, macOS and Windows runners (all
steps run in `bash`). The packaging tools themselves (`npm`, `cargo`, `dotnet`,
`swift`, the Maven or Gradle build) are not installed by the action: set them up in
earlier steps, as for a local build.

## Usage

```yaml
- uses: kduma-OSS/serverless-registry-cli/action@v1
  with:
    ecosystem: composer
    registry: https://packages.example.com
    token: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}
```

### Authentication

With `token`, the action exports it as `REGISTRY_TOKEN`. Use a `publish` credential of
the project, stored as a repository or environment secret.

Without `token`, the action authenticates with the job's GitHub Actions OIDC token
(`sreg … publish --oidc`). No secret is needed, but the job must be allowed to request
the token, and the registry must have a `publish` credential with a `gh` secret
trusting the repository (see
[A publish credential for GitHub Actions](https://github.com/kduma-OSS/serverless-registry-worker/blob/main/docs/deployment.md#a-publish-credential-for-github-actions-oidc-no-secret) in the Worker's deployment guide):

```yaml
jobs:
  publish:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      id-token: write
    steps:
      - uses: actions/checkout@v4
      - uses: kduma-OSS/serverless-registry-cli/action@v1
        with:
          ecosystem: composer
          registry: https://packages.example.com
          # oidc-project: acme-sdk   # optional; by default the registry finds the
          #                          # project from the repository
```

### Inputs

| Input | Default | Description |
| --- | --- | --- |
| `ecosystem` | – (required) | `composer`, `npm`, `cargo`, `nuget`, `generic`, `swift` or `maven`. |
| `registry` | – (required) | Base URL of the registry (`REGISTRY_URL`). |
| `token` | empty | Publish token (`REGISTRY_TOKEN`). Empty: OIDC (`--oidc`, needs `id-token: write`). |
| `oidc-project` | empty | With OIDC: the project of the token (`--oidc-project`). |
| `version` | empty | Version to publish (`--version`); a leading `v` is dropped, so `${{ github.ref_name }}` works. Required for `generic`. |
| `tag-prefix` | empty | Take the version from the tag on `HEAD` with this prefix (`--tag-prefix`), for monorepos. |
| `path` | empty | composer, swift: package directory (`--path`). |
| `workspace` | empty | npm: workspace to publish (`--workspace`). |
| `package` | empty | cargo: crate to publish (`--package`); generic: the package name. |
| `project` | empty | nuget: project file (`--project`). |
| `from-dir` | empty | maven: local repository the build deployed to (`--from-dir`). |
| `coords` | empty | maven: `<groupId>:<artifactId>` to publish (`--coords`). |
| `scope` | empty | swift: registry scope (`--scope`, required by `sreg swift publish`). |
| `generic-files` | empty | generic: files to publish, one per line; glob patterns are expanded. |
| `generic-labels` | empty | generic: `key=value` labels for every file, one per line or comma-separated. |
| `extra-flags` | empty | Additional `sreg … publish` flags before `--`, e.g. `--tag beta` or `--no-symbols` (shell-like quoting, no expansion). |
| `args` | empty | Arguments for the packaging tool, placed after `--`, e.g. `--locked` or `-c Release`. |
| `dry-run` | `false` | Stop after the registry accepted the upload declaration (`--dry-run`). |
| `skip-existing` | `true` | Succeed when the version is already published (`--skip-existing`), so re-running a workflow is safe. |
| `sreg-version` | `action/VERSION` | Release of `sreg` to install, e.g. `v0.1.0`. |

An input that does not apply to the selected ecosystem fails the step instead of being
ignored. Without `version` and `tag-prefix`, `sreg` takes the version from the
`v<semver>` tag on `HEAD` (npm, cargo and nuget fall back to their manifest), so
checking out a tag is usually enough.

### Outputs

| Output | Description |
| --- | --- |
| `name` | Name of the package. |
| `version` | Version. |
| `url` | URL of the published version; empty for a dry run or a skipped version. |
| `skipped` | `true` when the version was already published and `skip-existing` let the step pass. |

The step fails with `sreg`'s exit code: `1` for an error, `2` when the version exists
and `skip-existing` is `false`, `3` when the registry requires a newer `sreg` (raise
`sreg-version` or update the action).

## Examples

All examples run on a pushed release tag:

```yaml
on:
  push:
    tags: ["v*"]
```

### Composer

```yaml
- uses: actions/checkout@v4
- uses: kduma-OSS/serverless-registry-cli/action@v1
  with:
    ecosystem: composer
    registry: https://packages.example.com
    token: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}
```

### npm

```yaml
- uses: actions/checkout@v4
- uses: actions/setup-node@v4
  with:
    node-version: 22
- run: npm ci
- uses: kduma-OSS/serverless-registry-cli/action@v1
  with:
    ecosystem: npm
    registry: https://packages.example.com
    token: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}
    extra-flags: --tag next
    args: --ignore-scripts
```

### cargo

```yaml
- uses: actions/checkout@v4
- uses: dtolnay/rust-toolchain@stable
- uses: kduma-OSS/serverless-registry-cli/action@v1
  with:
    ecosystem: cargo
    registry: https://packages.example.com
    token: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}
    args: --locked
```

### NuGet

```yaml
- uses: actions/checkout@v4
- uses: actions/setup-dotnet@v4
  with:
    dotnet-version: 8.0.x
- uses: kduma-OSS/serverless-registry-cli/action@v1
  with:
    ecosystem: nuget
    registry: https://packages.example.com
    token: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}
    project: src/Acme.Sdk/Acme.Sdk.csproj
    args: -c Release
```

### Swift

```yaml
- uses: actions/checkout@v4
- uses: kduma-OSS/serverless-registry-cli/action@v1
  with:
    ecosystem: swift
    registry: https://packages.example.com
    token: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}
    scope: acme
```

### Maven and Gradle

Deploy into a local directory first, then publish from it:

```yaml
- uses: actions/checkout@v4
- uses: actions/setup-java@v4
  with:
    distribution: temurin
    java-version: 21
- run: mvn -B deploy -DaltDeploymentRepository=local::file:./out
- uses: kduma-OSS/serverless-registry-cli/action@v1
  with:
    ecosystem: maven
    registry: https://packages.example.com
    token: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}
    from-dir: out
    coords: com.acme:sdk
```

### Generic files

A CI matrix where every job adds its own build of the same version:

```yaml
jobs:
  build:
    strategy:
      matrix:
        include:
          - { os: ubuntu-latest, label: linux }
          - { os: macos-latest, label: macos }
          - { os: windows-latest, label: windows }
    runs-on: ${{ matrix.os }}
    permissions:
      contents: read
      id-token: write
    steps:
      - uses: actions/checkout@v4
      - run: ./build.sh # writes dist/acme-tool-*
        shell: bash
      - uses: kduma-OSS/serverless-registry-cli/action@v1
        with:
          ecosystem: generic
          registry: https://packages.example.com
          package: tools/acme-tool
          version: ${{ github.ref_name }}
          generic-files: |
            dist/acme-tool-*
          generic-labels: os=${{ matrix.label }}
```

### Monorepo

Tag every package with its own prefix (`sdk/v1.2.0`, `client/v0.4.0`) and derive the
package from the pushed tag:

```yaml
on:
  push:
    tags: ["*/v*"]

jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - id: tag
        run: echo "package=${GITHUB_REF_NAME%/v*}" >> "$GITHUB_OUTPUT"
      - uses: kduma-OSS/serverless-registry-cli/action@v1
        with:
          ecosystem: composer
          registry: https://packages.example.com
          token: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}
          path: packages/${{ steps.tag.outputs.package }}
          tag-prefix: ${{ steps.tag.outputs.package }}/v
```

For cargo use `package: <crate>`, for npm `workspace: <name>`, for NuGet
`project: <path>.csproj`, for Maven `coords: <group>:<artifact>`, together with the
same `tag-prefix`.

### Using the outputs

```yaml
- id: publish
  uses: kduma-OSS/serverless-registry-cli/action@v1
  with:
    ecosystem: cargo
    registry: https://packages.example.com
    token: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}
- if: steps.publish.outputs.skipped != 'true'
  run: echo "Published ${{ steps.publish.outputs.name }} ${{ steps.publish.outputs.version }}: ${{ steps.publish.outputs.url }}"
```

## Versioning

The action lives in the `action/` directory of the `sreg` repository and is referenced
as `kduma-OSS/serverless-registry-cli/action@<ref>`. `v1` is a moving tag: it is moved
to every release whose action inputs and outputs stay compatible, and `action/VERSION`
in that commit names the `sreg` release the action installs. Pin a release tag
(`@v0.1.0`) or a commit SHA for fully reproducible workflows.
See [`docs/releasing.md`](../docs/releasing.md).
