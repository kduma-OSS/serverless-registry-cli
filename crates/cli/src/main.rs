//! `sreg`: the command-line tool of Serverless Registry.

#![deny(unsafe_code)]

mod cli;
mod commands;
mod context;
mod exit;
mod login;
mod output;

use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::prelude::*;

use crate::cli::Cli;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            // Help and version output go to stdout and succeed. Usage errors must not
            // use clap's default code 2, which is reserved for "version exists".
            let code = if err.use_stderr() {
                exit::FAILURE
            } else {
                exit::SUCCESS
            };
            // Nothing sensible is left to do if printing the message fails.
            let _ = err.print();
            return ExitCode::from(code);
        }
    };

    init_logging(cli.global.verbose);

    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|runtime| runtime.block_on(commands::run(&cli)));

    match result {
        Ok(()) => ExitCode::from(exit::SUCCESS),
        Err(err) => {
            eprintln!("error: {err:#}");
            if let Some(hint) = exit::hint_for(&err) {
                eprintln!("hint: {hint}");
            }
            ExitCode::from(exit::code_for(&err))
        }
    }
}

/// Logs go to stderr. `-v` and `-vv` raise the level of sreg's own crates only, so
/// third-party crates never log request details such as presigned URLs.
fn init_logging(verbose: u8) {
    let level = match verbose {
        0 => LevelFilter::WARN,
        1 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };
    let filter = Targets::new()
        .with_default(LevelFilter::WARN)
        .with_target("sreg", level)
        .with_target("sreg_core", level)
        .with_target("sreg_eco_composer", level)
        .with_target("sreg_eco_npm", level)
        .with_target("sreg_eco_cargo", level)
        .with_target("sreg_eco_nuget", level)
        .with_target("sreg_eco_generic", level)
        .with_target("sreg_eco_swift", level)
        .with_target("sreg_eco_maven", level);
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(std::io::stderr),
        )
        .with(filter)
        .init();
}
