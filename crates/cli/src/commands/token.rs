//! `sreg token create|rotate|list|revoke` (admin).

use anyhow::{bail, Context};
use serde::Serialize;
use sreg_core::models::{
    format_timestamp, parse_timestamp, AddSecretRequest, AuthEntry, CreateCredentialRequest,
    Credential, Grant, Kind, SecretType,
};
use time::OffsetDateTime;

use crate::cli::{
    CredentialKind, GlobalArgs, TokenCommand, TokenCreateArgs, TokenRevokeArgs, TokenRotateArgs,
};
use crate::commands::whoami::grants_table;
use crate::context;
use crate::output::{indent, table, Output};
use crate::prompt;

const STORE_NOW: &str = "store it now, it will not be shown again";

pub async fn run(global: &GlobalArgs, out: &Output, command: &TokenCommand) -> anyhow::Result<()> {
    match command {
        TokenCommand::Create(args) => create(global, out, args).await,
        TokenCommand::Rotate(args) => rotate(global, out, args).await,
        TokenCommand::List => list(global, out).await,
        TokenCommand::Revoke(args) => revoke(global, out, args).await,
    }
}

async fn create(global: &GlobalArgs, out: &Output, args: &TokenCreateArgs) -> anyhow::Result<()> {
    let kind = match args.kind {
        CredentialKind::Read => Kind::Read,
        CredentialKind::Publish => Kind::Publish,
        CredentialKind::Admin => Kind::Admin,
    };
    let grants = args
        .grants
        .iter()
        .map(|grant| Grant::parse_shorthand(grant))
        .collect::<Result<Vec<_>, _>>()?;
    if kind == Kind::Admin && !grants.is_empty() {
        bail!("admin credentials cannot have grants");
    }
    if let Some(expires) = &args.expires {
        parse_timestamp(expires)
            .context("invalid --expires; use an RFC 3339 timestamp or YYYY-MM-DD")?;
    }
    let request = CreateCredentialRequest {
        project: args.project.name.clone(),
        kind,
        grants,
        secret: args
            .expires
            .clone()
            .map(|expires| AddSecretRequest::key(Some(expires))),
    };

    let client = context::client(global).await?;
    let response = client.create_credential(&request).await?;

    out.emit(&response, || {
        let credential = &response.credential;
        println!(
            "Created {} credential `{}`.",
            credential.kind, credential.project
        );
        if !credential.grants.is_empty() {
            println!("Grants:");
            println!("{}", indent(&grants_table(&credential.grants), 2));
        }
        if let Some(expires) = credential
            .auth
            .first()
            .and_then(|entry| entry.expires.as_ref())
        {
            println!("The token expires at {expires}.");
        }
        match &response.token {
            Some(token) => {
                println!("Token ({STORE_NOW}):");
                println!("{token}");
            }
            None => println!("No token was issued for the first secret."),
        }
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RotateReport {
    secret: AuthEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    old_secrets: Vec<OldSecret>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OldSecret {
    id: String,
    /// `expiring`, `unchanged` (it already expires earlier) or `failed`.
    status: &'static str,
    expires: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn rotate(global: &GlobalArgs, out: &Output, args: &TokenRotateArgs) -> anyhow::Result<()> {
    let grace = parse_duration(&args.expire_old)?;
    let project = &args.project.name;
    let client = context::client(global).await?;

    let credential = client.credential(project).await?;
    let now = OffsetDateTime::now_utc();
    let previous = active_keys(&credential, now);

    let added = client
        .add_secret(project, &AddSecretRequest::key(None))
        .await?;

    // The new token has been issued and is shown only once: from here on, failures are
    // reported after it has been printed.
    let target = now + grace;
    let target_text = format_timestamp(target)?;
    let mut old_secrets = Vec::new();
    for entry in previous
        .into_iter()
        .filter(|entry| entry.id != added.secret.id)
    {
        let current = entry.expires_at().ok().flatten();
        if current.is_some_and(|current| current <= target) {
            old_secrets.push(OldSecret {
                id: entry.id.clone(),
                status: "unchanged",
                expires: entry.expires.clone(),
                error: None,
            });
            continue;
        }
        let result = client
            .expire_secret(project, &entry.id, Some(target_text.clone()))
            .await;
        old_secrets.push(match result {
            Ok(response) => OldSecret {
                id: entry.id.clone(),
                status: "expiring",
                expires: response.secret.expires,
                error: None,
            },
            Err(err) => OldSecret {
                id: entry.id.clone(),
                status: "failed",
                expires: entry.expires.clone(),
                error: Some(err.to_string()),
            },
        });
    }

    let report = RotateReport {
        secret: added.secret.clone(),
        token: added.token.clone(),
        old_secrets,
    };
    out.emit(&report, || {
        match &report.token {
            Some(token) => {
                println!("New token for `{project}` ({STORE_NOW}):");
                println!("{token}");
            }
            None => println!("Added secret {} to `{project}`.", report.secret.id),
        }
        if report.old_secrets.is_empty() {
            println!("No previously active key secrets.");
        } else {
            println!("Previously active key secrets:");
            for old in &report.old_secrets {
                let expires = old.expires.as_deref().unwrap_or("never");
                let detail = match old.status {
                    "expiring" => format!("expires {expires}"),
                    "unchanged" => format!("unchanged, already expires {expires}"),
                    _ => format!(
                        "FAILED to set expiry: {}",
                        old.error.as_deref().unwrap_or("unknown error")
                    ),
                };
                println!("  {}  {detail}", old.id);
            }
        }
    })?;

    let failed = report
        .old_secrets
        .iter()
        .filter(|old| old.status == "failed")
        .count();
    if failed > 0 {
        bail!(
            "the new token was issued, but {failed} old secret(s) could not be expired; \
             retry with `sreg token rotate` or expire them in the panel"
        );
    }
    Ok(())
}

async fn list(global: &GlobalArgs, out: &Output) -> anyhow::Result<()> {
    let client = context::client(global).await?;
    let list = client.credentials().await?;
    let now = OffsetDateTime::now_utc();

    out.emit(&list, || {
        if list.credentials.is_empty() {
            println!("No credentials.");
            return;
        }
        let rows: Vec<Vec<String>> = list
            .credentials
            .iter()
            .map(|credential| {
                let active: Vec<&AuthEntry> = credential
                    .auth
                    .iter()
                    .filter(|entry| entry.is_active(now))
                    .collect();
                let nearest = active
                    .iter()
                    .filter_map(|entry| Some((entry.expires_at().ok().flatten()?, entry)))
                    .min_by_key(|(expires, _)| *expires)
                    .and_then(|(_, entry)| entry.expires.clone())
                    .unwrap_or_else(|| "never".to_owned());
                vec![
                    credential.project.clone(),
                    credential.kind.to_string(),
                    active.len().to_string(),
                    nearest,
                ]
            })
            .collect();
        println!(
            "{}",
            table(
                &["PROJECT", "KIND", "ACTIVE SECRETS", "NEAREST EXPIRY"],
                &rows
            )
        );
    })
}

async fn revoke(global: &GlobalArgs, out: &Output, args: &TokenRevokeArgs) -> anyhow::Result<()> {
    let project = &args.project.name;
    prompt::require(
        args.yes,
        &format!("Revoke credential `{project}`? Its tokens stop working within about a minute."),
    )?;

    let client = context::client(global).await?;
    let response = client.delete_credential(project).await?;

    out.emit(&response, || {
        println!("Revoked credential `{}`.", response.project);
    })
}

/// The `key` entries of `credential` that are active at `now`.
fn active_keys(credential: &Credential, now: OffsetDateTime) -> Vec<AuthEntry> {
    credential
        .auth
        .iter()
        .filter(|entry| entry.secret_type == SecretType::Key && entry.is_active(now))
        .cloned()
        .collect()
}

/// Parses `<N>s`, `<N>m`, `<N>h`, `<N>d` or `<N>w`.
fn parse_duration(value: &str) -> anyhow::Result<time::Duration> {
    let invalid = || {
        anyhow::anyhow!(
            "invalid duration `{value}`; expected a number followed by s, m, h, d or w, e.g. 14d"
        )
    };
    let value = value.trim();
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(invalid)?;
    let (number, unit) = value.split_at(split);
    let number: i64 = number.parse().map_err(|_| invalid())?;
    let seconds_per_unit = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        _ => return Err(invalid()),
    };
    let seconds = number.checked_mul(seconds_per_unit).ok_or_else(invalid)?;
    if seconds > 100 * 365 * 86_400 {
        return Err(invalid());
    }
    Ok(time::Duration::seconds(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("14d").unwrap(), time::Duration::days(14));
        assert_eq!(parse_duration("2w").unwrap(), time::Duration::weeks(2));
        assert_eq!(parse_duration("36h").unwrap(), time::Duration::hours(36));
        assert_eq!(parse_duration("90m").unwrap(), time::Duration::minutes(90));
        assert_eq!(parse_duration("0s").unwrap(), time::Duration::ZERO);
        for invalid in ["", "14", "d", "14x", "-1d", "1.5d", "99999999999w"] {
            assert!(parse_duration(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn active_keys_skip_expired_and_gh_entries() {
        let credential: Credential = serde_json::from_value(serde_json::json!({
            "project": "p1",
            "kind": "read",
            "auth": [
                { "id": "aaaaaaaaaaaa", "type": "key", "created": "2026-01-01T00:00:00Z", "expires": null },
                { "id": "bbbbbbbbbbbb", "type": "key", "created": "2026-01-01T00:00:00Z", "expires": "2026-01-02T00:00:00Z" },
                { "id": "cccccccccccc", "type": "gh", "repository": "acme/x", "ref": null, "workflow": null,
                  "created": "2026-01-01T00:00:00Z", "expires": null }
            ],
            "grants": []
        }))
        .unwrap();
        let now = parse_timestamp("2026-10-28T00:00:00Z").unwrap();

        let ids: Vec<String> = active_keys(&credential, now)
            .into_iter()
            .map(|entry| entry.id)
            .collect();

        assert_eq!(ids, ["aaaaaaaaaaaa"]);
    }
}
