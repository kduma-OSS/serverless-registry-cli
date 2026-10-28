//! `sreg login`: signs in through the browser and prints a short-lived token as an
//! `export` line, for several commands in a row (`eval "$(sreg login)"`).

use anyhow::{bail, Context};
use serde::Serialize;
use sreg_core::login::{LoginPurpose, PublishScope};
use sreg_core::models::Kind;
use sreg_core::Ecosystem;

use crate::cli::{GlobalArgs, LoginArgs};
use crate::commands::auth::shell_quote;
use crate::output::Output;

/// Most versions one publish token can name (the registry's limit).
const MAX_SCOPES: usize = 20;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IssuedReport {
    token: String,
    expires_at: String,
    project: String,
    kind: Kind,
}

pub async fn run(global: &GlobalArgs, out: &Output, args: &LoginArgs) -> anyhow::Result<()> {
    let purpose = purpose(&args.publish)?;
    let issued = crate::login::acquire(global, &purpose).await?;
    let report = IssuedReport {
        token: issued.token,
        expires_at: issued.expires_at,
        project: issued.project,
        kind: issued.kind,
    };
    out.emit(&report, || {
        println!("export REGISTRY_TOKEN={}", shell_quote(&report.token))
    })
}

/// A pull without `--publish`, otherwise a publish of the listed versions.
fn purpose(publish: &[String]) -> anyhow::Result<LoginPurpose> {
    if publish.is_empty() {
        return Ok(LoginPurpose::Pull { eco: None });
    }
    if publish.len() > MAX_SCOPES {
        bail!("a token can name at most {MAX_SCOPES} package versions");
    }
    let scopes = publish
        .iter()
        .map(|spec| parse_scope(spec))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(LoginPurpose::Publish { scopes })
}

/// Parses `<ecosystem>:<package>@<version>`. The ecosystem ends at the first `:` (a
/// Maven package is `group:artifact`) and the version starts after the last `@` (an npm
/// scope starts with one).
fn parse_scope(spec: &str) -> anyhow::Result<PublishScope> {
    let (eco, rest) = spec
        .split_once(':')
        .with_context(|| format!("`{spec}`: expected <ecosystem>:<package>@<version>"))?;
    let (package, version) = rest
        .rsplit_once('@')
        .with_context(|| format!("`{spec}`: expected <ecosystem>:<package>@<version>"))?;
    if package.is_empty() || version.is_empty() {
        bail!("`{spec}`: the package and the version must not be empty");
    }
    let eco = Ecosystem::ALL
        .into_iter()
        .find(|candidate| candidate.as_str() == eco)
        .with_context(|| format!("`{spec}`: unknown ecosystem `{eco}`"))?;
    Ok(PublishScope {
        eco,
        package: package.to_owned(),
        version: version.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(eco: Ecosystem, package: &str, version: &str) -> PublishScope {
        PublishScope {
            eco,
            package: package.into(),
            version: version.into(),
        }
    }

    #[test]
    fn parses_the_forms_of_every_ecosystem() {
        let cases = [
            (
                "composer:acme/sdk@1.4.0",
                scope(Ecosystem::Composer, "acme/sdk", "1.4.0"),
            ),
            (
                "npm:@acme/sdk@2.0.0-beta.1",
                scope(Ecosystem::Npm, "@acme/sdk", "2.0.0-beta.1"),
            ),
            (
                "maven:org.acme:lib@1.0",
                scope(Ecosystem::Maven, "org.acme:lib", "1.0"),
            ),
            (
                "generic:tools/cli@0.3.0",
                scope(Ecosystem::Generic, "tools/cli", "0.3.0"),
            ),
        ];
        for (spec, expected) in cases {
            assert_eq!(parse_scope(spec).unwrap(), expected, "{spec}");
        }
    }

    #[test]
    fn rejects_malformed_specs() {
        for spec in [
            "acme/sdk@1.0.0",
            "composer:acme/sdk",
            "composer:@1.0.0",
            "composer:acme/sdk@",
            "rpm:acme@1.0.0",
        ] {
            assert!(parse_scope(spec).is_err(), "{spec}");
        }
    }

    #[test]
    fn no_publish_flag_asks_for_a_pull() {
        assert_eq!(purpose(&[]).unwrap(), LoginPurpose::Pull { eco: None });
        let publish = purpose(&["npm:left-pad@1.0.0".to_owned()]).unwrap();
        assert!(matches!(publish, LoginPurpose::Publish { scopes } if scopes.len() == 1));
        let many: Vec<String> = (0..21).map(|i| format!("npm:p@1.0.{i}")).collect();
        assert!(purpose(&many).is_err());
    }
}
