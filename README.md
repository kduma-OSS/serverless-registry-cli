# sreg

`sreg` is the command-line tool of **Serverless Registry**, a private, serverless
package registry for Composer, npm, cargo, NuGet, Swift, Maven and generic files hosted
on Cloudflare (Workers, R2 and KV).

`sreg` is the only way to publish a version. It packages the project (natively for
Composer, through `npm pack`, `cargo package`, `dotnet pack` and `swift package
archive-source` for npm, cargo, NuGet and Swift; Maven and Gradle builds deploy into a
local directory that `sreg` publishes from), uploads the files straight to R2 through
presigned URLs and commits the version through the registry's API. Published versions are immutable. It also downloads generic
files and writes credentials for package managers, including GitHub Actions OIDC
tokens. Administration (credentials, yank, delete) is done in the registry's web panel;
`sreg` has no administrative commands.

The registry itself lives in the
[`serverless-registry-worker`](https://github.com/kduma-OSS/serverless-registry-worker)
repository; its
[deployment guide](https://github.com/kduma-OSS/serverless-registry-worker/blob/main/docs/deployment.md)
covers setting it up and creating credentials.

## Install

Release binaries for macOS (arm64, x86_64), Linux (x86_64 glibc and musl, arm64) and
Windows (x86_64) are published on the
[releases page](https://github.com/kduma-OSS/serverless-registry-cli/releases).

With the installer of the latest release (installs `sreg` into `~/.cargo/bin`):

```
# macOS and Linux
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/kduma-OSS/serverless-registry-cli/releases/latest/download/sreg-cli-installer.sh | sh

# Windows (PowerShell)
powershell -ExecutionPolicy Bypass -c "irm https://github.com/kduma-OSS/serverless-registry-cli/releases/latest/download/sreg-cli-installer.ps1 | iex"
```

From source, with Rust 1.85 or newer:

```
cargo install --git https://github.com/kduma-OSS/serverless-registry-cli --tag v0.1.0 --locked sreg-cli
```

Or download the archive for your platform by hand and verify it against the `.sha256`
file published next to it:

```
version=v0.1.0
target=x86_64-unknown-linux-musl   # see the release for the other targets
base=https://github.com/kduma-OSS/serverless-registry-cli/releases/download/$version
curl -LO "$base/sreg-cli-$target.tar.xz" -LO "$base/sreg-cli-$target.tar.xz.sha256"
sha256sum -c "sreg-cli-$target.tar.xz.sha256"   # macOS: shasum -a 256 -c …
tar -xJf "sreg-cli-$target.tar.xz"
install "sreg-cli-$target/sreg" ~/.local/bin/
```

On Windows, the archive is `sreg-cli-x86_64-pc-windows-msvc.zip`; compare
`CertUtil -hashfile sreg-cli-x86_64-pc-windows-msvc.zip SHA256` with the `.sha256` file.
In GitHub Actions, use the [publish action](#github-action), which does all of this.

## Commands

| Command | What it does |
| --- | --- |
| `sreg composer publish [--version <v> \| --tag-prefix <p>] [--path .]` | Archives the package directory `--path` (`git archive HEAD:<path>`, honouring `export-ignore`, falling back to a zip of the directory without `vendor/` and `.git/`) and reads its `composer.json`. The version comes from `--version` or the release tag on `HEAD` (`v1.2.3` → `1.2.3`, or `<prefix>1.2.3` with `--tag-prefix`); it must be semver with an optional `alpha`, `beta` or `rc` prerelease, `dev-*` versions are rejected. Arguments after `--` are rejected. See [Publishing a Composer package](#publishing-a-composer-package). |
| `sreg npm publish [--version <v> \| --tag-prefix <p>] [--tag <t>] [--workspace <w>] [-- <npm pack args>]` | Writes the version into `package.json` with `npm pkg set version=<v>` (when one is requested), runs `npm pack --json --pack-destination <tmp>` in the current directory and reads `package/package.json` from the tarball. Scoped packages only. `--tag` sets the dist-tag. See [Publishing an npm package](#publishing-an-npm-package). |
| `sreg cargo publish [--version <v> \| --tag-prefix <p>] [--package <crate>] [-- <cargo package args>]` | Writes the version into the crate's `Cargo.toml` (formatting preserved), runs `cargo package --no-verify --allow-dirty -p <crate> --target-dir <tmp>` and describes the crate with a sparse-index line built from `cargo metadata`. See [Publishing a crate](#publishing-a-crate). |
| `sreg nuget publish [--version <v> \| --tag-prefix <p>] [--project <csproj>] [--no-symbols] [-- <dotnet pack args>]` | Runs `dotnet pack <project> -o <tmp>` (the version as `-p:PackageVersion=<v>`), reads the `.nuspec` from the `.nupkg` and publishes the `.nupkg` and, unless `--no-symbols`, the `.snupkg`. See [Publishing a NuGet package](#publishing-a-nuget-package). |
| `sreg swift publish --scope <scope> [--version <v> \| --tag-prefix <p>] [--path .] [--repository-url <url>] [-- <archive-source args>]` | Describes the package with `swift package dump-package`, archives it with `swift package archive-source -o <tmp>/<name>-<version>.zip` and publishes the zip with `Package.swift` and its `Package@swift-*.swift` variants as `<scope>.<name>`. The version comes from `--version` or the release tag on `HEAD`. See [Publishing a Swift package](#publishing-a-swift-package). |
| `sreg maven publish [--version <v> \| --tag-prefix <p>] --from-dir <dir> [--coords <group:artifact>]` | Publishes one artifact version from a local Maven repository the build deployed to (`mvn deploy -DaltDeploymentRepository=local::file:./out`, or Gradle's `publish` to a `file://` repository): the effective POM as the manifest, the archive, sources, javadoc, Gradle module metadata and other files of the version directory. Arguments after `--` are rejected. See [Publishing Maven artifacts](#publishing-maven-artifacts). |
| `sreg generic publish <name> <version> <file>... [--label k=v]... [--meta k=v]... [--meta-file <json>] [--spec <files.json>] [--no-append]` | Publishes files as one version of a generic package. `--label` applies to every file; `--spec` lists files with their own labels instead. Files are appended to an already published version (within the registry's append window) unless `--no-append` is given. Arguments after `--` are rejected. |
| `sreg generic get <name> [<version>\|<constraint>\|latest] [--label k=v]... [--sole] [--out <path>] [--prerelease]` | Downloads the files of a version matching the labels into `--out` after verifying size and SHA-256 and prints their paths; `--json` prints the feed entry without downloading. A `<version>` that is not an exact semver version is a constraint (npm syntax: `^1.2`, `">=1 <2"`, `1.x`) that the registry resolves to its highest matching version (`/generic/<name>/latest?version=<constraint>`, prereleases only with `--prerelease`); the resolved version is printed on stderr. `--sole` fails unless the label filter selects exactly one file (`/generic/<name>/<version>/-/download`): when several match, the candidates are listed and `sreg` exits with `1`; `--out` may then also name the file to write. |
| `sreg whoami` | Shows the identity and grants of the current token (`GET /api/v1/whoami`). |
| `sreg auth <composer\|npm\|cargo\|nuget\|swift\|maven> [--oidc [--project <p>]] [--write <file>] [--registry-name <n>]` | Writes credentials to `auth.json`, `.npmrc`, `credentials.toml`, `nuget.config`, `.netrc` or `settings.xml`, or prints them for the environment (e.g. as `COMPOSER_AUTH`, a `dotnet nuget add source` or `swift package-registry login` command, or Maven `settings.xml` and Gradle snippets). With `--oidc`, inside a GitHub Actions job with `id-token: write`, it fetches a JWT for the registry and builds a `<project>:gh.<jwt>` token (`_:gh.<jwt>` without `--project`: the registry then finds the project from the repository the token was issued to); without it, it uses `REGISTRY_TOKEN`. See [`docs/cli-auth.md`](docs/cli-auth.md). |
| `sreg version` | Prints the version of `sreg` and, with `--registry`, the registry's version and the minimum client version it accepts. |

Every publish command computes the SHA-256 digest and size of each file, calls `init`,
uploads every file with `PUT` (3 attempts with backoff; a new `init` when the presigned
URLs expire) and calls `commit`. All of them accept `--dry-run` (stop after `init`),
`--skip-existing` (when the version, or an appended file, is already published, print
`version already published, skipping` and exit with `0` instead of `2`; with `--json`
the output is `{ "skipped": true, "eco", "name", "version" }`), and `--oidc` with an
optional `--oidc-project <p>` (authenticate with a GitHub Actions OIDC token instead of
`REGISTRY_TOKEN`: `<p>:gh.<jwt>`, or `_:gh.<jwt>` without `--oidc-project`, which lets
the registry find the project from the repository the token was issued to;
`sreg generic get` accepts them too; `sreg auth` calls the project `--project`). The composer, npm, cargo, nuget, swift and maven publish commands also
accept `--tag-prefix <p>` instead of `--version`: the version is then taken from the tag
on `HEAD` that starts with the prefix.

### Packaging: what `sreg` calls

`sreg` never runs the native tools' `publish`; it runs their packing commands, reads the
result and uploads it itself. The flags `sreg` sets are reserved and rejected after
`--`; everything else after `--` goes verbatim to the tool
(`sreg cargo publish --version 1.2.0 -- --features ssl --locked`).

| Ecosystem | Command | Flags set by `sreg` | Examples of flags to pass after `--` |
| --- | --- | --- | --- |
| composer | `git archive --format=zip HEAD:<path>` (fallback: a zip of the directory without `vendor/` and `.git/`) | – | – |
| npm | `npm pack` | `--json`, `--pack-destination <tmp>`, `-w <pkg>` from `--workspace` | `--ignore-scripts` |
| cargo | `cargo package` | `--no-verify`, `--allow-dirty`, `-p <crate>` from `--package`, `--target-dir <tmp>` | `--features`, `--locked`, `--manifest-path` |
| nuget | `dotnet pack <project>` | `-o <tmp>`, `-p:PackageVersion=<v>` from `--version` | `-c Release`, `--no-build`, `--include-symbols` |
| swift | `swift package dump-package`, then `swift package archive-source` (both in `--path`) | `-o <tmp>/<name>-<version>.zip`; `--package-path` is reserved too (use `--path`) | `--verbose` |
| maven | none – the build deploys into `--from-dir` first | – | – |
| generic | none – the files are ready | – | – |

### Monorepos

Every publish command publishes one package in one version and works from any
directory of the repository; several packages are several calls (a loop or a matrix).

- composer: `--path packages/foo` archives `HEAD:packages/foo`.
- npm: run in the root of the npm project and select the package with
  `--workspace <name|path>` (`npm pack -w <…>`, `npm pkg set version=<v> -w <…>`).
- cargo: `--package <crate>` (`cargo package -p <crate>`); without it, the crate of the
  current directory, or the only crate of the workspace. A crate with
  `version.workspace = true` cannot get its own `--version`: set the version in
  `[workspace.package]` instead.
- nuget: `--project <path to .csproj>`; without it, the single project file in the
  current directory. Solutions (`.sln`) are refused, because they produce several
  packages.
- swift: `--path packages/Kit` (the directory with `Package.swift`).
- maven: `--coords <groupId>:<artifactId>` selects one artifact when the build deployed
  several modules into `--from-dir`.
- Version: `--version <v>`, or the release tag on `HEAD`. Tag the packages with a
  prefix (`sdk/v1.2.0`, `cli-v0.4.0`) and pass `--tag-prefix sdk/v`; without
  `--tag-prefix` only `v<semver>` tags are considered, and more than one on `HEAD` is an
  error. npm, cargo and nuget fall back to the version in the project's own manifest
  when `HEAD` carries no `v<semver>` tag (composer and swift require a tag or
  `--version`; maven falls back to the only version directory in `--from-dir`).

When `sreg` writes the version into `package.json` or `Cargo.toml` it says so on stderr
(`note: set the version in …`); the change is left in the working tree.

Global options: `--registry <url>`, `--token <token>`, `--json` (machine-readable
output only on stdout) and `-v`/`--verbose` (logs of `sreg` itself to stderr; tokens
and presigned URLs are never logged).

## Publishing a Composer package

Tag the release and publish from the package directory, or from anywhere with `--path`:

```
git tag v1.2.3
sreg composer publish                    # publishes acme/sdk 1.2.3 from the tag
sreg composer publish --version 1.2.4    # or name the version explicitly
sreg composer publish --dry-run          # let the registry check everything, upload nothing

# a repository with several packages, tagged sdk/v1.2.3, client/v0.4.0, ...
sreg composer publish --path packages/sdk --tag-prefix sdk/v
```

- **Name**: `name` in `composer.json` is required and must be a valid Composer name
  (`vendor/package` in lower case).
- **Version**: without `--version`, the release tag pointing at `HEAD`
  (`git tag --points-at HEAD`) is used: a tag `v<semver>` (`v1.2.3` → `1.2.3`), or with
  `--tag-prefix <p>` a tag `<p><semver>` (`--tag-prefix sdk/v`: `sdk/v1.2.3` → `1.2.3`;
  `--tag-prefix ""` for bare `1.2.3` tags). Other tags are ignored. No such tag, or more
  than one on `HEAD`, is an error; so is combining `--version` with `--tag-prefix`.
  Versions are `MAJOR.MINOR.PATCH`, optionally followed by `-alpha`, `-beta` or `-rc` and a number
  (`1.0.0-rc1`, `1.0.0-beta.2`); `dev-*` branches, other prerelease labels and build
  metadata are rejected. If `composer.json` declares a `version`, it must be the one being
  published; usually it is best left out.
- **Archive**: inside a git repository, the archive is `git archive HEAD:<path>`, where
  `<path>` is the package directory relative to the repository root: the package
  directory becomes the root of the archive, only committed files are published and
  paths marked `export-ignore` are left out. Put the `export-ignore` rules into the
  package directory's own `.gitattributes` (rules for the subdirectory in the
  repository's top-level `.gitattributes` do not apply to a `HEAD:<path>` archive).
  `composer.json` must be committed and equal to the one on disk. Keep development
  files out of the archive like this:

  ```
  # .gitattributes
  /tests              export-ignore
  /.github            export-ignore
  /.gitattributes     export-ignore
  /phpunit.xml.dist   export-ignore
  ```

  Outside a git repository (or without `git` installed), the directory is zipped
  instead, without `vendor/` and `.git`; `.gitattributes` is not consulted then.
- The archive is uploaded as `<package>-<version>.zip` (`acme/sdk` 1.2.3 →
  `sdk-1.2.3.zip`) and `composer.json` is sent as the version's manifest.

In GitHub Actions, publish on tag pushes, with a token from a secret or with OIDC:

```yaml
on:
  push:
    tags: ["v*"]

jobs:
  publish:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      id-token: write   # only for --oidc
    steps:
      - uses: actions/checkout@v4
      - run: curl --proto '=https' --tlsv1.2 -LsSf https://github.com/kduma-OSS/serverless-registry-cli/releases/latest/download/sreg-cli-installer.sh | sh
      - run: sreg composer publish --oidc --oidc-project acme-sdk
        env:
          REGISTRY_URL: https://packages.example.com
```

## Publishing an npm package

```
sreg npm publish --version 1.2.3                  # writes 1.2.3 into package.json first
sreg npm publish                                  # the v<semver> tag on HEAD, or package.json
sreg npm publish --tag beta -- --ignore-scripts   # dist-tag beta, no lifecycle scripts

# npm workspaces, from the repository root, tagged sdk/v1.2.3
sreg npm publish --workspace packages/sdk --tag-prefix sdk/v
```

- Only scoped packages (`@scope/name`) are served by the registry; the version must be
  semver.
- The tarball is uploaded under the name `npm pack` gave it (`@acme/sdk` 1.2.3 →
  `acme-sdk-1.2.3.tgz`), and `package/package.json` from the tarball, without fields
  starting with `_`, is the manifest.
- `--tag` sets the dist-tag (`latest` by default, `next` for prereleases, decided by the
  registry).

## Publishing a crate

```
sreg cargo publish --version 1.4.2                # writes 1.4.2 into Cargo.toml first
sreg cargo publish --package acme-core --tag-prefix core/v -- --locked
```

- `cargo metadata --no-deps` finds the crate: `--package`, else the crate whose
  `Cargo.toml` is nearest to the current directory, else the only crate of the
  workspace. `--manifest-path` after `--` is honoured by both `cargo metadata` and
  `cargo package`.
- The `.crate` is uploaded as `<name>-<version>.crate`. The manifest is the crate's line
  of the cargo sparse index, built from `cargo metadata`: dependencies from crates.io
  point at `https://github.com/rust-lang/crates.io-index`, dependencies from any other
  registry (declare them with `registry = "<name>"`) are taken to come from this
  registry. Path-only dev-dependencies are left out, as `cargo publish` does.
- Make the registry known to cargo for consumers with `sreg auth cargo`.

## Publishing a NuGet package

```
sreg nuget publish --version 1.2.0 -- -c Release
sreg nuget publish --project src/Acme.Sdk/Acme.Sdk.csproj --tag-prefix sdk/v --no-symbols
```

- The `.nupkg` is uploaded as `<id>.<version>.nupkg` in lower case, and the `.snupkg`,
  when `dotnet pack` produced one (e.g. with `-p:IncludeSymbols=true
  -p:SymbolPackageFormat=snupkg`), as `<id>.<version>.snupkg` unless `--no-symbols` is
  given. Legacy `.symbols.nupkg` packages are ignored.
- The manifest holds the id, version, description, authors, project URL, license
  expression, tags and dependency groups of the `.nuspec`, plus the `.nuspec` itself.

## Publishing a Swift package

```
git tag v1.2.0
sreg swift publish --scope acme                       # acme.DemoKit 1.2.0 from the tag
sreg swift publish --scope acme --version 1.3.0-beta.1
sreg swift publish --scope acme --path packages/Kit --tag-prefix kit/v
```

- **Name**: Swift packages have no scope of their own, so `--scope` is required
  (1 to 39 characters of `A-Z`, `a-z`, `0-9` and `-`); the name is `name` in
  `Package.swift` as reported by `swift package dump-package` (1 to 100 characters of
  `A-Z`, `a-z`, `0-9`, `_` and `-`). The package is published as `<scope>.<name>`.
- **Version**: `Package.swift` has no version, so it comes from `--version` or the release
  tag on `HEAD` (`v1.2.0`, or `<prefix>1.2.0` with `--tag-prefix`); without either, the
  command fails. Versions are semver 2 without a leading `v`.
- **Files**: the zip that `swift package archive-source` writes (it archives the
  committed state of a git repository) is uploaded as `<name>-<version>.zip`; `Package.swift`
  and every `Package@swift-<version>.swift` next to it are uploaded as `manifest` files.
- **Manifest**: the tools version, the repository URL (`--repository-url`, or the URL of
  the `origin` remote without any credentials in it) and the complete output of
  `swift package dump-package`.
- Consumers point SwiftPM at `<registry>/swift` with
  `swift package-registry set <registry>/swift --scope acme` and log in with
  `sreg auth swift` (see [`docs/cli-auth.md`](docs/cli-auth.md)).

In GitHub Actions (macOS runners come with Swift):

```yaml
on:
  push:
    tags: ["v*"]

jobs:
  swift:
    runs-on: macos-latest
    permissions:
      contents: read
      id-token: write
    steps:
      - uses: actions/checkout@v4
      - run: curl --proto '=https' --tlsv1.2 -LsSf https://github.com/kduma-OSS/serverless-registry-cli/releases/latest/download/sreg-cli-installer.sh | sh
      - run: sreg swift publish --scope acme --oidc --skip-existing
        env:
          REGISTRY_URL: https://packages.example.com
```

## Publishing Maven artifacts

`sreg` does not run Maven or Gradle. Let the build deploy into a local directory (a
Maven-2 file repository) and publish one artifact version from it:

```
# Maven
mvn -B deploy -DaltDeploymentRepository=local::file:./out
sreg maven publish --from-dir out --coords com.acme:sdk

# Gradle, with a maven-publish repository pointing at a file:// URL (below)
./gradlew publish
sreg maven publish --from-dir build/repo --coords com.acme:sdk --version 1.4.0
```

With Gradle, declare the local repository next to the publication:

```kotlin
// build.gradle.kts
plugins {
    `java-library`
    `maven-publish`
}

java {
    withSourcesJar()
    withJavadocJar()
}

publishing {
    publications {
        create<MavenPublication>("maven") {
            from(components["java"])
        }
    }
    repositories {
        maven {
            name = "local"
            url = uri(layout.buildDirectory.dir("repo")) // a file:// URL
        }
    }
}
```

- **Artifact**: `sreg` looks for `<group/as/path>/<artifactId>/<version>/<artifactId>-<version>.pom`
  under `--from-dir`. When the build deployed several artifacts (a multi-module build),
  select one with `--coords <groupId>:<artifactId>`; publish each module with its own call.
  The package name in the registry is `<groupId>:<artifactId>`.
- **Version**: `--version` or the release tag on `HEAD` selects the version directory;
  without either, the directory must hold exactly one version. `sreg` does not change the
  build: the version the build used must be the one being published. `-SNAPSHOT` versions
  are rejected.
- **POM**: it must be an effective POM: a `<parent>`, or a `${…}` property in the
  coordinates, packaging or dependencies, is rejected. Gradle writes resolved POMs; with
  Maven, deploy a flattened POM (for example with the `flatten-maven-plugin`) and check
  it with `mvn help:effective-pom`. The POM's `groupId`, `artifactId` and `version` must
  match its location.
- **Files**: every file of the version directory except checksums (`.sha1`, `.md5`,
  `.sha256`, `.sha512`; the registry serves its own) and `maven-metadata*.xml`, with a role taken
  from its name: `<artifactId>-<version>.pom` is the `pom`, the file with the packaging's
  extension the `archive` (`jar` for `jar`, `maven-plugin`, `ejb` and `bundle`; `aar`,
  `war`, `ear` as they are; none for `pom` packaging), `-sources.jar` the `sources`,
  `-javadoc.jar` the `javadoc`, `.module` the Gradle `module` metadata, and anything
  else (classified jars, distributions, `.asc` signatures) `extra`.
- **Manifest**: `groupId`, `artifactId`, `version`, `packaging` (default `jar`), the POM
  itself, and the project's dependencies with their `scope` (default `compile`) and
  `optional` flag (`<dependencyManagement>` is ignored).
- Consumers configure credentials with `sreg auth maven` (a `settings.xml` `<server>` or a
  Gradle `credentials` block, see [`docs/cli-auth.md`](docs/cli-auth.md)).

In GitHub Actions:

```yaml
on:
  push:
    tags: ["v*"]

jobs:
  maven:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-java@v4
        with:
          distribution: temurin
          java-version: 21
      - run: mvn -B deploy -DskipTests -DaltDeploymentRepository=local::file:./out
      - run: curl --proto '=https' --tlsv1.2 -LsSf https://github.com/kduma-OSS/serverless-registry-cli/releases/latest/download/sreg-cli-installer.sh | sh
      - run: sreg maven publish --from-dir out --coords com.acme:sdk --skip-existing
        env:
          REGISTRY_URL: https://packages.example.com
          REGISTRY_TOKEN: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}

  gradle:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      id-token: write
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-java@v4
        with:
          distribution: temurin
          java-version: 21
      - uses: gradle/actions/setup-gradle@v4
      - run: ./gradlew publish -Pversion="${GITHUB_REF_NAME#v}"
      - run: sreg maven publish --from-dir build/repo --oidc --skip-existing
        env:
          REGISTRY_URL: https://packages.example.com
```

## GitHub Action

The composite action in [`action/`](action/README.md) installs the `sreg` release it was
published with, verifies its checksum and publishes the package of the repository:

```yaml
- uses: kduma-OSS/serverless-registry-cli/action@v1
  with:
    ecosystem: composer
    registry: https://packages.example.com
    token: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}
```

Without `token`, it authenticates with the job's OIDC token (the job needs
`permissions: id-token: write`). The action takes the ecosystem's options as inputs
(`version`, `tag-prefix`, `path`, `workspace`, `package`, `project`, `from-dir`,
`coords`, `scope`, `args`, …), skips already published versions by default and sets the
outputs `name`, `version`, `url` and `skipped`. See [`action/README.md`](action/README.md)
for every input and examples for each ecosystem and for monorepos.

## Publishing from GitHub Actions

Without the [action](#github-action), install `sreg` in the job and call it directly.
Publish on tag pushes; with `--oidc` no long-lived secret is needed (the job needs
`id-token: write`, and the registry a `publish` credential with a `gh` secret for the
repository, see
[A publish credential for GitHub Actions](https://github.com/kduma-OSS/serverless-registry-worker/blob/main/docs/deployment.md#a-publish-credential-for-github-actions-oidc-no-secret) in the Worker's deployment guide):

```yaml
on:
  push:
    tags: ["v*"]

jobs:
  npm:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      id-token: write
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 22
      - run: curl --proto '=https' --tlsv1.2 -LsSf https://github.com/kduma-OSS/serverless-registry-cli/releases/latest/download/sreg-cli-installer.sh | sh
      - run: sreg npm publish --oidc --oidc-project acme-sdk --skip-existing
        env:
          REGISTRY_URL: https://packages.example.com

  cargo:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: curl --proto '=https' --tlsv1.2 -LsSf https://github.com/kduma-OSS/serverless-registry-cli/releases/latest/download/sreg-cli-installer.sh | sh
      - run: sreg cargo publish --skip-existing -- --locked
        env:
          REGISTRY_URL: https://packages.example.com
          REGISTRY_TOKEN: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}

  nuget:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-dotnet@v4
        with:
          dotnet-version: 8.0.x
      - run: curl --proto '=https' --tlsv1.2 -LsSf https://github.com/kduma-OSS/serverless-registry-cli/releases/latest/download/sreg-cli-installer.sh | sh
      - run: sreg nuget publish --project src/Acme.Sdk/Acme.Sdk.csproj -- -c Release
        env:
          REGISTRY_URL: https://packages.example.com
          REGISTRY_TOKEN: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}
```

In a monorepo, tag each crate with its own prefix (`acme-core/v1.2.0`) and derive the
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
      - uses: dtolnay/rust-toolchain@stable
      - run: curl --proto '=https' --tlsv1.2 -LsSf https://github.com/kduma-OSS/serverless-registry-cli/releases/latest/download/sreg-cli-installer.sh | sh
      - run: |
          crate="${GITHUB_REF_NAME%/v*}"
          sreg cargo publish --package "$crate" --tag-prefix "$crate/v" --skip-existing
        env:
          REGISTRY_URL: https://packages.example.com
          REGISTRY_TOKEN: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}
```

## Configuration

There is no configuration file. Configuration comes only from flags and environment
variables:

| Flag | Environment variable | Description |
| --- | --- | --- |
| `--registry <url>` | `REGISTRY_URL` | Base URL of the registry, e.g. `https://packages.example.com`; it may carry the token (below). |
| `--token <token>` | `REGISTRY_TOKEN` | Token in the form `[<source>.]<project>:[<type>.]<secret>`. |

In CI, pass both as environment variables from repository secrets.

### Credentials in the registry URL

As with Composer, the registry URL may carry the token in its userinfo instead of
`--token`/`REGISTRY_TOKEN`:

```sh
export REGISTRY_URL='https://shop-frontend:k7Qx2mN9pLw4@packages.example.com/'
sreg whoami
```

The token is `<login>:<password>` after percent-decoding both parts, so a `@`, `:`, `/`,
`?`, `#` or `%` in the login or the password must be percent-encoded (`%40`, `%3A`,
`%2F`, `%3F`, `%23`, `%25`). Rules:

- Both a login and a password are required (`https://shop-frontend@…` is an error).
- Credentials are only accepted over `https`; plain `http` is refused, except for
  `localhost` and `127.0.0.1` (local development).
- `sreg` strips the credentials before using the URL: requests, output, logs and `--json`
  only ever show `https://packages.example.com/`.
- Precedence: `--oidc` wins over everything; otherwise `--token`/`REGISTRY_TOKEN` and the
  URL's credentials must agree when both are given (an error otherwise), and the URL's
  credentials are used when there is no explicit token.
- `sreg version` never sends them: its endpoint is anonymous.

Treat such a URL as a secret: in GitHub Actions, store it in `secrets.` rather than in
the workflow file.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success. |
| `1` | Generic error (including invalid usage). |
| `2` | Already published: the version exists (`409 version_exists`), or a file being appended exists (`409 file_exists`). With `--skip-existing` this is `0`. |
| `3` | This client is too old for the registry (`426`); the message names the minimum version and links to the releases. |

## Compatibility

`sreg` and the registry are released independently. `sreg` sends its version in
`X-Client-Version`; the registry answers with the oldest version it accepts in
`X-Min-Client-Version` and rejects older clients with `426`, which `sreg` reports with
the minimum version and a link to the releases (exit code `3`).
`sreg version --registry <url>` shows the registry's version and its minimum client
version. The registry keeps its API (`/api/v1`) for at least one minor release after a
change, and raises the minimum only after a `sreg` release that satisfies it is
available, so updating `sreg` (or the action) is enough. See
[`docs/releasing.md`](docs/releasing.md).

## Building a smaller binary

Each ecosystem is a Cargo feature of the `sreg-cli` crate: `composer`, `npm`, `cargo`,
`nuget`, `generic`, `swift` and `maven`, all enabled by default. A binary for a single ecosystem:

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

## Releasing

Releases are cut from `main` with the manual **Bump and release** workflow
(`.github/workflows/bump-and-release.yml`): it computes the next version (`prerelease`,
`patch`, `minor`, `custom`, …), updates `Cargo.toml`, `Cargo.lock`, `action/VERSION` and
`CHANGELOG.md`, commits and tags `v<version>`, and starts the cargo-dist `Release`
workflow. Tags must match the package version; use the Bump and release workflow rather
than tagging by hand. See [`docs/releasing.md`](docs/releasing.md).

## License

MIT, see [`LICENSE`](LICENSE).
