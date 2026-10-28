//! The `sreg` command tree.

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use sreg_core::Ecosystem;
use url::Url;

/// Publish packages to Serverless Registry and configure package managers for it.
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
    #[cfg(feature = "composer")]
    Composer {
        #[command(subcommand)]
        command: ComposerCommand,
    },

    /// Package and publish npm packages
    #[cfg(feature = "npm")]
    Npm {
        #[command(subcommand)]
        command: NpmCommand,
    },

    /// Package and publish cargo crates
    #[cfg(feature = "cargo")]
    Cargo {
        #[command(subcommand)]
        command: CargoCommand,
    },

    /// Package and publish NuGet packages
    #[cfg(feature = "nuget")]
    Nuget {
        #[command(subcommand)]
        command: NugetCommand,
    },

    /// Publish and download generic files
    #[cfg(feature = "generic")]
    Generic {
        #[command(subcommand)]
        command: GenericCommand,
    },

    /// Show the identity and grants of the current token
    Whoami,

    /// Write registry credentials for a package manager or print them for export
    Auth(AuthArgs),

    /// Show the version of sreg and, with --registry, of the registry
    Version,
}

impl Command {
    /// The full name of the command, e.g. `composer publish`.
    pub fn name(&self) -> &'static str {
        match self {
            #[cfg(feature = "composer")]
            Command::Composer {
                command: ComposerCommand::Publish(_),
            } => "composer publish",
            #[cfg(feature = "npm")]
            Command::Npm {
                command: NpmCommand::Publish(_),
            } => "npm publish",
            #[cfg(feature = "cargo")]
            Command::Cargo {
                command: CargoCommand::Publish(_),
            } => "cargo publish",
            #[cfg(feature = "nuget")]
            Command::Nuget {
                command: NugetCommand::Publish(_),
            } => "nuget publish",
            #[cfg(feature = "generic")]
            Command::Generic {
                command: GenericCommand::Publish(_),
            } => "generic publish",
            #[cfg(feature = "generic")]
            Command::Generic {
                command: GenericCommand::Get(_),
            } => "generic get",
            Command::Whoami => "whoami",
            Command::Auth(_) => "auth",
            Command::Version => "version",
        }
    }
}

/// Authentication through GitHub Actions OIDC instead of REGISTRY_TOKEN.
#[cfg(any(
    feature = "composer",
    feature = "npm",
    feature = "cargo",
    feature = "nuget",
    feature = "generic"
))]
#[derive(Args)]
pub struct OidcArgs {
    /// Authenticate with a GitHub Actions OIDC token instead of REGISTRY_TOKEN
    #[arg(long)]
    pub oidc: bool,

    /// Project the OIDC token is issued for (requires --oidc)
    #[arg(long = "oidc-project", value_name = "PROJECT", requires = "oidc")]
    pub project: Option<String>,
}

/// Options shared by the `publish` commands of composer, npm, cargo and nuget.
#[cfg(any(
    feature = "composer",
    feature = "npm",
    feature = "cargo",
    feature = "nuget"
))]
#[derive(Args)]
pub struct PublishArgs {
    /// Version to publish (composer: instead of the release tag on HEAD, e.g. v1.2.3 ->
    /// 1.2.3; npm, cargo and nuget: written into the manifest before packaging)
    #[arg(long = "version", id = "package_version", value_name = "VERSION")]
    pub version: Option<String>,

    /// Take the version from the tag on HEAD that starts with this prefix, e.g. `foo/v`
    /// for foo/v1.2.3 (default without --version: tags of the form v<semver>)
    #[arg(long, value_name = "PREFIX", conflicts_with = "package_version")]
    pub tag_prefix: Option<String>,

    /// Stop after the registry has accepted the upload declaration (`init`)
    #[arg(long)]
    pub dry_run: bool,

    /// Exit with 0 instead of 2 when the version is already published (409)
    #[arg(long)]
    pub skip_existing: bool,

    #[command(flatten)]
    pub oidc: OidcArgs,
}

#[cfg(feature = "composer")]
#[derive(Subcommand)]
pub enum ComposerCommand {
    /// Archive the project and publish it
    Publish(ComposerPublishArgs),
}

#[cfg(feature = "composer")]
#[derive(Args)]
pub struct ComposerPublishArgs {
    /// Directory of the package to publish
    #[arg(long, value_name = "DIR", default_value = ".")]
    pub path: PathBuf,

    #[command(flatten)]
    pub publish: PublishArgs,

    /// Composer packaging runs no external tool; arguments after `--` are rejected
    #[arg(last = true, hide = true, value_name = "ARGS")]
    pub passthrough: Vec<std::ffi::OsString>,
}

#[cfg(feature = "npm")]
#[derive(Subcommand)]
pub enum NpmCommand {
    /// Pack the project with `npm pack` and publish it
    Publish(NpmPublishArgs),
}

#[cfg(feature = "npm")]
#[derive(Args)]
pub struct NpmPublishArgs {
    #[command(flatten)]
    pub publish: PublishArgs,

    /// Distribution tag (defaults to `latest`, or `next` for prereleases)
    #[arg(long, value_name = "TAG")]
    pub tag: Option<String>,

    /// Publish this workspace (a name or a path) of the project in the current
    /// directory: `npm pack -w <WORKSPACE>`
    #[arg(long, value_name = "WORKSPACE")]
    pub workspace: Option<String>,

    /// Extra arguments passed verbatim to `npm pack`
    #[arg(last = true, value_name = "NPM_ARGS")]
    pub npm_args: Vec<std::ffi::OsString>,
}

#[cfg(feature = "cargo")]
#[derive(Subcommand)]
pub enum CargoCommand {
    /// Package the crate with `cargo package` and publish it
    Publish(CargoPublishArgs),
}

#[cfg(feature = "cargo")]
#[derive(Args)]
pub struct CargoPublishArgs {
    #[command(flatten)]
    pub publish: PublishArgs,

    /// Publish this member of the workspace around the current directory:
    /// `cargo package -p <CRATE>` (default: the crate of the current directory, or the
    /// only crate of the workspace)
    #[arg(long, value_name = "CRATE")]
    pub package: Option<String>,

    /// Extra arguments passed verbatim to `cargo package`
    #[arg(last = true, value_name = "CARGO_ARGS")]
    pub cargo_args: Vec<std::ffi::OsString>,
}

#[cfg(feature = "nuget")]
#[derive(Subcommand)]
pub enum NugetCommand {
    /// Pack the project with `dotnet pack` and publish it
    Publish(NugetPublishArgs),
}

#[cfg(feature = "nuget")]
#[derive(Args)]
pub struct NugetPublishArgs {
    #[command(flatten)]
    pub publish: PublishArgs,

    /// Project file to pack: `dotnet pack <PROJECT>` (default: the single .csproj in
    /// the current directory; solutions are refused)
    #[arg(long = "project", id = "nuget_project", value_name = "PROJECT")]
    pub project: Option<PathBuf>,

    /// Do not publish the .snupkg symbols package that `dotnet pack` produced
    #[arg(long)]
    pub no_symbols: bool,

    /// Extra arguments passed verbatim to `dotnet pack`
    #[arg(last = true, value_name = "DOTNET_ARGS")]
    pub dotnet_args: Vec<std::ffi::OsString>,
}

#[cfg(feature = "generic")]
#[derive(Subcommand)]
pub enum GenericCommand {
    /// Publish files as one version of a generic package
    Publish(GenericPublishArgs),

    /// Download the files of a generic package version
    Get(GenericGetArgs),
}

#[cfg(feature = "generic")]
#[derive(Args)]
pub struct GenericPublishArgs {
    /// Package name: [<group>/]<name>, lower case
    pub name: String,

    /// Version to publish (semver, e.g. 1.2.0 or 1.3.0-rc.1)
    #[arg(id = "package_version", value_name = "VERSION")]
    pub version: String,

    /// Files to publish; each one is published under its file name
    #[arg(
        value_name = "FILE",
        required_unless_present = "spec",
        conflicts_with = "spec"
    )]
    pub files: Vec<PathBuf>,

    /// Label applied to every file, e.g. os=linux (repeatable)
    #[arg(long = "label", value_name = "KEY=VALUE", conflicts_with = "spec")]
    pub labels: Vec<String>,

    /// Metadata entry of the version, as a string (repeatable)
    #[arg(long = "meta", value_name = "KEY=VALUE")]
    pub meta: Vec<String>,

    /// JSON object with the metadata of the version; --meta entries override its keys
    #[arg(long, value_name = "FILE")]
    pub meta_file: Option<PathBuf>,

    /// JSON file listing the files with their own labels:
    /// [{"path": "...", "labels": {"os": "linux"}}, ...]
    #[arg(long, value_name = "FILE")]
    pub spec: Option<PathBuf>,

    /// Require a new version instead of adding the files to an already published one
    /// (by default files are appended within the registry's append window)
    #[arg(long)]
    pub no_append: bool,

    /// Stop after the registry has accepted the upload declaration (`init`)
    #[arg(long)]
    pub dry_run: bool,

    /// Exit with 0 instead of 2 when the version is already published (409)
    #[arg(long)]
    pub skip_existing: bool,

    /// Generic publishing runs no external tool; arguments after `--` are rejected
    #[arg(last = true, hide = true, value_name = "ARGS")]
    pub passthrough: Vec<std::ffi::OsString>,

    #[command(flatten)]
    pub oidc: OidcArgs,
}

#[cfg(feature = "generic")]
#[derive(Args)]
pub struct GenericGetArgs {
    /// Package name: [<group>/]<name>
    pub name: String,

    /// Version to download, or `latest`
    #[arg(
        id = "package_version",
        value_name = "VERSION",
        default_value = "latest"
    )]
    pub version: String,

    /// Only files with this label, e.g. os=linux (repeatable; all must match)
    #[arg(long = "label", value_name = "KEY=VALUE")]
    pub labels: Vec<String>,

    /// Directory to write the files to
    #[arg(long, value_name = "DIR", default_value = ".")]
    pub out: PathBuf,

    /// With `latest`: consider prerelease versions too
    #[arg(long)]
    pub prerelease: bool,

    #[command(flatten)]
    pub oidc: OidcArgs,
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
    Nuget,
}

impl From<EcosystemArg> for Ecosystem {
    fn from(eco: EcosystemArg) -> Self {
        match eco {
            EcosystemArg::Composer => Ecosystem::Composer,
            EcosystemArg::Npm => Ecosystem::Npm,
            EcosystemArg::Cargo => Ecosystem::Cargo,
            EcosystemArg::Nuget => Ecosystem::Nuget,
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

    #[cfg(feature = "composer")]
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
        assert_eq!(args.publish.version.as_deref(), Some("1.2.3"));
        assert_eq!(args.path, PathBuf::from("."));
    }

    #[cfg(all(
        feature = "composer",
        feature = "npm",
        feature = "cargo",
        feature = "nuget"
    ))]
    #[test]
    fn every_publish_command_accepts_the_common_flags() {
        for eco in ["composer", "npm", "cargo", "nuget"] {
            let cli = Cli::try_parse_from([
                "sreg",
                eco,
                "publish",
                "--version",
                "2.0.0",
                "--dry-run",
                "--oidc",
                "--oidc-project",
                "ci-acme",
            ])
            .unwrap();
            assert_eq!(cli.command.name(), format!("{eco} publish"));
            // The OIDC project is --oidc-project; --project names the .csproj of nuget.
            let project =
                Cli::try_parse_from(["sreg", eco, "publish", "--oidc", "--project", "ci-acme"]);
            assert_eq!(project.is_ok(), eco == "nuget", "{eco}");
        }
        assert!(Cli::try_parse_from(["sreg", "cargo", "publish", "--tag", "next"]).is_err());
        // Only composer packs a directory itself; the others work in the current
        // directory and select the package with their own flags.
        assert!(Cli::try_parse_from(["sreg", "composer", "publish", "--path", "pkg"]).is_ok());
        for eco in ["npm", "cargo", "nuget"] {
            assert!(Cli::try_parse_from(["sreg", eco, "publish", "--path", "pkg"]).is_err());
        }
    }

    #[cfg(feature = "composer")]
    #[test]
    fn tag_prefix_conflicts_with_version() {
        let cli =
            Cli::try_parse_from(["sreg", "composer", "publish", "--tag-prefix", "sdk/v"]).unwrap();
        let Command::Composer {
            command: ComposerCommand::Publish(args),
        } = cli.command
        else {
            panic!("expected composer publish");
        };
        assert_eq!(args.publish.tag_prefix.as_deref(), Some("sdk/v"));
        assert!(Cli::try_parse_from([
            "sreg",
            "composer",
            "publish",
            "--tag-prefix",
            "sdk/v",
            "--version",
            "1.0.0"
        ])
        .is_err());
    }

    #[cfg(feature = "cargo")]
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
        assert_eq!(args.package, None);

        let cli =
            Cli::try_parse_from(["sreg", "cargo", "publish", "--package", "acme-core"]).unwrap();
        let Command::Cargo {
            command: CargoCommand::Publish(args),
        } = cli.command
        else {
            panic!("expected cargo publish");
        };
        assert_eq!(args.package.as_deref(), Some("acme-core"));
    }

    #[cfg(all(feature = "npm", feature = "nuget"))]
    #[test]
    fn npm_and_nuget_publish_pass_trailing_arguments() {
        let cli =
            Cli::try_parse_from(["sreg", "npm", "publish", "--", "--workspace", "a"]).unwrap();
        let Command::Npm {
            command: NpmCommand::Publish(args),
        } = cli.command
        else {
            panic!("expected npm publish");
        };
        assert_eq!(args.npm_args, ["--workspace", "a"]);
        assert_eq!(args.workspace, None);

        let cli = Cli::try_parse_from([
            "sreg",
            "npm",
            "publish",
            "--workspace",
            "packages/sdk",
            "--tag",
            "beta",
        ])
        .unwrap();
        let Command::Npm {
            command: NpmCommand::Publish(args),
        } = cli.command
        else {
            panic!("expected npm publish");
        };
        assert_eq!(args.workspace.as_deref(), Some("packages/sdk"));
        assert_eq!(args.tag.as_deref(), Some("beta"));

        let cli = Cli::try_parse_from([
            "sreg",
            "nuget",
            "publish",
            "--version",
            "1.0.0",
            "--",
            "-c",
            "Release",
        ])
        .unwrap();
        let Command::Nuget {
            command: NugetCommand::Publish(args),
        } = cli.command
        else {
            panic!("expected nuget publish");
        };
        assert_eq!(args.dotnet_args, ["-c", "Release"]);
        assert_eq!(args.project, None);
        assert!(!args.no_symbols);

        let cli = Cli::try_parse_from([
            "sreg",
            "nuget",
            "publish",
            "--project",
            "src/Acme.Sdk/Acme.Sdk.csproj",
            "--no-symbols",
            "--oidc",
            "--oidc-project",
            "ci-acme",
        ])
        .unwrap();
        let Command::Nuget {
            command: NugetCommand::Publish(args),
        } = cli.command
        else {
            panic!("expected nuget publish");
        };
        assert_eq!(
            args.project,
            Some(PathBuf::from("src/Acme.Sdk/Acme.Sdk.csproj"))
        );
        assert!(args.no_symbols);
        assert_eq!(args.publish.oidc.project.as_deref(), Some("ci-acme"));
        assert!(Cli::try_parse_from(["sreg", "nuget", "publish", "--tag", "next"]).is_err());
    }

    #[cfg(feature = "generic")]
    #[test]
    fn generic_publish_keeps_files_and_trailing_arguments_apart() {
        let cli = Cli::try_parse_from([
            "sreg", "generic", "publish", "cli", "1.0.0", "a.zip", "b.zip", "--", "--x",
        ])
        .unwrap();
        let Command::Generic {
            command: GenericCommand::Publish(args),
        } = cli.command
        else {
            panic!("expected generic publish");
        };
        assert_eq!(args.files, [PathBuf::from("a.zip"), PathBuf::from("b.zip")]);
        assert_eq!(args.passthrough, ["--x"]);
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

    #[cfg(feature = "generic")]
    #[test]
    fn generic_publish_takes_files_or_a_spec() {
        let cli = Cli::try_parse_from([
            "sreg",
            "generic",
            "publish",
            "tools/cli",
            "1.0.0",
            "a.zip",
            "b.zip",
            "--label",
            "os=linux",
            "--meta",
            "commit=abc",
        ])
        .unwrap();
        let Command::Generic {
            command: GenericCommand::Publish(args),
        } = cli.command
        else {
            panic!("expected generic publish");
        };
        assert_eq!(args.version, "1.0.0");
        assert_eq!(args.files, [PathBuf::from("a.zip"), PathBuf::from("b.zip")]);
        assert_eq!(args.labels, ["os=linux"]);
        assert!(!args.no_append);

        assert!(Cli::try_parse_from(["sreg", "generic", "publish", "cli", "1.0.0"]).is_err());
        assert!(Cli::try_parse_from([
            "sreg", "generic", "publish", "cli", "1.0.0", "a.zip", "--spec", "s.json"
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "sreg", "generic", "publish", "cli", "1.0.0", "--spec", "s.json", "--label", "a=b"
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "sreg", "generic", "publish", "cli", "1.0.0", "--spec", "s.json"
        ])
        .is_ok());
    }

    #[cfg(feature = "generic")]
    #[test]
    fn generic_get_defaults_to_latest_in_the_current_directory() {
        let cli = Cli::try_parse_from(["sreg", "generic", "get", "tools/cli"]).unwrap();
        let Command::Generic {
            command: GenericCommand::Get(args),
        } = cli.command
        else {
            panic!("expected generic get");
        };
        assert_eq!(args.version, "latest");
        assert_eq!(args.out, PathBuf::from("."));
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
