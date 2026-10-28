#!/usr/bin/env bash
# Installs the sreg release binary for the current runner into $RUNNER_TEMP/sreg and
# adds it to $GITHUB_PATH, after verifying the archive against the SHA-256 checksum
# published with the release.
#
# Environment:
#   SREG_VERSION       release tag to install (`v0.1.0` or `0.1.0`); empty: action/VERSION
#   SREG_ACTION_PATH   directory of the action (holds VERSION)
#   SREG_RELEASES_URL  base URL of the releases (default: the GitHub releases of
#                      kduma-OSS/serverless-registry-cli)
set -euo pipefail

releases_url="${SREG_RELEASES_URL:-https://github.com/kduma-OSS/serverless-registry-cli/releases/download}"
action_path="${SREG_ACTION_PATH:-$(cd "$(dirname "$0")" && pwd)}"

version="${SREG_VERSION:-}"
if [ -z "$version" ]; then
  version="$(tr -d '[:space:]' < "$action_path/VERSION")"
fi
version="v${version#v}"

os="${RUNNER_OS:-$(uname -s)}"
arch="${RUNNER_ARCH:-$(uname -m)}"
case "$os/$arch" in
  Linux/X64 | Linux/x86_64) target=x86_64-unknown-linux-musl ;;
  Linux/ARM64 | Linux/aarch64) target=aarch64-unknown-linux-gnu ;;
  macOS/X64 | Darwin/x86_64) target=x86_64-apple-darwin ;;
  macOS/ARM64 | Darwin/arm64) target=aarch64-apple-darwin ;;
  # Windows on ARM runs the x86_64 build through emulation.
  Windows/*) target=x86_64-pc-windows-msvc ;;
  *)
    echo "::error::sreg has no release build for $os/$arch" >&2
    exit 1
    ;;
esac

case "$target" in
  *windows*) archive="sreg-cli-$target.zip" ;;
  *) archive="sreg-cli-$target.tar.xz" ;;
esac

temp="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
dest="$temp/sreg"
download="$temp/sreg-download"
rm -rf "$dest" "$download"
mkdir -p "$dest" "$download"

echo "Installing sreg $version ($target)"
url="$releases_url/$version/$archive"
curl --fail --silent --show-error --location --retry 3 --retry-delay 2 \
  --output "$download/$archive" "$url"
curl --fail --silent --show-error --location --retry 3 --retry-delay 2 \
  --output "$download/$archive.sha256" "$url.sha256"

# The checksum file holds `<sha256>  <file name>`; compare the digest only, so the
# check does not depend on the tool or on the file name format.
expected="$(awk '{ print tolower($1); exit }' "$download/$archive.sha256")"
if command -v sha256sum > /dev/null 2>&1; then
  actual="$(sha256sum "$download/$archive" | awk '{ print tolower($1) }')"
else
  actual="$(shasum -a 256 "$download/$archive" | awk '{ print tolower($1) }')"
fi
if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
  echo "::error::checksum mismatch for $archive: expected ${expected:-nothing}, got $actual" >&2
  exit 1
fi
echo "Checksum verified: $actual"

case "$archive" in
  *.zip) unzip -q -o "$download/$archive" -d "$dest" ;;
  *) tar -xJf "$download/$archive" -C "$dest" ;;
esac
rm -rf "$download"

binary="$(find "$dest" -type f \( -name sreg -o -name sreg.exe \) | head -n 1)"
if [ -z "$binary" ]; then
  echo "::error::the archive $archive does not contain the sreg binary" >&2
  exit 1
fi
bin_dir="$(dirname "$binary")"
chmod +x "$binary" 2> /dev/null || true

if command -v cygpath > /dev/null 2>&1; then
  path_entry="$(cygpath -w "$bin_dir")"
else
  path_entry="$bin_dir"
fi
if [ -n "${GITHUB_PATH:-}" ]; then
  echo "$path_entry" >> "$GITHUB_PATH"
fi
"$binary" --version
