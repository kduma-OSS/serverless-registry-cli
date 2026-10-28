# Releasing sreg

`sreg` is released from this repository with [cargo-dist](https://opensource.axo.dev/cargo-dist/)
(`dist`). A release is a git tag `vX.Y.Z` that matches the version in `Cargo.toml`;
everything else (binaries, checksums, installers, the GitHub Release) is produced by the
`Release` workflow (`.github/workflows/release.yml`). Releases are cut with the manual
**Bump and release** workflow (`.github/workflows/bump-and-release.yml`), which sets the
version, rolls the changelog, tags and starts the `Release` workflow. The composite
GitHub Action in `action/` installs those release binaries; the release commit points it
at the new version.

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
   (then `cargo metadata`, which corrects the lock should anything be missing), and
   `action/VERSION` (`v<version>`), so the action installs the new release as soon as it
   is published.
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

`release.yml` is generated by cargo-dist and starts on a tag push only: it has no
`workflow_dispatch` trigger, so `gh workflow run release.yml -f tag=…` is not available,
and a tag pushed with the default `GITHUB_TOKEN` starts nothing. The run therefore uses,
in this order:

- **`RELEASE_TOKEN`** (recommended): a repository secret with a fine-grained personal
  access token (this repository, **Contents: read and write**) or a classic token with
  `repo`. The checkout uses it, so the release commit and the tag are pushed with it and
  the tag push starts the `Release` workflow like a hand-pushed tag. The token's owner must
  be allowed to push to `main`.
- **Dispatch**: when `release.yml` has a `workflow_dispatch` trigger, the run calls
  `gh workflow run release.yml --ref v<version> -f tag=v<version>` with `GITHUB_TOKEN`
  (dispatches are the exception to the rule above). cargo-dist generates that trigger
  with `dispatch-releases = true` in `dist-workspace.toml` (then `dist generate`); note
  that it replaces the tag-push trigger, so pushing a tag by hand no longer releases.

Without either the run fails before it changes anything, and says so.

### Repository settings

- **Settings → Actions → General → Workflow permissions** can stay at the default: the
  workflow declares `contents: write` and `actions: write` in its own `permissions`.
  "Allow GitHub Actions to create and approve pull requests" is not needed (no pull
  request is opened).
- Branch protection or rulesets on `main` must let the pusher (the `RELEASE_TOKEN` owner,
  or GitHub Actions) push directly; the release commit is not a pull request.

### Running the script locally

`scripts/bump-version.mjs` needs only Node.js (18 or newer) and does the same computation:

```
node scripts/bump-version.mjs prerelease            # prints the next version
node scripts/bump-version.mjs custom 0.2.0-rc.1     # with bump custom
node scripts/bump-version.mjs patch --dry-run       # also prints every change it would make
node scripts/bump-version.mjs patch --apply         # rewrites Cargo.toml, Cargo.lock, action/VERSION, CHANGELOG.md
node scripts/bump-version.mjs --current             # the current version
```

`--notes-file <path>` writes the release notes (the version's changelog section) and
`--date YYYY-MM-DD` overrides today's date. Its tests run in CI with
`node --test 'scripts/*.test.mjs'`.

## After the release

1. **Check the `Release` run** linked from the summary. A failed build can be re-run
   from the Actions tab; the tag and the release commit stay as they are.
2. **Move the action's major tag.** The action is used as
   `kduma-OSS/serverless-registry-cli/action@v1`. `v1` is a moving tag on the release
   commit (which also updated `action/VERSION`):

   ```
   git fetch origin --tags
   git tag -f v1 v<version>^{commit}
   git push -f origin v1
   ```

   This is the only tag that is ever moved. Move it only while the action's inputs and
   outputs stay compatible; an incompatible change to the action starts `v2` (and the
   READMEs are updated to it). `v1` does not match the release workflow's trigger
   (it needs a full `X.Y.Z` version), so moving it builds nothing. Moving it to a
   prerelease points every `@v1` user at that prerelease; usually move it for stable
   releases only.
3. **Check the result.** Run the shell installer from the new release, check
   `sreg --version`, and let a workflow that uses `@v1` publish (or `dry-run: true`) a
   package to a test registry.

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
  exists.** Release the CLI first, update the action (`action/VERSION` and `v1`), then
  release the Worker with the new minimum. Otherwise every pipeline using the action
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
