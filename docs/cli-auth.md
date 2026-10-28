# `sreg auth`: credentials for package managers

`sreg auth <eco>` hands the registry token to Composer, npm, cargo or NuGet. It either
writes the token into the package manager's credentials file (`--write <file>`) or prints
one line that puts it into the environment (for NuGet: a `dotnet nuget add source`
command).

Generic packages have no package manager: `sreg generic get` uses `--token` /
`REGISTRY_TOKEN` (or `--oidc [--oidc-project <PROJECT>]`) directly. The publish commands take
the same `--oidc [--oidc-project <PROJECT>]` pair; only `sreg auth` calls it `--project`.

```text
sreg auth <composer|npm|cargo|nuget> [--oidc [--project <PROJECT>]] [--write <FILE>]
          [--registry-name <NAME>] [--registry <URL>] [--token <TOKEN>] [--json]
```

`--registry` (or `REGISTRY_URL`) is always required: the host and path of the registry
determine where the token is stored. No request is made to the registry itself.

## Where the token comes from

- Without `--oidc`: `--token` or `REGISTRY_TOKEN`, used as is.
- With `--oidc` inside a GitHub Actions job with `permissions: id-token: write`: `sreg`
  requests a JWT from `ACTIONS_ID_TOKEN_REQUEST_URL` (with
  `Authorization: bearer $ACTIONS_ID_TOKEN_REQUEST_TOKEN` and
  `audience=<registry URL without trailing slash>`, which must equal the Worker's
  `PUBLIC_URL`) and builds the token `<PROJECT>:gh.<jwt>` with `--project <PROJECT>`, or
  `_:gh.<jwt>` without it. The reserved login `_` asks the registry to find the project
  from the JWT's `repository` claim; with `--project`, the credential `<PROJECT>` must
  have a `gh` auth entry matching the repository (and, if set, the ref and workflow).
  `--project` is only accepted together with `--oidc`.

GitHub OIDC tokens are short-lived (minutes), so run `sreg auth --oidc` right before the
step that installs packages.

## Without `--write`: environment line

| Ecosystem  | Printed line                                                                                                                 |
| ---------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `composer` | `export COMPOSER_AUTH='{"bearer":{"<host>":"<token>"}}'`                                                                     |
| `npm`      | `npm_config_//<host><path>/npm/:_authToken=<token>`                                                                          |
| `cargo`    | `export CARGO_REGISTRIES_<NAME>_TOKEN='<token>'`                                                                             |
| `nuget`    | `dotnet nuget add source '<registry>/nuget/index.json' -n '<name>' -u '<login>' -p '<password>' --store-password-in-clear-text` |

- `<host>` is the registry's host name, followed by `:<port>` when the URL has an explicit
  non-default port.
- `<path>` is the path of the registry URL without a trailing slash (empty for
  `https://packages.example.com`).
- `<NAME>` is `--registry-name` (default `sreg`) in upper case with `-` replaced by `_`;
  `<name>` is `--registry-name` as given.
- `<registry>` is the registry URL without a trailing slash (the Worker's `PUBLIC_URL`).
- `<login>` and `<password>` are the two sides of the token's first `:`
  (`[<source>.]<project>` and `[<type>.]<secret>`); NuGet sends them as HTTP Basic
  credentials, which the Worker accepts.
- The composer and cargo lines are POSIX shell `export` statements with the value in single
  quotes: use them with `eval "$(sreg auth composer)"`.
- The npm variable name contains `/` and `:`, which shells cannot `export`. npm reads it from
  the environment all the same, so the line is printed as plain `NAME=value`, ready for
  `"$GITHUB_ENV"` (`sreg auth npm >> "$GITHUB_ENV"`) or `env "$(sreg auth npm)" npm ci`.

- The NuGet command is printed with every argument in single quotes; run it with
  `eval "$(sreg auth nuget)"`. `dotnet` stores the password in clear text in the user's
  `NuGet.Config` (encrypted storage is only available on Windows).

With `--json` the output is `{ "eco": "<eco>", "name": "<variable>", "value": "<value>" }`,
for example to set the variable from a GitHub Actions step without shell quoting. For
`nuget` it is `{ "eco": "nuget", "source": "<url>", "name": "<name>", "username":
"<login>", "password": "<password>", "command": "<printed command>" }`.

The printed line contains the token; treat the output as a secret.

## With `--write <file>`: credentials file

The file is updated in place when it exists and created otherwise (missing parent
directories are created; a new file gets mode `0600` on Unix). Other content is kept.

| Ecosystem  | Typical file                     | Entry written                                    |
| ---------- | -------------------------------- | ------------------------------------------------ |
| `composer` | `auth.json`                      | `"bearer": { "<host>": "<token>" }`              |
| `npm`      | `.npmrc`                         | `//<host><path>/npm/:_authToken=<token>`         |
| `cargo`    | `~/.cargo/credentials.toml`      | `[registries.<name>]` with `token = "<token>"`   |
| `nuget`    | `nuget.config`                   | source `<name>` and its clear-text credentials   |

- **composer**: the file must contain a JSON object; `bearer` is created when missing and the
  entry for `<host>` is replaced. Key order is kept; the file is written with four-space
  indentation, like Composer does.
- **npm**: an existing line with the same key is replaced (further duplicates are removed),
  otherwise the line is appended. Other lines, including comments, are kept. The scope line
  `@<scope>:registry=<registry>/npm/` is not written because `sreg` does not know the
  scope; add it to the project's `.npmrc` yourself.
- **cargo**: `<name>` is `--registry-name` (default `sreg`). Only the `token` key of
  `[registries.<name>]` is changed; other tables and keys are kept, but comments and
  formatting of the file are not preserved. The token is stored as is (cargo sends it
  verbatim in `Authorization`). The registry itself is declared in `.cargo/config.toml`
  (`[registries.<name>] index = "sparse+<registry>/cargo/index/"`), which `sreg` does not
  write.

- **nuget**: the file must have a `<configuration>` root. Under `packageSources` an
  `<add key="<name>" value="<registry>/nuget/index.json"/>` entry is written, and under
  `packageSourceCredentials` a `<name>` element with `<add key="Username" value="<login>"/>`
  and `<add key="ClearTextPassword" value="<password>"/>`. Previous entries for `<name>` in
  both sections are replaced, missing sections are added, and everything else (other
  sources, `<clear />`, comments, formatting) is kept. `<name>` is `--registry-name`
  (default `sreg`) and must start with a letter because it is also an XML element name.
  A new file starts with an XML declaration.

With `--json` the output is `{ "eco": "<eco>", "file": "<path>", "key": "<entry>" }`; the
token is not printed. `key` is `bearer.<host>`, the npm key, `registries.<name>.token` or
`packageSourceCredentials.<name>`. In text mode a confirmation naming the token's login (the project,
which is not secret) and the file is printed.

## Examples

```sh
# Local development: store a read token for Composer.
sreg auth composer --write auth.json

# GitHub Actions: install npm packages with an OIDC token of the project `shop-frontend`.
sreg auth npm --oidc --project shop-frontend >> "$GITHUB_ENV"

# The same, letting the registry find the project from the repository.
sreg auth npm --oidc >> "$GITHUB_ENV"

# cargo with a custom registry name.
eval "$(sreg auth cargo --registry-name acme)"

# NuGet: add the source to the repository's nuget.config in CI.
sreg auth nuget --oidc --project shop-frontend --write nuget.config
```

## Exit codes

`0` on success, `1` on any error (no registry URL, no token, OIDC not available, a
credentials file that cannot be parsed or written).
