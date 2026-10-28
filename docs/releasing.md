# Releasing sreg

`sreg` is released from this repository with [cargo-dist](https://opensource.axo.dev/cargo-dist/)
(`dist`). A release is a git tag `vX.Y.Z`; everything else (binaries, checksums,
installers, the GitHub Release) is produced by the `Release` workflow
(`.github/workflows/release.yml`). The composite GitHub Action in `action/` installs
those release binaries, so it is updated after every release.

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

## Steps

1. **Prepare the release on `main`.**
   - Set the version in `[workspace.package]` of the root `Cargo.toml` (all crates share
     it) and in the internal `[workspace.dependencies]`, and update `Cargo.lock`
     (`cargo check --workspace`).
   - In `CHANGELOG.md`, move the `Unreleased` entries under
     `## [X.Y.Z] - YYYY-MM-DD`, start an empty `Unreleased` section and update the
     links at the bottom. Mark breaking changes clearly.
   - Commit (`chore: release X.Y.Z`), with fmt, clippy and the tests green.

2. **Tag and push.**

   ```
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

   The `Release` workflow runs `plan` → `build-local-artifacts` (one job per target) →
   `build-global-artifacts` (installers, `sha256.sum`) → `host` (creates the GitHub
   Release and uploads everything) → `announce`. A prerelease version (`vX.Y.Z-rc.1`)
   becomes a GitHub prerelease. Do not edit the assets of a published release: the
   action and the installers rely on the archive names and checksums.

3. **Point the action at the new binaries.** On `main`, write the tag into
   `action/VERSION` (`vX.Y.Z`) and commit it (`chore(action): install sreg X.Y.Z`).
   Until then the action keeps installing the previous release.

4. **Move the action's major tag.** The action is used as
   `kduma-OSS/serverless-registry-cli/action@v1`. `v1` is a moving tag on the commit
   that updated `action/VERSION`:

   ```
   git tag -f v1 <commit>
   git push -f origin v1
   ```

   This is the only tag that is ever moved. Move it only while the action's inputs and
   outputs stay compatible; an incompatible change to the action starts `v2` (and the
   READMEs are updated to it). `v1` does not match the release workflow's trigger
   (it needs a full `X.Y.Z` version), so moving it builds nothing.

5. **Check the result.** Run the shell installer from the new release, check
   `sreg --version`, and let a workflow that uses `@v1` publish (or `dry-run: true`) a
   package to a test registry.

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
