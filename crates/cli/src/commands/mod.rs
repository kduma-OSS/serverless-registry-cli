//! Command implementations.

mod auth;
mod generic;
mod publish;
mod version;
mod whoami;

use crate::cli::{
    CargoCommand, Cli, Command, ComposerCommand, GenericCommand, NpmCommand, NugetCommand,
};
use crate::output::Output;

/// Runs the parsed command.
pub async fn run(cli: &Cli) -> anyhow::Result<()> {
    let out = Output::new(cli.global.json);
    let global = &cli.global;
    tracing::debug!(command = cli.command.name(), "running command");
    match &cli.command {
        Command::Whoami => whoami::run(global, &out).await,
        Command::Version => version::run(global, &out).await,
        Command::Auth(args) => auth::run(global, &out, args).await,
        Command::Composer {
            command: ComposerCommand::Publish(args),
        } => {
            let packager = sreg_eco_composer::ComposerPackager;
            publish::run_packager(global, &out, &packager, &args.publish, None, Vec::new()).await
        }
        Command::Npm {
            command: NpmCommand::Publish(args),
        } => {
            let packager = sreg_eco_npm::NpmPackager;
            publish::run_packager(
                global,
                &out,
                &packager,
                &args.publish,
                args.tag.clone(),
                Vec::new(),
            )
            .await
        }
        Command::Cargo {
            command: CargoCommand::Publish(args),
        } => {
            let packager = sreg_eco_cargo::CargoPackager;
            publish::run_packager(
                global,
                &out,
                &packager,
                &args.publish,
                None,
                args.cargo_args.clone(),
            )
            .await
        }
        Command::Generic {
            command: GenericCommand::Publish(args),
        } => generic::publish(global, &out, args).await,
        Command::Generic {
            command: GenericCommand::Get(args),
        } => generic::get(global, &out, args).await,
        Command::Nuget {
            command: NugetCommand::Publish(args),
        } => {
            let packager = sreg_eco_nuget::NugetPackager;
            publish::run_packager(global, &out, &packager, &args.publish, None, Vec::new()).await
        }
    }
}
