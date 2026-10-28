# HTTP API contract (`/api/v1`)

This document is the contract between the Worker and the `sreg` CLI (and the admin panel). It
is the source of truth for the `/api/v1` surface: a client can be implemented from this
document alone. The native read protocols (`/composer`, `/npm`, `/cargo`) and the panel's
`/login` and `/logout` are described in [spec.md](spec.md) and are out of scope here. The
generic feed (`/generic/…`, section 8) and the NuGet feed (`/nuget/…`, section 9) are not part
of `/api/v1`, but the CLI and the NuGet clients depend on their exact shape, so they are
specified here.

Changes to this document must be backwards compatible (new optional request fields, new
response fields, new endpoints, new error codes) or must introduce `/api/v2`. When a new path
version is introduced, the previous one stays available for at least one minor release of the
Worker.

## 1. Conventions

### 1.1 Base URL and versioning

- Base URL: `<PUBLIC_URL>/api/v1`, e.g. `https://packages.example.com/api/v1`. `PUBLIC_URL`
  has no trailing slash. All paths below are relative to the host (they include `/api/v1`).
- The API version is part of the path. There is no version negotiation through media types.

### 1.2 Encoding

- Every request body and every response body is JSON encoded as UTF-8. There are no
  successful responses without a body (the API never answers `204`).
- Requests that carry a body must send `Content-Type: application/json`. A body with any other
  content type, a body that is not valid JSON, or a JSON value that is not an object is
  rejected with `400 invalid_request`. Requests without a body (all `GET`, and `POST`/`DELETE`
  endpoints whose body is optional) may omit the header and the body entirely; an empty body
  is equivalent to `{}`.
- Request bodies are limited to 4 MiB; larger bodies are rejected with
  `413 payload_too_large`. An upload (`init`) declares at most 64 files; more are rejected with
  `400 invalid_request`.
- Every response carries `Content-Type: application/json; charset=utf-8` and
  `Cache-Control: no-store`.
- Unknown fields in request bodies are ignored by the Worker, unless an endpoint says
  otherwise. Clients must ignore unknown fields in response bodies and unknown response
  headers.
- Field names are `camelCase`, except where a field mirrors an ecosystem-native format (the
  `manifest` object, cargo index fields, HTTP header names in `headers`).
- Timestamps are RFC 3339 strings in UTC with a `Z` suffix and second precision, e.g.
  `2026-10-28T10:12:00Z`. Where a request accepts a timestamp, a date-only value
  `YYYY-MM-DD` is also accepted and means `00:00:00Z` of that day. Responses always use the
  full form.
- `sha256` values are 64 lower-case hexadecimal characters.
- Sizes are integers in bytes.
- Lists are not paginated in `v1`.
- There are no CORS headers; the panel is served from the same origin.

### 1.3 Path parameters

| Parameter       | Format                                                                                                                                                                                                                                                                             |
| --------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `<eco>`         | `composer`, `npm`, `cargo`, `generic` or `nuget`, and only when it is enabled in `ECOSYSTEMS` (1.5). Any other value answers `404 not_found`.                                                                                                                                      |
| `<name>`        | Package name, percent-encoded as one path segment: `/` must be sent as `%2F`, `@` may be sent literally or as `%40`. Examples: `acme%2Fsdk` (composer), `%40acme%2Fsdk` (npm), `acme-core` (cargo), `acme%2Fplugin-foo` (generic), `Acme.Sdk` (nuget; matched case-insensitively). |
| `<version>`     | Exact version string as published, percent-encoded (versions never contain `/`); for nuget it is normalised first (5.2).                                                                                                                                                           |
| `<project>`     | A local project slug, `[a-z0-9][a-z0-9-]{1,62}`. A value containing `.` (a provider-backed project) answers `400 invalid_request`.                                                                                                                                                 |
| `<hash-prefix>` | 8 to 12 lower-case hexadecimal characters identifying an auth entry (see 6.1).                                                                                                                                                                                                     |

The Worker matches paths on the raw (still percent-encoded) path and decodes each segment
separately, so `%2F` inside `<name>` never splits the segment.

### 1.4 Evaluation order

The Worker processes an `/api/v1` request in this order and answers with the first failure:

1. Version check (`X-Client-Version`, section 3) → `400 invalid_request` or
   `426 client_too_old`.
2. Routing → `404 not_found` for an unknown path or an unsupported method on a known path.
3. Body parsing and size → `400 invalid_request`, `413 payload_too_large`.
4. Authentication → `401 unauthorized`, `503 provider_unavailable`.
5. Kind check → `403 forbidden`.
6. Field validation → `400 invalid_request`, `413 payload_too_large`.
7. Grant check and endpoint logic → `400 upload_incomplete`, `403 forbidden`, `404 not_found`,
   `409 …`.

### 1.5 Installation variables

These Worker variables (see the spec, "Deployment, configuration and updates") change what
the contract allows:

| Variable              | Default                            | Effect                                                                                                                                                                                                                                                 |
| --------------------- | ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `PUBLIC_URL`          | the request's origin               | base of every URL in responses (`url` fields, `aud` of `gh` tokens)                                                                                                                                                                                    |
| `ECOSYSTEMS`          | `composer,npm,cargo,generic,nuget` | enabled ecosystems; a disabled one is treated as unknown: `<eco>` paths answer `404 not_found`, `eco` in `init` is `400 invalid_request`, listings omit it and every path of its read protocol (e.g. `/generic/…`, `/nuget/…`) answers `404 not_found` |
| `MAX_UPLOAD_BYTES`    | `536870912` (512 MiB)              | largest `size` of each file in `init`                                                                                                                                                                                                                  |
| `APPEND_WINDOW_HOURS` | `24`                               | how long after `published` files may still be appended to a version (6.4.1)                                                                                                                                                                            |
| `TOKEN_PREFIX`        | empty                              | optional token prefix (2.1)                                                                                                                                                                                                                            |

## 2. Authentication

### 2.1 Token format

```text
[<TOKEN_PREFIX>][<source>.]<project>:[<type>.]<secret>
```

- `source` (optional): an identifier of an external credential provider configured in
  `AUTH_PROVIDERS`. Absent means the credential is stored in the Worker's KV. In `v1` only KV
  credentials are implemented; a token naming a source answers `401 unauthorized` until
  providers are implemented. When they are, an unreachable provider answers
  `503 provider_unavailable` for tokens of that source.
- `project`: slug `[a-z0-9][a-z0-9-]{1,62}`.
- `type` (optional): `key` (default; never written out) or `gh` (a GitHub Actions OIDC JWT).
- `secret`: for `key`, 32 base62 characters generated by the Worker; for `gh`, the whole JWT
  (it contains dots, which is fine: the secret is everything after `<type>.`).

Parsing:

1. If `TOKEN_PREFIX` is a non-empty string and the token starts with it, strip it.
2. Split at the **first** `:` into login and password. No `:` → `401`.
3. Login: one `.`-separated segment → `project`; two segments → `source.project`; anything
   else → `401`.
4. Password: no `.` → type `key`, the whole password is the secret; otherwise the part before
   the first `.` is the type and the rest is the secret. An unknown type → `401`.

`TOKEN_PREFIX` (a Worker variable, empty by default) is a fixed string such as `srg_` that the
Worker puts in front of every token it issues, so leaked tokens can be found with grep. When
it is set, tokens are accepted both with and without the prefix, so enabling it does not
invalidate existing tokens. The prefix must contain a character that cannot occur in a source
or project slug (the recommended form ends with `_`).

Examples:

```text
shop-frontend:k7Qx2mN9pLw4vR8sT1bYc3dF6hJ0aZ5e     project shop-frontend, type key
ci-acme-sdk:gh.eyJhbGciOi….eyJpc3Mi….SflKxw…       project ci-acme-sdk, type gh
srg_shop-frontend:k7Qx2mN9pLw4vR8sT1bYc3dF6hJ0aZ5e with TOKEN_PREFIX = "srg_"
```

### 2.2 Accepted `Authorization` forms

| Form           | Header                                         | Normalisation                                                                                                                       |
| -------------- | ---------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| Bearer         | `Authorization: Bearer <token>`                | the value after `Bearer ` is the token                                                                                              |
| Basic (token)  | `Authorization: Basic base64(<token>)`         | the decoded string is the token (Basic login = token login, Basic password = token password)                                        |
| Basic (pasted) | `Authorization: Basic base64(<login>:<token>)` | the decoded password contains `:`, so the password is the full token; the Basic login must equal the token's login, otherwise `401` |
| Raw            | `Authorization: <token>`                       | the whole header value is the token                                                                                                 |

The scheme names `Bearer` and `Basic` are case-insensitive. The CLI always uses the Bearer
form. A `gh` token is verified against GitHub's JWKS, with `aud` equal to `PUBLIC_URL`; the
CLI requests the OIDC token with exactly that audience.

### 2.3 Panel sessions

The panel calls the same endpoints authenticated by its session cookie instead of an
`Authorization` header. When both are present, `Authorization` wins. A cookie-authenticated
`POST`, `PATCH` or `DELETE` must carry a non-empty `X-Requested-With` header, otherwise it is
rejected with `403 forbidden` (CSRF protection). Requests authenticated with `Authorization`
do not need that header. The CLI never uses cookies.

### 2.4 Credential kinds

Every credential has exactly one kind:

| Kind      | May call                                                                                     |
| --------- | -------------------------------------------------------------------------------------------- |
| `read`    | `whoami`, package listing and generic feed (filtered by its grants)                          |
| `publish` | `whoami`, `publish/init`, `publish/commit` (within its grants)                               |
| `admin`   | `whoami`, package listing and generic feed (unfiltered), package administration, credentials |

`GET /api/v1/version` requires no credential. A missing, malformed, unknown, expired or
otherwise invalid credential answers `401 unauthorized` with
`WWW-Authenticate: Bearer realm="serverless-registry"`. A valid credential of the wrong kind,
or without a grant for the requested package and version, answers `403 forbidden`.

## 3. Client version negotiation

- The CLI sends `X-Client-Version: <semver>` (its own version, e.g. `0.3.1`) on every
  request. Browsers and other clients may omit it; then no check is made.
- Every `/api/v1` response, including errors and `404`s, carries
  `X-Min-Client-Version: <semver>`, the oldest CLI version this Worker supports.
- If `X-Client-Version` is present and lower than the minimum (semver precedence, so
  `0.2.0-beta.1` < `0.2.0`), the Worker answers `426 Upgrade Required` with error
  `client_too_old`, before authentication. A value that is not a valid semver version answers
  `400 invalid_request`.
- `GET /api/v1/version` is exempt from the check so that old clients can always discover the
  requirement.
- The CLI should also compare its own version with `X-Min-Client-Version` on successful
  responses and may warn when it is close to the minimum. On `426` it prints a message with a
  link to the latest release and exits with code `3`.

## 4. Errors

Every non-2xx response has this body:

```json
{ "error": "<snake_case_code>", "message": "<human-readable text>" }
```

`error` is stable and meant for programs; `message` is for humans and may change. Messages
never contain secrets (tokens, hashes, presigned URLs). Clients must treat an unknown `error`
value according to its HTTP status.

| Code                   | Status | Meaning                                                                                                                      |
| ---------------------- | ------ | ---------------------------------------------------------------------------------------------------------------------------- |
| `invalid_request`      | 400    | Malformed JSON, missing or invalid field, malformed header, constraint that cannot be parsed, and similar.                   |
| `upload_incomplete`    | 400    | On `commit`: at least one declared file is missing from staging (its `PUT` never completed).                                 |
| `unauthorized`         | 401    | No credential, or the credential could not be verified (unknown project, wrong secret, expired, bad JWT).                    |
| `forbidden`            | 403    | Valid credential, but wrong kind, no matching grant, missing `X-Requested-With`, or a protected operation.                   |
| `not_found`            | 404    | Unknown path or method, unknown or disabled ecosystem, unknown package, version, file, credential, auth entry or `uploadId`. |
| `version_exists`       | 409    | The version is already published (on `init` without `append`, and on `commit`).                                              |
| `version_closed`       | 409    | `append` to a version whose append window (`APPEND_WINDOW_HOURS`) has passed, or that is yanked.                             |
| `file_exists`          | 409    | `append` with a file name that already exists in the version.                                                                |
| `upload_mismatch`      | 409    | On `commit`: the size or SHA-256 of a staged file differs from the `init` declaration.                                       |
| `credential_exists`    | 409    | `POST /api/v1/credentials` for a project that already exists.                                                                |
| `payload_too_large`    | 413    | Request body over 4 MiB, or the `size` of a file in `init` above `MAX_UPLOAD_BYTES`.                                         |
| `client_too_old`       | 426    | `X-Client-Version` is lower than `X-Min-Client-Version`.                                                                     |
| `rate_limited`         | 429    | Reserved; not emitted in `v1`. If received, wait for `Retry-After` seconds (default 10) and retry.                           |
| `internal_error`       | 500    | Unexpected failure in the Worker.                                                                                            |
| `provider_unavailable` | 503    | The external credential provider of the token's source did not answer. Never falls back to no auth.                          |

To avoid leaking the existence of packages, a `read` credential asking for a package it has
no grant for receives `404 not_found`, not `403`.

## 5. Common types

Written as TypeScript for precision; `?` marks an optional field, `| null` a field that is
always present but may be `null`.

```ts
type Eco = "composer" | "npm" | "cargo" | "generic" | "nuget";
type Kind = "read" | "publish" | "admin";

interface Grant {
  eco: Eco;
  package: string; // exact name or a pattern with a trailing "*" (see 5.1)
  versions: string; // version constraint in the ecosystem's syntax, "*" = all
}

interface KeyAuthEntry {
  id: string; // 12 lower-case hex chars: the prefix of the secret's SHA-256
  type: "key";
  created: string; // RFC 3339
  expires: string | null; // RFC 3339; null = never
}

interface GhAuthEntry {
  id: string; // 12 lower-case hex chars, opaque and stable
  type: "gh";
  repository: string; // "owner/repo", compared with the JWT "repository" claim
  ref: string | null; // e.g. "refs/heads/main"; null = any ref
  workflow: string | null; // compared with the JWT "workflow" claim; null = any workflow
  created: string;
  expires: string | null;
}

type AuthEntry = KeyAuthEntry | GhAuthEntry;

interface Credential {
  project: string; // local slug
  kind: Kind;
  auth: AuthEntry[]; // includes expired entries until the hourly cron removes them
  grants: Grant[]; // always [] for kind "admin"
}

interface PackageFile {
  name: string; // unique within the version
  size: number;
  sha256: string;
  role?: string; // composer, npm, cargo, nuget: "archive" or (nuget) "symbols"; absent for generic
  labels?: Record<string, string>; // generic: always present ({} when none); absent elsewhere
}

interface PackageVersion {
  version: string;
  published: string; // RFC 3339, the first publish of the version (appends do not change it)
  yanked: boolean;
  files: PackageFile[]; // in upload order; appended files come last
}

// Stored at <eco>/pkg/<name>/<version>/manifest.json; no endpoint returns it as a whole.
interface Manifest {
  eco: Eco;
  name: string; // decoded, e.g. "@acme/sdk"; nuget: the id with its original casing
  version: string; // nuget: normalised (5.2)
  files: PackageFile[]; // non-empty, unique names
  published: string; // RFC 3339
  publishedBy: string; // [<source>.]<project> of the credential that published it
  yanked: boolean;
  native: Record<string, unknown>; // the init `manifest` as normalised by the adapter (generic: `meta`)
}

interface Package {
  eco: Eco;
  name: string; // decoded, e.g. "@acme/sdk"
  versions: PackageVersion[]; // sorted by version precedence, highest first
}
```

The stored KV record (see spec) keeps the full secret hash in `hash`; the API never returns
it and exposes `id` instead. An auth entry is active when `expires` is `null` or in the
future.

The version manifest is the durable data contract (see spec); package listings (6.6–6.8) and
the generic feed (section 8) are derived from it. Clients must treat an absent `labels` like
`{}`.

### 5.1 Grants

- `eco`: one of the five ecosystems. A grant may name an ecosystem that is disabled in
  `ECOSYSTEMS`; it takes effect once the ecosystem is enabled.
- `package`: an exact package name, or a pattern whose only `*` is its last character and
  starts a segment or ends one (`acme/*`, `@acme/*`, `acme-*`). `*` matches any sequence of
  characters that does not contain `/`. The pattern `*` alone matches every package of the
  ecosystem. For nuget, package ids and patterns are compared case-insensitively.
- `versions`: a constraint in the ecosystem's syntax (Composer, npm/node-semver, cargo
  `VersionReq`; generic uses the npm/node-semver syntax; nuget uses NuGet version ranges:
  `[1.0,2.0)`, `1.0` (= `>=1.0`), `[1.0]`, `(,1.0]`, floating `1.*`), or `*` for all versions.
  The ecosystem's adapter evaluates it. A constraint that does not parse is
  `400 invalid_request`.
- A credential without a grant for an ecosystem has no access to it. For `read` a grant means
  "may install", for `publish` "may publish".

Shorthand: `eco:package[:versions]`, split at the first two `:`; `versions` defaults to `*`.
Examples: `composer:acme/*:*`, `npm:@acme/sdk:^2.0`, `cargo:acme-core:>=1.4, <2`,
`generic:acme/*:^1.0`, `nuget:Acme.*:[1.0,2.0)`.

### 5.2 Versions

The version format is validated by the ecosystem's adapter (`validateVersion`), not by the
kernel:

- **composer, npm, cargo, generic**: semver 2.0.0 versions `MAJOR.MINOR.PATCH[-PRERELEASE]`
  without a leading `v` and without build metadata (`+…` is rejected because Composer cannot
  represent it and cargo ignores it for identity). For composer the prerelease, when present,
  must be `alpha`, `beta` or `rc`, optionally followed by a number with or without a dot
  (`1.0.0-beta.1`, `1.0.0-rc2`), because Composer does not understand other labels.
- **nuget**: NuGet versions: 1 to 4 numeric parts, optionally followed by a SemVer 2.0.0
  prerelease (`1.2.3-alpha.1`). Build metadata (`+…`) is accepted, ignored and stripped. The
  CLI sends the version as written; the Worker normalises it and stores and serves only the
  NuGet normalised form: lower case, leading zeros removed from the numeric parts, at least
  three parts (`1.2` → `1.2.0`) and a fourth part of `0` removed (`1.2.3.0` → `1.2.3`). For
  example `01.2.3.0-Beta+abc` becomes `1.2.3-beta`. Package ids are case-insensitive: they are
  lower-cased in R2 keys and in feed paths, but served with the original casing from the
  manifest.

Versions are compared as exact strings for immutability (for nuget, the normalised strings, so
`1.2` and `1.2.0` are the same version).

## 6. Endpoints

| Method | Path                                                  | Kind            | Section |
| ------ | ----------------------------------------------------- | --------------- | ------- |
| GET    | `/api/v1/version`                                     | none            | 6.2     |
| GET    | `/api/v1/whoami`                                      | any             | 6.3     |
| POST   | `/api/v1/publish/init`                                | `publish`       | 6.4     |
| POST   | `/api/v1/publish/commit`                              | `publish`       | 6.5     |
| GET    | `/api/v1/packages`                                    | `read`, `admin` | 6.6     |
| GET    | `/api/v1/packages/<eco>/<name>`                       | `read`, `admin` | 6.7     |
| POST   | `/api/v1/packages/<eco>/<name>/<version>/yank`        | `admin`         | 6.8     |
| DELETE | `/api/v1/packages/<eco>/<name>/<version>`             | `admin`         | 6.9     |
| DELETE | `/api/v1/packages/<eco>/<name>`                       | `admin`         | 6.10    |
| POST   | `/api/v1/packages/<eco>/<name>/reindex`               | `admin`         | 6.11    |
| GET    | `/api/v1/credentials`                                 | `admin`         | 6.12    |
| POST   | `/api/v1/credentials`                                 | `admin`         | 6.13    |
| GET    | `/api/v1/credentials/<project>`                       | `admin`         | 6.14    |
| PATCH  | `/api/v1/credentials/<project>`                       | `admin`         | 6.15    |
| DELETE | `/api/v1/credentials/<project>`                       | `admin`         | 6.16    |
| POST   | `/api/v1/credentials/<project>/secrets`               | `admin`         | 6.17    |
| DELETE | `/api/v1/credentials/<project>/secrets/<hash-prefix>` | `admin`         | 6.18    |

### 6.1 Auth entry identifiers

Each auth entry has an `id`: for `key` entries, the first 12 hex characters of the SHA-256 of
the secret (the same digest the record stores); for `gh` entries, an opaque 12-character hex
value that stays stable for the entry's lifetime. `<hash-prefix>` in a path is that `id` or
any prefix of it of at least 8 characters. It must match exactly one entry of the credential:
no match → `404 not_found`, more than one → `400 invalid_request`.

### 6.2 `GET /api/v1/version`

No authentication, no version check.

Response `200`:

```json
{ "version": "0.4.0", "minClientVersion": "0.1.0", "schemaVersion": 1 }
```

| Field              | Type    | Description                                                                          |
| ------------------ | ------- | ------------------------------------------------------------------------------------ |
| `version`          | string  | semver of the running Worker                                                         |
| `minClientVersion` | string  | oldest supported CLI version (same as `X-Min-Client-Version`)                        |
| `schemaVersion`    | integer | data schema version the running Worker uses (`schema_version` in KV after migration) |

### 6.3 `GET /api/v1/whoami`

Any kind. Response `200`:

```json
{
  "project": "shop-frontend",
  "kind": "read",
  "grants": [{ "eco": "npm", "package": "@acme/sdk", "versions": "^2.0" }]
}
```

`project` is `[<source>.]<project>` (e.g. `okta.shop-frontend` for a provider-backed
credential). `grants` is `[]` for `admin`.

### 6.4 `POST /api/v1/publish/init`

Kind `publish`. Validates the declaration, checks the grant and immutability (or the append
rules, 6.4.1), stores the declaration in staging and returns one presigned upload URL per file.

Request:

| Field      | Type     | Required | Description                                                                                                                                                                                                                                                                                                                                                                  |
| ---------- | -------- | -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `eco`      | string   | yes      | `composer`, `npm`, `cargo`, `generic` or `nuget`; must be enabled in `ECOSYSTEMS`                                                                                                                                                                                                                                                                                            |
| `name`     | string   | yes      | package name: composer `vendor/package` (lower case), npm `@scope/name` (scoped names only), cargo crate name, generic `[a-z0-9][a-z0-9._-]*` with an optional single group segment (`acme/plugin-foo`) and without `__` anywhere (R2 keys encode `/` as `__`, so `acme__x` would collide with `acme/x`), nuget the package id as in the `.nuspec` (`[A-Za-z0-9_.-]{1,100}`) |
| `version`  | string   | yes      | version, see 5.2                                                                                                                                                                                                                                                                                                                                                             |
| `files`    | object[] | yes      | the files of this upload, see below; 1 to 64 entries, names unique                                                                                                                                                                                                                                                                                                           |
| `manifest` | object   | yes      | ecosystem-native metadata, or the free-form `meta` for generic; see below                                                                                                                                                                                                                                                                                                    |
| `tag`      | string   | no       | npm only: dist-tag to point at this version. Default `latest`; for a prerelease version without `tag` the default is `next`.                                                                                                                                                                                                                                                 |
| `append`   | boolean  | no       | default `false`; `true` adds files to an existing version, generic only (6.4.1)                                                                                                                                                                                                                                                                                              |

`tag` must match `^[a-z][a-z0-9._-]{0,63}$` and must not itself be a valid semver range (for
example `v1` or `x`); sending it for any ecosystem other than npm is `400 invalid_request`.

Each entry of `files`:

| Field    | Type    | Required | Description                                                                                                                                                                            |
| -------- | ------- | -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `name`   | string  | yes      | file name, `[A-Za-z0-9._+-]{1,255}`, unique within the request (compared byte for byte); `.`, `..`, `manifest.json`, `meta.json` and names starting with `_` are reserved and rejected |
| `size`   | integer | yes      | size in bytes, `1 ≤ size ≤ MAX_UPLOAD_BYTES` (default 536870912) for each file individually; above → `413 payload_too_large`. There is no limit on the total of all files.             |
| `sha256` | string  | yes      | SHA-256 of the file, 64 lower-case hex characters                                                                                                                                      |
| `role`   | string  | no       | the file's role, see below; ignored for generic                                                                                                                                        |
| `labels` | object  | no       | generic only, see below; ignored for the other ecosystems                                                                                                                              |

Rules per ecosystem:

- **composer, npm, cargo**: exactly one file with `role: "archive"`; a single file without
  `role` is treated (and stored) as `archive`. The archive's name ends in `.zip` (composer),
  `.tgz` (npm) or `.crate` (cargo). No other roles are defined, so a second file, or a file
  with another role, is `400 invalid_request`. `labels` is ignored and not stored.
- **nuget**: exactly one file with `role: "archive"`, a `.nupkg` named
  `<id>.<version>.nupkg` in lower case, and optionally one file with `role: "symbols"`, a
  `.snupkg` with the same base name. `<version>` in the file name may be the version as sent
  or its normalised form. A single file without `role` is treated as `archive`; any other file
  is `400 invalid_request`. `labels` is ignored and not stored.
- **generic**: any number of files; `role` is ignored and not stored. `labels` is an optional
  object mapping strings to strings: at most 32 entries, keys `[a-z0-9][a-z0-9._-]{0,63}`,
  values strings of at most 256 characters; anything else is `400 invalid_request`. Labels are
  stored as sent (`{}` when absent) and are not interpreted, only filtered on (8.1).

`manifest` per ecosystem:

- **composer**: the complete `composer.json` of the version as a JSON object. `name` is
  required and must equal `name`. If it contains `version`, that must equal `version`. The
  Worker adds `version`, `version_normalized` and `dist` itself when building the index.
- **npm**: the `package.json` found inside the tarball (`package/package.json`), as a JSON
  object. `name` and `version` must equal the request's `name` and `version`. The Worker adds
  `_id` and `dist` (`tarball` pointing to the Worker and `integrity` as
  `sha256-<base64>` of the verified digest) itself; fields starting with `_` are dropped.
- **cargo**: one cargo sparse-index line without the fields the Worker computes (`cksum`,
  `yanked`, `v`), produced by the CLI from `cargo metadata`:

  ```json
  {
    "name": "acme-core",
    "vers": "1.4.2",
    "deps": [
      {
        "name": "serde",
        "req": "^1.0",
        "features": ["derive"],
        "optional": false,
        "default_features": true,
        "target": null,
        "kind": "normal",
        "registry": "https://github.com/rust-lang/crates.io-index",
        "package": null
      }
    ],
    "features": { "default": ["std"], "std": [] },
    "features2": { "json": ["dep:serde_json"] },
    "links": null,
    "rust_version": "1.75"
  }
  ```

  - `name`, `vers`: required, equal to the request's `name` and `version`.
  - `deps`: required array in the index shape: `name` is the name used in code (the rename
    when the dependency is renamed) and `package` is the real crate name when renamed,
    otherwise `null`; `req` is the requirement as written; `kind` is `normal`, `dev` or
    `build`; `target` is the `cfg(…)` or triple string or `null`; `registry` is the index URL
    of the dependency's registry, or `null` when it comes from this registry. Dependencies
    from crates.io use `https://github.com/rust-lang/crates.io-index`.
  - `features`: required object; `features2` optional object holding the features that use the
    `dep:` or `pkg?/feature` syntax, split exactly like `cargo publish` does. The Worker
    always serves index lines with `v: 2`.
  - `links`, `rust_version`: optional, string or `null`.
  - `cksum` is the `sha256` of the archive file.

- **generic**: the version's `meta`: any JSON object, possibly empty (`{}`), at most 64 KiB
  (65536 bytes) in its compact JSON serialisation as UTF-8; larger is `400 invalid_request`.
  The registry does not interpret it; it stores it unchanged and returns it as `meta` in the
  generic feed (section 8).
- **nuget**: the package metadata, parsed by the CLI from the `.nuspec` inside the `.nupkg`:

  ```json
  {
    "id": "Acme.Sdk",
    "version": "1.2.0",
    "nuspec": "<?xml version=\"1.0\" encoding=\"utf-8\"?><package …>…</package>",
    "description": "Acme SDK for .NET",
    "authors": "Acme",
    "projectUrl": "https://github.com/acme/sdk-dotnet",
    "licenseExpression": "MIT",
    "tags": ["acme", "sdk"],
    "dependencyGroups": [
      {
        "targetFramework": "net8.0",
        "dependencies": [{ "id": "Newtonsoft.Json", "range": "[13.0.1, )" }]
      },
      { "targetFramework": null, "dependencies": [] }
    ]
  }
  ```

  - `id`, `version`: required, equal to the request's `name` and `version` (as sent).
  - `nuspec`: required, the complete `.nuspec` XML as a string. The Worker serves it verbatim
    (9.5).
  - `dependencyGroups`: required array; `targetFramework` is the target framework moniker or
    `null` for a group without one; `dependencies[].id` is a package id and `range` the NuGet
    version range as written in the `.nuspec`.
  - `description`, `authors`, `projectUrl`, `licenseExpression`: optional strings; `tags`:
    optional array of strings.
  - The Worker builds the registration and search responses (9.6, 9.7) from these fields.

Example request (npm):

```json
{
  "eco": "npm",
  "name": "@acme/sdk",
  "version": "2.3.1",
  "files": [
    {
      "name": "sdk-2.3.1.tgz",
      "size": 48213,
      "sha256": "9f2c4e0a1b3d5f7092a4c6e8b0d2f4a6c8e0b2d4f6a8c0e2b4d6f8a0c2e4b6d8",
      "role": "archive"
    }
  ],
  "manifest": { "name": "@acme/sdk", "version": "2.3.1", "main": "dist/index.js" },
  "tag": "latest"
}
```

Example request (generic, one job of a CI matrix):

```json
{
  "eco": "generic",
  "name": "acme/plugin-foo",
  "version": "1.4.0",
  "files": [
    {
      "name": "plugin_foo-x86_64-pc-windows-msvc.dll",
      "size": 1048576,
      "sha256": "2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae",
      "labels": { "platform": "x86_64-pc-windows-msvc", "kind": "library" }
    }
  ],
  "manifest": { "minAppVersion": "3.2" },
  "append": true
}
```

Response `200`:

```json
{
  "uploadId": "01JBX9T6Q4ZK3M8N2P5R7S9V0W",
  "expires": "2026-10-28T10:22:00Z",
  "uploads": [
    {
      "name": "sdk-2.3.1.tgz",
      "putUrl": "https://<account>.r2.cloudflarestorage.com/<bucket>/_staging/01JBX9T6Q4ZK3M8N2P5R7S9V0W/sdk-2.3.1.tgz?X-Amz-Algorithm=AWS4-HMAC-SHA256&…",
      "headers": {
        "content-length": "48213",
        "x-amz-checksum-sha256": "nyxOChs9X3CSpMbosNL0psjgstT2qMDitNb4oMLkttg="
      }
    }
  ]
}
```

| Field      | Type     | Description                                                                |
| ---------- | -------- | -------------------------------------------------------------------------- |
| `uploadId` | string   | opaque identifier, `[A-Za-z0-9_-]{16,64}`; pass it to `commit`             |
| `expires`  | string   | when every `putUrl` of this upload stops working (10 minutes after `init`) |
| `uploads`  | object[] | one entry per file of the request, in the same order                       |

Each entry of `uploads`:

| Field     | Type   | Description                                                                                  |
| --------- | ------ | -------------------------------------------------------------------------------------------- |
| `name`    | string | the file name from the request                                                               |
| `putUrl`  | string | presigned SigV4 `PUT` URL on the R2 S3 endpoint; the host is not the Worker's host           |
| `headers` | object | exactly `content-length` (decimal string) and `x-amz-checksum-sha256` (base64 of the digest) |

Every file is uploaded with `PUT <putUrl>` of its entry, with the file bytes as the body and
exactly the headers from its `headers` (plus `Host`). The CLI must send a fixed-length body (no
chunked transfer encoding), must not add `x-amz-*` or `Content-Type` headers, and must not
modify `putUrl`. The signature covers both headers, so R2 rejects any other content. R2 answers
`200` on success. The files may be uploaded in any order, also in parallel.

Errors: `400 invalid_request` (validation, name or version mismatch with `manifest`, `append`
for an ecosystem other than generic), `403 forbidden` (no `publish` grant covering the name and
version), `409 version_exists`, `409 version_closed`, `409 file_exists`,
`413 payload_too_large`.

A pending `init` reserves neither the version nor the file names: two uploads of the same
version may coexist, and `commit` checks the rules again (6.5). Uncommitted uploads are deleted
by the hourly cron after 24 hours. `--dry-run` stops after this call.

#### 6.4.1 Appending files to an existing version

A CI matrix that builds one file per job publishes them as one version by sending
`append: true` from every job:

- Only generic supports it. `append: true` for any other ecosystem is `400 invalid_request`,
  whether or not the version exists. `append: false` (or no `append`) is accepted everywhere.
- On a version that does not exist, `append: true` behaves exactly like a normal publish.
- On an existing version it is allowed when `published` is less than `APPEND_WINDOW_HOURS`
  (default 24) hours ago and the version is not yanked, otherwise `409 version_closed`; and
  when every file name is new in the version, otherwise `409 file_exists`.
- The grant check is the same as for a new version. `manifest` is validated but ignored: the
  version keeps the `meta` of its first publish. `published` and `publishedBy` do not change.
- Without `append`, an existing version is `409 version_exists`, as for every ecosystem.

A file, once committed, is immutable; appending only adds entries to the version's `files`.

### 6.5 `POST /api/v1/publish/commit`

Kind `publish`, and the credential must be the one that called `init`.

Request:

```json
{ "uploadId": "01JBX9T6Q4ZK3M8N2P5R7S9V0W" }
```

The Worker runs `head()` on every staged file and compares it with the declaration, checks
immutability again (for an upload with `append: true`, the rules of 6.4.1), copies the files
to their final keys, writes the version manifest (an append adds its entries to `files` with a
conditional write, retried on conflict), rebuilds the package index (and npm dist-tags), and
deletes the staged files.

The rules are checked against the state at commit time. Without `append`, the first commit of
a version wins and later ones get `409 version_exists`. With `append`, a commit that finds the
version already published (for example by a parallel job of the same CI matrix) is applied as
an append when 6.4.1 allows it, and fails with `409 version_closed` or `409 file_exists`
otherwise.

Response `201`:

```json
{
  "eco": "npm",
  "name": "@acme/sdk",
  "version": "2.3.1",
  "url": "https://packages.example.com/npm/@acme/sdk/-/sdk-2.3.1.tgz",
  "files": [
    {
      "name": "sdk-2.3.1.tgz",
      "url": "https://packages.example.com/npm/@acme/sdk/-/sdk-2.3.1.tgz"
    }
  ]
}
```

For generic:

```json
{
  "eco": "generic",
  "name": "acme/plugin-foo",
  "version": "1.4.0",
  "url": "https://packages.example.com/generic/acme%2Fplugin-foo/1.4.0",
  "files": [
    {
      "name": "plugin_foo-x86_64-pc-windows-msvc.dll",
      "url": "https://packages.example.com/generic/acme%2Fplugin-foo/1.4.0/plugin_foo-x86_64-pc-windows-msvc.dll"
    }
  ]
}
```

`version` is the stored version (normalised for nuget). `url` is the archive's download URL in
the ecosystem's native protocol, or the version's URL in the generic feed:

| Eco        | `url`                                                                      |
| ---------- | -------------------------------------------------------------------------- |
| `composer` | `<PUBLIC_URL>/composer/dist/<vendor>/<package>/<version>.zip`              |
| `npm`      | `<PUBLIC_URL>/npm/<name>/-/<file>`                                         |
| `cargo`    | `<PUBLIC_URL>/cargo/dl/<crate>/<version>`                                  |
| `generic`  | `<PUBLIC_URL>/generic/<name>/<version>`                                    |
| `nuget`    | `<PUBLIC_URL>/nuget/flat/<id>/<version>/<id>.<version>.nupkg` (lower case) |

`files` lists the files added by this upload (for an append, only the appended ones; the status
is `201` either way), each with its download URL. For generic that is
`<PUBLIC_URL>/generic/<name>/<version>/<file>`; generic names are encoded as described in 8.1.
For composer, npm, cargo and nuget the archive's entry has the same `url` as the response; the
nuget `symbols` file has no download endpoint and its `url` is `null`.

Errors:

- `400 upload_incomplete`: at least one staged file is missing (its `PUT` never completed).
  Nothing is published and the upload stays as it was: the client may finish the missing
  `PUT`s while the URLs are valid and commit again, or start again from `init`.
- `404 not_found`: `uploadId` unknown, expired (cleaned up) or created by another credential.
- `409 upload_mismatch`: the size or SHA-256 of a staged file differs from the declaration.
  Nothing is published; start again from `init`.
- `409 version_exists`: the version was published after this `init` (upload without `append`).
- `409 version_closed`, `409 file_exists`: an append upload whose rules (6.4.1) no longer hold.

Commit is idempotent: after a successful commit the Worker keeps a small record of the result
under the upload's staging prefix until the cron removes it (24 hours), and a repeated
`commit` with the same `uploadId` returns the same `201` body. A client that lost the response
to `commit` can therefore simply retry it.

### 6.6 `GET /api/v1/packages`

Kind `read` or `admin`. Lists packages with their versions. For `read`, only versions covered
by the credential's grants are listed and packages with no such version are omitted; for
`admin`, everything is listed. Yanked versions are included and marked. Ecosystems disabled in
`ECOSYSTEMS` are omitted.

Response `200`:

```json
{
  "packages": [
    {
      "eco": "composer",
      "name": "acme/sdk",
      "versions": [
        {
          "version": "1.2.0",
          "published": "2026-10-28T10:12:00Z",
          "yanked": false,
          "files": [
            { "name": "sdk-1.2.0.zip", "size": 183204, "sha256": "3b1f…", "role": "archive" }
          ]
        }
      ]
    },
    {
      "eco": "generic",
      "name": "acme/plugin-foo",
      "versions": [
        {
          "version": "1.4.0",
          "published": "2026-10-28T11:02:00Z",
          "yanked": false,
          "files": [
            {
              "name": "plugin_foo-x86_64-pc-windows-msvc.dll",
              "size": 1048576,
              "sha256": "2c26…",
              "labels": { "platform": "x86_64-pc-windows-msvc", "kind": "library" }
            }
          ]
        }
      ]
    }
  ]
}
```

Packages are sorted by `eco` and then `name` (byte order); versions by precedence, highest
first. For nuget, `name` is the id with the casing of its highest version's manifest.

### 6.7 `GET /api/v1/packages/<eco>/<name>`

Kind `read` or `admin`. Response `200`: one `Package` object (same filtering as 6.6), e.g.
`GET /api/v1/packages/npm/%40acme%2Fsdk`:

```json
{
  "eco": "npm",
  "name": "@acme/sdk",
  "versions": [
    {
      "version": "2.3.1",
      "published": "2026-10-28T10:12:00Z",
      "yanked": false,
      "files": [{ "name": "sdk-2.3.1.tgz", "size": 48213, "sha256": "9f2c…", "role": "archive" }]
    }
  ]
}
```

Errors: `404 not_found` when the package does not exist or, for `read`, when no version of it
is covered by the grants.

### 6.8 `POST /api/v1/packages/<eco>/<name>/<version>/yank`

Kind `admin`. No body. Marks the version as yanked: it disappears from the composer and npm
indexes, is listed with `yanked: true` in the cargo index, is left out of the generic feed
unless `?include=yanked` (section 8) and gets `listed: false` in the NuGet registration while
the flat container keeps it (section 9). Its files stay downloadable, so consumers' lockfiles
keep working. An npm dist-tag pointing at it moves to the highest remaining stable version.
Yanking an already yanked version succeeds. A yanked generic version is closed for appending.

Response `200`: the `PackageVersion` plus `eco` and `name`:

```json
{
  "eco": "npm",
  "name": "@acme/sdk",
  "version": "2.3.1",
  "published": "2026-10-28T10:12:00Z",
  "yanked": true,
  "files": [{ "name": "sdk-2.3.1.tgz", "size": 48213, "sha256": "9f2c…", "role": "archive" }]
}
```

Errors: `404 not_found` (package or version).

### 6.9 `DELETE /api/v1/packages/<eco>/<name>/<version>`

Kind `admin`. No body. Deletes the manifest and all files of the version and rebuilds the
index; consumers' lockfiles that point at it stop working, and the version can be published
again. Dist-tags move as for yank.

Response `200`:

```json
{ "eco": "npm", "name": "@acme/sdk", "version": "2.3.1", "deleted": true }
```

Errors: `404 not_found`.

### 6.10 `DELETE /api/v1/packages/<eco>/<name>`

Kind `admin`. No body. Deletes every version, the package's indexes and its entry in
`_meta/packages.json`. Confirmation (typing the name) is the client's responsibility.

Response `200`:

```json
{ "eco": "composer", "name": "acme/sdk", "deletedVersions": ["1.2.0", "1.1.0"] }
```

Errors: `404 not_found`.

### 6.11 `POST /api/v1/packages/<eco>/<name>/reindex`

Kind `admin`. No body. Rebuilds the package's index files from the listing of its manifests.

Response `200`:

```json
{ "eco": "composer", "name": "acme/sdk", "manifests": 2 }
```

`manifests` is the number of version manifests the index was built from. Errors:
`404 not_found` when the package has no manifests.

### 6.12 `GET /api/v1/credentials`

Kind `admin`. Lists credentials stored in KV (provider-backed projects are not listed),
sorted by `project`.

Response `200`:

```json
{
  "credentials": [
    {
      "project": "shop-frontend",
      "kind": "read",
      "auth": [
        {
          "id": "3f9a1c0b7d2e",
          "type": "key",
          "created": "2026-10-28T09:00:00Z",
          "expires": null
        },
        {
          "id": "a41d07e9c2b5",
          "type": "key",
          "created": "2026-06-01T08:30:00Z",
          "expires": "2026-11-11T00:00:00Z"
        },
        {
          "id": "c08e5b1f9a27",
          "type": "gh",
          "repository": "acme/shop-frontend",
          "ref": "refs/heads/main",
          "workflow": null,
          "created": "2026-10-28T09:05:00Z",
          "expires": null
        }
      ],
      "grants": [
        { "eco": "composer", "package": "acme/*", "versions": "*" },
        { "eco": "npm", "package": "@acme/sdk", "versions": "^2.0" },
        { "eco": "cargo", "package": "acme-core", "versions": ">=1.4, <2" }
      ]
    }
  ]
}
```

### 6.13 `POST /api/v1/credentials`

Kind `admin`. Creates a credential and, by default, its first `key` secret.

Request:

| Field     | Type    | Required | Description                                                                       |
| --------- | ------- | -------- | --------------------------------------------------------------------------------- |
| `project` | string  | yes      | slug `[a-z0-9][a-z0-9-]{1,62}`                                                    |
| `kind`    | string  | yes      | `read`, `publish` or `admin`                                                      |
| `grants`  | Grant[] | no       | default `[]`; must be empty or absent for `admin`                                 |
| `secret`  | object  | no       | the first auth entry, same shape as the body of 6.17; default `{ "type": "key" }` |

Example:

```json
{
  "project": "shop-frontend",
  "kind": "read",
  "grants": [{ "eco": "composer", "package": "acme/*", "versions": "*" }]
}
```

Response `201`:

```json
{
  "credential": {
    "project": "shop-frontend",
    "kind": "read",
    "auth": [
      { "id": "3f9a1c0b7d2e", "type": "key", "created": "2026-10-28T09:00:00Z", "expires": null }
    ],
    "grants": [{ "eco": "composer", "package": "acme/*", "versions": "*" }]
  },
  "token": "shop-frontend:k7Qx2mN9pLw4vR8sT1bYc3dF6hJ0aZ5e"
}
```

`token` (including `TOKEN_PREFIX` when set) is present only when the first secret is of type
`key`, and is never returned again. Errors: `400 invalid_request`, `409 credential_exists`.

### 6.14 `GET /api/v1/credentials/<project>`

Kind `admin`. Response `200`: one `Credential`. Errors: `404 not_found`.

### 6.15 `PATCH /api/v1/credentials/<project>`

Kind `admin`. Replaces the grants and nothing else.

Request:

```json
{ "grants": [{ "eco": "npm", "package": "@acme/*", "versions": "*" }] }
```

`grants` is required and replaces the whole list (`[]` removes all access). Any other known
field (`project`, `kind`, `auth`) is `400 invalid_request`: kind and identity cannot change,
secrets are managed with 6.17 and 6.18. Non-empty `grants` on an `admin` credential is
`400 invalid_request`.

Response `200`: the updated `Credential`. Errors: `400 invalid_request`, `404 not_found`.

### 6.16 `DELETE /api/v1/credentials/<project>`

Kind `admin`. No body. Deletes the credential; its tokens stop working (KV propagation can
take up to about 60 seconds).

Response `200`:

```json
{ "project": "shop-frontend", "deleted": true }
```

Errors: `404 not_found`; `403 forbidden` when it is the last `admin` credential (deleting it
would lock everyone out of the panel).

### 6.17 `POST /api/v1/credentials/<project>/secrets`

Kind `admin`. Adds an auth entry.

Request:

| Field        | Type           | Required       | Description                                                      |
| ------------ | -------------- | -------------- | ---------------------------------------------------------------- |
| `type`       | string         | yes            | `key` or `gh`                                                    |
| `expires`    | string \| null | no             | RFC 3339 or `YYYY-MM-DD`; default `null` (never)                 |
| `repository` | string         | `gh`: yes      | `owner/repo`, matched exactly against the JWT `repository` claim |
| `ref`        | string \| null | no (`gh` only) | e.g. `refs/heads/main` or `refs/tags/v1.2.3`; `null` = any       |
| `workflow`   | string \| null | no (`gh` only) | matched exactly against the JWT `workflow` claim; `null` = any   |

`repository`, `ref` or `workflow` on a `key` entry is `400 invalid_request`, as is a `gh`
entry identical (same `repository`, `ref` and `workflow`) to an existing active one.

Response `201` for `key`:

```json
{
  "secret": {
    "id": "7c21de4490ab",
    "type": "key",
    "created": "2026-10-28T09:10:00Z",
    "expires": null
  },
  "token": "shop-frontend:Q2w9Er7Ty5Ui3Op1As8Df6Gh4Jk2Lz0X"
}
```

For `gh` the response has `secret` only; the CI job builds its token as
`<project>:gh.<jwt>`. Errors: `400 invalid_request`, `404 not_found`.

To rotate a secret, a client (the panel) calls this endpoint and then 6.18 with an `expires`
in the future for every previously active `key` entry.

### 6.18 `DELETE /api/v1/credentials/<project>/secrets/<hash-prefix>`

Kind `admin`. Expires an auth entry (it is not removed immediately; the hourly cron removes
entries whose `expires` has passed).

Optional request body:

```json
{ "expires": "2026-11-11T00:00:00Z" }
```

Default `expires` is now; a value in the past is treated as now. Response `200`:

```json
{
  "secret": {
    "id": "a41d07e9c2b5",
    "type": "key",
    "created": "2026-06-01T08:30:00Z",
    "expires": "2026-11-11T00:00:00Z"
  }
}
```

Errors: `400 invalid_request` (ambiguous prefix), `404 not_found`.

## 7. Client behaviour

### 7.1 Configuration

The CLI reads the base URL from `--registry` or `REGISTRY_URL` and the token from `--token`
or `REGISTRY_TOKEN` (or, with `--oidc`, builds `<project>:gh.<jwt>` from a GitHub Actions
OIDC token requested with audience `PUBLIC_URL`). It sends `Authorization: Bearer <token>`,
`X-Client-Version` and `Accept: application/json` on every API request, and
`Authorization: Bearer <token>` on every request to the Worker's generic feed.

### 7.2 Publishing sequence

The publish commands build the request as follows:

- `sreg composer publish`, `sreg npm publish`, `sreg cargo publish`: one file with
  `role: "archive"` and the ecosystem-native `manifest` (6.4).
- `sreg nuget publish [--version <v>] [--path .]`: runs `dotnet pack` (with
  `-p:PackageVersion=<v>` when `--version` is given), unpacks the `.nupkg` (a zip), reads the
  `.nuspec` into `manifest` (6.4) and sends the `.nupkg` as `archive`, plus the `.snupkg` as
  `symbols` when `dotnet pack` produced one.
- `sreg generic publish`: the given files with their `labels` and the `meta` as `manifest`. It
  sends `append: true` by default, so that every job of a CI matrix can add its files to the
  same version (6.4.1); `--no-append` sends `append: false` to require a fresh version.

Every publish then follows these steps:

1. Compute `size` and `sha256` of every file and build `manifest`.
2. `POST /api/v1/publish/init`. Stop here with `--dry-run`.
3. For every entry in `uploads`, `PUT <putUrl>` with exactly its `headers`:
   - up to 3 attempts per file in total, waiting 1 s and then 2 s (exponential backoff, ±20%
     jitter) between attempts;
   - retry on network errors, timeouts and `5xx` from R2;
   - if `expires` has passed (or is less than 30 seconds away) before an attempt, or R2
     answers `403` because the request expired, call `init` again with the same declaration
     and upload all files again with the new `uploadId` and `uploads` (files uploaded under the
     previous `uploadId` are not reused); `init` is called at most twice per publish;
   - any other `4xx` from R2 (e.g. a checksum mismatch because the file changed) is fatal.
4. `POST /api/v1/publish/commit`. On a network error or `5xx`, retry the commit (it is
   idempotent) with the same backoff. On `409 upload_mismatch` or `400 upload_incomplete`,
   start again from `init` once.
5. Print the result (`--json` prints the `201` body).

### 7.3 Downloading generic files

`sreg generic get <name> [<version>|latest] [--label k=v]... [--out <dir>] [--prerelease]`:

1. `GET /generic/<name>/<version>`, or `GET /generic/<name>/latest` (the default, with
   `?channel=prerelease` for `--prerelease`), with one `?label=k:v` per `--label` (8.4, 8.5).
   With `--json`, print this response and stop.
2. For every entry in `files`, `GET` its `url` with the `Authorization` header and without
   following redirects automatically. The answer is `302`; follow it manually with a `GET` of
   the `Location` URL that carries **no** `Authorization` header (and no other credential): the
   presigned URL authorises itself, the token must not reach the storage host, and R2 rejects
   a request that combines both. If the presigned `GET` answers `403` because it expired,
   request the `302` endpoint again (once per file).
3. Verify the size and SHA-256 of every downloaded file against the feed entry; on a mismatch,
   delete the file and fail.
4. Write the files into `--out` (default: the current directory) under their `name` and print
   the paths.

### 7.4 Other retries

`GET` requests, including the generic feed, may be retried on network errors, `5xx` and `429`
(honouring `Retry-After`). Clients do not retry non-idempotent admin calls
(`POST /api/v1/credentials`, `POST …/secrets`) automatically.

### 7.5 Exit codes

| Code | When                                                                                                                                                                |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0    | success                                                                                                                                                             |
| 1    | any error not listed below                                                                                                                                          |
| 2    | already published: `409 version_exists` or `409 file_exists` from `init` or `commit` (the pipeline can treat it as a no-op; a re-run matrix job gets `file_exists`) |
| 3    | `426 client_too_old` (the CLI prints the minimum version and a link to the release)                                                                                 |

## 8. Generic feed (`/generic/…`)

The read protocol of the generic ecosystem: a JSON feed for HTTP clients such as auto-updaters,
and the protocol `sreg generic get` uses. It is not part of `/api/v1`: sections 1.2, 1.4 and 3
do not apply. It exists only while `generic` is enabled in `ECOSYSTEMS`; otherwise every
`/generic/…` path answers `404 not_found`.

### 8.1 Conventions

- **Authentication**: any form of 2.2. Kind `read` sees only the versions covered by its
  `generic` grants, kind `admin` sees everything, kind `publish` gets `403 forbidden`. A
  missing or invalid credential gets `401 unauthorized` with
  `WWW-Authenticate: Bearer realm="serverless-registry"`. A `read` credential without a grant
  for the package gets `404 not_found`, as in section 4.
- **Methods**: only `GET`; any other method answers `404 not_found`. `/generic` is the same as
  `/generic/`.
- **Paths**: `<name>` is one path segment; the `/` of a grouped name is sent as `%2F`
  (`acme%2Fplugin-foo`), and the Worker matches on the raw path as in 1.3. A literal `/` would
  be ambiguous (`/generic/acme/latest`). `<version>` is the exact version, `<file>` the exact
  file name, both percent-encoded.
- **Responses**: JSON in UTF-8 (`Content-Type: application/json; charset=utf-8`), and
  `Cache-Control: private, no-store` on every response, like every native protocol. Errors use
  the body of section 4 (`invalid_request`, `unauthorized`, `forbidden`, `not_found`,
  `internal_error`, `provider_unavailable`).
- **ETag**: every `200` JSON response carries a strong `ETag`: a quoted prefix of the hex
  SHA-256 of the body (clients treat it as opaque). A request whose `If-None-Match` contains
  that value (or `*`) gets `304 Not Modified` with the `ETag` and no body. The body depends on
  the credential, so the `ETag` does too. `no-store` keeps shared caches out; an application
  that keeps its own copy of a response revalidates it with `If-None-Match`.
- **URLs**: every `url` field is the URL of the `302` endpoint of 8.6
  (`<PUBLIC_URL>/generic/<name>/<version>/<file>`), never a presigned URL, so a stored response
  stays valid.
- **Query parameters**:
  - `label=<key>:<value>`, repeatable: split at the first `:`; the key must match the label
    key format of 6.4, otherwise `400 invalid_request`. A file matches when it has every
    requested label with exactly the requested value. Labels filter only the files: a version
    without a matching file is still listed, with an empty `files` array.
  - `include=yanked`: also return yanked versions.
  - `channel=prerelease` (8.4 only): also consider prerelease versions.
  - Any other value of `include` or `channel` is `400 invalid_request`; unknown parameters
    are ignored.
- **Versions** are compared by semver precedence. `latest` is the highest non-yanked stable
  version covered by the credential's grants, computed on the fly from the manifests, or
  `null` when there is none.
- A file entry is `{ name, size, sha256, labels, url }`; `labels` is always present (`{}` when
  the file has none).

### 8.2 `GET /generic/`

Packages the credential can see (at least one version covered by its grants, yanked
included), sorted by name:

```json
{
  "packages": [
    { "name": "acme/plugin-foo", "latest": "1.4.0" },
    { "name": "acme/plugin-bar", "latest": null }
  ]
}
```

### 8.3 `GET /generic/<name>`

All versions of a package covered by the grants, highest first (clients must not rely on the
order of object keys); yanked versions only with `?include=yanked`. `?label=` filters the files
of every version. `latest` is not affected by `?label=` or `?include=`.

```json
{
  "name": "acme/plugin-foo",
  "latest": "1.4.0",
  "versions": {
    "1.4.0": {
      "published": "2026-10-28T11:02:00Z",
      "yanked": false,
      "meta": { "minAppVersion": "3.2" },
      "files": [
        {
          "name": "plugin_foo-x86_64-pc-windows-msvc.dll",
          "size": 1048576,
          "sha256": "2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae",
          "labels": { "platform": "x86_64-pc-windows-msvc", "kind": "library" },
          "url": "https://packages.example.com/generic/acme%2Fplugin-foo/1.4.0/plugin_foo-x86_64-pc-windows-msvc.dll"
        }
      ]
    }
  }
}
```

Errors: `404 not_found` when the package does not exist or no version of it is covered by
the grants.

### 8.4 `GET /generic/<name>/latest`

The highest non-yanked stable version covered by the grants; with `?channel=prerelease`,
prerelease versions are considered too. `?label=` filters its files as above. Response `200`
in the single-version shape:

```json
{
  "name": "acme/plugin-foo",
  "version": "1.4.0",
  "published": "2026-10-28T11:02:00Z",
  "yanked": false,
  "meta": { "minAppVersion": "3.2" },
  "files": [
    {
      "name": "plugin_foo-x86_64-pc-windows-msvc.dll",
      "size": 1048576,
      "sha256": "2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae",
      "labels": { "platform": "x86_64-pc-windows-msvc", "kind": "library" },
      "url": "https://packages.example.com/generic/acme%2Fplugin-foo/1.4.0/plugin_foo-x86_64-pc-windows-msvc.dll"
    }
  ]
}
```

Errors: `404 not_found` when no version qualifies.

### 8.5 `GET /generic/<name>/<version>`

One version in the shape of 8.4; `?label=` filters its files. Errors: `404 not_found` when the
version does not exist, is not covered by the grants, or is yanked and `?include=yanked` is
not given.

### 8.6 `GET /generic/<name>/<version>/<file>`

`302 Found` with `Location` set to a presigned `GET` URL of the file, valid for 15 minutes, and
no body. Files of yanked versions stay downloadable. Errors: `404 not_found` when the version
is not covered by the grants or the file does not exist. Clients follow the redirect without
the `Authorization` header (7.3).

## 9. NuGet feed (`/nuget/…`)

The NuGet V3 protocol, as used by `dotnet` and NuGet clients. It is not part of `/api/v1`:
sections 1.2, 1.4 and 3 do not apply. It exists only while `nuget` is enabled in `ECOSYSTEMS`;
otherwise every `/nuget/…` path answers `404 not_found`.

### 9.1 Conventions

- **Authentication**: any form of 2.2. NuGet clients send Basic credentials from
  `packageSourceCredentials` in `nuget.config` (user name = the token's login, password = the
  token's password); Bearer works too. Kind `read` sees only the versions covered by its
  `nuget` grants, kind `admin` sees everything, kind `publish` gets `403 forbidden`. A missing
  or invalid credential gets `401 unauthorized` with both
  `WWW-Authenticate: Basic realm="serverless-registry"` and
  `WWW-Authenticate: Bearer realm="serverless-registry"`, so that `dotnet` prompts for
  credentials. A `read` credential without a grant for the package gets `404 not_found`.
- **Methods**: only `GET`; any other method answers `404 not_found`.
- **Paths**: `<id>` and `<ver>` are lower case, with `<ver>` in the normalised form (5.2); the
  Worker matches `<id>` case-insensitively and normalises `<ver>` before matching.
- **Responses**: JSON in UTF-8, except the `.nuspec` (XML) and the `302` redirect;
  `Cache-Control: private, no-store` on every response. Error bodies follow section 4.
- **Yank**: a yanked version has `listed: false` in the registration and is left out of
  search, but the flat container still lists it and serves its `.nupkg` and `.nuspec`.
- All URLs below are absolute and start with `PUBLIC_URL`.

### 9.2 `GET /nuget/index.json`

The service index:

```json
{
  "version": "3.0.0",
  "resources": [
    {
      "@id": "https://packages.example.com/nuget/flat/",
      "@type": "PackageBaseAddress/3.0.0"
    },
    {
      "@id": "https://packages.example.com/nuget/registration/",
      "@type": "RegistrationsBaseUrl/3.6.0"
    },
    {
      "@id": "https://packages.example.com/nuget/search",
      "@type": "SearchQueryService/3.5.0"
    }
  ]
}
```

### 9.3 `GET /nuget/flat/<id>/index.json`

The versions of the package covered by the grants, yanked ones included, lower case and
normalised, in ascending order:

```json
{ "versions": ["1.0.0", "1.2.0-beta.1", "1.2.0"] }
```

Errors: `404 not_found` when the package does not exist or no version is covered.

### 9.4 `GET /nuget/flat/<id>/<ver>/<id>.<ver>.nupkg`

`302 Found` to a presigned `GET` URL of the `.nupkg`, valid for 15 minutes. .NET drops the
`Authorization` header when it follows a redirect to another host, so the redirect to R2 works.
Errors: `404 not_found`.

### 9.5 `GET /nuget/flat/<id>/<ver>/<id>.nuspec`

The stored `nuspec` string of the version, verbatim, with
`Content-Type: application/xml; charset=utf-8`. Errors: `404 not_found`.

### 9.6 `GET /nuget/registration/<id>/index.json`

The registration index with one page, inlined, holding every version covered by the grants
(yanked ones with `listed: false`) in ascending order. No separate page or leaf documents are
served, so their `@id` values are the URL of this document with a fragment.

```json
{
  "count": 1,
  "items": [
    {
      "@id": "https://packages.example.com/nuget/registration/acme.sdk/index.json#page/1.2.0/1.2.0",
      "count": 1,
      "lower": "1.2.0",
      "upper": "1.2.0",
      "items": [
        {
          "@id": "https://packages.example.com/nuget/registration/acme.sdk/index.json#1.2.0",
          "packageContent": "https://packages.example.com/nuget/flat/acme.sdk/1.2.0/acme.sdk.1.2.0.nupkg",
          "catalogEntry": {
            "@id": "https://packages.example.com/nuget/registration/acme.sdk/index.json#1.2.0",
            "id": "Acme.Sdk",
            "version": "1.2.0",
            "listed": true,
            "published": "2026-10-28T10:12:00Z",
            "packageContent": "https://packages.example.com/nuget/flat/acme.sdk/1.2.0/acme.sdk.1.2.0.nupkg",
            "dependencyGroups": [
              {
                "targetFramework": "net8.0",
                "dependencies": [{ "id": "Newtonsoft.Json", "range": "[13.0.1, )" }]
              }
            ],
            "description": "Acme SDK for .NET",
            "authors": "Acme",
            "tags": ["acme", "sdk"]
          }
        }
      ]
    }
  ]
}
```

`catalogEntry` carries `id` (original casing), `version`, `listed` (not yanked), `published`,
`packageContent`, `dependencyGroups`, and `description`, `authors` and `tags` when the manifest
has them. A dependency group whose `targetFramework` is `null` is served without
`targetFramework`. Errors: `404 not_found`.

### 9.7 `GET /nuget/search?q=&skip=&take=&prerelease=`

A simple search over the packages in `_meta/packages.json`, filtered by the grants (it is what
`dotnet package search` uses).

| Parameter    | Default | Meaning                                                              |
| ------------ | ------- | -------------------------------------------------------------------- |
| `q`          | empty   | case-insensitive substring of the package id; empty matches every id |
| `skip`       | `0`     | number of results to skip                                            |
| `take`       | `20`    | number of results to return                                          |
| `prerelease` | `false` | `true` also considers prerelease versions                            |

A non-numeric `skip` or `take`, or a `prerelease` other than `true` or `false`, is
`400 invalid_request`; unknown parameters (such as `semVerLevel`) are ignored. Only listed (not
yanked) versions are considered, and packages without such a version are left out. Results are
sorted by id; `totalHits` counts all matches before `skip` and `take`.

```json
{
  "totalHits": 1,
  "data": [
    {
      "id": "Acme.Sdk",
      "version": "1.2.0",
      "description": "Acme SDK for .NET",
      "authors": "Acme",
      "tags": ["acme", "sdk"],
      "registration": "https://packages.example.com/nuget/registration/acme.sdk/index.json",
      "totalDownloads": 0,
      "versions": [
        {
          "version": "1.2.0",
          "downloads": 0,
          "@id": "https://packages.example.com/nuget/registration/acme.sdk/index.json#1.2.0"
        }
      ]
    }
  ]
}
```

`version` is the highest considered version; `versions` lists every considered version.
Download counts are not tracked and are always `0`.
