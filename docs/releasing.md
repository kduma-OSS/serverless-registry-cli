# Releasing sreg

`sreg` is released from this repository with [cargo-dist](https://opensource.axo.dev/cargo-dist/)
(`dist`). A release is a git tag `vX.Y.Z` that matches the version in `Cargo.toml`;
everything else (binaries, checksums, installers, the GitHub Release) is produced by the
`Release` workflow (`.github/workflows/release.yml`). Releases are cut with the manual
**Bump and release** workflow (`.github/workflows/bump-and-release.yml`), which sets the
version, rolls the changelog, tags and starts the `Release` workflow. The GitHub Action,
[`kduma-OSS/serverless-registry-publish-action`](https://github.com/kduma-OSS/serverless-registry-publish-action),
lives in its own repository and is released on its own; it installs these release
binaries, and a stable CLI release makes it release itself with the new version as its
default ([below](#releasing-the-github-action)).

## What a release produces

For every tag, `dist` builds `sreg-cli` (the only package with
`[package.metadata.dist] dist = true`; the library crates are not released) for:

| Target | Archive |
| --- | --- |
| `aarch64-apple-darwin` | `sreg-cli-aarch64-apple-darwin.tar.xz` |
| `x86_64-apple-darwin` | `sreg-cli-x86_64-apple-darwin.tar.xz` |
| `x86_64-unknown-linux-gnu` | `sreg-cli-x86_64-unknown-linux-gnu.tar.xz` |
| `x86_64-unknown-linux-musl` | `sreg-cli-x86_64-unknown-linux-musl.tar.xz` |
| `aarch64-unknown-linux-gnu` | `sreg-cli-aarch64-unknown-linux-gnu.tar.xz` |
| `x86_64-pc-windows-msvc` | `sreg-cli-x86_64-pc-windows-msvc.zip` |

Each archive holds the `sreg` binary (`sreg.exe` on Windows), `README.md`, `LICENSE` and
`CHANGELOG.md`, and has a `<archive>.sha256` file next to it (`<sha256> *<archive>`);
`sha256.sum` lists all of them. The release also carries the installers
`sreg-cli-installer.sh` and `sreg-cli-installer.ps1` (they install into
`$CARGO_HOME/bin`, usually `~/.cargo/bin`) and a source archive. The release notes are
taken from the matching `CHANGELOG.md` section.

The configuration lives in `dist-workspace.toml` and the `dist` profile in
`Cargo.toml`. The workflow is generated: after changing the configuration or
upgrading `dist`, regenerate it instead of editing it by hand:

```
cargo install cargo-dist --version <cargo-dist-version> --locked
dist generate        # rewrites .github/workflows/release.yml
dist plan            # shows what a release would build
```

`dist plan` (and the `plan` job on pull requests, `pr-run-mode = "plan"`) fails when
the workflow is out of date with the configuration.

## Tags must match the package version

The `Release` workflow builds the version that is in `Cargo.toml` and announces it under the
tag that started it, so **a tag must match the package version**: `v0.1.1-beta.1` releases
only when `[workspace.package] version` is `0.1.1-beta.1` on the tagged commit. A tag that
does not match (for example a hand-made `v0.0.0-alpha.1` while `Cargo.toml` says `0.1.0`)
makes `dist` fail in the `plan` job and nothing is released. Do not tag by hand; use the
**Bump and release** workflow, which changes the version, the changelog and the tag
together.

## Bump and release

`.github/workflows/bump-and-release.yml` cuts a release from `main` in one run. Start it
from **Actions** → **Bump and release** → **Run workflow** (or
`gh workflow run bump-and-release.yml -f bump=prerelease`). It only runs on `main`; started
from another branch it fails straight away, so releases stay linear. Runs are serialised by
the `release` concurrency group.

### Inputs

`bump` says how the version changes; the current version is read from
`[workspace.package] version` in `Cargo.toml`. `version` is used with `bump: custom` only.

| `bump` | Result | Examples |
| --- | --- | --- |
| `prerelease` (default) | The next prerelease. A prerelease increments its last number; a stable version starts a `-beta.1` of the next patch (like `npm version prerelease --preid beta`, but counting from `1`). | `1.2.0-beta.3` → `1.2.0-beta.4`, `1.2.0-rc.1` → `1.2.0-rc.2`, `1.2.0` → `1.2.1-beta.1` |
| `prepatch`, `preminor`, `premajor` | The first prerelease (`-beta.1`) of the next patch, minor or major version. | `1.2.0` → `1.2.1-beta.1`, `1.3.0-beta.1`, `2.0.0-beta.1` |
| `patch`, `minor`, `major` | Like `npm version`: the next patch, minor or major version; from a prerelease of that version, its stable base. | `1.2.3` → `1.2.4`; `1.2.0-beta.3` → `1.2.0` (`patch` and `minor`); `2.0.0-beta.1` → `2.0.0` (`major`) |
| `release` | Drops the prerelease suffix; an error for a stable version. | `1.2.0-beta.3` → `1.2.0` |
| `none` | Releases the current version as it is; an error when its tag exists. | `0.1.0` → `0.1.0` |
| `custom` | The `version` input: strict semver without a leading `v` or build metadata, greater than the current version. | `1.2.0` → `1.3.0-rc.1` |

Every computed version must not have a tag yet.

### What the run does

1. Checks out `main` with its full history and tags.
2. Computes the version with `scripts/bump-version.mjs` and fails when its tag exists.
3. Applies it: `[workspace.package] version` and the internal crates in
   `[workspace.dependencies]` of `Cargo.toml`, the workspace packages in `Cargo.lock`
   (then `cargo metadata`, which corrects the lock should anything be missing).
4. Rolls `CHANGELOG.md`: the `[Unreleased]` entries move under
   `## [<version>] - <today>` (UTC), an empty `[Unreleased]` stays above, and the links at
   the bottom are updated (`[Unreleased]` compares from the new tag, the new version
   compares with the previous one). When the version already has a section (a section
   written ahead, as for `0.1.0`), the `[Unreleased]` entries are merged into its
   `### Added`/`### Changed`/… groups and its date is set to today. An empty
   `[Unreleased]` without a section for the version is an error (nothing to release),
   except with `bump: none`.
5. Runs `cargo fmt --all -- --check` and `cargo test --workspace --locked`.
6. Commits `chore(release): v<version>` as `github-actions[bot]`, creates the annotated
   tag `v<version>` with the changelog section as its message, and pushes both to `main`
   atomically.
7. Starts the `Release` workflow for the tag (below). cargo-dist builds the binaries and
   creates the GitHub Release with the changelog section as its notes; a version with a
   suffix becomes a GitHub prerelease.
8. Writes a summary with the version, the tag, the release notes and a link to the
   `Release` run.

The release commit and tag are pushed by the workflow, so CI does not run on the release
commit (pushes made with `GITHUB_TOKEN` start no workflows); the run checks formatting and
the tests itself before pushing.

### Starting the Release workflow

`release.yml` is generated by cargo-dist with `dispatch-releases = true` in
`dist-workspace.toml`, so it starts through `workflow_dispatch` with a `tag` input and not
on a tag push. The "Bump and release" run calls
`gh workflow run release.yml --ref v<version> -f tag=v<version>` with the default
`GITHUB_TOKEN` (dispatching a workflow is the one thing that token may start), so no
personal access token is needed. Pushing a tag by hand therefore releases nothing; run the
workflow instead, or dispatch `release.yml` with the tag yourself.

If `release.yml` is ever regenerated without `dispatch-releases`, the run falls back to a
`RELEASE_TOKEN` repository secret (a fine-grained personal access token for this
repository with **Contents: read and write**): the checkout then uses it, so the tag push
starts the `Release` workflow like a hand-pushed tag. Without either the run fails before it
changes anything, and says so.

### Repository settings

- **Settings → Actions → General → Workflow permissions** can stay at the default: the
  workflow declares `contents: write` and `actions: write` in its own `permissions`.
  "Allow GitHub Actions to create and approve pull requests" is not needed (no pull
  request is opened).
- Branch protection or rulesets on `main` must let GitHub Actions (or the `RELEASE_TOKEN`
  owner, when that fallback is used) push directly; the release commit is not a pull request.

### Running the script locally

`scripts/bump-version.mjs` needs only Node.js (18 or newer) and does the same computation:

```
node scripts/bump-version.mjs prerelease            # prints the next version
node scripts/bump-version.mjs custom 0.2.0-rc.1     # with bump custom
node scripts/bump-version.mjs patch --dry-run       # also prints every change it would make
node scripts/bump-version.mjs patch --apply         # rewrites Cargo.toml, Cargo.lock, CHANGELOG.md
node scripts/bump-version.mjs --current             # the current version
```

`--notes-file <path>` writes the release notes (the version's changelog section) and
`--date YYYY-MM-DD` overrides today's date. Its tests run in CI with
`node --test 'scripts/*.test.mjs'`.

## After the release

1. **Check the `Release` run** linked from the summary. A failed build can be re-run
   from the Actions tab; the tag and the release commit stay as they are.
2. **Check the action release.** For a stable release, the `Release` workflow ends with
   the `custom-release-action` job, which starts the action's release with the new
   version as its default `sreg-version` ([below](#releasing-the-github-action)). Its
   summary links the action's Release workflow; if it only printed a notice, the action
   repository picks the release up within an hour.
3. **Check the result.** Run the shell installer from the new release, check
   `sreg --version`, and, once the action is released, let a workflow that uses
   `kduma-OSS/serverless-registry-publish-action@v1` publish (or `dry-run: true`) a
   package to a test registry.

## Releasing the GitHub Action

The action
([`kduma-OSS/serverless-registry-publish-action`](https://github.com/kduma-OSS/serverless-registry-publish-action))
has its own versions, changelog and release workflow; `@v1` follows its latest 1.x
release. The `sreg` release it installs by default is the `default:` of its
`sreg-version` input, so a new CLI release reaches `@v1` users only through a release of
the action. That release happens on its own for every stable CLI release (prereleases are
skipped), in two ways:

1. **Push (immediate).** `dist-workspace.toml` declares the cargo-dist custom job
   `post-announce-jobs = ["./release-action"]`, so the generated `release.yml` calls
   `.github/workflows/release-action.yml` (with the dist plan) after the GitHub Release is
   announced. That job reads the tag from the plan (falling back to `github.ref_name`),
   mints a token with `actions/create-github-app-token` and runs

   ```
   gh workflow run release.yml -R kduma-OSS/serverless-registry-publish-action \
     -f sreg-version=v<version> -f bump=patch -f only-if-newer-sreg=true
   ```

   When the App is not configured (e.g. in a fork) it prints a notice and succeeds;
   when minting the token or the dispatch fails it prints a warning and still succeeds,
   since the release itself is already published.
2. **Hourly sync (fallback, no token).** The action repository's
   `sync-cli-release.yml` checks the newest stable CLI release every hour and releases
   the action when it is newer than the default. This covers a dispatch that is not
   configured or failed.

Either way the action's own `GITHUB_TOKEN` makes the release commit, the tag, the GitHub
release and moves `v1`; `only-if-newer-sreg` makes the second trigger a no-op. By hand:
run the action repository's **Release** workflow with `sreg-version` (that is also the
only way to make a CLI prerelease the default). See the action's
[`docs/releasing.md`](https://github.com/kduma-OSS/serverless-registry-publish-action/blob/main/docs/releasing.md).

### The GitHub App for the push

Set up once; no personal access token is involved.

1. Create a GitHub App for the `kduma-OSS` organization (**Settings** → **Developer
   settings** → **GitHub Apps** → **New GitHub App**), named e.g. "Serverless Registry
   Release", with this repository as its homepage URL. Untick **Webhook** → **Active**
   (no webhook). Repository permissions: **Actions: Read and write** and **Metadata:
   Read-only** (the default); nothing else.
2. Install it on `kduma-OSS` with **Only select repositories** →
   `kduma-OSS/serverless-registry-publish-action` (not on this repository).
3. Generate a private key on the App's page (a `.pem` file is downloaded).
4. In this repository (**Settings** → **Secrets and variables** → **Actions**), add the
   App ID as the repository **variable** `RELEASE_APP_ID` and the contents of the `.pem`
   file as the repository **secret** `RELEASE_APP_PRIVATE_KEY`.

The App can only start workflows in the action repository; it cannot push there.

## Releasing by hand

Only when the workflow cannot be used: on an up-to-date `main`, run
`node scripts/bump-version.mjs <bump> --apply`, check with fmt, clippy and the tests,
commit `chore(release): vX.Y.Z`, then `git tag -a vX.Y.Z` and
`git push origin main vX.Y.Z`. The tag must match the version in `Cargo.toml`. Do not
edit the assets of a published release: the action and the installers rely on the
archive names and checksums.

## Client and Worker versions

The CLI and the Worker (`serverless-registry-worker`) are released independently. They
protect each other from silent incompatibility with two headers (API contract,
section 3):

- `sreg` sends its own version as `X-Client-Version` on every request. This is the
  version in `Cargo.toml` at the time of the build, so it is the release version for
  release binaries.
- Every `/api/v1` response carries `X-Min-Client-Version`, the oldest CLI version the
  Worker accepts (`minClientVersion` in `GET /api/v1/version`). An older client gets
  `426 client_too_old` before authentication; `sreg` then prints the minimum version and
  a link to the releases and exits with code `3`. `sreg version --registry <url>`
  shows both versions (that endpoint is exempt from the check).

What this means for releases:

- **The Worker raises `minClientVersion` only after a CLI release that satisfies it
  exists.** Release the CLI first, let the action release itself with the new default (or run
  its Release workflow), then release the Worker with the new minimum. Otherwise every pipeline using the action
  fails with exit code `3` until a new CLI is published.
- A CLI release must keep working with Workers that are still on the previous minor
  version: the Worker keeps the previous API version (`/api/v1`) for at least one minor
  release, and the CLI must not depend on a response field that older Workers do not
  send without handling its absence.
- Prereleases compare lower than their release (`0.2.0-rc.1` < `0.2.0`): a Worker
  requiring `0.2.0` rejects `0.2.0-rc.1`.
- Pipelines that pin `sreg-version` (or the action at a full release tag) do not pick
  up new releases; when a Worker raises the minimum past the pinned version, the step
  fails with exit code `3` and a message to raise `sreg-version`.
