//! Composer packaging for `sreg`.
//!
//! [`ComposerPackager`] reads `composer.json`, picks the version (from `--version` or
//! the release tag on `HEAD`) and archives the package directory as a zip: with
//! `git archive HEAD:<path>`, which honours `export-ignore` in `.gitattributes`, or,
//! outside a git repository, by zipping the directory without `vendor/` and `.git/`.

#![deny(unsafe_code)]

mod archive;
mod validate;

use std::path::Path;

use serde_json::Value;
use sreg_core::git::Git;
use sreg_core::{
    ArchiveOptions, CommandRunner, Ecosystem, Error, Package, PackageFile, PackageOptions,
    Packager, SystemRunner, Workdir,
};

pub use validate::{validate_name, validate_version};

/// Packages Composer libraries for publication.
#[derive(Debug, Clone, Default)]
pub struct ComposerPackager<R = SystemRunner> {
    runner: R,
}

impl ComposerPackager<SystemRunner> {
    /// A packager running the system's `git`.
    pub fn new() -> Self {
        ComposerPackager {
            runner: SystemRunner,
        }
    }
}

impl<R: CommandRunner> ComposerPackager<R> {
    /// A packager running `git` through `runner`.
    pub fn with_runner(runner: R) -> Self {
        ComposerPackager { runner }
    }
}

impl<R: CommandRunner> Packager for ComposerPackager<R> {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Composer
    }

    /// Describes a zip built earlier from the `composer.json` inside it.
    ///
    /// The version is the one `composer.json` declares; `--version` must agree with it,
    /// or supplies it when `composer.json` has none. Nothing is rewritten.
    fn package_archive(&self, opts: &ArchiveOptions) -> Result<Package, Error> {
        let manifest = archive::read_prebuilt_composer_json(&opts.path)?;
        let name = manifest_name(&manifest)?;
        let version = match (manifest.get("version"), &opts.version) {
            (Some(Value::String(declared)), Some(requested)) if declared != requested => {
                return Err(Error::InvalidInput(format!(
                    "the archived composer.json declares version {declared}, but --version \
                     {requested} was given"
                )))
            }
            (Some(Value::String(declared)), _) => declared.clone(),
            (Some(declared), _) => {
                return Err(Error::InvalidInput(format!(
                    "the archived composer.json declares version {declared}, which is not a string"
                )))
            }
            (None, Some(requested)) => requested.clone(),
            (None, None) => {
                return Err(Error::InvalidInput(
                    "the archived composer.json declares no version; pass --version".into(),
                ))
            }
        };
        validate_version(&version)?;
        let file_name = format!("{}-{version}.zip", short_name(&name));
        let file = PackageFile::archive(&opts.path, file_name)?;
        Ok(Package {
            eco: Ecosystem::Composer,
            name,
            version,
            files: vec![file],
            manifest,
            workdir: None,
            notes: Vec::new(),
        })
    }

    /// Reads and validates `composer.json` in `opts.path`, determines the version and
    /// builds the zip archive `<package>-<version>.zip` in a temporary directory owned
    /// by the package.
    ///
    /// Inside a git work tree the archive is `git archive HEAD:<path>`, where `<path>`
    /// is the package directory relative to the top level, so the package directory is
    /// the root of the archive wherever it lives in the repository.
    fn package(&self, opts: &PackageOptions) -> Result<Package, Error> {
        if !opts.extra_args.is_empty() {
            return Err(Error::InvalidInput(
                "composer packaging runs no external tool; remove the arguments after `--`".into(),
            ));
        }
        if opts.version.is_some() && opts.tag_prefix.is_some() {
            return Err(Error::InvalidInput(
                "--version and --tag-prefix cannot be combined".into(),
            ));
        }
        let dir = opts.path.as_path();
        let manifest = read_manifest(dir)?;
        let name = manifest_name(&manifest)?;

        let git = Git::new(&self.runner, dir);
        let toplevel = git.toplevel()?;
        let version = match (&opts.version, &toplevel) {
            (Some(version), _) => version.clone(),
            (None, Some(_)) => git.release_version(opts.tag_prefix.as_deref())?,
            (None, None) => {
                return Err(Error::InvalidInput(format!(
                    "no version given and `{}` is not inside a git repository; pass --version",
                    dir.display()
                )))
            }
        };
        validate_version(&version)?;
        check_manifest_version(&manifest, &version)?;

        let workdir = Workdir::new()?;
        let file_name = format!("{}-{version}.zip", short_name(&name));
        let archive_path = workdir.path().join(&file_name);
        if let Some(toplevel) = toplevel {
            let prefix = archive::relative_prefix(&toplevel, dir)?;
            archive::git_archive(&self.runner, &toplevel, &prefix, &archive_path)?;
            let archived = archive::read_composer_json(&archive_path)?.ok_or_else(|| {
                Error::InvalidInput(
                    "the archive of HEAD has no composer.json at its root; commit \
                     composer.json and make sure it is not marked export-ignore"
                        .into(),
                )
            })?;
            if archived != manifest {
                return Err(Error::InvalidInput(
                    "composer.json differs from the committed version; `git archive` packs \
                     HEAD, so commit the changes first"
                        .into(),
                ));
            }
        } else {
            tracing::debug!(dir = %dir.display(), "not a git repository; zipping the directory");
            archive::zip_directory(dir, &archive_path)?;
        }

        let file = PackageFile::archive(&archive_path, file_name)?;
        Ok(Package {
            eco: Ecosystem::Composer,
            name,
            version,
            files: vec![file],
            manifest,
            workdir: Some(workdir),
            notes: Vec::new(),
        })
    }
}

/// Reads `<dir>/composer.json` as a JSON object.
fn read_manifest(dir: &Path) -> Result<Value, Error> {
    let path = dir.join("composer.json");
    if !path.is_file() {
        return Err(Error::InvalidInput(format!(
            "`{}` does not exist; run sreg in the package directory or pass --path",
            path.display()
        )));
    }
    let text = std::fs::read_to_string(&path)?;
    parse_manifest(&text)
}

fn parse_manifest(text: &str) -> Result<Value, Error> {
    let manifest: Value = serde_json::from_str(text)
        .map_err(|err| Error::InvalidInput(format!("composer.json is not valid JSON: {err}")))?;
    if !manifest.is_object() {
        return Err(Error::InvalidInput(
            "composer.json must contain a JSON object".into(),
        ));
    }
    Ok(manifest)
}

fn manifest_name(manifest: &Value) -> Result<String, Error> {
    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidInput("composer.json has no `name`".into()))?;
    validate_name(name)?;
    Ok(name.to_owned())
}

/// A `version` in composer.json is optional, but when present it must be the version
/// being published.
fn check_manifest_version(manifest: &Value, version: &str) -> Result<(), Error> {
    match manifest.get("version") {
        None => Ok(()),
        Some(Value::String(declared)) if declared == version => Ok(()),
        Some(declared) => Err(Error::InvalidInput(format!(
            "composer.json declares version {declared}, but the version being published is \
             `{version}`; remove `version` from composer.json or make them equal"
        ))),
    }
}

/// `acme/sdk` → `sdk`.
fn short_name(name: &str) -> &str {
    name.split_once('/').map_or(name, |(_, package)| package)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reports_composer_ecosystem() {
        assert_eq!(ComposerPackager::new().ecosystem(), Ecosystem::Composer);
    }

    #[test]
    fn short_name_is_the_part_after_the_vendor() {
        assert_eq!(short_name("acme/sdk"), "sdk");
        assert_eq!(short_name("acme/http-client"), "http-client");
    }

    #[test]
    fn manifest_must_be_an_object_with_a_valid_name() {
        assert!(parse_manifest("[]").is_err());
        assert!(parse_manifest("{").is_err());
        assert!(manifest_name(&json!({})).is_err());
        assert!(manifest_name(&json!({ "name": 1 })).is_err());
        assert!(manifest_name(&json!({ "name": "Acme/Sdk" })).is_err());
        assert_eq!(
            manifest_name(&json!({ "name": "acme/sdk" })).unwrap(),
            "acme/sdk"
        );
    }

    #[test]
    fn manifest_version_must_match_when_present() {
        assert!(check_manifest_version(&json!({}), "1.2.3").is_ok());
        assert!(check_manifest_version(&json!({ "version": "1.2.3" }), "1.2.3").is_ok());
        let err = check_manifest_version(&json!({ "version": "1.2.0" }), "1.2.3").unwrap_err();
        assert!(err.to_string().contains("\"1.2.0\""), "{err}");
        assert!(check_manifest_version(&json!({ "version": 1 }), "1.2.3").is_err());
    }
}
