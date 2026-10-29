//! `sreg login`: signs in through the browser and prints a short-lived token as an
//! `export` line, for several commands in a row (`eval "$(sreg login)"`).

use anyhow::{bail, Context};
use serde::Serialize;
use sreg_core::login::{LoginPurpose, PackageScope, PublishScope};
use sreg_core::models::Kind;
use sreg_core::Ecosystem;

use crate::cli::{GlobalArgs, LoginArgs};
use crate::commands::auth::shell_quote;
use crate::output::Output;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IssuedReport {
    token: String,
    expires_at: String,
    project: String,
    kind: Kind,
}

pub async fn run(global: &GlobalArgs, out: &Output, args: &LoginArgs) -> anyhow::Result<()> {
    let purpose = purpose(args)?;
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

/// A pull without options, otherwise a publish of the listed versions (`--publish`), a yank
/// (`--yank`) or a delete (`--delete`, `--delete-package`). How many the registry accepts is
/// its `MAX_PUBLISH_SCOPES` setting, so it answers for the count.
fn purpose(args: &LoginArgs) -> anyhow::Result<LoginPurpose> {
    if !args.yank.is_empty() {
        let scopes = args
            .yank
            .iter()
            .map(|spec| parse_scope(spec).map(PackageScope::from))
            .collect::<anyhow::Result<Vec<_>>>()?;
        return Ok(LoginPurpose::Yank { scopes });
    }
    if !args.delete.is_empty() || !args.delete_package.is_empty() {
        let versions = args
            .delete
            .iter()
            .map(|spec| parse_scope(spec).map(PackageScope::from));
        let packages = args.delete_package.iter().map(|spec| parse_package(spec));
        let scopes = versions
            .chain(packages)
            .collect::<anyhow::Result<Vec<_>>>()?;
        return Ok(LoginPurpose::Delete { scopes });
    }
    if args.publish.is_empty() {
        return Ok(LoginPurpose::Pull { eco: None });
    }
    let scopes = args
        .publish
        .iter()
        .map(|spec| parse_scope(spec))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(LoginPurpose::Publish { scopes })
}

/// Parses `<ecosystem>:<package>`, a whole package for `--delete-package`.
fn parse_package(spec: &str) -> anyhow::Result<PackageScope> {
    let (eco, package) = spec
        .split_once(':')
        .with_context(|| format!("`{spec}`: expected <ecosystem>:<package>"))?;
    if package.is_empty() {
        bail!("`{spec}`: the package must not be empty");
    }
    Ok(PackageScope {
        eco: parse_ecosystem(spec, eco)?,
        package: package.to_owned(),
        version: None,
    })
}

fn parse_ecosystem(spec: &str, eco: &str) -> anyhow::Result<Ecosystem> {
    Ecosystem::ALL
        .into_iter()
        .find(|candidate| candidate.as_str() == eco)
        .with_context(|| format!("`{spec}`: unknown ecosystem `{eco}`"))
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
    Ok(PublishScope {
        eco: parse_ecosystem(spec, eco)?,
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

    fn args(publish: &[&str], yank: &[&str], delete: &[&str], packages: &[&str]) -> LoginArgs {
        let owned = |list: &[&str]| list.iter().map(|spec| (*spec).to_owned()).collect();
        LoginArgs {
            publish: owned(publish),
            yank: owned(yank),
            delete: owned(delete),
            delete_package: owned(packages),
        }
    }

    #[test]
    fn no_flag_asks_for_a_pull() {
        let none = args(&[], &[], &[], &[]);
        assert_eq!(purpose(&none).unwrap(), LoginPurpose::Pull { eco: None });
    }

    #[test]
    fn publish_does_not_limit_the_count() {
        // The limit is the registry's to enforce, and an operator can raise it.
        let many: Vec<String> = (0..21).map(|i| format!("npm:p@1.0.{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(matches!(
            purpose(&args(&many, &[], &[], &[])).unwrap(),
            LoginPurpose::Publish { scopes } if scopes.len() == 21
        ));
    }

    #[test]
    fn yank_asks_for_the_listed_versions() {
        let purpose = purpose(&args(&[], &["npm:@acme/sdk@2.3.1"], &[], &[])).unwrap();
        assert_eq!(
            purpose,
            LoginPurpose::Yank {
                scopes: vec![PackageScope {
                    eco: Ecosystem::Npm,
                    package: "@acme/sdk".into(),
                    version: Some("2.3.1".into()),
                }]
            }
        );
    }

    #[test]
    fn delete_mixes_versions_and_whole_packages() {
        let purpose = purpose(&args(
            &[],
            &[],
            &["cargo:acme-core@1.4.2"],
            &["maven:org.acme:lib"],
        ))
        .unwrap();
        let LoginPurpose::Delete { scopes } = purpose else {
            panic!("expected a delete");
        };
        assert_eq!(scopes.len(), 2);
        assert_eq!(scopes[0].version.as_deref(), Some("1.4.2"));
        assert_eq!(scopes[1].package, "org.acme:lib");
        assert_eq!(scopes[1].version, None);
    }

    #[test]
    fn rejects_malformed_package_specs() {
        for spec in ["acme", "npm:", "rpm:acme"] {
            assert!(parse_package(spec).is_err(), "{spec}");
        }
    }
}
