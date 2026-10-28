//! Generic file packages for `sreg`.
//!
//! A generic version is an explicit list of files with labels plus a free-form `meta`
//! object. Nothing is packed: the [`GenericPackager`] only validates the input and
//! computes the size and SHA-256 digest of every file.

#![deny(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sreg_core::{Ecosystem, Error, Package, PackageFile, PackageOptions, Packager};

/// One file to publish and its labels.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenericFileSpec {
    /// Local path; the file is published under its file name.
    pub path: PathBuf,
    /// Labels of the file.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// Builds a generic [`Package`] from an explicit list of files.
#[derive(Debug, Clone, PartialEq)]
pub struct GenericPackager {
    /// Package name: `[a-z0-9][a-z0-9._-]*`, optionally prefixed by one `group/`.
    pub name: String,
    /// Files of the version.
    pub files: Vec<GenericFileSpec>,
    /// Free-form metadata sent as the version's `manifest`.
    pub meta: serde_json::Map<String, serde_json::Value>,
}

impl Packager for GenericPackager {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Generic
    }

    /// Validates the name, the version (`opts.version`, required) and the files.
    fn package(&self, opts: &PackageOptions) -> Result<Package, Error> {
        validate_name(&self.name)?;
        let version = opts
            .version
            .as_deref()
            .ok_or_else(|| Error::InvalidInput("a generic package needs a version".into()))?;
        validate_version(version)?;
        if self.files.is_empty() {
            return Err(Error::InvalidInput("no files to publish".into()));
        }

        let mut names = BTreeSet::new();
        let mut files = Vec::with_capacity(self.files.len());
        for spec in &self.files {
            for (key, value) in &spec.labels {
                validate_label(key, value)?;
            }
            if !spec.path.is_file() {
                return Err(Error::InvalidInput(format!(
                    "`{}` is not a file",
                    spec.path.display()
                )));
            }
            let file = PackageFile::from_path(&spec.path, None, None, spec.labels.clone())?;
            validate_file_name(&file.name)?;
            if !names.insert(file.name.clone()) {
                return Err(Error::InvalidInput(format!(
                    "duplicate file name `{}`; every file of a version needs a unique name",
                    file.name
                )));
            }
            files.push(file);
        }

        Ok(Package {
            eco: Ecosystem::Generic,
            name: self.name.clone(),
            version: version.to_owned(),
            files,
            manifest: serde_json::Value::Object(self.meta.clone()),
        })
    }
}

/// Parses a `--spec` document: `[ { "path": "...", "labels": { ... } }, ... ]`.
///
/// Relative paths are resolved against `base`.
pub fn parse_spec(json: &str, base: &Path) -> Result<Vec<GenericFileSpec>, Error> {
    let specs: Vec<GenericFileSpec> = serde_json::from_str(json)
        .map_err(|err| Error::InvalidInput(format!("invalid file spec: {err}")))?;
    Ok(specs
        .into_iter()
        .map(|spec| GenericFileSpec {
            path: base.join(spec.path),
            labels: spec.labels,
        })
        .collect())
}

/// Parses `key=value` (`--label`, `--meta`).
pub fn parse_key_value(value: &str) -> Result<(String, String), Error> {
    match value.split_once('=') {
        Some((key, value)) if !key.is_empty() => Ok((key.to_owned(), value.to_owned())),
        _ => Err(Error::InvalidInput(format!(
            "invalid `{value}`; expected <key>=<value>"
        ))),
    }
}

/// Checks a label: the key is `[A-Za-z0-9._-]+` (it must not contain `:`, which
/// separates key and value in feed queries) and the value is not empty.
pub fn validate_label(key: &str, value: &str) -> Result<(), Error> {
    let key_valid = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !key_valid {
        return Err(Error::InvalidInput(format!(
            "invalid label key `{key}`; use letters, digits, `.`, `_` and `-`"
        )));
    }
    if value.is_empty() {
        return Err(Error::InvalidInput(format!(
            "label `{key}` has an empty value"
        )));
    }
    Ok(())
}

/// Checks a package name: `[a-z0-9][a-z0-9._-]*`, optionally with one `group/` segment.
pub fn validate_name(name: &str) -> Result<(), Error> {
    let segment_valid = |segment: &str| {
        let mut chars = segment.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && chars.all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
            })
    };
    let segments: Vec<&str> = name.split('/').collect();
    if segments.len() <= 2 && segments.iter().all(|segment| segment_valid(segment)) {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "invalid package name `{name}`; expected [<group>/]<name> with lower-case \
             letters, digits, `.`, `_` and `-`"
        )))
    }
}

/// Checks a version: semver `MAJOR.MINOR.PATCH[-PRERELEASE]` without build metadata.
pub fn validate_version(version: &str) -> Result<(), Error> {
    match semver::Version::parse(version) {
        Ok(parsed) if parsed.build.is_empty() => Ok(()),
        _ => Err(Error::InvalidInput(format!(
            "invalid version `{version}`; expected MAJOR.MINOR.PATCH[-PRERELEASE] without a \
             leading `v` or build metadata"
        ))),
    }
}

fn validate_file_name(name: &str) -> Result<(), Error> {
    let valid = !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'));
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "invalid file name `{name}`; use letters, digits, `.`, `_`, `-` and `+`"
        )))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn write(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn opts(version: &str) -> PackageOptions {
        PackageOptions {
            version: Some(version.into()),
            ..PackageOptions::default()
        }
    }

    #[test]
    fn packages_files_with_labels_and_meta() {
        let dir = tempfile::tempdir().unwrap();
        let linux = write(dir.path(), "cli-linux.tar.gz", b"hello");
        let packager = GenericPackager {
            name: "tools/cli".into(),
            files: vec![GenericFileSpec {
                path: linux,
                labels: BTreeMap::from([("os".to_owned(), "linux".to_owned())]),
            }],
            meta: json!({ "channel": "stable" }).as_object().unwrap().clone(),
        };

        let package = packager.package(&opts("1.2.0-rc.1")).unwrap();

        assert_eq!(package.eco, Ecosystem::Generic);
        assert_eq!(package.version, "1.2.0-rc.1");
        assert_eq!(package.manifest, json!({ "channel": "stable" }));
        let file = &package.files[0];
        assert_eq!(file.name, "cli-linux.tar.gz");
        assert_eq!(file.size, 5);
        assert_eq!(file.role, None);
        assert_eq!(file.labels["os"], "linux");
        assert_eq!(
            file.sha256,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn rejects_duplicate_file_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        std::fs::create_dir(dir.path().join("b")).unwrap();
        let packager = GenericPackager {
            name: "cli".into(),
            files: vec![
                GenericFileSpec {
                    path: write(&dir.path().join("a"), "cli.zip", b"a"),
                    labels: BTreeMap::new(),
                },
                GenericFileSpec {
                    path: write(&dir.path().join("b"), "cli.zip", b"b"),
                    labels: BTreeMap::new(),
                },
            ],
            meta: serde_json::Map::new(),
        };

        let err = packager.package(&opts("1.0.0")).unwrap_err();

        assert!(
            matches!(err, Error::InvalidInput(ref m) if m.contains("duplicate")),
            "{err}"
        );
    }

    #[test]
    fn rejects_missing_files_versions_and_empty_lists() {
        let dir = tempfile::tempdir().unwrap();
        let packager = GenericPackager {
            name: "cli".into(),
            files: vec![GenericFileSpec {
                path: dir.path().join("missing.zip"),
                labels: BTreeMap::new(),
            }],
            meta: serde_json::Map::new(),
        };
        assert!(packager.package(&opts("1.0.0")).is_err());
        assert!(packager.package(&PackageOptions::default()).is_err());

        let empty = GenericPackager {
            files: Vec::new(),
            ..packager
        };
        assert!(empty.package(&opts("1.0.0")).is_err());
    }

    #[test]
    fn validates_names() {
        for valid in ["cli", "tools/cli", "a1.b_c-d", "0tools/x"] {
            assert!(validate_name(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "", "Cli", "-cli", "a/b/c", "/cli", "cli/", "tools/.x", "a b",
        ] {
            assert!(validate_name(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn validates_versions() {
        for valid in ["1.0.0", "0.1.0-beta.1", "2.0.0-rc2"] {
            assert!(validate_version(valid).is_ok(), "{valid}");
        }
        for invalid in ["v1.0.0", "1.0", "1.0.0+build.1", "latest"] {
            assert!(validate_version(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn validates_labels_and_key_values() {
        assert_eq!(
            parse_key_value("os=linux").unwrap(),
            ("os".to_owned(), "linux".to_owned())
        );
        assert_eq!(parse_key_value("note=a=b").unwrap().1, "a=b");
        assert!(parse_key_value("=x").is_err());
        assert!(parse_key_value("novalue").is_err());
        assert!(validate_label("os", "linux").is_ok());
        assert!(validate_label("o:s", "linux").is_err());
        assert!(validate_label("os", "").is_err());
    }

    #[test]
    fn parses_spec_documents() {
        let specs = parse_spec(
            r#"[{ "path": "dist/cli.zip", "labels": { "os": "linux" } }, { "path": "/abs/x.txt" }]"#,
            Path::new("/work"),
        )
        .unwrap();
        assert_eq!(specs[0].path, PathBuf::from("/work/dist/cli.zip"));
        assert_eq!(specs[0].labels["os"], "linux");
        assert_eq!(specs[1].path, PathBuf::from("/abs/x.txt"));
        assert!(parse_spec(r#"{"path": "x"}"#, Path::new(".")).is_err());
        assert!(parse_spec(r#"[{"path": "x", "label": {}}]"#, Path::new(".")).is_err());
    }
}
