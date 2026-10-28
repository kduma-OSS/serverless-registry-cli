//! The `sreg` command tree.

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use sreg_core::Ecosystem;
use url::Url;

/// Publish packages to Serverless Registry and manage its credentials.
#[derive(Parser)]
#[command(name = "sreg", version, about, long_about = None)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

/// Options accepted by every command.
#[derive(Args)]
#[command(next_help_heading = "Global options")]
pub struct GlobalArgs {
    /// Base URL of the registry, e.g. https://packages.example.com
    #[arg(long, env = "REGISTRY_URL", value_name = "URL", global = true)]
    pub registry: Option<Url>,

    /// Registry token ([<source>.]<project>:[<type>.]<secret>)
    #[arg(
        long,
        env = "REGISTRY_TOKEN",
        hide_env_values = true,
        value_name = "TOKEN",
        global = true
    )]
    pub token: Option<String>,

    /// Print machine-readable JSON output only
    #[arg(long, global = true)]
    pub json: bool,

    /// Increase log verbosity (-v for debug, -vv for trace); logs go to stderr
    #[arg(short, long, action = ArgAction::Count, global = true)]
    pub verbose: u8,
}

#[derive(Subcommand)]
pub enum Command {
    /// Package and publish Composer packages
    Composer {
        #[command(subcommand)]
        command: ComposerCommand,
    },

    /// Package and publish npm packages
    Npm {
        #[command(subcommand)]
        command: NpmCommand,
    },

    /// Package and publish cargo crates
    Cargo {
        #[command(subcommand)]
        command: CargoCommand,
    },

    /// Show the identity and grants of the current token
    Whoami,

    /// Manage credentials (requires an admin token)
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },

    /// Yank a version so it is no longer offered for new installs (admin)
    Yank(VersionArgs),

    /// Delete a version, or with --all a whole package, from the registry (admin)
    Delete(DeleteArgs),

    /// Write registry credentials for a package manager or print them for export
    Auth(AuthArgs),

    /// Show the version of sreg and, with --registry, of the registry
    Version,
}

impl Command {
    /// The full name of the command, e.g. `composer publish`.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Composer {
                command: ComposerCommand::Publish(_),
            } => "composer publish",
            Command::Npm {
                command: NpmCommand::Publish(_),
            } => "npm publish",
            Command::Cargo {
                command: CargoCommand::Publish(_),
            } => "cargo publish",
            Command::Whoami => "whoami",
            Command::Token { command } => match command {
                TokenCommand::Create(_) => "token create",
                TokenCommand::Rotate(_) => "token rotate",
                TokenCommand::List => "token list",
                TokenCommand::Revoke(_) => "token revoke",
            },
            Command::Yank(_) => "yank",
            Command::Delete(_) => "delete",
            Command::Auth(_) => "auth",
            Command::Version => "version",
        }
    }
}

/// Options shared by all `publish` commands.
#[derive(Args)]
pub struct PublishArgs {
    /// Directory of the project to publish
    #[arg(long, value_name = "DIR", default_value = ".")]
    pub path: PathBuf,

    /// Stop after the registry has accepted the upload declaration (`init`)
    #[arg(long)]
    pub dry_run: bool,

    /// Authenticate with a GitHub Actions OIDC token instead of REGISTRY_TOKEN
    #[arg(long)]
    pub oidc: bool,

    /// Project the OIDC token is issued for
    #[arg(long, value_name = "PROJECT", requires = "oidc")]
    pub project: Option<String>,
}

#[derive(Subcommand)]
pub enum ComposerCommand {
    /// Archive the project and publish it
    Publish(ComposerPublishArgs),
}

#[derive(Args)]
pub struct ComposerPublishArgs {
    #[command(flatten)]
    pub publish: PublishArgs,

    /// Version to publish (defaults to the git tag of HEAD, e.g. v1.2.3 -> 1.2.3)
    #[arg(long = "version", id = "package_version", value_name = "VERSION")]
    pub version: Option<String>,
}

#[derive(Subcommand)]
pub enum NpmCommand {
    /// Pack the project with `npm pack` and publish it
    Publish(NpmPublishArgs),
}

#[derive(Args)]
pub struct NpmPublishArgs {
    #[command(flatten)]
    pub publish: PublishArgs,

    /// Distribution tag (defaults to `latest`, or `next` for prereleases)
    #[arg(long, value_name = "TAG")]
    pub tag: Option<String>,
}

#[derive(Subcommand)]
pub enum CargoCommand {
    /// Package the crate with `cargo package` and publish it
    Publish(CargoPublishArgs),
}

#[derive(Args)]
pub struct CargoPublishArgs {
    #[command(flatten)]
    pub publish: PublishArgs,

    /// Extra arguments passed to `cargo package`
    #[arg(last = true, value_name = "CARGO_ARGS")]
    pub cargo_args: Vec<String>,
}

#[derive(Subcommand)]
pub enum TokenCommand {
    /// Create a credential and print its token once
    Create(TokenCreateArgs),

    /// Issue a new secret for a credential and let the old one expire
    Rotate(TokenRotateArgs),

    /// List credentials
    List,

    /// Revoke a credential
    Revoke(TokenRevokeArgs),
}

#[derive(Args)]
pub struct TokenCreateArgs {
    #[command(flatten)]
    pub project: ProjectArgs,

    /// Role of the credential
    #[arg(long, value_enum)]
    pub kind: CredentialKind,

    /// Grant in the form <ECO>:<PACKAGE>[:<VERSIONS>], e.g. composer:acme/*:* (repeatable;
    /// VERSIONS defaults to *)
    #[arg(long = "grant", value_name = "GRANT")]
    pub grants: Vec<String>,

    /// Expiry of the first secret (RFC 3339 timestamp or YYYY-MM-DD); never by default
    #[arg(long, value_name = "DATE")]
    pub expires: Option<String>,
}

#[derive(Args)]
pub struct TokenRotateArgs {
    #[command(flatten)]
    pub project: ProjectArgs,

    /// Let the previously active key secrets expire after this period
    /// (<N>s, <N>m, <N>h, <N>d or <N>w)
    #[arg(long, value_name = "DURATION", default_value = "14d")]
    pub expire_old: String,
}

#[derive(Args)]
pub struct TokenRevokeArgs {
    #[command(flatten)]
    pub project: ProjectArgs,

    /// Do not ask for confirmation
    #[arg(long, short = 'y')]
    pub yes: bool,
}

#[derive(Args)]
pub struct ProjectArgs {
    /// Project (credential) name
    #[arg(long = "project", value_name = "PROJECT")]
    pub name: String,
}

/// Role of a credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum CredentialKind {
    /// Install packages (consumers)
    Read,
    /// Publish packages (package CI)
    Publish,
    /// Administer the registry
    Admin,
}

#[derive(Args)]
pub struct VersionArgs {
    /// Ecosystem of the package
    #[arg(value_enum)]
    pub eco: EcosystemArg,

    /// Package name
    pub name: String,

    /// Version
    pub version: String,
}

#[derive(Args)]
pub struct DeleteArgs {
    /// Ecosystem of the package
    #[arg(value_enum)]
    pub eco: EcosystemArg,

    /// Package name
    pub name: String,

    /// Version to delete
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    pub version: Option<String>,

    /// Delete the whole package with every version
    #[arg(long)]
    pub all: bool,

    /// Do not ask for confirmation
    #[arg(long, short = 'y')]
    pub yes: bool,
}

#[derive(Args)]
pub struct AuthArgs {
    /// Package manager to configure
    #[arg(value_enum)]
    pub eco: EcosystemArg,

    /// Build the token from a GitHub Actions OIDC token instead of REGISTRY_TOKEN
    #[arg(long)]
    pub oidc: bool,

    /// Project the OIDC token is issued for
    #[arg(long, value_name = "PROJECT", requires = "oidc")]
    pub project: Option<String>,

    /// Write the credentials to this file (auth.json, .npmrc or credentials.toml)
    /// instead of printing them; an existing file is updated in place
    #[arg(long, value_name = "FILE")]
    pub write: Option<PathBuf>,

    /// Name of the registry in cargo's configuration (cargo only)
    #[arg(long, value_name = "NAME", default_value = "sreg")]
    pub registry_name: String,
}

/// Ecosystem selector for positional arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum EcosystemArg {
    Composer,
    Npm,
    Cargo,
}

impl From<EcosystemArg> for Ecosystem {
    fn from(eco: EcosystemArg) -> Self {
        match eco {
            EcosystemArg::Composer => Ecosystem::Composer,
            EcosystemArg::Npm => Ecosystem::Npm,
            EcosystemArg::Cargo => Ecosystem::Cargo,
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn command_tree_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn composer_publish_accepts_version() {
        let cli =
            Cli::try_parse_from(["sreg", "composer", "publish", "--version", "1.2.3"]).unwrap();
        let Command::Composer {
            command: ComposerCommand::Publish(args),
        } = cli.command
        else {
            panic!("expected composer publish");
        };
        assert_eq!(args.version.as_deref(), Some("1.2.3"));
        assert_eq!(args.publish.path, PathBuf::from("."));
    }

    #[test]
    fn cargo_publish_passes_trailing_arguments() {
        let cli =
            Cli::try_parse_from(["sreg", "cargo", "publish", "--", "--features", "x"]).unwrap();
        let Command::Cargo {
            command: CargoCommand::Publish(args),
        } = cli.command
        else {
            panic!("expected cargo publish");
        };
        assert_eq!(args.cargo_args, ["--features", "x"]);
    }

    #[test]
    fn global_flags_are_accepted_after_subcommands() {
        let cli = Cli::try_parse_from([
            "sreg",
            "whoami",
            "--registry",
            "https://packages.example.com",
            "--json",
            "-vv",
        ])
        .unwrap();
        assert_eq!(cli.command.name(), "whoami");
        assert!(cli.global.json);
        assert_eq!(cli.global.verbose, 2);
        assert_eq!(
            cli.global.registry.map(String::from).as_deref(),
            Some("https://packages.example.com/")
        );
    }

    #[test]
    fn yank_parses_positional_arguments() {
        let cli = Cli::try_parse_from(["sreg", "yank", "npm", "@acme/sdk", "2.3.1"]).unwrap();
        let Command::Yank(args) = cli.command else {
            panic!("expected yank");
        };
        assert_eq!(Ecosystem::from(args.eco), Ecosystem::Npm);
        assert_eq!(args.name, "@acme/sdk");
        assert_eq!(args.version, "2.3.1");
    }

    #[test]
    fn token_create_collects_grants() {
        let cli = Cli::try_parse_from([
            "sreg",
            "token",
            "create",
            "--project",
            "shop-frontend",
            "--kind",
            "read",
            "--grant",
            "composer:acme/*:*",
            "--grant",
            "npm:@acme/sdk:^2.0",
        ])
        .unwrap();
        let Command::Token {
            command: TokenCommand::Create(args),
        } = cli.command
        else {
            panic!("expected token create");
        };
        assert_eq!(args.project.name, "shop-frontend");
        assert_eq!(args.kind, CredentialKind::Read);
        assert_eq!(args.grants, ["composer:acme/*:*", "npm:@acme/sdk:^2.0"]);
    }

    #[test]
    fn delete_requires_a_version_or_all() {
        assert!(Cli::try_parse_from(["sreg", "delete", "npm", "@acme/sdk"]).is_err());
        assert!(
            Cli::try_parse_from(["sreg", "delete", "npm", "@acme/sdk", "1.0.0", "--all"]).is_err()
        );
        let cli =
            Cli::try_parse_from(["sreg", "delete", "npm", "@acme/sdk", "--all", "--yes"]).unwrap();
        let Command::Delete(args) = cli.command else {
            panic!("expected delete");
        };
        assert!(args.all && args.yes);
        assert_eq!(args.version, None);
    }

    #[test]
    fn token_rotate_defaults_to_fourteen_days() {
        let cli = Cli::try_parse_from(["sreg", "token", "rotate", "--project", "p1"]).unwrap();
        let Command::Token {
            command: TokenCommand::Rotate(args),
        } = cli.command
        else {
            panic!("expected token rotate");
        };
        assert_eq!(args.expire_old, "14d");
    }

    #[test]
    fn version_is_a_subcommand() {
        let cli = Cli::try_parse_from(["sreg", "version"]).unwrap();
        assert_eq!(cli.command.name(), "version");
    }

    #[test]
    fn project_requires_oidc() {
        assert!(Cli::try_parse_from(["sreg", "auth", "composer", "--project", "p"]).is_err());
        assert!(
            Cli::try_parse_from(["sreg", "auth", "composer", "--oidc", "--project", "p"]).is_ok()
        );
    }
}
