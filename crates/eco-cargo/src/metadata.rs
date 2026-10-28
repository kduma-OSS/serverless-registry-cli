//! The parts of `cargo metadata --format-version 1 --no-deps` the packager needs, and
//! choosing the crate to publish.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use sreg_core::Error;

/// Output of `cargo metadata --format-version 1 --no-deps`.
#[derive(Debug, Clone, Deserialize)]
pub struct Metadata {
    /// The workspace members (with `--no-deps`, nothing else is listed).
    pub packages: Vec<MetadataPackage>,
    /// Ids of the workspace members.
    #[serde(default)]
    pub workspace_members: Vec<String>,
}

/// One package of the workspace.
#[derive(Debug, Clone, Deserialize)]
pub struct MetadataPackage {
    /// Crate name.
    pub name: String,
    /// Version declared in (or inherited by) `Cargo.toml`.
    pub version: String,
    /// Package id, as listed in `workspace_members`.
    pub id: String,
    /// Absolute path of the crate's `Cargo.toml`.
    pub manifest_path: PathBuf,
    /// Dependencies of every kind, as declared.
    #[serde(default)]
    pub dependencies: Vec<MetadataDependency>,
    /// Features in the order cargo lists them, including the implicit features of
    /// optional dependencies.
    #[serde(default)]
    pub features: serde_json::Map<String, serde_json::Value>,
    /// The `links` key.
    #[serde(default)]
    pub links: Option<String>,
    /// The `rust-version` key.
    #[serde(default)]
    pub rust_version: Option<String>,
}

/// One dependency as declared in the manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct MetadataDependency {
    /// The real crate name.
    pub name: String,
    /// `registry+<url>`, `sparse+<url>`, `git+<url>`, or `None` for path dependencies.
    #[serde(default)]
    pub source: Option<String>,
    /// Version requirement, e.g. `^1.0`; `*` when none is given.
    pub req: String,
    /// `None` for normal dependencies, `dev` or `build`.
    #[serde(default)]
    pub kind: Option<String>,
    /// The name used in code when the dependency is renamed.
    #[serde(default)]
    pub rename: Option<String>,
    /// Whether the dependency is optional.
    #[serde(default)]
    pub optional: bool,
    /// Whether the dependency's default features are enabled.
    #[serde(default = "default_true")]
    pub uses_default_features: bool,
    /// Features enabled on the dependency.
    #[serde(default)]
    pub features: Vec<String>,
    /// Platform the dependency is limited to (`cfg(…)` or a target triple).
    #[serde(default)]
    pub target: Option<String>,
    /// Index URL of an alternative registry; `None` means crates.io.
    #[serde(default)]
    pub registry: Option<String>,
    /// Local path of a path dependency.
    #[serde(default)]
    pub path: Option<PathBuf>,
}

fn default_true() -> bool {
    true
}

impl Metadata {
    /// Parses the standard output of `cargo metadata`.
    pub fn parse(stdout: &[u8]) -> Result<Self, Error> {
        serde_json::from_slice(stdout).map_err(|err| {
            Error::InvalidInput(format!(
                "could not read the output of `cargo metadata`: {err}"
            ))
        })
    }

    /// The workspace members.
    fn members(&self) -> impl Iterator<Item = &MetadataPackage> {
        self.packages.iter().filter(|package| {
            self.workspace_members.is_empty() || self.workspace_members.contains(&package.id)
        })
    }

    /// The package to publish: the one named `name` (`--package`), else the package
    /// whose manifest is `current_manifest` (the `Cargo.toml` cargo itself would use,
    /// as when running `cargo package` in a member's directory), else the only member.
    pub fn select(
        &self,
        name: Option<&str>,
        current_manifest: Option<&Path>,
    ) -> Result<&MetadataPackage, Error> {
        let names = || {
            self.members()
                .map(|package| package.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Some(name) = name {
            return self
                .members()
                .find(|package| package.name == name)
                .ok_or_else(|| {
                    Error::InvalidInput(format!(
                        "the workspace has no package `{name}`; its packages are: {}",
                        names()
                    ))
                });
        }
        if let Some(current) = current_manifest {
            let current = canonical(current);
            if let Some(package) = self
                .members()
                .find(|package| canonical(&package.manifest_path) == current)
            {
                return Ok(package);
            }
        }
        let mut members = self.members();
        match (members.next(), members.next()) {
            (Some(package), None) => Ok(package),
            (None, _) => Err(Error::InvalidInput(
                "`cargo metadata` lists no package to publish".into(),
            )),
            (Some(_), Some(_)) => Err(Error::InvalidInput(format!(
                "the workspace has several packages ({}); select one with --package",
                names()
            ))),
        }
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The `Cargo.toml` cargo uses when run in `dir`: the nearest one in `dir` or above.
pub fn nearest_manifest(dir: &Path) -> Option<PathBuf> {
    let dir = std::path::absolute(dir).ok()?;
    dir.ancestors()
        .map(|ancestor| ancestor.join("Cargo.toml"))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn metadata(packages: &[(&str, &str)]) -> Metadata {
        let packages: Vec<_> = packages
            .iter()
            .map(|(name, manifest)| {
                json!({
                    "name": name,
                    "version": "0.1.0",
                    "id": format!("path+file:///w#{name}@0.1.0"),
                    "manifest_path": manifest,
                })
            })
            .collect();
        let members: Vec<_> = packages
            .iter()
            .map(|package| package["id"].clone())
            .collect();
        Metadata::parse(
            json!({ "packages": packages, "workspace_members": members })
                .to_string()
                .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn selects_by_name_by_manifest_or_the_only_package() {
        let one = metadata(&[("a", "/w/a/Cargo.toml")]);
        assert_eq!(one.select(None, None).unwrap().name, "a");

        let two = metadata(&[("a", "/w/a/Cargo.toml"), ("b", "/w/b/Cargo.toml")]);
        assert_eq!(two.select(Some("b"), None).unwrap().name, "b");
        assert_eq!(
            two.select(None, Some(Path::new("/w/b/Cargo.toml")))
                .unwrap()
                .name,
            "b"
        );
        let err = two
            .select(None, Some(Path::new("/w/Cargo.toml")))
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("several packages (a, b); select one with --package"),
            "{err}"
        );
        let err = two.select(Some("c"), None).unwrap_err();
        assert!(err.to_string().contains("no package `c`"), "{err}");
        assert!(metadata(&[]).select(None, None).is_err());
    }

    #[test]
    fn invalid_output_is_an_error() {
        assert!(Metadata::parse(b"warning: x").is_err());
    }

    #[test]
    fn finds_the_nearest_manifest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("src/bin")).unwrap();
        assert_eq!(
            nearest_manifest(&dir.path().join("src/bin")).unwrap(),
            dir.path().join("Cargo.toml")
        );
    }
}
