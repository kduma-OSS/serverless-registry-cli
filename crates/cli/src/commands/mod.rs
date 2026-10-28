//! Command implementations.

mod auth;
mod version;
mod whoami;

use crate::cli::{Cli, Command};
use crate::output::Output;

/// Runs the parsed command.
pub async fn run(cli: &Cli) -> anyhow::Result<()> {
    let out = Output::new(cli.global.json);
    let global = &cli.global;
    match &cli.command {
        Command::Whoami => whoami::run(global, &out).await,
        Command::Version => version::run(global, &out).await,
        Command::Auth(args) => auth::run(global, &out, args).await,
        other => anyhow::bail!("`sreg {}` is not implemented yet", other.name()),
    }
}
