//! npm packaging for `sreg`.
//!
//! [`NpmPackager`] writes the version to publish into `package.json` with
//! `npm pkg set version=<v>` (when one is requested), packs the project with
//! `npm pack --json --pack-destination <tmp>` and reads `package/package.json` from the
//! resulting tarball, which becomes the version's manifest.

#![deny(unsafe_code)]

mod tarball;
mod validate;

use std::path::Path;

use serde_json::Value;
use sreg_core::git::Git;
use sreg_core::{
    ArchiveOptions, CommandRunner, Ecosystem, Error, Package, PackageFile, PackageOptions,
    Packager, SystemRunner, Workdir,
};

pub use validate::{validate_name, validate_tag, validate_version};

/// Flags `sreg` passes to `npm pack` itself; they are rejected after `--`.
const RESERVED_FLAGS: [&str; 6] = [
    "--json",
    "--pack-destination",
    "-w",
    "--workspace",
    "--workspaces",
    "-ws",
];

/// Packages npm projects for publication.
#[derive(Debug, Clone, Default)]
pub struct NpmPackager<R = SystemRunner> {
    runner: R,
    workspace: Option<String>,
}

impl NpmPackager<SystemRunner> {
    /// A packager running the system's `npm` (and `git`, for the release tag).
    pub fn new() -> Self {
        NpmPackager::with_runner(SystemRunner)
    }
}

impl<R: CommandRunner> NpmPackager<R> {
    /// A packager running `npm` and `git` through `runner`.
    pub fn with_runner(runner: R) -> Self {
        NpmPackager {
            runner,
            workspace: None,
        }
    }

    /// Packs the workspace `workspace` (a name or a path, as accepted by
    /// `npm pack -w`) instead of the project in the root directory.
    pub fn workspace(mut self, workspace: Option<String>) -> Self {
        self.workspace = workspace;
        self
    }

    /// `-w <workspace>` when a workspace is selected.
    fn workspace_args(&self) -> Vec<&str> {
        match &self.workspace {
            Some(workspace) => vec!["-w", workspace.as_str()],
            None => Vec::new(),
        }
    }

    fn run_npm(&self, args: &[&str], dir: &Path) -> Result<Vec<u8>, Error> {
        let command = format!("npm {}", args.join(" "));
        let output = self.runner.run("npm", args, dir).map_err(|err| match err {
            Error::Io(io) if io.kind() == std::io::ErrorKind::NotFound => Error::InvalidInput(
                "`npm` was not found; install Node.js and npm to publish npm packages".into(),
            ),
            other => other,
        })?;
        Ok(output.into_success(&command)?.stdout)
    }
}

impl<R: CommandRunner> Packager for NpmPackager<R> {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Npm
    }

    fn package_archive(&self, opts: &ArchiveOptions) -> Result<Package, Error> {
        self.package_tarball(opts)
    }

    /// Packs the project in `opts.path` (the root of the npm project or of the
    /// workspaces) into a tarball in a temporary directory owned by the package.
    ///
    /// The version is `--version`, else the release tag on `HEAD`, else the version in
    /// `package.json`; the first two are written into `package.json` with
    /// `npm pkg set version=<v>` before packing, so that the tarball carries the version
    /// being published.
    fn package(&self, opts: &PackageOptions) -> Result<Package, Error> {
        check_extra_args(&opts.extra_args)?;
        if let Some(tag) = &opts.tag {
            validate_tag(tag)?;
        }
        if let Some(workspace) = &self.workspace {
            if workspace.trim().is_empty() {
                return Err(Error::InvalidInput("--workspace must not be empty".into()));
            }
        }
        let dir = opts.path.as_path();
        if !dir.join("package.json").is_file() {
            return Err(Error::InvalidInput(format!(
                "`{}` has no package.json; run sreg in the root of the npm project (select a \
                 workspace with --workspace)",
                dir.display()
            )));
        }

        let requested = Git::new(&self.runner, dir)
            .requested_version(opts.version.as_deref(), opts.tag_prefix.as_deref())?;
        let mut notes = Vec::new();
        if let Some(version) = &requested {
            validate_version(version)?;
            let assignment = format!("version={version}");
            let mut args = vec!["pkg", "set", assignment.as_str()];
            args.extend(self.workspace_args());
            self.run_npm(&args, dir)?;
            notes.push(match &self.workspace {
                Some(workspace) => format!(
                    "set the version in package.json of workspace `{workspace}` to {version} \
                     (npm pkg set)"
                ),
                None => format!(
                    "set the version in {} to {version} (npm pkg set)",
                    dir.join("package.json").display()
                ),
            });
        }

        let workdir = Workdir::new()?;
        let destination = workdir.path().to_str().ok_or_else(|| {
            Error::InvalidInput("the temporary directory path is not valid UTF-8".into())
        })?;
        let mut args = vec!["pack", "--json", "--pack-destination", destination];
        args.extend(self.workspace_args());
        args.extend(opts.extra_args.iter().map(String::as_str));
        let stdout = self.run_npm(&args, dir)?;
        let packed = tarball::parse_pack_output(&stdout)?;

        let archive_path = workdir.path().join(&packed.filename);
        if !archive_path.is_file() {
            return Err(Error::InvalidInput(format!(
                "`npm pack` reported `{}`, but the file was not created",
                packed.filename
            )));
        }
        let manifest = tarball::read_package_json(&archive_path)?;
        let (name, version) = manifest_identity(&manifest)?;
        validate_name(&name)?;
        validate_version(&version)?;
        if packed.name.as_deref().is_some_and(|packed| packed != name)
            || packed
                .version
                .as_deref()
                .is_some_and(|packed| packed != version)
        {
            return Err(Error::InvalidInput(format!(
                "`npm pack` reported {}@{}, but the tarball contains {name}@{version}",
                packed.name.as_deref().unwrap_or("?"),
                packed.version.as_deref().unwrap_or("?")
            )));
        }
        if let Some(requested) = &requested {
            if requested != &version {
                return Err(Error::InvalidInput(format!(
                    "the tarball contains version {version}, but {requested} was requested; \
                     check that --workspace selects the package that was changed"
                )));
            }
        }

        let file = PackageFile::archive(&archive_path, packed.filename)?;
        Ok(Package {
            eco: Ecosystem::Npm,
            name,
            version,
            files: vec![file],
            manifest: strip_private_fields(manifest),
            workdir: Some(workdir),
            notes,
        })
    }
}

impl<R: CommandRunner> NpmPackager<R> {
    fn package_tarball(&self, opts: &ArchiveOptions) -> Result<Package, Error> {
        if self.workspace.is_some() {
            return Err(Error::InvalidInput(
                "--workspace selects a project to pack and cannot be combined with --archive"
                    .into(),
            ));
        }
        let manifest = tarball::read_package_json(&opts.path)?;
        let (name, version) = manifest_identity(&manifest)?;
        validate_name(&name)?;
        validate_version(&version)?;
        if let Some(requested) = &opts.version {
            if requested != &version {
                return Err(Error::InvalidInput(format!(
                    "the tarball contains {name}@{version}, but --version {requested} was given"
                )));
            }
        }
        let file = PackageFile::archive(&opts.path, archive_name(&name, &version))?;
        Ok(Package {
            eco: Ecosystem::Npm,
            name,
            version,
            files: vec![file],
            manifest: strip_private_fields(manifest),
            workdir: None,
            notes: Vec::new(),
        })
    }
}

/// The file name `npm pack` gives a tarball: `@acme/demo` 1.2.3 → `acme-demo-1.2.3.tgz`.
fn archive_name(name: &str, version: &str) -> String {
    format!(
        "{}-{version}.tgz",
        name.trim_start_matches('@').replace('/', "-")
    )
}

/// Rejects the flags `sreg` sets itself, in both the `--flag value` and the
/// `--flag=value` form.
fn check_extra_args(args: &[String]) -> Result<(), Error> {
    for arg in args {
        let flag = arg.split_once('=').map_or(arg.as_str(), |(flag, _)| flag);
        if RESERVED_FLAGS.contains(&flag) {
            return Err(Error::InvalidInput(format!(
                "`{arg}` cannot be passed to `npm pack` after `--`: sreg sets --json and \
                 --pack-destination itself, and selects one workspace with its own \
                 --workspace flag"
            )));
        }
    }
    Ok(())
}

/// `name` and `version` of the tarball's `package.json`.
fn manifest_identity(manifest: &Value) -> Result<(String, String), Error> {
    let field = |key: &str| {
        manifest
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                Error::InvalidInput(format!("package.json in the tarball has no `{key}` string"))
            })
    };
    Ok((field("name")?, field("version")?))
}

/// Drops the fields starting with `_`, which npm reserves for registry metadata.
fn strip_private_fields(manifest: Value) -> Value {
    match manifest {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(key, _)| !key.starts_with('_'))
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reports_npm_ecosystem() {
        assert_eq!(NpmPackager::new().ecosystem(), Ecosystem::Npm);
    }

    #[test]
    fn reserved_flags_are_rejected_in_both_forms() {
        for arg in [
            "--json",
            "--json=false",
            "--pack-destination",
            "--pack-destination=/x",
            "-w",
            "--workspace=a",
            "--workspaces",
            "-ws",
        ] {
            let err = check_extra_args(&[arg.to_owned()]).unwrap_err();
            assert!(err.to_string().contains("cannot be passed"), "{arg}: {err}");
        }
        assert!(check_extra_args(&["--ignore-scripts".to_owned(), "--dry".to_owned()]).is_ok());
    }

    #[test]
    fn archive_names_follow_npm_pack() {
        assert_eq!(archive_name("@acme/demo", "1.2.3"), "acme-demo-1.2.3.tgz");
        assert_eq!(
            archive_name("@a/b.js", "2.0.0-rc.1"),
            "a-b.js-2.0.0-rc.1.tgz"
        );
    }

    #[test]
    fn private_fields_are_dropped() {
        let manifest = json!({ "name": "@a/b", "_id": "x", "_resolved": "y", "main": "i.js" });
        assert_eq!(
            strip_private_fields(manifest),
            json!({ "name": "@a/b", "main": "i.js" })
        );
    }

    #[test]
    fn identity_needs_string_name_and_version() {
        assert!(manifest_identity(&json!({ "name": "@a/b" })).is_err());
        assert!(manifest_identity(&json!({ "name": "@a/b", "version": 1 })).is_err());
        assert_eq!(
            manifest_identity(&json!({ "name": "@a/b", "version": "1.0.0" })).unwrap(),
            ("@a/b".to_owned(), "1.0.0".to_owned())
        );
    }
}
