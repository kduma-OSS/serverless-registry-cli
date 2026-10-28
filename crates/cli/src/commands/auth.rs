//! `sreg auth <eco>`: credentials for Composer, npm and cargo.
//!
//! With `--write <file>` the token is merged into the package manager's credentials
//! file; without it a line for the environment is printed. See `docs/cli-auth.md`.

use std::path::Path;

use anyhow::{bail, Context};
use serde::Serialize;
use sreg_core::token::Token;
use sreg_core::Ecosystem;
use url::Url;

use crate::cli::{AuthArgs, GlobalArgs};
use crate::context;
use crate::output::Output;

#[derive(Serialize)]
struct Written<'a> {
    eco: Ecosystem,
    file: &'a Path,
    key: String,
}

#[derive(Serialize)]
struct Variable {
    eco: Ecosystem,
    name: String,
    value: String,
}

pub async fn run(global: &GlobalArgs, out: &Output, args: &AuthArgs) -> anyhow::Result<()> {
    let registry = context::registry(global)?;
    let eco = Ecosystem::from(args.eco);
    let target = Target::new(eco, &registry, &args.registry_name)?;
    let token = context::token(global, &registry, args.oidc, args.project.as_deref()).await?;
    let login = Token::login(&token)?.to_owned();

    match &args.write {
        Some(path) => {
            let existing = match std::fs::read_to_string(path) {
                Ok(content) => Some(content),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                Err(err) => {
                    return Err(err).with_context(|| format!("failed to read {}", path.display()))
                }
            };
            let updated = target
                .merge(existing.as_deref(), &token)
                .with_context(|| format!("failed to update {}", path.display()))?;
            write_private(path, &updated, existing.is_none())
                .with_context(|| format!("failed to write {}", path.display()))?;

            let report = Written {
                eco,
                file: path,
                key: target.key(),
            };
            out.emit(&report, || {
                println!(
                    "Wrote the token of `{login}` for {} to {}.",
                    report.key,
                    path.display()
                );
            })
        }
        None => {
            let (name, value) = target.variable(&token)?;
            let line = target.shell_line(&name, &value);
            let report = Variable { eco, name, value };
            out.emit(&report, || println!("{line}"))
        }
    }
}

/// Where a package manager looks for the registry's token.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// `auth.json` `bearer` entry for a host (`host[:port]`).
    Composer { host: String },
    /// `.npmrc` key `//host[:port][/path]/npm/:_authToken`.
    Npm { key: String },
    /// `credentials.toml` `[registries.<name>]`.
    Cargo { name: String },
}

impl Target {
    fn new(eco: Ecosystem, registry: &Url, registry_name: &str) -> anyhow::Result<Self> {
        let host = registry
            .host_str()
            .context("the registry URL has no host")?;
        let host = match registry.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        };
        Ok(match eco {
            Ecosystem::Composer => Target::Composer { host },
            Ecosystem::Npm => Target::Npm {
                key: format!(
                    "//{host}{}/npm/:_authToken",
                    registry.path().trim_end_matches('/')
                ),
            },
            Ecosystem::Cargo => {
                let valid = !registry_name.is_empty()
                    && registry_name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
                if !valid {
                    bail!("invalid --registry-name `{registry_name}`: use letters, digits, `-` and `_`");
                }
                Target::Cargo {
                    name: registry_name.to_owned(),
                }
            }
        })
    }

    /// A description of the entry that holds the token.
    fn key(&self) -> String {
        match self {
            Target::Composer { host } => format!("bearer.{host}"),
            Target::Npm { key } => key.clone(),
            Target::Cargo { name } => format!("registries.{name}.token"),
        }
    }

    /// The environment variable carrying the token and its value.
    fn variable(&self, token: &str) -> anyhow::Result<(String, String)> {
        Ok(match self {
            Target::Composer { host } => {
                let value = serde_json::json!({ "bearer": { host: token } });
                ("COMPOSER_AUTH".to_owned(), serde_json::to_string(&value)?)
            }
            Target::Npm { key } => (format!("npm_config_{key}"), token.to_owned()),
            Target::Cargo { name } => (
                format!(
                    "CARGO_REGISTRIES_{}_TOKEN",
                    name.to_ascii_uppercase().replace('-', "_")
                ),
                token.to_owned(),
            ),
        })
    }

    /// The line printed without `--write`.
    ///
    /// The npm variable name contains `/` and `:`, which POSIX shells cannot `export`,
    /// so it is printed as a plain `NAME=value` line (for `$GITHUB_ENV` or `env`).
    fn shell_line(&self, name: &str, value: &str) -> String {
        match self {
            Target::Npm { .. } => format!("{name}={value}"),
            _ => format!("export {name}={}", shell_quote(value)),
        }
    }

    /// Merges the token into the existing content of the credentials file.
    fn merge(&self, existing: Option<&str>, token: &str) -> anyhow::Result<String> {
        match self {
            Target::Composer { host } => merge_auth_json(existing, host, token),
            Target::Npm { key } => Ok(merge_npmrc(existing, key, token)),
            Target::Cargo { name } => merge_credentials_toml(existing, name, token),
        }
    }
}

fn merge_auth_json(existing: Option<&str>, host: &str, token: &str) -> anyhow::Result<String> {
    let mut root = match existing
        .map(str::trim)
        .filter(|content| !content.is_empty())
    {
        Some(content) => serde_json::from_str::<serde_json::Value>(content)
            .context("the file is not valid JSON")?,
        None => serde_json::json!({}),
    };
    let object = root
        .as_object_mut()
        .context("the file does not contain a JSON object")?;
    let bearer = object
        .entry("bearer")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("`bearer` is not a JSON object")?;
    bearer.insert(host.to_owned(), serde_json::Value::String(token.to_owned()));

    let mut buffer = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
    root.serialize(&mut serializer)?;
    buffer.push(b'\n');
    Ok(String::from_utf8(buffer)?)
}

fn merge_npmrc(existing: Option<&str>, key: &str, token: &str) -> String {
    let entry = format!("{key}={token}");
    let mut replaced = false;
    let mut lines: Vec<String> = existing
        .unwrap_or_default()
        .lines()
        .map(|line| {
            let matches = line
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == key);
            if matches && !replaced {
                replaced = true;
                entry.clone()
            } else if matches {
                // Drop duplicates of the entry so the new token is the only one.
                String::new()
            } else {
                line.to_owned()
            }
        })
        .collect();
    if !replaced {
        lines.push(entry);
    }
    let mut content = lines
        .into_iter()
        .filter(|line| !line.is_empty() || existing.is_some())
        .collect::<Vec<_>>()
        .join("\n");
    content.push('\n');
    content
}

fn merge_credentials_toml(
    existing: Option<&str>,
    name: &str,
    token: &str,
) -> anyhow::Result<String> {
    let mut root: toml::Table = match existing {
        Some(content) => content.parse().context("the file is not valid TOML")?,
        None => toml::Table::new(),
    };
    let registries = root
        .entry("registries")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .context("`registries` is not a table")?;
    let registry = registries
        .entry(name)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .with_context(|| format!("`registries.{name}` is not a table"))?;
    registry.insert("token".to_owned(), toml::Value::String(token.to_owned()));
    Ok(toml::to_string(&root)?)
}

/// Quotes `value` for POSIX shells.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Writes `content`; a newly created file is readable by its owner only (Unix).
fn write_private(path: &Path, content: &str, create: bool) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    if create {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        std::io::Write::write_all(&mut file, content.as_bytes())
    } else {
        std::fs::write(path, content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "shop-frontend:k7Qx2mN9pLw4vR8sT1bYc3dF6hJ0aZ5e";

    fn target(eco: Ecosystem, url: &str) -> Target {
        Target::new(eco, &Url::parse(url).unwrap(), "sreg").unwrap()
    }

    #[test]
    fn composer_uses_the_host_with_explicit_port() {
        assert_eq!(
            target(Ecosystem::Composer, "https://packages.example.com/"),
            Target::Composer {
                host: "packages.example.com".into()
            }
        );
        assert_eq!(
            target(Ecosystem::Composer, "http://localhost:8787"),
            Target::Composer {
                host: "localhost:8787".into()
            }
        );
    }

    #[test]
    fn npm_key_keeps_the_path_prefix() {
        assert_eq!(
            target(Ecosystem::Npm, "https://packages.example.com").key(),
            "//packages.example.com/npm/:_authToken"
        );
        assert_eq!(
            target(Ecosystem::Npm, "https://example.com/registry/").key(),
            "//example.com/registry/npm/:_authToken"
        );
    }

    #[test]
    fn cargo_registry_name_is_validated() {
        let url = Url::parse("https://packages.example.com").unwrap();
        assert!(Target::new(Ecosystem::Cargo, &url, "my registry").is_err());
        assert!(Target::new(Ecosystem::Cargo, &url, "").is_err());
    }

    #[test]
    fn environment_lines_per_ecosystem() {
        let composer = target(Ecosystem::Composer, "https://packages.example.com");
        let (name, value) = composer.variable(TOKEN).unwrap();
        assert_eq!(
            composer.shell_line(&name, &value),
            format!(r#"export COMPOSER_AUTH='{{"bearer":{{"packages.example.com":"{TOKEN}"}}}}'"#)
        );

        let npm = target(Ecosystem::Npm, "https://packages.example.com");
        let (name, value) = npm.variable(TOKEN).unwrap();
        assert_eq!(
            npm.shell_line(&name, &value),
            format!("npm_config_//packages.example.com/npm/:_authToken={TOKEN}")
        );

        let cargo = Target::new(
            Ecosystem::Cargo,
            &Url::parse("https://packages.example.com").unwrap(),
            "acme-private",
        )
        .unwrap();
        let (name, value) = cargo.variable(TOKEN).unwrap();
        assert_eq!(
            cargo.shell_line(&name, &value),
            format!("export CARGO_REGISTRIES_ACME_PRIVATE_TOKEN='{TOKEN}'")
        );
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
    }

    #[test]
    fn auth_json_merge_keeps_other_entries_and_order() {
        let existing = r#"{
    "github-oauth": { "github.com": "gho_x" },
    "bearer": { "other.example.com": "o", "packages.example.com": "old" }
}"#;
        let merged = merge_auth_json(Some(existing), "packages.example.com", TOKEN).unwrap();
        let value: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(value["github-oauth"]["github.com"], "gho_x");
        assert_eq!(value["bearer"]["other.example.com"], "o");
        assert_eq!(value["bearer"]["packages.example.com"], TOKEN);
        assert!(merged.find("github-oauth") < merged.find("bearer"));
        assert!(merged.contains("\n    \"bearer\""));
    }

    #[test]
    fn auth_json_is_created_and_rejects_non_objects() {
        let created = merge_auth_json(None, "h", TOKEN).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&created).unwrap(),
            serde_json::json!({ "bearer": { "h": TOKEN } })
        );
        assert!(merge_auth_json(Some("[]"), "h", TOKEN).is_err());
        assert!(merge_auth_json(Some(r#"{"bearer": 1}"#), "h", TOKEN).is_err());
    }

    #[test]
    fn npmrc_merge_replaces_or_appends_the_entry() {
        let key = "//packages.example.com/npm/:_authToken";
        let existing = format!("@acme:registry=https://packages.example.com/npm/\n{key}=old\n");
        assert_eq!(
            merge_npmrc(Some(&existing), key, TOKEN),
            format!("@acme:registry=https://packages.example.com/npm/\n{key}={TOKEN}\n")
        );
        assert_eq!(
            merge_npmrc(Some("save-exact=true"), key, TOKEN),
            format!("save-exact=true\n{key}={TOKEN}\n")
        );
        assert_eq!(merge_npmrc(None, key, TOKEN), format!("{key}={TOKEN}\n"));
    }

    #[test]
    fn credentials_toml_merge_keeps_other_registries() {
        let existing = "[registry]\ntoken = \"crates-io\"\n\n[registries.other]\ntoken = \"o\"\n";
        let merged = merge_credentials_toml(Some(existing), "sreg", TOKEN).unwrap();
        let value: toml::Table = merged.parse().unwrap();
        assert_eq!(value["registry"]["token"].as_str(), Some("crates-io"));
        assert_eq!(value["registries"]["other"]["token"].as_str(), Some("o"));
        assert_eq!(value["registries"]["sreg"]["token"].as_str(), Some(TOKEN));
        assert!(merge_credentials_toml(Some("registries = 1"), "sreg", TOKEN).is_err());
    }
}
