//! The packaging abstraction implemented by every ecosystem crate.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::Error;

/// A package ecosystem supported by the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ecosystem {
    /// PHP packages installed with Composer.
    Composer,
    /// JavaScript packages installed with npm.
    Npm,
    /// Rust crates installed with cargo.
    Cargo,
}

impl Ecosystem {
    /// All supported ecosystems.
    pub const ALL: [Ecosystem; 3] = [Ecosystem::Composer, Ecosystem::Npm, Ecosystem::Cargo];

    /// The identifier used in the API and on the command line.
    pub const fn as_str(self) -> &'static str {
        match self {
            Ecosystem::Composer => "composer",
            Ecosystem::Npm => "npm",
            Ecosystem::Cargo => "cargo",
        }
    }
}

impl fmt::Display for Ecosystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Ecosystem {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ecosystem::ALL
            .into_iter()
            .find(|eco| eco.as_str() == s)
            .ok_or_else(|| {
                Error::InvalidInput(format!(
                    "unknown ecosystem `{s}`; expected one of: composer, npm, cargo"
                ))
            })
    }
}

/// Options shared by all packagers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageOptions {
    /// Directory of the project to package.
    pub path: PathBuf,
    /// Version to publish, overriding the one detected from the project.
    pub version: Option<String>,
    /// Distribution tag (npm only).
    pub tag: Option<String>,
    /// Extra arguments passed through to the native packaging tool.
    pub extra_args: Vec<String>,
}

/// A packaged archive ready to be published.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    /// Ecosystem the package belongs to.
    pub eco: Ecosystem,
    /// Package name as understood by the ecosystem.
    pub name: String,
    /// Semantic version of the package.
    pub version: String,
    /// File name of the archive in the registry.
    pub file_name: String,
    /// Local path of the archive.
    pub archive: PathBuf,
    /// Size of the archive in bytes.
    pub size: u64,
    /// Lower-case hex SHA-256 digest of the archive.
    pub sha256: String,
    /// Native manifest extracted from the archive.
    pub manifest: serde_json::Value,
}

/// Turns a project directory into a publishable [`Package`].
pub trait Packager {
    /// The ecosystem handled by this packager.
    fn ecosystem(&self) -> Ecosystem;

    /// Packages the project described by `opts`.
    fn package(&self, opts: &PackageOptions) -> Result<Package, Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecosystem_round_trips_through_strings() {
        for eco in Ecosystem::ALL {
            assert_eq!(eco.to_string().parse::<Ecosystem>().unwrap(), eco);
        }
    }

    #[test]
    fn ecosystem_rejects_unknown_names() {
        assert!(matches!(
            "pypi".parse::<Ecosystem>(),
            Err(Error::InvalidInput(_))
        ));
        assert!("Composer".parse::<Ecosystem>().is_err());
    }

    #[test]
    fn ecosystem_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Ecosystem::Npm).unwrap(), "\"npm\"");
        assert_eq!(
            serde_json::from_str::<Ecosystem>("\"cargo\"").unwrap(),
            Ecosystem::Cargo
        );
    }
}
