# HTTP API contract (`/api/v1`)

This document is the contract between the Worker and the `sreg` CLI (and the admin panel). It
is the source of truth for the `/api/v1` surface: a client can be implemented from this
document alone. The native read protocols (`/composer`, `/npm`, `/cargo`) and the panel's
`/login` and `/logout` are described in [spec.md](spec.md) and are out of scope here.

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
  `413 payload_too_large`.
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

| Parameter       | Format                                                                                                                                                                                              |
| --------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `<eco>`         | `composer`, `npm` or `cargo`. Any other value answers `404 not_found`.                                                                                                                              |
| `<name>`        | Package name, percent-encoded as one path segment: `/` must be sent as `%2F`, `@` may be sent literally or as `%40`. Examples: `acme%2Fsdk` (composer), `%40acme%2Fsdk` (npm), `acme-core` (cargo). |
| `<version>`     | Exact version string as published, percent-encoded (versions never contain `/`).                                                                                                                    |
| `<project>`     | A local project slug, `[a-z0-9][a-z0-9-]{1,62}`. A value containing `.` (a provider-backed project) answers `400 invalid_request`.                                                                  |
| `<hash-prefix>` | 8 to 12 lower-case hexadecimal characters identifying an auth entry (see 6.1).                                                                                                                      |

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
7. Grant check and endpoint logic → `403 forbidden`, `404 not_found`, `409 …`.

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

| Kind      | May call                                                                    |
| --------- | --------------------------------------------------------------------------- |
| `read`    | `whoami`, package listing (filtered by its grants)                          |
| `publish` | `whoami`, `publish/init`, `publish/commit` (within its grants)              |
| `admin`   | `whoami`, package listing (unfiltered), package administration, credentials |

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

| Code                   | Status | Meaning                                                                                                    |
| ---------------------- | ------ | ---------------------------------------------------------------------------------------------------------- |
| `invalid_request`      | 400    | Malformed JSON, missing or invalid field, malformed header, constraint that cannot be parsed, and similar. |
| `unauthorized`         | 401    | No credential, or the credential could not be verified (unknown project, wrong secret, expired, bad JWT).  |
| `forbidden`            | 403    | Valid credential, but wrong kind, no matching grant, missing `X-Requested-With`, or a protected operation. |
| `not_found`            | 404    | Unknown path or method, unknown package, version, credential, auth entry or `uploadId`.                    |
| `version_exists`       | 409    | The version is already published (on `init` and on `commit`).                                              |
| `upload_mismatch`      | 409    | On `commit`: the staged object is missing or its size or SHA-256 differs from the `init` declaration.      |
| `credential_exists`    | 409    | `POST /api/v1/credentials` for a project that already exists.                                              |
| `payload_too_large`    | 413    | Request body over 4 MiB, or `size` in `init` above `MAX_UPLOAD_BYTES`.                                     |
| `client_too_old`       | 426    | `X-Client-Version` is lower than `X-Min-Client-Version`.                                                   |
| `rate_limited`         | 429    | Reserved; not emitted in `v1`. If received, wait for `Retry-After` seconds (default 10) and retry.         |
| `internal_error`       | 500    | Unexpected failure in the Worker.                                                                          |
| `provider_unavailable` | 503    | The external credential provider of the token's source did not answer. Never falls back to no auth.        |

To avoid leaking the existence of packages, a `read` credential asking for a package it has
no grant for receives `404 not_found`, not `403`.

## 5. Common types

Written as TypeScript for precision; `?` marks an optional field, `| null` a field that is
always present but may be `null`.

```ts
type Eco = "composer" | "npm" | "cargo";
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

interface PackageVersion {
  version: string;
  published: string; // RFC 3339
  yanked: boolean;
  size: number;
  sha256: string;
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

### 5.1 Grants

- `eco`: one of the three ecosystems.
- `package`: an exact package name, or a pattern whose only `*` is its last character and
  starts a segment or ends one (`acme/*`, `@acme/*`, `acme-*`). `*` matches any sequence of
  characters that does not contain `/`. The pattern `*` alone matches every package of the
  ecosystem.
- `versions`: a constraint in the ecosystem's syntax (Composer, npm/node-semver, cargo
  `VersionReq`), or `*` for all versions. A constraint that does not parse is
  `400 invalid_request`.
- A credential without a grant for an ecosystem has no access to it. For `read` a grant means
  "may install", for `publish` "may publish".

CLI shorthand: `eco:package[:versions]`, split at the first two `:`; `versions` defaults to
`*`. Examples: `composer:acme/*:*`, `npm:@acme/sdk:^2.0`, `cargo:acme-core:>=1.4, <2`
(quote it in the shell).

### 5.2 Versions

All ecosystems accept only semver 2.0.0 versions `MAJOR.MINOR.PATCH[-PRERELEASE]` without a
leading `v` and without build metadata (`+…` is rejected because Composer cannot represent it
and cargo ignores it for identity). For composer the prerelease, when present, must be
`alpha`, `beta` or `rc`, optionally followed by a number with or without a dot
(`1.0.0-beta.1`, `1.0.0-rc2`), because Composer does not understand other labels. Versions
are compared as exact strings for immutability.

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

Kind `publish`. Validates the declaration, checks the grant and immutability, stores the
declaration in staging and returns a presigned upload URL.

Request:

| Field      | Type    | Required | Description                                                                                                                  |
| ---------- | ------- | -------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `eco`      | string  | yes      | `composer`, `npm` or `cargo`                                                                                                 |
| `name`     | string  | yes      | package name: composer `vendor/package` (lower case), npm `@scope/name` (scoped names only), cargo crate name                |
| `version`  | string  | yes      | version, see 5.2                                                                                                             |
| `file`     | string  | yes      | archive file name, `[A-Za-z0-9._+-]{1,255}`, extension `.zip` (composer), `.tgz` (npm) or `.crate` (cargo)                   |
| `size`     | integer | yes      | archive size in bytes, `1 ≤ size ≤ MAX_UPLOAD_BYTES` (default 536870912); above → `413 payload_too_large`                    |
| `sha256`   | string  | yes      | SHA-256 of the archive, 64 lower-case hex characters                                                                         |
| `manifest` | object  | yes      | ecosystem-native metadata, see below                                                                                         |
| `tag`      | string  | no       | npm only: dist-tag to point at this version. Default `latest`; for a prerelease version without `tag` the default is `next`. |

`tag` must match `^[a-z][a-z0-9._-]{0,63}$` and must not itself be a valid semver range (for
example `v1` or `x`); sending it for composer or cargo is `400 invalid_request`.

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
  - `cksum` is the request's `sha256`.

Example request:

```json
{
  "eco": "npm",
  "name": "@acme/sdk",
  "version": "2.3.1",
  "file": "sdk-2.3.1.tgz",
  "size": 48213,
  "sha256": "9f2c4e0a1b3d5f7092a4c6e8b0d2f4a6c8e0b2d4f6a8c0e2b4d6f8a0c2e4b6d8",
  "manifest": { "name": "@acme/sdk", "version": "2.3.1", "main": "dist/index.js" },
  "tag": "latest"
}
```

Response `200`:

```json
{
  "uploadId": "01JBX9T6Q4ZK3M8N2P5R7S9V0W",
  "putUrl": "https://<account>.r2.cloudflarestorage.com/<bucket>/_staging/01JBX9T6Q4ZK3M8N2P5R7S9V0W/sdk-2.3.1.tgz?X-Amz-Algorithm=AWS4-HMAC-SHA256&…",
  "expires": "2026-10-28T10:22:00Z",
  "headers": {
    "content-length": "48213",
    "x-amz-checksum-sha256": "nyxOChs9X3CSpMbosNL0psjgstT2qMDitNb4oMLkttg="
  }
}
```

| Field      | Type   | Description                                                                                  |
| ---------- | ------ | -------------------------------------------------------------------------------------------- |
| `uploadId` | string | opaque identifier, `[A-Za-z0-9_-]{16,64}`; pass it to `commit`                               |
| `putUrl`   | string | presigned SigV4 `PUT` URL on the R2 S3 endpoint; the host is not the Worker's host           |
| `expires`  | string | when `putUrl` stops working (10 minutes after `init`)                                        |
| `headers`  | object | exactly `content-length` (decimal string) and `x-amz-checksum-sha256` (base64 of the digest) |

The upload is `PUT <putUrl>` with the archive bytes as the body and exactly the headers from
`headers` (plus `Host`). The CLI must send a fixed-length body (no chunked transfer encoding),
must not add `x-amz-*` or `Content-Type` headers, and must not modify `putUrl`. The signature
covers both headers, so R2 rejects any other content. R2 answers `200` on success.

Errors: `400 invalid_request` (validation, name or version mismatch with `manifest`),
`403 forbidden` (no `publish` grant covering the name and version), `409 version_exists`,
`413 payload_too_large`.

A pending `init` does not reserve the version: two uploads of the same version may coexist;
the first `commit` wins and the second gets `409 version_exists`. Uncommitted uploads are
deleted by the hourly cron after 24 hours. `--dry-run` stops after this call.

### 6.5 `POST /api/v1/publish/commit`

Kind `publish`, and the credential must be the one that called `init`.

Request:

```json
{ "uploadId": "01JBX9T6Q4ZK3M8N2P5R7S9V0W" }
```

The Worker checks the staged object (`head()`) against the declaration, checks immutability
again, moves the archive to its final key, writes the version manifest, rebuilds the package
index (and npm dist-tags), and deletes the staged archive.

Response `201`:

```json
{
  "eco": "npm",
  "name": "@acme/sdk",
  "version": "2.3.1",
  "url": "https://packages.example.com/npm/@acme/sdk/-/sdk-2.3.1.tgz"
}
```

`url` is the archive's download URL in the ecosystem's native protocol:

| Eco        | `url`                                                         |
| ---------- | ------------------------------------------------------------- |
| `composer` | `<PUBLIC_URL>/composer/dist/<vendor>/<package>/<version>.zip` |
| `npm`      | `<PUBLIC_URL>/npm/<name>/-/<file>`                            |
| `cargo`    | `<PUBLIC_URL>/cargo/dl/<crate>/<version>`                     |

Errors:

- `404 not_found`: `uploadId` unknown, expired (cleaned up) or created by another credential.
- `409 upload_mismatch`: the staged object is missing (the `PUT` never completed) or its size
  or SHA-256 differs from the declaration. Nothing is published; start again from `init`.
- `409 version_exists`: the version was published after this `init`.

Commit is idempotent: after a successful commit the Worker keeps a small record of the result
under the upload's staging prefix until the cron removes it (24 hours), and a repeated
`commit` with the same `uploadId` returns the same `201` body. A client that lost the response
to `commit` can therefore simply retry it.

### 6.6 `GET /api/v1/packages`

Kind `read` or `admin`. Lists packages with their versions. For `read`, only versions covered
by the credential's grants are listed and packages with no such version are omitted; for
`admin`, everything is listed. Yanked versions are included and marked.

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
          "size": 183204,
          "sha256": "3b1f…"
        }
      ]
    }
  ]
}
```

Packages are sorted by `eco` and then `name` (byte order); versions by precedence, highest
first.

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
      "size": 48213,
      "sha256": "9f2c…"
    }
  ]
}
```

Errors: `404 not_found` when the package does not exist or, for `read`, when no version of it
is covered by the grants.

### 6.8 `POST /api/v1/packages/<eco>/<name>/<version>/yank`

Kind `admin`. No body. Marks the version as yanked: it disappears from the composer and npm
indexes and is listed with `yanked: true` in the cargo index; the archive stays downloadable.
An npm dist-tag pointing at it moves to the highest remaining stable version. Yanking an
already yanked version succeeds.

Response `200`: the `PackageVersion` plus `eco` and `name`:

```json
{
  "eco": "npm",
  "name": "@acme/sdk",
  "version": "2.3.1",
  "published": "2026-10-28T10:12:00Z",
  "yanked": true,
  "size": 48213,
  "sha256": "9f2c…"
}
```

Errors: `404 not_found` (package or version).

### 6.9 `DELETE /api/v1/packages/<eco>/<name>/<version>`

Kind `admin`. No body. Deletes the manifest and the archive and rebuilds the index; the
version can be published again. Dist-tags move as for yank.

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

Example (`sreg token create --project shop-frontend --kind read --grant composer:acme/*:*`):

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

`sreg token rotate --project <p> --expire-old 14d` is this call followed by 6.18 with
`expires` = now + 14 days for every previously active `key` entry.

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
`X-Client-Version` and `Accept: application/json` on every API request.

### 7.2 Publishing sequence

1. Compute `size` and `sha256` of the archive and extract `manifest`.
2. `POST /api/v1/publish/init`. Stop here with `--dry-run`.
3. `PUT <putUrl>` with exactly `headers`:
   - up to 3 attempts in total, waiting 1 s and then 2 s (exponential backoff, ±20% jitter)
     between attempts;
   - retry on network errors, timeouts and `5xx` from R2;
   - if `expires` has passed (or is less than 30 seconds away) before an attempt, or R2
     answers `403` because the request expired, call `init` again and continue with the new
     `uploadId`, `putUrl` and `headers`; `init` is called at most twice per publish;
   - any other `4xx` from R2 (e.g. a checksum mismatch because the file changed) is fatal.
4. `POST /api/v1/publish/commit`. On a network error or `5xx`, retry the commit (it is
   idempotent) with the same backoff. On `409 upload_mismatch`, start again from `init` once.
5. Print the result (`--json` prints the `201` body).

### 7.3 Other retries

`GET` requests may be retried on network errors, `5xx` and `429` (honouring `Retry-After`).
Non-idempotent admin calls (`POST /api/v1/credentials`, `POST …/secrets`) are not retried
automatically.

### 7.4 Exit codes

| Code | When                                                                                |
| ---- | ----------------------------------------------------------------------------------- |
| 0    | success                                                                             |
| 1    | any error not listed below                                                          |
| 2    | `409 version_exists` from `init` or `commit` (the pipeline can treat it as a no-op) |
| 3    | `426 client_too_old` (the CLI prints the minimum version and a link to the release) |
