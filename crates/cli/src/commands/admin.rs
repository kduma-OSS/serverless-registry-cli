//! `sreg yank` and `sreg delete` (admin).

use anyhow::bail;
use sreg_core::Ecosystem;

use crate::cli::{DeleteArgs, GlobalArgs, VersionArgs};
use crate::context;
use crate::output::Output;
use crate::prompt;

pub async fn yank(global: &GlobalArgs, out: &Output, args: &VersionArgs) -> anyhow::Result<()> {
    let eco = Ecosystem::from(args.eco);
    let client = context::client(global).await?;
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
            let client = context::client(global).await?;
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
            let client = context::client(global).await?;
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
