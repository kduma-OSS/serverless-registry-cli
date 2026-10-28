# Serverless Registry – specification

## Goals and assumptions

We are building a private, serverless package repository for Composer, npm, cargo, NuGet and arbitrary files (generic), hosted entirely on Cloudflare (Workers, R2, KV), with publishing done through our own CLI written in Rust. It is the successor of the current Serverless Satis solution: a new deployment, a new address, no backwards compatibility.

**What we build**

- A Worker serving the native read protocols of four ecosystems plus a simple JSON feed for generic files, from a single R2 bucket.
- A push model: the CLI packs and uploads a version in the publishing pipeline (GHA); the Worker owns the index.
- Named credentials per consumer project, with grants for packages and version ranges, safe to commit into the consumer's repository.
- Versions are immutable from the CLI; an admin can delete or yank a version.
- All file transfer goes through presigned URLs to R2 (upload and download); the Worker serves metadata only.
- A home page with login, a list of available packages and ready-to-use configuration instructions; a minimal admin panel.
- Open source, deployable with one button, updated by syncing the fork.

**What we do not build**

- Scanning of VCS repositories (pull) – replaced by push from the CLI.
- Publishing endpoints for native clients (`npm publish`, `cargo publish` straight to the Worker) – publishing happens exclusively through the CLI.
- An audit log, Durable Objects, Cloudflare Access, front-end frameworks, external dependencies in the panel.
- Mirroring of public registries.

## Architecture

One Worker with a shared kernel and five adapters, one R2 bucket as the source of truth, one KV namespace for credentials and sessions. Indexes are derived from manifests, not separate state.

_Architecture diagram (5 components):_ the package manager clients (Composer, npm, cargo, NuGet), HTTP clients of the generic feed and the `sreg` CLI talk only to the Worker for metadata; the Worker reads and writes the R2 bucket (package files, manifests, indexes) and the `AUTH` KV namespace (credentials, sessions, schema version); file bytes flow directly between the clients/CLI and R2 through presigned URLs; the hourly cron (`scheduled` handler of the same Worker) cleans up R2 staging and expired secrets in KV.

The Worker never passes archive bytes through itself: the CLI uploads them with a presigned PUT, and clients get a `302` to a presigned GET; only indexes, manifests and the page go through the Worker.

| Component          | Role                                                                      | Technology                                      |
| ------------------ | ------------------------------------------------------------------------- | ----------------------------------------------- |
| Worker             | Auth, publishing API, serving indexes, redirects to files, page and panel | TypeScript, Workers Static Assets for HTML/CSS  |
| R2                 | Package files, version manifests, generated indexes                       | R2 binding + S3 API key (secret) for presigning |
| KV                 | Credentials, panel sessions, `schema_version`                             | `AUTH` binding                                  |
| CLI                | Packing and publishing versions from CI                                   | Rust, binaries in GitHub Releases               |
| Cron (`scheduled`) | Cleaning up uncommitted uploads and expired secrets                       | handler in the same Worker                      |

**Kernel** (`src/core/`): parsing and verifying credentials, evaluating grants, SigV4 presigning, writing manifests and rebuilding indexes with optimistic concurrency, routing to adapters, sessions, cron.

**Adapter** (`src/adapters/<eco>/`): knows how to extract the name, version and dependencies from an archive and manifest, how to build its ecosystem's index from a list of manifests, and how to filter it for a given credential. It does not touch auth or R2 directly.

**The index as a derivative.** Every version is, in R2, a `manifest.json` (normalised metadata) plus its files (usually one: the archive). Indexes (`p2/*.json`, the npm packument, cargo sparse index files) are generated from the manifests at publish time and cached in R2; an admin can rebuild them from the listing. Per-credential filtering happens on the fly when serving, so a consumer with a limited grant never sees versions it has no access to.

## Credentials

One credential = the identity of a project (a consumer, a package's CI, or an admin) with exactly one role and a list of auth entries. The record comes from KV or from an external HTTP provider. The token is `[<source>.]<project>:[<type>.]<secret>` – a login:password pair in one string, which works as Bearer, Basic and as a raw header.

**Token format**

```text
[<source>.]<project>:[<type>.]<secret>

shop-frontend:k7Qx2mN9pLw4vR8sT1bYc3dF6hJ0aZ5e          KV, static key (default type)
ci-acme-sdk:gh.eyJhbGciOi….eyJpc3Mi….SflKxw…              KV, GitHub OIDC
okta.shop-frontend:Zm9vYmFy…                            provider "okta", key
okta.ci-acme-sdk:gh.eyJ…                                provider "okta", OIDC
```

- The colon separates the identity (login) from the credential (password); nothing on the left is secret, nothing on the right ever reaches the logs.
- `source` (optional): a provider identifier from `vars.AUTH_PROVIDERS`; absent = record in KV.
- `project`: a slug `[a-z0-9][a-z0-9-]{1,62}`, unique within the source.
- `type` (optional): the kind of secret – `key` (default, omitted) or `gh` (a JWT from GitHub OIDC); more types in the future.
- `secret`: for `key`, 32 base62 characters (~190 bits), generated by the Worker and shown once; for `gh`, the whole JWT (it contains dots – irrelevant, because the secret is everything after the type).
- An optional fixed prefix (e.g. `srg_`) in front of the whole token, to detect leaks with grep; enabled with the `TOKEN_PREFIX` flag.

**Parsing**: 1) split at the first colon into login and password; 2) login: one segment → project, two → `source.project`; 3) password: no dot → type `key`, with a dot → the first segment is the type, the rest is the secret. The lists of providers and types are used only for validation, not for determining boundaries.

**Authentication forms accepted by the Worker**

| Header                                         | Who sends it                                                        | Normalisation                                                                                                             |
| ---------------------------------------------- | ------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `Authorization: Bearer <token>`                | Composer (`bearer`), npm (`_authToken`), CLI, cargo                 | parser as above                                                                                                           |
| `Authorization: Basic base64(<token>)`         | Composer (`http-basic`), npm (`_auth`), cargo (`token = "Basic …"`) | the Basic login and password are exactly the two sides of the colon in the token; after decoding, the string is the token |
| `Authorization: Basic base64(<login>:<token>)` | the user pasted the whole token as the password                     | the password contains a colon → treated as the full token, the login must match, otherwise `401`                          |
| `Authorization: <token>` (raw)                 | cargo with `token = "<token>"`                                      | as Bearer                                                                                                                 |

| Token                           | Login                | Password            |
| ------------------------------- | -------------------- | ------------------- |
| `shop-frontend:k7Qx…`           | `shop-frontend`      | `k7Qx…`             |
| `ci-acme-sdk:gh.eyJ….eyJ….Sfl…` | `ci-acme-sdk`        | `gh.eyJ….eyJ….Sfl…` |
| `okta.shop-frontend:Zm9v…`      | `okta.shop-frontend` | `Zm9v…`             |
| `okta.ci-acme-sdk:gh.eyJ…`      | `okta.ci-acme-sdk`   | `gh.eyJ…`           |

**Record sources** – an interface `CredentialSource { resolve(project, type, secret): Record | null }` with two implementations:

- `KvSource` (no source prefix): reads `cred:<project>` and verifies the secret with the verifier for its type: `key` → SHA-256 compared in constant time with every active entry; `gh` → the JWT signature checked against the keys from `https://token.actions.githubusercontent.com/.well-known/jwks` (cached in KV for 24 h), `iss`, `aud` = `PUBLIC_URL`, `exp`, and the `repository`/`ref`/`workflow` claims against the entry in the record; a positive verification is cached in KV under the hash of the JWT until its exp, so the signature is not verified on every index request.
- `HttpSource` (`<source>` from `AUTH_PROVIDERS`): `POST <url>` with `Authorization: Bearer <shared secret from vars>` and the body `{ "project", "type", "secret" }`; the provider answers `200` with a record in the same shape as in KV (`kind`, `grants`, optionally `expires`, `cacheSeconds`) or `403`. Positive answers are cached in KV under the hash of the whole token (TTL from `cacheSeconds`, default 60 s, max 300 s), negative ones for 10 s. Provider unavailable → `503` for tokens from that source, never a fallback to no auth. Projects from providers are not editable in the panel; logging in with them works normally.

`admin` credentials are always local in KV. `whoami` and `publishedBy` in the manifest record `[<source>.]<project>`.

**Version 1** implements the parser for the full format, `KvSource` and the `key` and `gh` types (for both roles); `HttpSource` is reserved and described here so that the format does not change when it is added.

**Record in KV** (`cred:<project>`):

```json
{
  "project": "shop-frontend",
  "kind": "read",
  "auth": [
    { "type": "key", "hash": "sha256:…", "created": "2026-10-28", "expires": null },
    { "type": "key", "hash": "sha256:…", "created": "2026-06-01", "expires": "2026-11-11" },
    { "type": "gh", "repository": "acme/shop-frontend", "ref": "refs/heads/main", "workflow": null }
  ],
  "grants": [
    { "eco": "composer", "package": "acme/*", "versions": "*" },
    { "eco": "npm", "package": "@acme/sdk", "versions": "^2.0" },
    { "eco": "cargo", "package": "acme-core", "versions": ">=1.4, <2" }
  ]
}
```

- `kind`: exactly one role per credential – `read` (consumer: installation, token committed in the consumer's repository), `publish` (a package's CI: only `init`/`commit`, no reading of indexes; token in GHA secrets) or `admin` (panel and administrative API). A single field, not an array: combinations are deliberately impossible, so that a token with write permission never ends up in a consumer's repository.
- `admin` has no grants (`grants` are ignored) and cannot install or publish anything – it is used only by a human in the browser (panel) or directly through the API.
- `grants.package`: an exact name or a `*` glob at the end of a segment; `grants.versions`: a semver constraint in the syntax of the given ecosystem, evaluated in the Worker, `*` = all. No grant for an ecosystem = no access. For `read` a grant says "what you may install", for `publish` "what you may release".
- `auth`: a list of credentials of different types; it enables rotation without changing the identity or the grants (the admin adds a new entry, sets `expires` on the old one, the cron removes expired ones) and combining types: a consumer credential usually has a `key` (committed, for humans) and optionally a `gh` (for the consumer's CI, with no secret in the repository); a `publish` credential may have only `gh`. The JWT exists only inside a GitHub Actions job, so `gh` never replaces `key` for local work.
- KV is eventually consistent (up to ~60 s for propagation); for token revocation this is acceptable.

**Panel session**: after login, the Worker issues an `HttpOnly; Secure; SameSite=Lax` cookie pointing to `session:<id>` in KV with a TTL of 12 h; the session stores only the project name.

## Data layout

R2 holds archives and manifests as the source of truth, and indexes as a cache; KV holds only credentials, sessions and the schema version.

**Keys in R2**

| Key                                        | Contents                                                                        |
| ------------------------------------------ | ------------------------------------------------------------------------------- |
| `<eco>/pkg/<name>/<version>/manifest.json` | normalised version manifest (below)                                             |
| `<eco>/pkg/<name>/<version>/<file>`        | version files: `.zip` (composer), `.tgz` (npm), `.crate` (cargo), any (generic) |
| `<eco>/index/…`                            | indexes generated from manifests, in the layout required by the ecosystem       |
| `_staging/<upload-id>/<file>`              | file uploaded via presign, not yet committed                                    |
| `_meta/packages.json`                      | list of all packages per ecosystem (for the home page and the panel)            |

The package name in the key is encoded safely: `/` and `@` in composer/npm names are replaced (`acme/sdk` → `acme__sdk`, `@acme/sdk` → `@acme__sdk`), so that listing by prefix is unambiguous. Generic names are encoded the same way (`acme/plugin-foo` → `acme__plugin-foo`). Composer names cannot collide under this encoding, because Composer forbids consecutive separators; npm names could collide only through a scope that itself contains `__`, because decoding reverses only the first `__` (`@a__b/c` and `@a/b__c` both become `@a__b__c`).

**Version manifest** (common format, the adapter fills in `native`):

```json
{
  "eco": "npm",
  "name": "@acme/sdk",
  "version": "2.3.1",
  "files": [
    { "name": "sdk-2.3.1.tgz", "size": 48213, "sha256": "…", "role": "archive", "labels": {} }
  ],
  "published": "2026-10-28T10:12:00Z",
  "publishedBy": "ci-acme-sdk",
  "yanked": false,
  "native": { "...ecosystem-specific fields, e.g. deps, dist-tags, autoload..." }
}
```

`files` is a list: `role` identifies a file within the ecosystem. The kernel requires only a non-empty list with unique names; the adapter decides about roles: composer, npm and cargo require exactly one `archive` file (and may allow additional ones, e.g. `attestation` for npm provenance), generic does not use `role` but filters by arbitrary `labels` instead. Both fields are optional in the kernel.

`native` is literally what the adapter needs to reproduce the entry in the index: for composer the whole `composer.json` of the version, for npm the `versions[v]` entry of the packument, for cargo the index line (`deps`, `features`, `cksum`).

**Keys in KV**

| Key              | Contents                 | TTL  |
| ---------------- | ------------------------ | ---- |
| `cred:<project>` | credential record        | none |
| `session:<id>`   | `{ project, createdAt }` | 12 h |
| `schema_version` | integer                  | none |

**Concurrency.** Writing an index is `r2.put(key, body, { onlyIf: { etagMatches } })` with the etag from the read; on conflict the Worker reads again and retries up to 5 times. Manifests are added under a unique key (immutability), so they do not conflict. We do not use Durable Objects. If things drift apart, the admin has a "rebuild package index" action that generates it from the listing of manifests.

## Publishing

Publishing is three steps: `init` (validation and presign), `PUT` directly to R2, `commit` (object verification, writing the manifest, rebuilding the index). A version, once committed, is immutable.

_Publishing flow diagram (6 steps):_ 1) the CLI calls `init` on the Worker with the declared metadata and list of files; 2) the Worker validates the grant and immutability and returns one presigned `PUT` URL per file; 3) the CLI uploads every file directly to R2 staging with those URLs and then calls `commit`; 4) the Worker runs `head()` on every staged object and verifies its size and checksum; 5) the Worker copies the files to their final keys, writes `manifest.json` and rebuilds the package index; 6) the Worker deletes the staging objects (cleanup).

The Worker sees only metadata and one `head()` per file; if an object in staging does not match the declaration from `init`, commit rejects it and nothing goes into the index.

**`POST /api/v1/publish/init`** (kind `publish`)

```json
{
  "eco": "npm",
  "name": "@acme/sdk",
  "version": "2.3.1",
  "files": [{ "name": "sdk-2.3.1.tgz", "size": 48213, "sha256": "…", "role": "archive" }],
  "manifest": { "...native fields from the archive..." }
}
```

The Worker: checks the `publish` grant for the package and version, rejects with `409` if the `manifest.json` of this version exists, writes `_staging/<upload-id>/meta.json` and returns:

```json
{
  "uploadId": "…",
  "expires": "2026-10-28T10:22:00Z",
  "uploads": [
    {
      "name": "sdk-2.3.1.tgz",
      "putUrl": "https://<account>.r2.cloudflarestorage.com/<bucket>/_staging/<upload-id>/sdk-2.3.1.tgz?X-Amz-…",
      "headers": { "content-length": "48213", "x-amz-checksum-sha256": "<base64>" }
    }
  ]
}
```

The presign (SigV4, `PUT`, 10 min) covers `content-length` and `x-amz-checksum-sha256`, so it is impossible to upload to R2 any file other than the declared one.

**`PUT <putUrl>`** – the CLI sends every file from the `uploads` list with exactly these headers. Network error → the CLI retries the PUT; after the URL expires → it starts again from `init`.

**`POST /api/v1/publish/commit`** `{ "uploadId": "…" }`

The Worker: runs `head()` on every object in staging, compares the size and `checksums.sha256` with `meta.json` (any file missing → `400`), checks immutability again (someone may have committed in the meantime), copies the objects to the target keys (`r2.put` with the stream from `get()`), writes `manifest.json`, runs `adapter.rebuildIndex(name)`, deletes the staging objects. Response `201` with `{ name, version, url }`; `409` when the version already exists.

**Immutability and admin operations**

- The CLI has no way to overwrite or delete a version; `init` on an existing version ends with `409`.
- The admin (`kind admin`, the panel or `DELETE /api/v1/packages/<eco>/<name>/<version>`) has two operations: `yank` (the manifest gets `yanked: true`; the version disappears from dependency resolution, but the file stays downloadable, so consumers' lockfiles keep working – table below) and `delete` (manifest and files deleted, index rebuilt, consumers' lockfiles stop working, the version is free to be published again).
- Deleting a whole package (`DELETE /api/v1/packages/<eco>/<name>`, from the panel after typing its name) = `delete` of all versions plus removing the entry from `_meta/packages.json`.

| Ecosystem | What a yank looks like in the index                                                                                                    | Download from a lockfile                                                                         |
| --------- | -------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| cargo     | native: the entry stays in the sparse index with `"yanked": true`; `cargo update` skips it                                             | yes, natively (`Cargo.lock`)                                                                     |
| npm       | no native concept; here the entry is removed from `versions` and `dist-tags` in the packument, the tarball is still at `/-/<file>.tgz` | yes – `package-lock.json` has the tarball URL and `integrity`, it does not ask for the packument |
| Composer  | no native concept; here the entry is removed from `p2/<pkg>.json`, the dist is still at `/dist/…`                                      | yes – `composer.lock` has the dist URL and shasum                                                |
| generic   | `"yanked": true` in the version JSON, skipped in the list and in `latest`, visible with `?include=yanked`                              | yes, directly by URL                                                                             |
| NuGet     | native: `listed: false` in the registration; the flat container still returns the version and the `.nupkg`                             | yes                                                                                              |

In npm and Composer a yanked version is invisible, not marked – the client gets no warning, it simply stops seeing it; this is deliberately closer to the cargo semantics than to npm's `deprecated`, which only warns and still lets the version be chosen.

**Appending files to an existing version** (generic only): building `cdylib` libraries or installers in a CI matrix produces one file per job, while there is only one version. That is why `init` with `"append": true` on an existing version is allowed when (a) the adapter permits it, (b) less than `APPEND_WINDOW_HOURS` (default 24) have passed since `published`, and (c) every file name is new. A file, once committed, is immutable; `commit` appends the entries to `files[]` in the manifest (a write with `onlyIf`) and rebuilds the index. Once the window has passed, the version is closed. An alternative without a window: a job that collects the artifacts and publishes once.

**Cleanup** – a cron every hour: deletes `_staging/*` entries older than 24 h and secrets whose `expires` date has passed.

**Limits** – a single presigned PUT up to 5 GB (the R2 limit for one object without multipart); `init` rejects a `size` above `MAX_UPLOAD_BYTES` (default 512 MB).

## Adapters and endpoints

Each ecosystem is one module implementing a common interface; the kernel does not know the formats, the adapter does not know R2 or auth. The Worker serves only reads of the native protocols plus its own publishing and administration API.

**Modularity.** Adapters are modules in one repository (not plugins loaded at runtime): in the Worker, `src/adapters/<eco>/` directories registered in `src/adapters/index.ts`; in the CLI, `crates/eco-<eco>` crates linked into one binary. Adding an ecosystem = a PR to both repositories. Selective enabling: in the Worker, `vars.ECOSYSTEMS` (a list, default `composer,npm,cargo,generic,nuget`) – a disabled adapter is not routed and does not appear on the page or in the panel; in the CLI, Cargo feature flags (`--features composer,npm`, all by default), so that `sreg` can be built with only the adapters that are needed.

**Adapter contract** (`src/adapters/<eco>/index.ts`):

```ts
interface Adapter {
  eco: "composer" | "npm" | "cargo" | "generic" | "nuget";
  // validation and normalisation of what the CLI sent in init
  parseManifest(input: PublishInit): Manifest;
  // how to name the keys of the version's files in R2
  fileKey(m: Manifest, file: ManifestFile): string;
  // rebuild the index of one package from the list of manifests (including yanked)
  buildIndex(name: string, manifests: Manifest[]): IndexFile[]; // { key, body, contentType }
  // read routing: returns an index file to filter or an archive key to redirect to
  route(req: Request, url: URL): IndexRoute | ArchiveRoute | null;
  // index filter for a credential: only versions covered by grants
  filterIndex(body: unknown, allowed: (name: string, version: string) => boolean): unknown;
  // configuration snippets for the home page
  clientConfig(baseUrl: string, token: string, packages: string[]): Snippet[];
  // version format, validation and comparison (semver / Composer / cargo / NuGet)
  validateVersion(version: string): boolean;
  satisfies(version: string, constraint: string): boolean;
}
```

**Composer** (`/composer/…`)

| Endpoint                                                          | Returns                                                                                                         |
| ----------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `GET /composer/packages.json`                                     | `{ "metadata-url": "/composer/p2/%package%.json", "available-packages": [...] }` – list filtered by grants      |
| `GET /composer/p2/<vendor>/<pkg>.json`                            | stable versions in the Composer 2 format (`minified: composer/2.0`), only allowed ones                          |
| `GET /composer/p2/<vendor>/<pkg>~dev.json` (always an empty list) | Composer 2 asks for this file; the registry does not serve `dev-*` versions, so it returns `{ "packages": {} }` |
| `GET /composer/dist/<vendor>/<pkg>/<version>.zip`                 | `302` → presigned GET (15 min)                                                                                  |

Client: `auth.json` with `bearer` or `http-basic` and `repositories: [{ type: composer, url: https://<domain>/composer }]`, or without `auth.json`, with the token in the URL: `https://<token>@<domain>/composer` (Composer extracts the userinfo as `http-basic`). npm and cargo do not support credentials in the URL.

**npm** (`/npm/…`)

| Endpoint                                      | Returns                                                                                                                            |
| --------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `GET /npm/<name>` or `/npm/@<scope>%2f<name>` | packument: `versions`, `dist-tags` (`latest` = the highest non-yanked stable version), `time`; `dist.tarball` points to the Worker |
| `GET /npm/<name>/<version>`                   | a single version entry                                                                                                             |
| `GET /npm/<name>/-/<file>.tgz`                | `302` → presigned GET                                                                                                              |
| `GET /npm/-/ping`, `GET /npm/-/whoami`        | `200` for checking auth; `whoami` returns `{ username: <project> }`                                                                |

Client: `.npmrc` with `@scope:registry=https://<domain>/npm/` and `//<domain>/npm/:_authToken=<token>`. Scoped packages are required (pointing the whole `registry` at the private registry would cut off public packages, because we do not mirror).

**cargo** (`/cargo/…`, sparse registry)

| Endpoint                                                                 | Returns                                                                                                                   |
| ------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------- |
| `GET /cargo/index/config.json`                                           | `{ "dl": "https://<domain>/cargo/dl/{crate}/{version}", "api": "https://<domain>/cargo", "auth-required": true }`         |
| `GET /cargo/index/1/<c>`, `/2/<cc>`, `/3/<c>/<ccc>`, `/<cc>/<cc>/<name>` | index file: one JSON line per version (`name`, `vers`, `deps`, `cksum`, `features`, `yanked`, `v: 2`), filtered by grants |
| `GET /cargo/dl/<crate>/<version>`                                        | `302` → presigned GET                                                                                                     |
| `GET /cargo/api/v1/crates/<name>`                                        | optional metadata for the panel, not required by cargo                                                                    |

Client: `.cargo/config.toml` with `[registries.<name>] index = "sparse+https://<domain>/cargo/index/"` and `token = "<token>"` (cargo sends it verbatim as `Authorization`). `auth-required: true` enforces the token also when fetching the index and files.

**NuGet** (`/nuget/…`, V3 protocol)

| Endpoint                                       | Returns                                                                                                                        |
| ---------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `GET /nuget/index.json`                        | service index with the resources `PackageBaseAddress/3.0.0`, `RegistrationsBaseUrl/3.6.0`, `SearchQueryService/3.5.0`          |
| `GET /nuget/flat/<id>/index.json`              | `{ "versions": [...] }` – the versions covered by a grant, lower case, normalised                                              |
| `GET /nuget/flat/<id>/<ver>/<id>.<ver>.nupkg`  | `302` → presigned GET                                                                                                          |
| `GET /nuget/flat/<id>/<ver>/<id>.nuspec`       | the `.nuspec` from the manifest (`native`)                                                                                     |
| `GET /nuget/registration/<id>/index.json`      | pages with a `catalogEntry` per version: `id`, `version`, `dependencyGroups` (by target framework), `listed`, `packageContent` |
| `GET /nuget/search?q=&skip=&take=&prerelease=` | a simple search over `_meta/packages.json`, filtered by grants (needed for `dotnet package search`)                            |

Normalisation: identifiers and versions in paths are lower case; NuGet versions allow 4 parts (`1.0.0.0`) and have their own comparison rules, so the adapter provides its own `satisfies` and version validation. Yank = `listed: false`. An optional second file, a `.snupkg`, has the role `symbols`.

Client: `dotnet nuget add source https://<domain>/nuget/index.json -n <name> -u <login> -p <password> --store-password-in-clear-text` (`packageSourceCredentials` in `nuget.config` → Basic; .NET drops `Authorization` on a redirect to another host, so the `302` to R2 works).

**generic** (`/generic/…`, own JSON protocol)

An ecosystem for files without a native package manager: DLL plugins loaded by an application, `cdylib` libraries built for many architectures, `msi`/`pkg` installers as an auto-updater feed. One version = any number of files, each with arbitrary labels; the version metadata (`meta`) is arbitrary JSON from the publisher.

- Package name: `[a-z0-9][a-z0-9._-]*`, optionally with one group segment (`acme/plugin-foo`) (editorial note: a name must not contain `__`, two consecutive underscores, because `/` is encoded as `__` in R2 keys and `acme__x` would collide with `acme/x`). Version: semver, immutable as everywhere.
- Files: `name` (unique within the version), `size`, `sha256` and optional `labels` – a string→string map with arbitrary keys, which the registry does not interpret but only filters by. The convention shown in the README (not a schema): `platform` = Rust target triple (`x86_64-pc-windows-msvc`, `aarch64-apple-darwin`), `kind` = `installer` / `library` / `symbols`, `profile` = `release` / `debug`.
- `meta`: e.g. `{ "minAppVersion": "3.2", "notes": "…", "releaseNotesUrl": "…" }` – the registry does not interpret it, it only passes it on.

| Endpoint                               | Returns                                                                                                                                                                                                                                                                            |
| -------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET /generic/`                        | list of the packages available to the credential, with the latest version of each                                                                                                                                                                                                  |
| `GET /generic/<name>`                  | `{ name, latest, versions: { "<v>": { published, yanked, meta, files: [ { name, size, sha256, labels, url } ] } } }` – only versions covered by a grant; `?label=<key>:<value>` (repeatable, all conditions must be met) filters the files, `?include=yanked` adds yanked versions |
| `GET /generic/<name>/latest`           | the highest non-yanked stable version; `?channel=prerelease` admits prereleases; `?label=` as above                                                                                                                                                                                |
| `GET /generic/<name>/<version>`        | a single version                                                                                                                                                                                                                                                                   |
| `GET /generic/<name>/<version>/<file>` | `302` → presigned GET                                                                                                                                                                                                                                                              |

JSON responses have an `ETag`; `If-None-Match` → `304`, so that an auto-updater can poll often at no cost. `url` in the files is the address of the `302` endpoint (not a presigned URL), so the response can be cached on the client side. Versions are compared as semver; `latest` is computed on the fly from the manifests.

Client: any HTTP client with the `Authorization: Bearer <token>` header (or Basic); in a pipeline, `sreg generic get`. The instructions page shows a `curl` and an `sreg` example.

**Own API** (`/api/v1/…`, JSON, Bearer)

| Endpoint                                                                         | Kind      | Description                                                       |
| -------------------------------------------------------------------------------- | --------- | ----------------------------------------------------------------- |
| `POST /api/v1/publish/init`, `POST /api/v1/publish/commit`                       | `publish` | see Publishing                                                    |
| `GET /api/v1/packages`                                                           | `read`    | packages and versions available to the credential (home page)     |
| `GET /api/v1/packages/<eco>/<name>`                                              | `read`    | versions of one package                                           |
| `POST /api/v1/packages/<eco>/<name>/<version>/yank`, `DELETE …/<version>`        | `admin`   | yank, delete                                                      |
| `POST /api/v1/packages/<eco>/<name>/reindex`                                     | `admin`   | rebuild the index from manifests                                  |
| `GET/POST /api/v1/credentials`, `GET/PATCH/DELETE /api/v1/credentials/<project>` | `admin`   | list, create, edit grants, delete                                 |
| `POST /api/v1/credentials/<project>/secrets`, `DELETE …/secrets/<hash-prefix>`   | `admin`   | add a new secret (returns the full token once), expire an old one |
| `GET /api/v1/whoami`                                                             | any       | project name, kind, grants                                        |
| `GET /api/v1/version`                                                            | none      | Worker version, `minClientVersion`, `schema_version`              |

All native-protocol responses have `Cache-Control: private, no-store` – indexes depend on the credential.

## Home page and panel

One HTML + CSS application with minimal vanilla JS (only `fetch` to `/api/v1`), served from Workers Static Assets, without a bundler or external dependencies. The user's role follows from the kind field of the credential they logged in with.

**Login** (`GET /`): a form with a single "token" field, or two fields "login / password" (a toggle). `POST /login` verifies like any other request, creates a session in KV and sets the cookie. `POST /logout` deletes the session.

**Consumer view** (kind `read`):

- The list of packages available to this credential, grouped by ecosystem, each with the versions covered by the grant (yanked ones marked).
- Under each ecosystem, ready-to-paste snippets with the domain, token and package names filled in: `composer.json` + `auth.json` (bearer and http-basic variants) and a variant without auth.json, with the token as userinfo in the repository URL (https://\<token>@\<domain>/composer – the token is already a login:password pair), `.npmrc`, `.cargo/config.toml`. The snippets are generated by `adapter.clientConfig`.
- A "copy" button next to each snippet; nothing more.

**Admin view** (kind `admin`), additional tabs:

- Packages: all of them, with versions; yank, delete and reindex actions (confirmation through a browser dialog).
- Credentials: list of projects with the number of active secrets and the nearest expiry date; a creation form (name, kind as a read/publish/admin radio, grants as eco/package/versions rows – hidden for admin); editing grants; "add secret" shows the new token once; "expire" sets `expires` on the old one; deleting a project.
- Settings: read-only – Worker version, flags from `vars`, a link to the latest release on GitHub (if `CHECK_UPDATES=true`).

**First run**: no credential at all in KV → the page shows a "create admin" form (name + generated secret, shown once). Once the first credential of kind `admin` is created, the form disappears permanently. Without this, the user would have to type hashes into KV by hand.

**Panel security**: all mutations via `POST`/`PATCH`/`DELETE` with the `X-Requested-With` header (CSRF protection with a `SameSite=Lax` cookie), `Content-Security-Policy: default-src 'self'`, no inline scripts.

## Rust CLI

The CLI is the only way to publish: for npm and cargo it delegates packing to the native tools, for Composer it packs by itself. Binary name: `sreg`.

**Commands**

| Command                                                                                                 | What it does                                                                                                                                                                                                                                                                                                                                                                                     |
| ------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `sreg composer publish [--version <v>] [--path .]`                                                      | packs the directory (`git archive HEAD` honouring `export-ignore`, falling back to a zip of the directory without `vendor/` and `.git/`), reads `composer.json`; the version comes from `--version` or the git tag (`v1.2.3` → `1.2.3`); it must be semver, `dev-*` is rejected                                                                                                                  |
| `sreg npm publish [--version <v>] [--tag <t>] [--path .]`                                               | with `--version`, first runs `npm pkg set version=<v>` (the version in `package.json` inside the tarball must match the registry), then `npm pack --json --pack-destination <tmp>`, reads the name, version and `package.json` from the tarball; `--tag` sets the dist-tag for this version (default `latest`, prerelease → `next`)                                                              |
| `sreg cargo publish [--version <v>] [--path .]`                                                         | with `--version`, replaces `package.version` in `Cargo.toml` (a formatting-preserving edit, `toml_edit`), then runs `cargo package --no-verify --allow-dirty` (flags can be passed through), reads the `.crate` and the metadata from `cargo metadata`                                                                                                                                           |
| `sreg whoami`                                                                                           | `GET /api/v1/whoami`                                                                                                                                                                                                                                                                                                                                                                             |
| `sreg generic publish <name> <version> <file>... [--label k=v]... [--meta k=v]... \| --spec files.json` | publishes the given files as one version; `--label` applies to all files in the call, files with different labels = separate calls (in a CI matrix every job sends its own file of the same version, see Publishing: appending files); `--spec` accepts JSON with a list of `{ path, labels }` for unusual cases; the CLI adds no default labels                                                 |
| `sreg generic get <name> [<version>\|latest] [--label k=v]... [--out <dir>] [--prerelease]`             | downloads the files of a version (`latest` by default, all files without `--label`), verifies the sha256, prints the paths; `--json` returns the manifest without downloading                                                                                                                                                                                                                    |
| `sreg nuget publish [--version <v>] [--path .]`                                                         | runs `dotnet pack` (version through `-p:PackageVersion=<v>`), unpacks the `.nupkg` (zip), reads the `.nuspec` (id, version, dependencies), attaches the `.snupkg` as `symbols` if one was produced                                                                                                                                                                                               |
| `sreg auth <eco> --oidc [--project <p>] [--write <file>]`                                               | in a GHA job (permissions: id-token: write) fetches a JWT from ACTIONS\_ID\_TOKEN\_REQUEST\_URL with audience = PUBLIC\_URL, assembles \<project>:gh.\<jwt> and writes it to auth.json / .npmrc / credentials.toml (or prints it for export as COMPOSER\_AUTH / a variable); without --oidc it does the same with REGISTRY\_TOKEN. The publish commands use the JWT directly when --oidc is set. |

The common `--version` of all publish commands: overrides the version from the project manifest and, where the client verifies consistency (npm, cargo), also replaces it in the manifest file before packing; the CLI prints that the file was modified. The CLI has no administrative commands (tokens, yank, delete) – the panel does that; they can be added later on top of the `/api/v1` API.

Every publish command: computes the sha256 and size of every file, calls `init`, does the `PUT` of every file with retry (3 attempts, backoff), calls `commit`, prints the result. `--dry-run` stops after `init`. Exit code `2` on `409` (version exists), so that the pipeline can tell it apart from an error.

**Configuration**: `--registry <url>` or `REGISTRY_URL`, `--token` or `REGISTRY_TOKEN`; in CI exclusively environment variables from GHA secrets. No configuration file.

**Workspace structure** (`serverless-registry-cli`):

- `crates/core` – HTTP client (reqwest + rustls), presigned upload, API models, version handling.
- `crates/eco-composer`, `crates/eco-npm`, `crates/eco-cargo`, `crates/eco-generic`, `crates/eco-nuget` – packing and manifest extraction; a common trait `Packager { fn package(&self, opts) -> Result<Package> }`.
- `crates/cli` – clap, subcommands, text and `--json` output.

**CLI ↔ Worker compatibility**: the CLI sends `X-Client-Version: <semver>`, the Worker answers with `X-Min-Client-Version`; CLI too old → a message with a link to the release and exit code `3`. The API is versioned in the path (`/api/v1`); the next Worker version keeps the previous one for at least one minor release.

**Distribution**: GitHub Releases through `cargo-dist` (linux-gnu/musl, macOS arm64/x86\_64, Windows; checksums), plus a GitHub Action `<org>/publish-action@v1` (composite: downloads the binary at a pinned version, verifies the checksum, runs `sreg <eco> publish`). Example in the README:

```yaml
- uses: <org>/publish-action@v1
  with:
    ecosystem: composer
    registry: https://packages.example.com
    token: ${{ secrets.REGISTRY_PUBLISH_TOKEN }}
```

## Deployment, configuration and updates

Principle: there is nothing installation-specific in the repository. Thanks to that, updating is "Sync fork" and an automatic deploy, without conflicts.

**Deployment paths** (all supported by the same repository):

| Path                                                                        | For whom                             | What the user does                                                                                                                                      |
| --------------------------------------------------------------------------- | ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| "Deploy to Cloudflare" button (`deploy.workers.cloudflare.com/?url=<repo>`) | default                              | clicks; Cloudflare copies the repository to their GitHub, provisions R2 and KV from `wrangler.jsonc`, hooks up Workers Builds (auto-deploy from `main`) |
| Fork + GitHub Actions (`wrangler deploy`)                                   | those who do not want Workers Builds | fork, `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` secrets, run the workflow                                                                      |
| Locally `npm run deploy`                                                    | developers                           | clone, `wrangler login`, deploy                                                                                                                         |

**`wrangler.jsonc` in the repository** – bindings without identifiers, so that Wrangler provisions them or matches them by name:

```jsonc
{
  "name": "serverless-registry",
  "main": "src/index.ts",
  "compatibility_date": "2026-10-01",
  "assets": { "directory": "./public", "binding": "ASSETS" },
  "kv_namespaces": [{ "binding": "AUTH" }],
  "r2_buckets": [{ "binding": "R2_BUCKET", "bucket_name": "serverless-registry" }],
  "triggers": { "crons": ["0 * * * *"] },
  "vars": {
    "ECOSYSTEMS": "composer,npm,cargo,generic,nuget",
    "MAX_UPLOAD_BYTES": "536870912",
    "CHECK_UPDATES": "true",
    "TOKEN_PREFIX": "",
    "R2_BUCKET_NAME": "serverless-registry",
    "APPEND_WINDOW_HOURS": "24",
  },
}
```

**Installation configuration** – only in the Cloudflare dashboard (or `wrangler secret put`), never in files:

| Key                                                                                      | Kind   | Description                                                                                      |
| ---------------------------------------------------------------------------------------- | ------ | ------------------------------------------------------------------------------------------------ |
| `PUBLIC_URL`                                                                             | var    | full address of the registry, e.g. `https://packages.example.com` (used in indexes and snippets) |
| `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY`                                               | secret | R2 S3 API key for presigning (permissions for this bucket only)                                  |
| `S3_ENDPOINT`                                                                            | var    | `https://<account>.r2.cloudflarestorage.com`                                                     |
| `ECOSYSTEMS`, `MAX_UPLOAD_BYTES`, `APPEND_WINDOW_HOURS`, `TOKEN_PREFIX`, `CHECK_UPDATES` | var    | override the defaults from the repository                                                        |
| `R2_BUCKET_NAME`                                                                         | var    | name of the bucket used in presigned URLs; must match `bucket_name` of the `R2_BUCKET` binding   |

Domain: a custom domain of the Worker set in the dashboard, not through `routes` in the file. The README walks through this step by step with screenshots.

**Updates**

- Releases tagged with semver, `CHANGELOG.md` with "breaking" and "requires migration" sections.
- The user: "Sync fork" (or merge a tag) → Workers Builds / GHA deploys. Zero file edits on their side, so zero conflicts.
- The panel shows the version and, with `CHECK_UPDATES=true`, compares it with the latest release on GitHub (one `fetch` per day, cached in KV).
- Data migrations: `schema_version` in KV; on the first request after a new version is deployed, the Worker compares it and runs the migrations in order (idempotent, small), and for large operations (e.g. a change of index format) a `reindex` of all packages from manifests is enough – the manifest is the only durable data contract.
- The CLI and the Worker are updated independently; the version headers (CLI section) protect against silent incompatibility.

**Licence and repositories**: MIT; `serverless-registry-worker` and `serverless-registry-cli` as two repositories with separate release cycles.

## Repository structure and decisions

**`serverless-registry-worker`**

```text
src/
  index.ts              # fetch + scheduled, routing by path prefix
  core/
    auth.ts             # header parsing, cred: lookup, hash comparison, sessions
    grants.ts           # grant evaluation (glob + constraint via adapter.satisfies)
    presign.ts          # SigV4 for R2 (PUT with checksum, GET)
    storage.ts          # R2 keys, manifests, onlyIf retry, listing
    publish.ts          # init / commit
    admin.ts            # yank, delete, reindex, credentials
    cron.ts             # cleanup of staging and expired secrets
    migrate.ts          # schema_version and migrations
  adapters/
    types.ts            # interface Adapter, Manifest, IndexFile
    composer/  npm/  cargo/  generic/  nuget/
  pages/                # HTML rendering of the home page and the panel (template strings)
public/                 # CSS, minimal JS (Static Assets)
test/                   # vitest + @cloudflare/vitest-pool-workers, adapter tests on real fixtures
wrangler.jsonc  CHANGELOG.md  README.md
```

**`serverless-registry-cli`** – the workspace from the CLI section (`crates/core`, `crates/eco-*`, `crates/cli`), `dist-workspace.toml` for cargo-dist, an `action/` directory with the composite GitHub Action.

**Rollout order**

1. Kernel + composer adapter + CLI `composer publish` and auth, with the key and gh types (closes the existing use case and publishing without secrets).
2. Home page with login and snippets, admin panel, first run.
3. npm adapter + `npm publish`.
4. cargo adapter + `cargo publish`; generic adapter + generic publish/get; nuget adapter + nuget publish.
5. Deploy button, cargo-dist, publish-action, documentation.

**Settled decisions**

- CLI name: `sreg`.
- npm `dist-tags`: the CLI accepts `--tag` (default `latest`); a prerelease version without `--tag` gets `next`. Tags are kept in the package's `_meta`; yanking/deleting the version a tag points to moves the tag to the highest remaining stable version.
- No support for `dev-*` in Composer: the version format is defined by the adapter (semver for composer/npm/cargo/generic, NuGet versions for nuget), prereleases allowed, all versions immutable. `sreg composer publish` rejects a non-semver version. Testing a branch before a release = a VCS repository temporarily listed in the consumer's `composer.json`.
- Presigned GET valid for 15 min.
- The panel allows deleting a whole package; confirmation by typing its name.

## Changes

| Version | Date       | Change                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ------- | ---------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 3       | 2026-10-28 | Fifth ecosystem, NuGet (V3 protocol: service index, flat container, registration, search; Basic auth from `nuget.config`; `.snupkg` as `symbols`; `sreg nuget publish`). The version format is defined by the adapter (`validateVersion`), not by the kernel – NuGet allows 4 parts. A table of yank semantics per ecosystem in the Publishing section.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| 2       | 2026-10-28 | A package version is a list of files (`files[]`), not a single file: the manifest, `init`/`commit`, the adapter contract (`fileKey`) and the CLI accept multiple files; the "exactly one `archive`" rule belongs to the composer/npm/cargo adapters. New **generic** ecosystem: arbitrary files with an open `labels` map (`?label=k:v` filter) and `meta`, a JSON feed (list, `latest`, version, `302` to the file, `ETag`), the commands `sreg generic publish` (`--label k=v` per call, optional `--spec`) and `sreg generic get`, appending files to a version within the `APPEND_WINDOW_HOURS` window. Modularity clarified: adapters as modules in the repository, enabled through `vars.ECOSYSTEMS` and Cargo feature flags. CLI: `--version` also in `npm`/`cargo publish` (replacing the version in the manifest file), administrative commands removed – administration only through the panel and the API. |
| 1       | 2026-10-28 | First version of the specification.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
