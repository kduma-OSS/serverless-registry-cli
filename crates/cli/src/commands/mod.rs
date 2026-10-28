//! Command implementations.

mod auth;
#[cfg(feature = "generic")]
mod generic;
mod nuget_config;
#[cfg(any(
    feature = "composer",
    feature = "npm",
    feature = "cargo",
    feature = "nuget",
    feature = "generic",
    feature = "swift",
    feature = "maven"
))]
mod publish;
mod version;
mod whoami;

#[cfg(feature = "cargo")]
use crate::cli::CargoCommand;
#[cfg(feature = "composer")]
use crate::cli::ComposerCommand;
#[cfg(feature = "generic")]
use crate::cli::GenericCommand;
#[cfg(feature = "maven")]
use crate::cli::MavenCommand;
#[cfg(feature = "npm")]
use crate::cli::NpmCommand;
#[cfg(feature = "nuget")]
use crate::cli::NugetCommand;
#[cfg(feature = "swift")]
use crate::cli::SwiftCommand;
use crate::cli::{Cli, Command};
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
        #[cfg(feature = "composer")]
        Command::Composer {
            command: ComposerCommand::Publish(args),
        } => {
            let packager = sreg_eco_composer::ComposerPackager::new();
            publish::run_packager(
                global,
                &out,
                &packager,
                &args.publish,
                &args.path,
                None,
                &args.passthrough,
            )
            .await
        }
        #[cfg(feature = "npm")]
        Command::Npm {
            command: NpmCommand::Publish(args),
        } => {
            let packager = sreg_eco_npm::NpmPackager::new().workspace(args.workspace.clone());
            publish::run_packager(
                global,
                &out,
                &packager,
                &args.publish,
                std::path::Path::new("."),
                args.tag.clone(),
                &args.npm_args,
            )
            .await
        }
        #[cfg(feature = "cargo")]
        Command::Cargo {
            command: CargoCommand::Publish(args),
        } => {
            let packager = sreg_eco_cargo::CargoPackager::new().package_name(args.package.clone());
            publish::run_packager(
                global,
                &out,
                &packager,
                &args.publish,
                std::path::Path::new("."),
                None,
                &args.cargo_args,
            )
            .await
        }
        #[cfg(feature = "swift")]
        Command::Swift {
            command: SwiftCommand::Publish(args),
        } => {
            let packager = sreg_eco_swift::SwiftPackager::new();
            publish::run_packager(
                global,
                &out,
                &packager,
                &args.publish,
                &args.path,
                None,
                &[],
            )
            .await
        }
        #[cfg(feature = "maven")]
        Command::Maven {
            command: MavenCommand::Publish(args),
        } => {
            // The coordinates (`args.coords`) reach the packager once it is implemented.
            let packager = sreg_eco_maven::MavenPackager;
            publish::run_packager(
                global,
                &out,
                &packager,
                &args.publish,
                &args.from_dir,
                None,
                &[],
            )
            .await
        }
        #[cfg(feature = "generic")]
        Command::Generic {
            command: GenericCommand::Publish(args),
        } => generic::publish(global, &out, args).await,
        #[cfg(feature = "generic")]
        Command::Generic {
            command: GenericCommand::Get(args),
        } => generic::get(global, &out, args).await,
        #[cfg(feature = "nuget")]
        Command::Nuget {
            command: NugetCommand::Publish(args),
        } => {
            let packager = sreg_eco_nuget::NugetPackager::new()
                .project(args.project.clone())
                .symbols(!args.no_symbols);
            publish::run_packager(
                global,
                &out,
                &packager,
                &args.publish,
                std::path::Path::new("."),
                None,
                &args.dotnet_args,
            )
            .await
        }
    }
}
