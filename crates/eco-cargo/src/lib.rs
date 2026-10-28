//! cargo packaging for `sreg`.
//!
//! [`CargoPackager`] finds the crate with `cargo metadata`, writes the version to
//! publish into its `Cargo.toml` (a formatting-preserving edit) when one is requested,
//! packages it with `cargo package --no-verify --allow-dirty` and describes it with one
//! line of the cargo sparse index built from `cargo metadata`.

#![deny(unsafe_code)]

mod edit;
mod index;
mod metadata;

use std::path::{Path, PathBuf};

use sreg_core::git::Git;
use sreg_core::{
    CommandRunner, Ecosystem, Error, Package, PackageFile, PackageOptions, Packager, SystemRunner,
    Workdir,
};

pub use edit::set_package_version;
pub use index::{index_line, IndexDependency, CRATES_IO_INDEX};
pub use metadata::{Metadata, MetadataDependency, MetadataPackage};

/// Flags `sreg` passes to `cargo package` itself, or that would package more than
/// one crate; they are rejected after `--`.
const RESERVED_FLAGS: [&str; 7] = [
    "--no-verify",
    "--allow-dirty",
    "-p",
    "--package",
    "--target-dir",
    "--workspace",
    "--all",
];

/// Packages cargo crates for publication.
#[derive(Debug, Clone, Default)]
pub struct CargoPackager<R = SystemRunner> {
    runner: R,
    package: Option<String>,
}

impl CargoPackager<SystemRunner> {
    /// A packager running the system's `cargo` (and `git`, for the release tag).
    pub fn new() -> Self {
        CargoPackager::with_runner(SystemRunner)
    }
}

impl<R: CommandRunner> CargoPackager<R> {
    /// A packager running `cargo` and `git` through `runner`.
    pub fn with_runner(runner: R) -> Self {
        CargoPackager {
            runner,
            package: None,
        }
    }

    /// Publishes the workspace member `package` (`cargo package -p <package>`).
    pub fn package_name(mut self, package: Option<String>) -> Self {
        self.package = package;
        self
    }

    fn run_cargo(&self, args: &[&str], dir: &Path) -> Result<Vec<u8>, Error> {
        let command = format!("cargo {}", args.join(" "));
        let output = self
            .runner
            .run("cargo", args, dir)
            .map_err(|err| match err {
                Error::Io(io) if io.kind() == std::io::ErrorKind::NotFound => Error::InvalidInput(
                    "`cargo` was not found; install Rust to publish crates".into(),
                ),
                other => other,
            })?;
        Ok(output.into_success(&command)?.stdout)
    }

    fn metadata(&self, dir: &Path, manifest_path: Option<&str>) -> Result<Metadata, Error> {
        let mut args = vec!["metadata", "--format-version", "1", "--no-deps"];
        if let Some(manifest_path) = manifest_path {
            args.extend(["--manifest-path", manifest_path]);
        }
        Metadata::parse(&self.run_cargo(&args, dir)?)
    }
}

impl<R: CommandRunner> Packager for CargoPackager<R> {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Cargo
    }

    /// Packages the crate selected in the workspace around `opts.path` into a `.crate`
    /// in a temporary directory owned by the package.
    ///
    /// The version is `--version`, else the release tag on `HEAD`, else the version in
    /// `Cargo.toml`; when it differs from the crate's version, it is written into the
    /// crate's `Cargo.toml` first, so that the `.crate` carries the version being
    /// published.
    fn package(&self, opts: &PackageOptions) -> Result<Package, Error> {
        check_extra_args(&opts.extra_args)?;
        if opts.tag.is_some() {
            return Err(Error::InvalidInput(
                "dist-tags exist only for npm packages".into(),
            ));
        }
        let dir = opts.path.as_path();
        let manifest_path = manifest_path_arg(&opts.extra_args);
        let current_manifest = match manifest_path {
            Some(path) => Some(dir.join(path)),
            None => metadata::nearest_manifest(dir),
        };

        let metadata = self.metadata(dir, manifest_path)?;
        let selected = metadata.select(self.package.as_deref(), current_manifest.as_deref())?;
        let name = selected.name.clone();
        let requested = Git::new(&self.runner, dir)
            .requested_version(opts.version.as_deref(), opts.tag_prefix.as_deref())?;

        let mut notes = Vec::new();
        let mut package = selected.clone();
        if let Some(version) = &requested {
            validate_version(version)?;
            if *version != package.version {
                write_version(&package.manifest_path, version)?;
                notes.push(format!(
                    "set the version in {} to {version} (was {})",
                    package.manifest_path.display(),
                    package.version
                ));
                let metadata = self.metadata(dir, manifest_path)?;
                package = metadata.select(Some(&name), None)?.clone();
                if package.version != *version {
                    return Err(Error::InvalidInput(format!(
                        "`cargo metadata` still reports version {} for `{name}` after writing \
                         {version} into {}",
                        package.version,
                        package.manifest_path.display()
                    )));
                }
            }
        }
        let version = package.version.clone();
        validate_version(&version)?;

        let workdir = Workdir::new()?;
        let target_dir = workdir.path().join("target");
        let target_dir_arg = target_dir.to_str().ok_or_else(|| {
            Error::InvalidInput("the temporary directory path is not valid UTF-8".into())
        })?;
        let mut args = vec![
            "package",
            "--no-verify",
            "--allow-dirty",
            "-p",
            name.as_str(),
            "--target-dir",
            target_dir_arg,
        ];
        args.extend(opts.extra_args.iter().map(String::as_str));
        self.run_cargo(&args, dir)?;

        let file_name = format!("{name}-{version}.crate");
        let crate_path: PathBuf = target_dir.join("package").join(&file_name);
        if !crate_path.is_file() {
            return Err(Error::InvalidInput(format!(
                "`cargo package` did not produce {}",
                crate_path.display()
            )));
        }
        let file = PackageFile::archive(&crate_path, file_name)?;
        Ok(Package {
            eco: Ecosystem::Cargo,
            manifest: index_line(&package, &version),
            name,
            version,
            files: vec![file],
            workdir: Some(workdir),
            notes,
        })
    }
}

/// Checks a version: semver without a leading `v` (`1.2.3`, `2.0.0-rc.1`).
pub fn validate_version(version: &str) -> Result<(), Error> {
    semver::Version::parse(version).map(|_| ()).map_err(|err| {
        Error::InvalidInput(format!(
            "invalid version `{version}`: {err}; expected semver such as 1.2.3 or 2.0.0-rc.1, \
             without a leading `v`"
        ))
    })
}

fn write_version(manifest_path: &Path, version: &str) -> Result<(), Error> {
    let text = std::fs::read_to_string(manifest_path)?;
    let edited = set_package_version(&text, version).map_err(|err| match err {
        Error::InvalidInput(message) => {
            Error::InvalidInput(format!("{}: {message}", manifest_path.display()))
        }
        other => other,
    })?;
    std::fs::write(manifest_path, edited)?;
    Ok(())
}

/// Rejects the flags `sreg` sets itself, in both the `--flag value` and the
/// `--flag=value` form (and `-p<name>`).
fn check_extra_args(args: &[String]) -> Result<(), Error> {
    for arg in args {
        let flag = arg.split_once('=').map_or(arg.as_str(), |(flag, _)| flag);
        let short_package = arg.starts_with("-p") && !arg.starts_with("--");
        if RESERVED_FLAGS.contains(&flag) || short_package {
            return Err(Error::InvalidInput(format!(
                "`{arg}` cannot be passed to `cargo package` after `--`: sreg sets \
                 --no-verify, --allow-dirty and --target-dir itself, and packages one crate \
                 selected with its own --package flag"
            )));
        }
    }
    Ok(())
}

/// The value of `--manifest-path` among the arguments for `cargo package`, which
/// `cargo metadata` must see too.
fn manifest_path_arg(args: &[String]) -> Option<&str> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--manifest-path" {
            return args.next().map(String::as_str);
        }
        if let Some(path) = arg.strip_prefix("--manifest-path=") {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_cargo_ecosystem() {
        assert_eq!(CargoPackager::new().ecosystem(), Ecosystem::Cargo);
    }

    #[test]
    fn reserved_flags_are_rejected() {
        for arg in [
            "--no-verify",
            "--allow-dirty",
            "-p",
            "-pfoo",
            "--package=foo",
            "--target-dir",
            "--target-dir=/x",
            "--workspace",
            "--all",
        ] {
            assert!(check_extra_args(&[arg.to_owned()]).is_err(), "{arg}");
        }
        let allowed: Vec<String> = ["--features", "ssl", "--locked", "--manifest-path", "a"]
            .iter()
            .map(|arg| (*arg).to_owned())
            .collect();
        assert!(check_extra_args(&allowed).is_ok());
    }

    #[test]
    fn finds_the_manifest_path_argument() {
        let args =
            |list: &[&str]| -> Vec<String> { list.iter().map(|arg| (*arg).to_owned()).collect() };
        assert_eq!(
            manifest_path_arg(&args(&["--locked", "--manifest-path", "a/Cargo.toml"])),
            Some("a/Cargo.toml")
        );
        assert_eq!(
            manifest_path_arg(&args(&["--manifest-path=b/Cargo.toml"])),
            Some("b/Cargo.toml")
        );
        assert_eq!(manifest_path_arg(&args(&["--locked"])), None);
    }

    #[test]
    fn versions_are_semver() {
        assert!(validate_version("1.2.3-rc.1").is_ok());
        assert!(validate_version("v1.2.3").is_err());
        assert!(validate_version("1.2").is_err());
    }
}
