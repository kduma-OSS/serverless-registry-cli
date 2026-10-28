#!/usr/bin/env bash
# Runs `sreg <ecosystem> publish --json` with the action's inputs (INPUT_* variables)
# and writes the `name`, `version`, `url` and `skipped` outputs to $GITHUB_OUTPUT.
set -euo pipefail

fail() {
  echo "::error::$*" >&2
  exit 1
}

# Splits a string into words with shell-like quoting (no expansion, no command
# substitution) and appends them to the array named by $1.
split_words() {
  local __name="$1" __input="$2" __word
  case "$__input" in
    *[![:space:]]*) ;;
    *) return 0 ;;
  esac
  while IFS= read -r -d '' __word; do
    eval "$__name+=(\"\$__word\")"
  done < <(printf '%s\n' "$__input" | xargs printf '%s\0')
}

is_true() {
  case "$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')" in
    true | yes | 1 | on) return 0 ;;
    false | no | 0 | off | "") return 1 ;;
    *) fail "expected true or false, got '$1'" ;;
  esac
}

# Rejects an input that the selected ecosystem does not take.
only_for() {
  local input="$1" value="$2"
  shift 2
  [ -z "$value" ] && return 0
  local eco
  for eco in "$@"; do
    [ "$eco" = "$ecosystem" ] && return 0
  done
  fail "the input '$input' does not apply to $ecosystem (only to: $*)"
}

ecosystem="$(printf '%s' "${INPUT_ECOSYSTEM:-}" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
case "$ecosystem" in
  composer | npm | cargo | nuget | generic | swift | maven) ;;
  "") fail "the input 'ecosystem' is required" ;;
  *) fail "unknown ecosystem '$ecosystem' (composer, npm, cargo, nuget, generic, swift or maven)" ;;
esac
[ -n "${REGISTRY_URL:-}" ] || fail "the input 'registry' is required"

only_for path "${INPUT_PATH:-}" composer swift
only_for workspace "${INPUT_WORKSPACE:-}" npm
only_for package "${INPUT_PACKAGE:-}" cargo generic
only_for project "${INPUT_PROJECT:-}" nuget
only_for from-dir "${INPUT_FROM_DIR:-}" maven
only_for coords "${INPUT_COORDS:-}" maven
only_for scope "${INPUT_SCOPE:-}" swift
only_for generic-files "${INPUT_GENERIC_FILES:-}" generic
only_for generic-labels "${INPUT_GENERIC_LABELS:-}" generic

cmd=(sreg "$ecosystem" publish --json)

if [ "$ecosystem" = generic ]; then
  [ -n "${INPUT_PACKAGE:-}" ] || fail "generic: the input 'package' (the package name) is required"
  [ -n "${INPUT_VERSION:-}" ] || fail "generic: the input 'version' is required"
  [ -z "${INPUT_TAG_PREFIX:-}" ] || fail "generic: 'tag-prefix' is not supported, pass 'version'"
  cmd+=("$INPUT_PACKAGE" "${INPUT_VERSION#v}")
  files=()
  while IFS= read -r pattern || [ -n "$pattern" ]; do
    pattern="${pattern#"${pattern%%[![:space:]]*}"}"
    pattern="${pattern%"${pattern##*[![:space:]]}"}"
    [ -z "$pattern" ] && continue
    matched=0
    # Unquoted on purpose: expand the glob pattern.
    # shellcheck disable=SC2206
    for file in $pattern; do
      [ -e "$file" ] || continue
      files+=("$file")
      matched=1
    done
    [ "$matched" = 1 ] || fail "generic: no file matches '$pattern'"
  done <<< "${INPUT_GENERIC_FILES:-}"
  [ "${#files[@]}" -gt 0 ] || fail "generic: the input 'generic-files' is required"
  cmd+=("${files[@]}")
  while IFS= read -r label || [ -n "$label" ]; do
    label="${label#"${label%%[![:space:]]*}"}"
    label="${label%"${label##*[![:space:]]}"}"
    [ -z "$label" ] && continue
    case "$label" in
      *=*) cmd+=(--label "$label") ;;
      *) fail "generic: the label '$label' is not key=value" ;;
    esac
  done < <(printf '%s\n' "${INPUT_GENERIC_LABELS:-}" | tr ',' '\n')
else
  [ -z "${INPUT_VERSION:-}" ] || cmd+=(--version "${INPUT_VERSION#v}")
  [ -z "${INPUT_TAG_PREFIX:-}" ] || cmd+=(--tag-prefix "$INPUT_TAG_PREFIX")
  [ -z "${INPUT_PATH:-}" ] || cmd+=(--path "$INPUT_PATH")
  [ -z "${INPUT_WORKSPACE:-}" ] || cmd+=(--workspace "$INPUT_WORKSPACE")
  [ -z "${INPUT_PACKAGE:-}" ] || cmd+=(--package "$INPUT_PACKAGE")
  [ -z "${INPUT_PROJECT:-}" ] || cmd+=(--project "$INPUT_PROJECT")
  [ -z "${INPUT_FROM_DIR:-}" ] || cmd+=(--from-dir "$INPUT_FROM_DIR")
  [ -z "${INPUT_COORDS:-}" ] || cmd+=(--coords "$INPUT_COORDS")
  [ -z "${INPUT_SCOPE:-}" ] || cmd+=(--scope "$INPUT_SCOPE")
fi

# A registry URL with userinfo (https://<login>:<password>@host/) carries the token.
url_has_credentials() {
  [[ "${REGISTRY_URL:-}" =~ ^[A-Za-z][A-Za-z0-9+.-]*://[^/?#]*@ ]]
}

if [ -z "${REGISTRY_TOKEN:-}" ] && url_has_credentials; then
  # The token comes from the registry URL.
  unset REGISTRY_TOKEN
  [ -z "${INPUT_OIDC_PROJECT:-}" ] || fail "'oidc-project' applies only to OIDC, not to credentials in 'registry'"
elif [ -z "${REGISTRY_TOKEN:-}" ]; then
  # No token: authenticate with the job's OIDC token instead.
  unset REGISTRY_TOKEN
  [ -n "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" ] ||
    fail "no token given and no OIDC token available: grant the job 'permissions: id-token: write' or pass 'token'"
  cmd+=(--oidc)
  [ -z "${INPUT_OIDC_PROJECT:-}" ] || cmd+=(--oidc-project "$INPUT_OIDC_PROJECT")
else
  [ -z "${INPUT_OIDC_PROJECT:-}" ] || fail "'oidc-project' applies only without 'token'"
fi

if is_true "${INPUT_DRY_RUN:-false}"; then cmd+=(--dry-run); fi
if is_true "${INPUT_SKIP_EXISTING:-true}"; then cmd+=(--skip-existing); fi
split_words cmd "${INPUT_EXTRA_FLAGS:-}"

trailing=()
split_words trailing "${INPUT_ARGS:-}"
if [ "${#trailing[@]}" -gt 0 ]; then
  cmd+=(-- "${trailing[@]}")
fi

printf 'Running:'
printf ' %q' "${cmd[@]}"
printf '\n'

status=0
result="$("${cmd[@]}")" || status=$?
printf '%s\n' "$result"
if [ "$status" -ne 0 ]; then
  case "$status" in
    2) echo "::error::the version is already published (set skip-existing: true to ignore this)" >&2 ;;
    3) echo "::error::this sreg release is too old for the registry; raise sreg-version" >&2 ;;
  esac
  exit "$status"
fi

# `tr` drops the carriage returns jq prints on Windows.
field() { jq -r "$1" <<< "$result" | tr -d '\r'; }
name="$(field '.name // empty')"
version="$(field '.version // empty')"
url="$(field '.url // empty')"
skipped="$(field 'if .skipped == true then "true" else "false" end')"

if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "name=$name"
    echo "version=$version"
    echo "url=$url"
    echo "skipped=$skipped"
  } >> "$GITHUB_OUTPUT"
fi
