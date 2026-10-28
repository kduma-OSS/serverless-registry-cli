//! `sreg yank` and `sreg delete` (admin).

use anyhow::bail;
use sreg_core::login::{LoginPurpose, PackageScope};
use sreg_core::{Client, Ecosystem};

use crate::cli::{DeleteArgs, GlobalArgs, OidcArgs, VersionArgs};
use crate::context;
use crate::output::Output;
use crate::prompt;

pub async fn yank(global: &GlobalArgs, out: &Output, args: &VersionArgs) -> anyhow::Result<()> {
    let eco = Ecosystem::from(args.eco);
    let purpose = LoginPurpose::Yank {
        scopes: vec![PackageScope {
            eco,
            package: args.name.clone(),
            version: Some(args.version.clone()),
        }],
    };
    let client = client(global, &args.oidc, purpose).await?;
    let response = client.yank(eco, &args.name, &args.version).await?;

    out.emit(&response, || {
        println!(
            "Yanked {} {} {}; it is no longer offered for new installs.",
            response.eco, response.name, response.version.version
        );
    })
}

pub async fn delete(global: &GlobalArgs, out: &Output, args: &DeleteArgs) -> anyhow::Result<()> {
    let eco = Ecosystem::from(args.eco);
    let name = &args.name;

    match &args.version {
        Some(version) => {
            prompt::require(
                args.yes,
                &format!("Delete {eco} {name} {version} and its archive?"),
            )?;
            let purpose = LoginPurpose::Delete {
                scopes: vec![PackageScope {
                    eco,
                    package: name.clone(),
                    version: Some(version.clone()),
                }],
            };
            let client = client(global, &args.oidc, purpose).await?;
            let response = client.delete_version(eco, name, version).await?;
            out.emit(&response, || {
                println!(
                    "Deleted {} {} {}; the version can be published again.",
                    response.eco, response.name, response.version
                );
            })
        }
        None => {
            if !args.yes
                && !prompt::confirm_by_typing(
                    &format!("Delete {eco} package {name} with every version and archive?"),
                    name,
                )?
            {
                bail!("aborted");
            }
            let purpose = LoginPurpose::Delete {
                scopes: vec![PackageScope {
                    eco,
                    package: name.clone(),
                    version: None,
                }],
            };
            let client = client(global, &args.oidc, purpose).await?;
            let response = client.delete_package(eco, name).await?;
            out.emit(&response, || {
                let versions = &response.deleted_versions;
                if versions.is_empty() {
                    println!("Deleted {} {}.", response.eco, response.name);
                } else {
                    println!(
                        "Deleted {} {} with {} version(s): {}.",
                        response.eco,
                        response.name,
                        versions.len(),
                        versions.join(", ")
                    );
                }
            })
        }
    }
}

/// The client for an administrative call: signed in through the browser for `purpose` with
/// `--login`, authenticated with GitHub Actions OIDC with `--oidc`, otherwise with the token.
async fn client(
    global: &GlobalArgs,
    oidc: &OidcArgs,
    purpose: LoginPurpose,
) -> anyhow::Result<Client> {
    if oidc.login {
        context::client_with_login(global, &purpose).await
    } else {
        context::client_with(global, oidc.oidc, oidc.project.as_deref()).await
    }
}
