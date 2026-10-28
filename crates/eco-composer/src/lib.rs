//! Composer packaging for `sreg`.
//!
//! [`ComposerPackager`] reads `composer.json`, picks the version (from `--version` or
//! the git tag of `HEAD`) and archives the project as a zip: with `git archive HEAD`,
//! which honours `export-ignore` in `.gitattributes`, or, outside a git repository, by
//! zipping the directory without `vendor/` and `.git/`.

#![deny(unsafe_code)]

mod archive;
mod validate;

use std::path::Path;

use serde_json::Value;
use sreg_core::{
    CommandRunner, Ecosystem, Error, Package, PackageFile, PackageOptions, Packager, SystemRunner,
    Workdir,
};

pub use validate::{validate_name, validate_version, version_from_tag};

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

    /// Reads and validates `composer.json`, determines the version and builds the zip
    /// archive `<package>-<version>.zip` in a temporary directory owned by the package.
    fn package(&self, opts: &PackageOptions) -> Result<Package, Error> {
        let dir = opts.path.as_path();
        let manifest = read_manifest(dir)?;
        let name = manifest_name(&manifest)?;

        let git = archive::Git::new(&self.runner, dir);
        let in_repository = git.is_repository()?;
        let version = match &opts.version {
            Some(version) => version.clone(),
            None if in_repository => git.head_tag()?,
            None => {
                return Err(Error::InvalidInput(format!(
                    "no version given and `{}` is not a git repository; pass --version",
                    dir.display()
                )))
            }
        };
        validate_version(&version)?;
        check_manifest_version(&manifest, &version)?;

        let workdir = Workdir::new()?;
        let file_name = format!("{}-{version}.zip", short_name(&name));
        let archive_path = workdir.path().join(&file_name);
        if in_repository {
            git.archive(&archive_path)?;
            let archived = archive::read_composer_json(&archive_path)?.ok_or_else(|| {
                Error::InvalidInput(
                    "`git archive HEAD` produced no composer.json at the root; commit \
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
