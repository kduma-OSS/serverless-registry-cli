//! `sreg`: the command-line tool of Serverless Registry.

#![deny(unsafe_code)]

// Parsed arguments are consumed as the commands get implemented. `allow` rather than
// `expect`: whether rustc reports these fields as unread differs between versions.
#[allow(dead_code)]
mod cli;
mod exit;

use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::filter::LevelFilter;

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

    match run(&cli) {
        Ok(()) => ExitCode::from(exit::SUCCESS),
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(exit::code_for(&err))
        }
    }
}

fn init_logging(verbose: u8) {
    let level = match verbose {
        0 => LevelFilter::WARN,
        1 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

fn run(cli: &Cli) -> anyhow::Result<()> {
    let command = cli.command.name();
    tracing::debug!(command, "parsed command line");
    anyhow::bail!("`sreg {command}` is not implemented yet")
}
