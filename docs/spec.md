# Serverless Registry – specification

## Goals and assumptions

We are building a private, serverless package repository for Composer, npm, cargo, NuGet, Swift, Maven (Java/Kotlin/Android) and arbitrary files (generic), hosted entirely on Cloudflare (Workers, R2, KV), with publishing done through our own CLI written in Rust. It is the successor of the current Serverless Satis solution: a new deployment, a new address, no backwards compatibility.

**What we build**

- A Worker serving the native read protocols of six ecosystems plus a simple JSON feed for generic files, from a single R2 bucket.
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

One Worker with a shared kernel and seven adapters, one R2 bucket as the source of truth, one KV namespace for credentials. Indexes are derived from manifests, not separate state.

_Architecture diagram (5 components):_ the package manager clients (Composer, npm, cargo, NuGet, Swift, Maven/Gradle), HTTP clients of the generic feed and the `sreg` CLI talk only to the Worker for metadata; the Worker reads and writes the R2 bucket (package files, manifests, indexes) and the `AUTH` KV namespace (credentials, the `gh-repo` index, schema version); file bytes flow directly between the clients/CLI and R2 through presigned URLs; the hourly cron (`scheduled` handler of the same Worker) cleans up R2 staging and expired secrets in KV.

The Worker never passes archive bytes through itself: the CLI uploads them with a presigned PUT, and clients get a `302` to a presigned GET; only indexes, manifests and the page go through the Worker.

| Component          | Role                                                                      | Technology                                      |
| ------------------ | ------------------------------------------------------------------------- | ----------------------------------------------- |
| Worker             | Auth, publishing API, serving indexes, redirects to files, page and panel | TypeScript, Workers Static Assets for HTML/CSS  |
| R2                 | Package files, version manifests, generated indexes                       | R2 binding + S3 API key (secret) for presigning |
| KV                 | Credentials, the `gh-repo` index, `schema_version`                        | `AUTH` binding                                  |
| CLI                | Packing and publishing versions from CI                                   | Rust, binaries in GitHub Releases               |
| Cron (`scheduled`) | Cleaning up uncommitted uploads and expired secrets                       | handler in the same Worker                      |

**Kernel** (`src/core/`): parsing and verifying credentials, evaluating grants, SigV4 presigning, writing manifests and rebuilding indexes with optimistic concurrency, routing to adapters, sessions, cron.

**Adapter** (`src/adapters/<eco>/`): knows how to extract the name, version and dependencies from an archive and manifest, how to build its ecosystem's index from a list of manifests, and how to filter it for a given credential. It does not touch auth or R2 directly.

**The index as a derivative.** Every version is, in R2, a `manifest.json` (normalised metadata) plus its files (usually one: the archive). Indexes (`p2/*.json`, the npm packument, cargo sparse index files) are generated from the manifests at publish time and cached in R2; an admin can rebuild them from the listing. Per-credential filtering (explicit grants plus the dependency closure, see Credentials) happens on the fly when serving, so a consumer with a limited grant never sees versions it has no access to.

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

**Login `_` (identity from the JWT)**: the reserved login `_` (not allowed by the slug regex) means "the project follows from the credential" and is allowed only with the `gh` type: `_:gh.<jwt>`. The Worker verifies the JWT as usual and then, using the `gh-repo:<owner/repo>` index in KV (a list of projects, maintained on every credential write in the panel/API), picks the credential whose `gh` entry matches the `repository`/`ref`/`workflow` claims. Ambiguity is resolved by the kind of request: the publishing endpoints pick `kind: publish`, reads pick `kind: read`, so one repository can be a consumer and a publisher at the same time. When more than one project matches for the given kind → `401` with a message saying that the project must be given. `whoami` and `publishedBy` record the resolved name, never `_`.

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

- `KvSource` (no source prefix): reads `cred:<project>` and verifies the secret with the verifier for its type: `key` → SHA-256 compared in constant time with every active entry; `gh` → the JWT signature checked against the keys from `https://token.actions.githubusercontent.com/.well-known/jwks` (cached for 24 h), `iss`, `aud` = `PUBLIC_URL`, `exp`, and the `repository`/`ref`/`workflow` claims against the entry in the record; the JWKS is cached in the Cache API (24 h), and the JWT itself is verified on every request – an ES256/RS256 signature costs a fraction of a millisecond, while a cache in KV would eat up the write limit.
- `HttpSource` (`<source>` from `AUTH_PROVIDERS`): `POST <url>` with `Authorization: Bearer <shared secret from vars>` and the body `{ "project", "type", "secret" }`; the provider answers `200` with a record in the same shape as in KV (`kind`, `grants`, optionally `expires`, `cacheSeconds`) or `403`. Positive answers are cached in the Cache API under the hash of the whole token (TTL from `cacheSeconds`, default 60 s, max 300 s), negative ones for 10 s. Provider unavailable → `503` for tokens from that source, never a fallback to no auth. Projects from providers are not editable in the panel; logging in with them works normally.

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
    {
      "type": "gh",
      "repository": "acme/shop-frontend",
      "repository_id": "123456789",
      "ref": "refs/heads/main",
      "workflow": null
    }
  ],
  "grants": [
    { "eco": "composer", "package": "acme/*", "versions": "*" },
    { "eco": "npm", "package": "@acme/sdk", "versions": "^2.0", "transitive": false },
    { "eco": "cargo", "package": "acme-core", "versions": ">=1.4, <2" }
  ]
}
```

A `gh` entry matches the claims of a GitHub Actions JWT; every field except `repository` is optional and means "any" when omitted or `null`:

| Field                 | JWT claim             | Value                                                                                                                                                                                                                                                                                  | Example                           |
| --------------------- | --------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------- |
| `repository`          | `repository`          | required; `owner/repo` or a glob `owner/*`                                                                                                                                                                                                                                             | `acme/shop-frontend`              |
| `repository_id`       | `repository_id`       | numeric id of the repository (a string); recommended, because an owner/repo name can be taken again after the repository is deleted, an id cannot; the panel fills it in automatically from the GitHub API once the name is entered (for public repositories) or it is entered by hand | `"123456789"`                     |
| `repository_owner_id` | `repository_owner_id` | numeric id of the organisation or user; useful with an `owner/*` glob instead of `repository_id`                                                                                                                                                                                       | `"98765"`                         |
| `ref`                 | `ref`                 | glob over the full name of the ref                                                                                                                                                                                                                                                     | `refs/heads/main`, `refs/tags/v*` |
| `workflow`            | `workflow_ref`        | path of the workflow file in the repository (without `owner/repo@ref`)                                                                                                                                                                                                                 | `.github/workflows/release.yml`   |
| `environment`         | `environment`         | name of the GHA environment                                                                                                                                                                                                                                                            | `production`                      |
| `expires`             | –                     | as for `key`: expiry date of the entry                                                                                                                                                                                                                                                 | `2027-01-01`                      |

For `publish`, at least `ref: refs/tags/v*` or `workflow` is recommended, so that only the release workflow can publish, not any PR. The `iss`, `aud`, `exp` and `nbf` fields are checked by the kernel, not by the entry.

- `kind`: exactly one role per credential – `read` (consumer: installation, token committed in the consumer's repository), `publish` (a package's CI: only `init`/`commit`, no reading of indexes; token in GHA secrets) or `admin` (panel and administrative API). A single field, not an array: combinations are deliberately impossible, so that a token with write permission never ends up in a consumer's repository.
- `admin` has no grants (`grants` are ignored) and cannot install or publish anything – it is used only by a human in the browser (panel) or directly through the API.
- `grants.package`: an exact name or a `*` glob at the end of a segment; `grants.versions`: a semver constraint in the syntax of the given ecosystem, evaluated in the Worker, `*` = all. No grant for an ecosystem = no access. For `read` a grant says "what you may install", for `publish` "what you may release".
- `grants.transitive` (`read` only, default `true`): the grant also covers the package's dependencies that are in this registry. The Worker computes the closure from the **constraints** declared in the manifests (`native`): starting from the versions covered by the grant, it takes their dependencies that point to packages of the same ecosystem in the registry, admits the versions that satisfy the constraint, and so on recursively. The result is exactly the set of versions the consumer's resolver may legitimately choose: A `1.x` with `"B": "^1.1"` gives access to B 1.1.0 and 1.4.0, but not to B 2.0; when A 1.5 declares `"B": "^2.0"`, the closure extends by itself. `optional`, `dev` and `peer` dependencies count like normal ones. Transitive access is not a grant on B: a consumer that wants B directly, with its own, wider constraint, needs an explicit grant. `generic` declares no dependencies, so it does not take part. `publish` always requires an explicit grant for every package it releases. The closure is computed from the dependency graph `_meta/deps/<eco>.json` in R2 (updated at commit, yank and delete together with the index) and cached in the Cache API with a TTL of 60 s – with no writes to KV and no invalidation.
- `auth`: a list of credentials of different types; it enables rotation without changing the identity or the grants (the admin adds a new entry, sets `expires` on the old one, the cron removes expired ones) and combining types: a consumer credential usually has a `key` (committed, for humans) and optionally a `gh` (for the consumer's CI, with no secret in the repository); a `publish` credential usually has `gh`, and optionally also `key` (e.g. for publishing from outside GitHub Actions). The JWT exists only inside a GitHub Actions job, so `gh` never replaces `key` for local work.
- KV is eventually consistent (up to ~60 s for propagation); for token revocation this is acceptable.

**Panel session**: after login, the Worker issues an `HttpOnly; Secure; SameSite=Lax` cookie containing a payload `{ project, exp }` signed with HMAC (secret `SESSION_SECRET`), valid for 12 h – with nothing written to KV; the lack of revocation before expiry is acceptable at this horizon, and changing `SESSION_SECRET` invalidates all sessions.

## Data layout

R2 holds archives and manifests as the source of truth, and indexes as a cache; KV holds only credentials (with the `gh-repo` index) and the schema version.

**Keys in R2**

| Key                                        | Contents                                                                        |
| ------------------------------------------ | ------------------------------------------------------------------------------- |
| `<eco>/pkg/<name>/<version>/manifest.json` | normalised version manifest (below)                                             |
| `<eco>/pkg/<name>/<version>/<file>`        | version files: `.zip` (composer), `.tgz` (npm), `.crate` (cargo), any (generic) |
| `<eco>/index/…`                            | indexes generated from manifests, in the layout required by the ecosystem       |
| `_staging/<upload-id>/<file>`              | file uploaded via presign, not yet committed                                    |
| `_meta/packages.json`                      | list of all packages per ecosystem (for the home page and the panel)            |
| `_meta/deps/<eco>.json`                    | dependency graph of the ecosystem for transitive grants (below)                 |

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

`files[].digests` (optional): additional digests besides `sha256`, e.g. `{ "sha1": "…", "md5": "…" }`, needed by ecosystems whose clients verify other algorithms (Maven). The adapter declares in `init` the list of required algorithms (`digests: ["sha1", "md5"]`), and the Worker computes them at `commit` while streaming the object from staging to its target key, and stores them in the manifest. Without a declaration by the adapter nothing is computed; `reindex` does not recompute digests. The `sha256` field stays mandatory and separate: it is the only digest that the CLI computes before sending, that the Worker enforces in the presign (`x-amz-checksum-sha256`) and checks at `commit`, so it is the proof of the upload's integrity, while `digests` are only derivatives for clients.

**Keys in KV**

| Key                    | Contents                                                                       | TTL  |
| ---------------------- | ------------------------------------------------------------------------------ | ---- |
| `cred:<project>`       | credential record                                                              | none |
| `schema_version`       | integer                                                                        | none |
| `gh-repo:<owner/repo>` | list of the projects with a `gh` entry for this repository (for the `_` login) | none |

KV holds only state changed by a human (credentials, the `gh-repo` index, the schema version). Everything that is a cache or derived data lives outside KV, because the free plan allows only 1,000 writes and 1,000 deletes a day:

| Data                              | Where                                                                           | Why not KV                                                                                                                |
| --------------------------------- | ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| closure of transitive grants      | computed from `_meta/deps/<eco>.json` in R2, cached in the Cache API (TTL 60 s) | a write after every publication × every credential, plus invalidation through `list` + `delete`, would exhaust the limits |
| GitHub JWKS, `HttpSource` answers | Cache API                                                                       | a per-colo cache with a TTL is enough                                                                                     |
| verified JWTs                     | not cached – verified on every request                                          | every CI job is a new JWT = a write                                                                                       |
| panel sessions                    | signed cookie (HMAC, `SESSION_SECRET`)                                          | zero writes                                                                                                               |
| credential record                 | KV + a cache in the isolate's memory (TTL 60 s)                                 | limits KV reads, because consecutive requests of one installation usually hit the same isolate                            |

`_meta/deps/<eco>.json`: the graph `{ "<package>": { "<version>": { "<dependency>": "<constraint>" } } }`, updated at `commit`, yank and delete together with the index (the same `onlyIf`).

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
| Swift     | native: a `problem` field on the release in the release list, `410` for the version metadata                                           | yes – `Package.resolved` points at the version, the archive is still served                      |
| Maven     | no native concept; here the version disappears from `maven-metadata.xml`, the files under their full path stay                         | yes – the Gradle lockfile and Maven use full paths                                               |

In npm and Composer a yanked version is invisible, not marked – the client gets no warning, it simply stops seeing it; this is deliberately closer to the cargo semantics than to npm's `deprecated`, which only warns and still lets the version be chosen.

**Appending files to an existing version** (generic only): building `cdylib` libraries or installers in a CI matrix produces one file per job, while there is only one version. That is why `init` with `"append": true` on an existing version is allowed when (a) the adapter permits it, (b) less than `APPEND_WINDOW_HOURS` (default 24) have passed since `published`, and (c) every file name is new. A file, once committed, is immutable; `commit` appends the entries to `files[]` in the manifest (a write with `onlyIf`) and rebuilds the index. Once the window has passed, the version is closed. An alternative without a window: a job that collects the artifacts and publishes once.

**Cleanup** – a cron every hour: deletes `_staging/*` entries older than 24 h and secrets whose `expires` date has passed.

**Limits** – a single presigned PUT up to 5 GB (the R2 limit for one object without multipart); `init` rejects a `size` above `MAX_UPLOAD_BYTES` (default 512 MB).

## Adapters and endpoints

Each ecosystem is one module implementing a common interface; the kernel does not know the formats, the adapter does not know R2 or auth. The Worker serves only reads of the native protocols plus its own publishing and administration API.

**Modularity.** Adapters are modules in one repository (not plugins loaded at runtime): in the Worker, `src/adapters/<eco>/` directories registered in `src/adapters/index.ts`; in the CLI, `crates/eco-<eco>` crates linked into one binary. Adding an ecosystem = a PR to both repositories. Selective enabling: in the Worker, `vars.ECOSYSTEMS` (a list, default: all) – a disabled adapter is not routed and does not appear on the page or in the panel; in the CLI, Cargo feature flags (`--features composer,npm`, all by default), so that `sreg` can be built with only the adapters that are needed.

**Adapter contract** (`src/adapters/<eco>/index.ts`):

```ts
interface Adapter {
  eco: "composer" | "npm" | "cargo" | "generic" | "nuget" | "swift" | "maven";
  // validation and normalisation of what the CLI sent in init
  parseManifest(input: PublishInit): Manifest;
  // how to name the keys of the version's files in R2
  fileKey(m: Manifest, file: ManifestFile): string;
  // rebuild the index of one package from the list of manifests (including yanked)
  buildIndex(name: string, manifests: Manifest[]): IndexFile[]; // { key, body, contentType }
  // read routing: returns an index file to filter or an archive key to redirect to
  route(req: Request, url: URL): IndexRoute | ArchiveRoute | null;
  // index filter for a credential: only versions covered by grants
  // (editorial note: the implementation also passes the IndexRoute being served as a third argument)
  filterIndex(body: unknown, allowed: (name: string, version: string) => boolean): unknown;
  // configuration snippets for the home page
  clientConfig(baseUrl: string, token: string, packages: string[]): Snippet[];
  // dependencies declared in a manifest, for transitive grants (none for generic)
  dependencies(m: Manifest): Array<{ name: string; constraint: string }>;
  // version format, validation and comparison (semver / Composer / cargo / NuGet)
  validateVersion(version: string): boolean;
  satisfies(version: string, constraint: string): boolean;
}
```

(editorial note: the sketch is abridged. The implemented interface in `src/adapters/types.ts` passes `buildIndex` an `IndexContext` with the public URL as a third argument and `filterIndex` the `IndexRoute` being served as a third argument, and adds members such as `fileUrl`, `versionUrl`, `compareVersions`, `validateRange` and the optional `supportsAppend`, `digests`, `buildListing`, `buildConfig`, `selectFile`, `finalizeResponse`, `storedName`, `storedVersion` and `displayName`.)

**Composer** (`/composer/…`)

| Endpoint                                                          | Returns                                                                                                         |
| ----------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `GET /composer/packages.json`                                     | `{ "metadata-url": "/composer/p2/%package%.json", "available-packages": [...] }` – list filtered by grants      |
| `GET /composer/p2/<vendor>/<pkg>.json`                            | stable versions in the Composer 2 format (`minified: composer/2.0`), only allowed ones                          |
| `GET /composer/p2/<vendor>/<pkg>~dev.json` (always an empty list) | Composer 2 asks for this file; the registry does not serve `dev-*` versions, so it returns `{ "packages": {} }` |
| `GET /composer/dist/<vendor>/<pkg>/<version>.zip`                 | `302` → presigned GET (15 min)                                                                                  |

Client: `auth.json` with `bearer` or `http-basic` and `repositories: [{ type: composer, url: https://<domain>/composer }]`, or without `auth.json`, with the token in the URL: `https://<token>@<domain>/composer` (Composer extracts the userinfo as `http-basic`). npm and cargo do not support credentials in the URL.

**npm** (`/npm/…`)

| Endpoint                                      | Returns                                                                                                                                                                                                             |
| --------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET /npm/<name>` or `/npm/@<scope>%2f<name>` | packument: `versions`, `dist-tags` (`latest` = the highest non-yanked stable version), `time`; `dist.tarball` points to the Worker (editorial note: the exact shape and the dist-tag rules are in the API contract) |
| `GET /npm/<name>/<version>`                   | a single version entry (editorial note: a dist-tag is accepted in place of the version)                                                                                                                             |
| `GET /npm/<name>/-/<file>.tgz`                | `302` → presigned GET                                                                                                                                                                                               |
| `GET /npm/-/ping`, `GET /npm/-/whoami`        | `200` for checking auth; `whoami` returns `{ username: <project> }` (editorial note: both answer any credential kind, `publish` included)                                                                           |

Client: `.npmrc` with `@scope:registry=https://<domain>/npm/` and `//<domain>/npm/:_authToken=<token>`. Scoped packages are required (pointing the whole `registry` at the private registry would cut off public packages, because we do not mirror).

**cargo** (`/cargo/…`, sparse registry)

| Endpoint                                                                 | Returns                                                                                                                                                                                                                                                  |
| ------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET /cargo/index/config.json`                                           | `{ "dl": "https://<domain>/cargo/dl/{crate}/{version}", "api": "https://<domain>/cargo", "auth-required": true }` (editorial note: this file also requires a credential)                                                                                 |
| `GET /cargo/index/1/<c>`, `/2/<cc>`, `/3/<c>/<ccc>`, `/<cc>/<cc>/<name>` | index file: one JSON line per version (`name`, `vers`, `deps`, `cksum`, `features`, `yanked`, `v: 2`), filtered by grants (editorial note: lines in ascending version order, yanked ones included; `features2`, `links` and `rust_version` when present) |
| `GET /cargo/dl/<crate>/<version>`                                        | `302` → presigned GET                                                                                                                                                                                                                                    |
| `GET /cargo/api/v1/crates/<name>`                                        | optional metadata for the panel, not required by cargo (editorial note: `{ crate: { name, max_version, versions: [ { num, yanked } ] } }`, highest version first)                                                                                        |

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

**Swift** (`/swift/…`, Package Registry SE-0292)

A package is `scope.name` (`scope` `[a-zA-Z0-9-]{1,39}`, `name` `[a-zA-Z0-9_-]{1,100}`); versions are semver 2.0 only. The client sends `Accept: application/vnd.swift.registry.v1+json`, and responses carry `Content-Version: 1`. Base URL with a path prefix (`https://<domain>/swift`) – to be confirmed at the start of the implementation.

| Endpoint                                                              | Returns                                                                                                                                                                  |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `GET /swift/<scope>/<name>`                                           | `{ "releases": { "1.2.0": { "url": "…" }, … } }` – the versions covered by a grant; yanked ones with a `problem` field                                                   |
| `GET /swift/<scope>/<name>/<version>`                                 | release metadata: `id`, `version`, `resources: [{ name: "source-archive", type: "application/zip", checksum }]`, `metadata`, `Link` headers to the neighbouring versions |
| `GET /swift/<scope>/<name>/<version>/Package.swift[?swift-version=X]` | the manifest from `files[]` (role `manifest`), also the `Package@swift-X.swift` variants                                                                                 |
| `GET /swift/<scope>/<name>/<version>.zip`                             | `302` → presigned GET (the specification allows a redirect), with a `Digest: sha-256=…` header                                                                           |
| `GET /swift/identifiers?url=<repo-url>`                               | mapping of a repository URL → `scope.name`, from `meta.repositoryURL` of the manifest                                                                                    |

Files: the `source-archive` `.zip` (role `archive`) plus the manifests `Package.swift` / `Package@swift-X.swift` (role `manifest`). Yank: the native `problem` in the release list and `410` for the metadata; the archive is still served for `Package.resolved`. Signatures (SE-0391) are out of scope, but `files[]` can carry them.

Client: `swift package-registry login https://<domain>/swift --username <login> --password <password>` (Basic) or `--token <token>` (Bearer); the credentials are kept in the keychain/`.netrc`, in CI `--netrc-file`.

**Maven** (`/maven/…`, Maven 2 layout – Maven and Gradle, Android included)

The package name = `groupId:artifactId`; versions are arbitrary strings ordered by the `ComparableVersion` rules, ranges are `[1.0,2.0)` – the adapter provides `validateVersion` and `satisfies`. `-SNAPSHOT` versions are not supported (they are mutable). Any path prefix (the client configures the full URL).

| Path                                                                                    | Returns                                                                                                             |
| --------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `/maven/<group/as/path>/<artifact>/maven-metadata.xml`                                  | `<versioning>` with `<versions>`, `<latest>`, `<release>`, `<lastUpdated>` – from the manifests, filtered by grants |
| `/maven/<group/as/path>/<artifact>/<version>/<artifact>-<version>[-<classifier>].<ext>` | `302` → presigned GET for every file of the version                                                                 |
| `…/<file>.sha1`, `.md5`, `.sha256`, `.sha512`                                           | checksums from `files[].sha256` and `files[].digests` (the adapter requests `sha1`, `md5`, `sha512` in `init`)      |

Files: the `.pom` (mandatory, role `pom`), the main archive (role `archive`; extension from `<packaging>`: `.jar`, `.aar`, `.war`; none for `pom`/BOM), `-sources.jar`, `-javadoc.jar`, `.module` (Gradle Module Metadata – important for Android: AGP variants; Gradle reads it instead of the POM when it finds the `published-with-gradle-metadata` marker). Dependencies for transitive grants come from the POM (`compile`/`runtime`); the CLI provides the effective POM. Yank: the version disappears from `maven-metadata.xml`, the files under their full path stay.

Client: Maven `settings.xml` → `<server><id>…</id><username>…</username><password>…</password>` (Basic); Gradle `credentials { username; password }` or `HttpHeaderCredentials` (Bearer). Both follow the `302` and do not carry `Authorization` over to another host.

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

- One list of the packages from explicit grants, sorted by name (case-insensitive), not grouped by ecosystem: each row shows an ecosystem badge, the name, the number of versions covered by the grant and the latest of them (marked when yanked), and links to the package page. A filter bar (name, ecosystem chips) only hides rows; without JavaScript the full list stays usable. Packages available transitively are not a separate list.
- The package page `GET /packages/<eco>/<name>` (the name as one URL segment, `/` as `%2F`): the versions the credential may see, each a link with its date, yanked marker and number of files; the "dependencies" subtree, collapsed by default and omitted when empty, with the packages and version ranges that this grant opens (each linking to its own page, which a `read` credential may open through the closure); and ready-to-paste snippets for this one package with the domain and package name filled in and `<token>` as a placeholder: `composer.json` + `auth.json` (bearer and http-basic variants) and a variant without auth.json, with the token as userinfo in the repository URL (https://\<token>@\<domain>/composer – the token is already a login:password pair), `.npmrc`, `.cargo/config.toml`. The snippets are generated by `adapter.clientConfig(baseUrl, token, [name])`, each with a header row (title, file name, "copy" button).
- The version page `GET /packages/<eco>/<name>/<version>`: its files with size, role and labels, `sha256` and any extra `digests`, the native download URL for reference, and a download link `GET /packages/<eco>/<name>/<version>/files/<file>`, which checks the session's access to the version exactly like the native routes (explicit grants, then the closure) and answers `302` to a presigned `GET` (15 minutes) for every ecosystem.
- The package routes need a session (without one they redirect to the login page); `admin` sees every package and version, `publish` gets its notice with `403`, and a package or version the credential may not see is a `404`.

**Admin view** (kind `admin`), additional tabs:

- Packages: all of them, with versions; yank, delete and reindex actions (confirmation through a browser dialog).
- Credentials: list of projects with the number of active secrets and the nearest expiry date; a creation form (name, kind as a read/publish/admin radio, grants as eco/package/versions rows – hidden for admin); editing grants; "add secret" shows the new token once; "expire" sets `expires` on the old one; deleting a project.
- Settings: read-only – Worker version, flags from `vars` (editorial note: an unset variable shows the default the Worker uses, marked as such), a link to the latest release on GitHub (if `CHECK_UPDATES=true`).

**First run**: no credential at all in KV → the page shows a "create admin" form (name + generated secret, shown once). Once the first credential of kind `admin` is created, the form disappears permanently. Without this, the user would have to type hashes into KV by hand.

**Panel security**: all mutations via `POST`/`PATCH`/`DELETE` with the `X-Requested-With` header (CSRF protection with a `SameSite=Lax` cookie), `Content-Security-Policy: default-src 'self'`, no inline scripts.

## Rust CLI

The CLI is the only way to publish: for npm and cargo it delegates packing to the native tools, for Composer it packs by itself. Binary name: `sreg`.

**Commands**

| Command                                                                                                               | What it does                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| --------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `sreg composer publish [--version <v> \| --tag-prefix <p>] [--path <dir>]`                                            | packs the directory (`git archive HEAD` honouring `export-ignore`, falling back to a zip of the directory without `vendor/` and `.git/`), reads `composer.json`; the version comes from `--version` or the git tag (`v1.2.3` → `1.2.3`); it must be semver, `dev-*` is rejected                                                                                                                                                                                                                                                                      |
| `sreg npm publish [--version <v> \| --tag-prefix <p>] [--tag <t>] [--workspace <w>] [-- <npm pack flags>]`            | with `--version`, first runs `npm pkg set version=<v>` (the version in `package.json` inside the tarball must match the registry), then `npm pack --json --pack-destination <tmp>`, reads the name, version and `package.json` from the tarball; `--tag` sets the dist-tag for this version (default `latest`, prerelease → `next`)                                                                                                                                                                                                                  |
| `sreg cargo publish [--version <v> \| --tag-prefix <p>] [--package <crate>] [-- <cargo package flags>]`               | with `--version`, replaces `package.version` in `Cargo.toml` (a formatting-preserving edit, `toml_edit`), then runs `cargo package --no-verify --allow-dirty` (flags can be passed through), reads the `.crate` and the metadata from `cargo metadata`                                                                                                                                                                                                                                                                                               |
| `sreg whoami`                                                                                                         | `GET /api/v1/whoami`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `sreg generic publish <name> <version> <file>... [--label k=v]... [--meta k=v]... \| --spec files.json`               | publishes the given files as one version; `--label` applies to all files in the call, files with different labels = separate calls (in a CI matrix every job sends its own file of the same version, see Publishing: appending files); `--spec` accepts JSON with a list of `{ path, labels }` for unusual cases; the CLI adds no default labels                                                                                                                                                                                                     |
| `sreg generic get <name> [<version>\|latest] [--label k=v]... [--out <dir>] [--prerelease]`                           | downloads the files of a version (`latest` by default, all files without `--label`), verifies the sha256, prints the paths; `--json` returns the manifest without downloading                                                                                                                                                                                                                                                                                                                                                                        |
| `sreg nuget publish [--version <v> \| --tag-prefix <p>] [--project <csproj>] [--no-symbols] [-- <dotnet pack flags>]` | runs `dotnet pack` (version through `-p:PackageVersion=<v>`), unpacks the `.nupkg` (zip), reads the `.nuspec` (id, version, dependencies), attaches the `.snupkg` as `symbols` if one was produced, unless `--no-symbols` is given                                                                                                                                                                                                                                                                                                                   |
| `sreg swift publish [--version <v> \| --tag-prefix <p>] [--path <dir>]`                                               | runs `swift package archive-source -o <tmp>` (an archive compatible with the registry) and `swift package dump-package` (JSON with the dependencies → `native`); attaches `Package.swift` and `Package@swift-X.swift` as `manifest` files                                                                                                                                                                                                                                                                                                            |
| `sreg maven publish [--version <v> \| --tag-prefix <p>] --from-dir <dir> [--coords <group:artifact>]`                 | reads the finished artifacts from the local file repository the build pushed them to (`mvn deploy -DaltDeploymentRepository=local::file:./out`, Gradle `publish` to `url = file://…`); reads the `.pom` (coordinates, `<dependencies>` with their scope → `native`; an effective POM is required), collects all files of the version; `--coords` narrows the choice when the directory contains several artifacts; `--version` through `-Dversion=` / `-Pversion=` after `--`, without editing `pom.xml`                                             |
| `sreg auth <eco> --oidc [--project <p>] [--write <file>]`                                                             | in a GHA job (`permissions: id-token: write`) fetches a JWT from `ACTIONS_ID_TOKEN_REQUEST_URL` with audience = `PUBLIC_URL`, assembles `<project>:gh.<jwt>` – or, without `--project`, `_:gh.<jwt>`, and the Worker resolves the project from the `repository` claim itself – and writes it to `auth.json` / `.npmrc` / `credentials.toml` (or prints it for export as `COMPOSER_AUTH` / a variable); without `--oidc` it does the same with `REGISTRY_TOKEN`. The `publish` commands use the JWT directly with `--oidc`, also without `--project`. |

The common `--version` of all publish commands: overrides the version from the project manifest and, where the client verifies consistency (npm, cargo), also replaces it in the manifest file before packing; the CLI prints that the file was modified. The CLI has no administrative commands (tokens, yank, delete) – the panel does that; they can be added later on top of the `/api/v1` API.

**Packaging: what the CLI calls.** `sreg` does not use the native tools' `publish`, only their packing commands, and does the upload itself. The flags it sets itself are reserved; everything after `--` goes verbatim to the tool (`sreg cargo publish --version 1.2.0 -- --features ssl --locked`).

| Ecosystem | Command                                                                                                  | Flags set by `sreg`                                                                 | Examples of flags to pass after `--`            |
| --------- | -------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- | ----------------------------------------------- |
| composer  | `git archive --format=zip HEAD:<path>` (fallback: a zip of the directory without `vendor/` and `.git/`)  | –                                                                                   | –                                               |
| npm       | `npm pack`                                                                                               | `--json`, `--pack-destination <tmp>`, `-w <pkg>` from `--workspace`                 | `--ignore-scripts`                              |
| cargo     | `cargo package`                                                                                          | `--no-verify`, `--allow-dirty`, `-p <crate>` from `--package`, `--target-dir <tmp>` | `--features`, `--locked`, `--manifest-path`     |
| nuget     | `dotnet pack <project>`                                                                                  | `-o <tmp>`, `-p:PackageVersion=<v>` from `--version`                                | `-c Release`, `--no-build`, `--include-symbols` |
| swift     | `swift package archive-source` + `swift package dump-package`                                            | `-o <tmp>`                                                                          | –                                               |
| maven     | none – artifacts from a local file repository (`--from-dir`) produced by `mvn deploy` / `gradle publish` | –                                                                                   | –                                               |
| generic   | none – the files are ready                                                                               | –                                                                                   | –                                               |

**Monorepo.** Every `publish` has a pointer to the package within the repository, and all of them work from any working directory:

- composer: `--path packages/foo` → `git archive HEAD:packages/foo` (the subdirectory becomes the root of the archive; `export-ignore` from `.gitattributes` is honoured in subdirectories too); `composer.json` is read from that path.
- npm: `--workspace <name|path>` → `npm pack -w <…>`; `npm pkg set version` with `-w`.
- cargo: `--package <crate>` → `cargo package -p <crate>`; the version is edited in that crate's `Cargo.toml` (with `version.workspace = true` the CLI refuses `--version` and asks for the version to be set in the workspace).
- nuget: `--project <path to .csproj>`; a `.sln` is rejected (it produces several packages).
- Version from a tag: in a monorepo the tag `v1.2.0` does not say which package it belongs to, so `--tag-prefix <prefix>` (e.g. `foo/v`, `foo-v`) selects the tag on HEAD with that prefix; without `--tag-prefix` and without `--version`, the CLI accepts only a `v<semver>` tag and refuses when there is more than one on HEAD.
- One call = one package, one version. Publishing several packages from a monorepo means several calls (a loop in the workflow or a matrix); `sreg` does not publish whole workspaces at once.

Every publish command: computes the sha256 and size of every file, calls `init`, does the `PUT` of every file with retry (3 attempts, backoff), calls `commit`, prints the result. Common flags: `--dry-run` stops after `init`; `--skip-existing` turns a `409` (version exists) into exit code `0` with a message instead of an error, so that re-running a workflow is idempotent. Without this flag, `409` gives exit code `2`, so that the pipeline can tell it apart from an error.

**Configuration**: `--registry <url>` or `REGISTRY_URL`, `--token` or `REGISTRY_TOKEN`; in CI exclusively environment variables from GHA secrets. No configuration file.

**Workspace structure** (`serverless-registry-cli`):

- `crates/core` – HTTP client (reqwest + rustls), presigned upload, API models, version handling.
- `crates/eco-composer`, `crates/eco-npm`, `crates/eco-cargo`, `crates/eco-generic`, `crates/eco-nuget`, `crates/eco-swift`, `crates/eco-maven` – packing and manifest extraction; a common trait `Packager { fn package(&self, opts) -> Result<Package> }`.
- `crates/cli` – clap, subcommands, text and `--json` output.

**CLI ↔ Worker compatibility**: the CLI sends `X-Client-Version: <semver>`, the Worker answers with `X-Min-Client-Version`; CLI too old → a message with a link to the release and exit code `3`. The API is versioned in the path (`/api/v1`); the next Worker version keeps the previous one for at least one minor release.

**Distribution**: GitHub Releases through `cargo-dist` (linux-gnu/musl, macOS arm64/x86\_64, Windows; checksums), plus a GitHub Action `kduma-OSS/serverless-registry-cli/action@v1` (composite: downloads the binary at a pinned version, verifies the checksum, runs `sreg <eco> publish`) (editorial note: the action lives in the `action/` directory of the CLI repository rather than in a separate `<org>/publish-action` repository). Example in the README:

```yaml
- uses: kduma-OSS/serverless-registry-cli/action@v1
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
    "ECOSYSTEMS": "composer,npm,cargo,generic,nuget,swift,maven",
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
| `SESSION_SECRET`                                                                         | secret | HMAC key for signing the panel session cookie; changing it invalidates all sessions              |
| `R2_BUCKET_NAME`                                                                         | var    | name of the bucket used in presigned URLs; must match `bucket_name` of the `R2_BUCKET` binding   |

_Editorial note:_ the implementation sets `"keep_vars": true` in `wrangler.jsonc` and keeps the defaults above in code instead of `vars`, so the overrides set in the dashboard survive deploys (a deploy neither deletes nor resets them).

Domain: a custom domain of the Worker set in the dashboard, not through `routes` in the file. The README walks through this step by step with screenshots.

**Cloudflare free plan limits** (as of the date of this specification; check before a release): the registry is meant to run entirely on the free plan for a small team. After the KV optimisation (Data layout section), the binding limit is **Workers Free: 100,000 requests a day** – the same as 100,000 KV reads a day, so both limits run out at the same point (~2,500 installs at 40 requests a day). R2 free (10 GB, 1 million writes and 10 million reads a month) and the cron are not limiting. A larger team, or CI that runs very often, needs Workers Paid ($5 a month: 10 million requests, 10 million KV reads); the README describes this. An alternative, noted but not planned: D1 instead of KV (free: 5 million row reads and 100,000 row writes a day, SQL, strong consistency) – it makes sense once queries are needed (search, reports); the change would affect only the `storage`/`auth` layer.

**Updates**

- Releases tagged with semver, `CHANGELOG.md` with "breaking" and "requires migration" sections.
- The user: "Sync fork" (or merge a tag) → Workers Builds / GHA deploys. Zero file edits on their side, so zero conflicts.
- The panel shows the version and, with `CHECK_UPDATES=true`, compares it with the latest release on GitHub (one `fetch` per day, cached in the Cache API) (editorial note: earlier versions said "cached in KV"; like the other derived data in the Data layout section, the answer is kept in the Cache API, so the check makes no KV write).
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
    composer/  npm/  cargo/  generic/  nuget/  swift/  maven/
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
4. cargo adapter + `cargo publish`; generic adapter + generic publish/get; nuget adapter + nuget publish; swift adapter + swift publish; maven adapter + maven publish.
5. Deploy button, cargo-dist, publish-action, documentation.

**Settled decisions**

- CLI name: `sreg`.
- npm `dist-tags`: the CLI accepts `--tag` (default `latest`); a prerelease version without `--tag` gets `next`. Tags are kept in the package's `_meta`; yanking/deleting the version a tag points to moves the tag to the highest remaining stable version. (Editorial note: the implementation does not store tags; it derives them from the manifests on every request, see the API contract, section 12.)
- No support for `dev-*` in Composer: the version format is defined by the adapter (semver for composer/npm/cargo/generic/swift, NuGet versions for nuget, ComparableVersion for maven), prereleases allowed, all versions immutable. `sreg composer publish` rejects a non-semver version. Testing a branch before a release = a VCS repository temporarily listed in the consumer's `composer.json`.
- Presigned GET valid for 15 min.
- The panel allows deleting a whole package; confirmation by typing its name.

## Changes

| Version | Date       | Change                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ------- | ---------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 8       | 2026-10-28 | Optimisation for the free plan limits: KV only for state changed by a human. The grant closure from the `_meta/deps/<eco>.json` graph in R2 and the Cache API (no `closure:*` keys and no invalidation), JWKS and `HttpSource` in the Cache API, JWTs not cached, panel sessions as signed cookies (`SESSION_SECRET`, no `session:*` in KV), a credential cache in the isolate's memory. A new "Free plan limits" section in Deploy; D1 noted as an alternative.                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| 7       | 2026-10-28 | The Swift ecosystem (Package Registry SE-0292: release list, metadata, manifests as `manifest` files, archive through `302`, identifiers; `sreg swift publish` through `archive-source` + `dump-package`) and the Maven ecosystem (Maven 2 layout for Maven/Gradle/Android: `maven-metadata.xml`, files through `302`, checksums from `digests`; several files per version, `archive` from `<packaging>` including `.aar`, `.module`; no SNAPSHOT; `sreg maven publish --from-dir`).                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| 6       | 2026-10-28 | `files[].digests`: an adapter can request additional digests in `init` (sha1, md5, sha512), computed by the Worker at `commit` while copying from staging; `sha256` stays mandatory. The `_` login for `gh` tokens: the project is resolved from the `repository` claim through the `gh-repo:<owner/repo>` index in KV, and the kind of request selects `read`/`publish`; `sreg auth --oidc` and `publish --oidc` work without `--project`. The `gh` entry clarified: a table of fields (`repository`, `repository_id`, `repository_owner_id`, `ref`, `workflow`, `environment`, `expires`) mapped to claims; a `publish` credential may also have a `key`.                                                                                                                                                                                                                                                           |
| 5       | 2026-10-28 | Transitive grants (`grants.transitive`, default `true` for `read`): access to dependencies from the registry, computed from the constraints in the manifests, cached as `closure:<project>:<eco>` in KV; `publish` unchanged. The home page shows dependencies as a collapsed subtree under the source package.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| 4       | 2026-10-28 | CLI: a table of packing commands (`npm pack`, `cargo package`, `dotnet pack`, `git archive`) with the flags reserved by `sreg` and the rest passed through after `--`; monorepo support: `--path`, `--workspace`, `--package`, `--project`, `--tag-prefix`; one call = one package, one version. New flags: the common `--skip-existing` (code `0` on `409`), `--no-symbols` for nuget.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| 3       | 2026-10-28 | Fifth ecosystem, NuGet (V3 protocol: service index, flat container, registration, search; Basic auth from `nuget.config`; `.snupkg` as `symbols`; `sreg nuget publish`). The version format is defined by the adapter (`validateVersion`), not by the kernel – NuGet allows 4 parts. A table of `yank` semantics per ecosystem in the Publishing section.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| 2       | 2026-10-28 | A package version is a list of files (`files[]`), not a single file: the manifest, `init`/`commit`, the adapter contract (`fileKey`) and the CLI accept multiple files; the "exactly one `archive`" rule belongs to the composer/npm/cargo adapters. New **generic** ecosystem: arbitrary files with an open `labels` map (`?label=k:v` filter) and `meta`, a JSON feed (list, `latest`, version, `302` to the file, `ETag`), the commands `sreg generic publish` (`--label k=v` per call, optional `--spec`) and `sreg generic get`, appending files to a version within the `APPEND_WINDOW_HOURS` window. Modularity clarified: adapters as modules in the repository, enabled through `vars.ECOSYSTEMS` and Cargo feature flags. CLI: `--version` also in `npm`/`cargo publish` (replacing the version in the manifest file), administrative commands removed – administration only through the panel and the API. |
| 1       | 2026-10-28 | First version of the specification.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
