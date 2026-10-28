//! NuGet packaging for `sreg`.
//!
//! [`NugetPackager`] packs one project with `dotnet pack <project> -o <tmp>` (passing
//! the version to publish as `-p:PackageVersion=<v>`), reads the `.nuspec` from the
//! resulting `.nupkg` into the version's manifest and publishes the `.nupkg` as the
//! `archive` and the `.snupkg`, when one was produced, as `symbols`.

#![deny(unsafe_code)]

mod nuspec;
mod version;

use std::path::{Path, PathBuf};

use sreg_core::git::Git;
use sreg_core::{
    CommandRunner, Ecosystem, Error, Package, PackageFile, PackageOptions, Packager, SystemRunner,
    Workdir,
};

pub use nuspec::{read_from_nupkg, Dependency, DependencyGroup, Nuspec, ANY_RANGE};
pub use version::{normalize_version, validate_version};

/// Role of the symbols package of a NuGet version.
pub const SYMBOLS_ROLE: &str = "symbols";

/// Extensions of the project files `dotnet pack` packs into one package.
const PROJECT_EXTENSIONS: [&str; 3] = ["csproj", "fsproj", "vbproj"];

/// Extensions of solution files, which produce one package per project.
const SOLUTION_EXTENSIONS: [&str; 3] = ["sln", "slnx", "slnf"];

/// Packages NuGet projects for publication.
#[derive(Debug, Clone)]
pub struct NugetPackager<R = SystemRunner> {
    runner: R,
    project: Option<PathBuf>,
    symbols: bool,
}

impl Default for NugetPackager<SystemRunner> {
    fn default() -> Self {
        NugetPackager::new()
    }
}

impl NugetPackager<SystemRunner> {
    /// A packager running the system's `dotnet` (and `git`, for the release tag).
    pub fn new() -> Self {
        NugetPackager::with_runner(SystemRunner)
    }
}

impl<R: CommandRunner> NugetPackager<R> {
    /// A packager running `dotnet` and `git` through `runner`.
    pub fn with_runner(runner: R) -> Self {
        NugetPackager {
            runner,
            project: None,
            symbols: true,
        }
    }

    /// Packs this project file (relative to the project directory) instead of the
    /// single project file in the directory.
    pub fn project(mut self, project: Option<PathBuf>) -> Self {
        self.project = project;
        self
    }

    /// Whether to publish the `.snupkg` that `dotnet pack` produced (the default);
    /// `false` for `--no-symbols`.
    pub fn symbols(mut self, symbols: bool) -> Self {
        self.symbols = symbols;
        self
    }

    /// The project file to pack, as passed to `dotnet pack` (relative to `dir` when
    /// found there).
    fn project_file(&self, dir: &Path) -> Result<PathBuf, Error> {
        if let Some(project) = &self.project {
            let extension = extension(project);
            if SOLUTION_EXTENSIONS.contains(&extension.as_str()) {
                return Err(Error::InvalidInput(format!(
                    "`{}` is a solution, which produces several packages (multi-package \
                     output); one call publishes one package, so pass the project file of one \
                     package with --project",
                    project.display()
                )));
            }
            if !PROJECT_EXTENSIONS.contains(&extension.as_str()) {
                return Err(Error::InvalidInput(format!(
                    "`{}` is not a project file; pass a .csproj (or .fsproj, .vbproj) with \
                     --project",
                    project.display()
                )));
            }
            if !dir.join(project).is_file() {
                return Err(Error::InvalidInput(format!(
                    "the project file `{}` does not exist",
                    dir.join(project).display()
                )));
            }
            return Ok(project.clone());
        }

        let mut projects: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_file() && PROJECT_EXTENSIONS.contains(&extension(path).as_str()))
            .filter_map(|path| path.file_name().map(PathBuf::from))
            .collect();
        projects.sort();
        match projects.as_slice() {
            [project] => Ok(project.clone()),
            [] => Err(Error::InvalidInput(format!(
                "`{}` has no .csproj; run sreg in the project directory or pass --project",
                dir.display()
            ))),
            many => Err(Error::InvalidInput(format!(
                "`{}` has several project files ({}); select one with --project",
                dir.display(),
                many.iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }
}

impl<R: CommandRunner> Packager for NugetPackager<R> {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Nuget
    }

    /// Packs the project into a temporary directory owned by the package and reads the
    /// `.nuspec` of the produced `.nupkg`.
    ///
    /// The version is `--version`, else the release tag on `HEAD` (both passed as
    /// `-p:PackageVersion=<v>`), else whatever the project declares.
    fn package(&self, opts: &PackageOptions) -> Result<Package, Error> {
        check_extra_args(&opts.extra_args)?;
        if opts.tag.is_some() {
            return Err(Error::InvalidInput(
                "dist-tags exist only for npm packages".into(),
            ));
        }
        let dir = opts.path.as_path();
        let project = self.project_file(dir)?;
        let requested = Git::new(&self.runner, dir)
            .requested_version(opts.version.as_deref(), opts.tag_prefix.as_deref())?;
        if let Some(version) = &requested {
            validate_version(version)?;
        }

        let workdir = Workdir::new()?;
        let output = workdir.path().join("out");
        std::fs::create_dir(&output)?;
        let project_arg = utf8(&project)?;
        let output_arg = utf8(&output)?;
        let version_arg = requested
            .as_ref()
            .map(|version| format!("-p:PackageVersion={version}"));
        let mut args = vec!["pack", project_arg, "-o", output_arg];
        args.extend(version_arg.as_deref());
        args.extend(opts.extra_args.iter().map(String::as_str));
        let command = format!("dotnet {}", args.join(" "));
        self.runner
            .run("dotnet", &args, dir)
            .map_err(|err| match err {
                Error::Io(io) if io.kind() == std::io::ErrorKind::NotFound => Error::InvalidInput(
                    "`dotnet` was not found; install the .NET SDK to publish NuGet packages".into(),
                ),
                other => other,
            })?
            .into_success(&command)?;

        let (nupkg, snupkg) = produced_packages(&output)?;
        let nuspec = nuspec::read_from_nupkg(&nupkg)?;
        validate_id(&nuspec.id)?;
        validate_version(&nuspec.version)
            .map_err(|err| Error::InvalidInput(format!("the .nuspec of the package: {err}")))?;
        if let Some(requested) = &requested {
            if normalize_version(requested) != normalize_version(&nuspec.version) {
                return Err(Error::InvalidInput(format!(
                    "the .nupkg has version {}, but {requested} was requested",
                    nuspec.version
                )));
            }
        }

        // The registry requires `<id>.<version>.nupkg` in lower case.
        let base = format!(
            "{}.{}",
            nuspec.id.to_ascii_lowercase(),
            nuspec.version.to_ascii_lowercase()
        );
        let mut files = vec![PackageFile::archive(
            rename(&nupkg, &workdir, &format!("{base}.nupkg"))?,
            format!("{base}.nupkg"),
        )?];
        match snupkg {
            Some(snupkg) if self.symbols => {
                let name = format!("{base}.snupkg");
                files.push(PackageFile::from_path(
                    rename(&snupkg, &workdir, &name)?,
                    Some(name),
                    Some(SYMBOLS_ROLE.to_owned()),
                    Default::default(),
                )?);
            }
            Some(snupkg) => {
                tracing::debug!(file = %snupkg.display(), "--no-symbols: not publishing");
            }
            None => {}
        }

        Ok(Package {
            eco: Ecosystem::Nuget,
            name: nuspec.id.clone(),
            version: nuspec.version.clone(),
            files,
            manifest: nuspec.manifest(),
            workdir: Some(workdir),
            notes: Vec::new(),
        })
    }
}

/// The `.nupkg` and the optional `.snupkg` with the same base name that `dotnet pack`
/// wrote into `output`. Legacy `.symbols.nupkg` packages are ignored.
fn produced_packages(output: &Path) -> Result<(PathBuf, Option<PathBuf>), Error> {
    let mut nupkgs = Vec::new();
    let mut snupkgs = Vec::new();
    for entry in std::fs::read_dir(output)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if name.ends_with(".symbols.nupkg") {
            tracing::debug!(file = %path.display(), "ignoring a legacy symbols package");
        } else if name.ends_with(".nupkg") {
            nupkgs.push(path);
        } else if name.ends_with(".snupkg") {
            snupkgs.push(path);
        }
    }
    let nupkg = match nupkgs.as_slice() {
        [nupkg] => nupkg.clone(),
        [] => {
            return Err(Error::InvalidInput(
                "`dotnet pack` produced no .nupkg; check that the project is packable \
                 (IsPackable)"
                    .into(),
            ))
        }
        many => {
            let mut names: Vec<String> = many
                .iter()
                .filter_map(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .collect();
            names.sort();
            return Err(Error::InvalidInput(format!(
                "`dotnet pack` produced several packages ({}); one call publishes one package, \
                 so pass the project file of one package with --project",
                names.join(", ")
            )));
        }
    };
    let stem = |path: &Path| {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
    };
    let snupkg = snupkgs
        .into_iter()
        .find(|snupkg| stem(snupkg) == stem(&nupkg));
    Ok((nupkg, snupkg))
}

/// Moves `path` to `name` in the work directory.
fn rename(path: &Path, workdir: &Workdir, name: &str) -> Result<PathBuf, Error> {
    let target = workdir.path().join(name);
    std::fs::rename(path, &target)?;
    Ok(target)
}

/// Checks a package id: `[A-Za-z0-9_.-]{1,100}`.
pub fn validate_id(id: &str) -> Result<(), Error> {
    let valid = !id.is_empty()
        && id.len() <= 100
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "invalid NuGet package id `{id}`; expected 1 to 100 characters of A-Z, a-z, 0-9, \
             `_`, `.` and `-`"
        )))
    }
}

/// Rejects the flags `sreg` sets itself: the output directory (`-o`, `--output`) and
/// the `PackageVersion` property (`-p:PackageVersion=…` in any of MSBuild's spellings).
fn check_extra_args(args: &[String]) -> Result<(), Error> {
    let reserved = |arg: &str| {
        Error::InvalidInput(format!(
            "`{arg}` cannot be passed to `dotnet pack` after `--`: sreg sets the output \
             directory itself and passes the version with its own --version flag"
        ))
    };
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let (flag, inline) = match arg.split_once([':', '=']) {
            Some((flag, value)) => (flag, Some(value)),
            None => (arg.as_str(), None),
        };
        if matches!(flag, "-o" | "--output") {
            return Err(reserved(arg));
        }
        let property = matches!(
            flag.to_ascii_lowercase().as_str(),
            "-p" | "/p" | "--property" | "-property" | "/property"
        );
        if !property {
            continue;
        }
        let value = match inline {
            Some(value) => Some(value),
            None => iter.peek().map(|value| value.as_str()),
        };
        let sets_version = value.is_some_and(|value| {
            value.split([';', ',']).any(|assignment| {
                assignment
                    .split_once('=')
                    .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case("PackageVersion"))
            })
        });
        if sets_version {
            return Err(reserved(arg));
        }
    }
    Ok(())
}

fn extension(path: &Path) -> String {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn utf8(path: &Path) -> Result<&str, Error> {
    path.to_str().ok_or_else(|| {
        Error::InvalidInput(format!("the path `{}` is not valid UTF-8", path.display()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn reports_nuget_ecosystem() {
        assert_eq!(NugetPackager::new().ecosystem(), Ecosystem::Nuget);
    }

    #[test]
    fn reserved_flags_are_rejected() {
        for list in [
            &["-o", "out"][..],
            &["-o:out"],
            &["--output=out"],
            &["-p:PackageVersion=1.0.0"],
            &["/p:Configuration=Release;packageversion=1.0.0"],
            &["--property:PackageVersion=1.0.0"],
            &["-p", "PackageVersion=1.0.0"],
        ] {
            assert!(check_extra_args(&args(list)).is_err(), "{list:?}");
        }
        assert!(check_extra_args(&args(&[
            "-c",
            "Release",
            "--no-build",
            "-p:ContinuousIntegrationBuild=true",
            "--include-symbols",
            "-p",
            "Version=1.0.0",
        ]))
        .is_ok());
    }

    #[test]
    fn ids_follow_the_contract() {
        assert!(validate_id("Acme.Sdk_2-x").is_ok());
        for id in ["", "Acme Sdk", "Acme/Sdk", &"a".repeat(101)] {
            assert!(validate_id(id).is_err(), "{id}");
        }
    }
}
