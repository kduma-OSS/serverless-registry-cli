//! The packaging abstraction implemented by every ecosystem crate.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::digest::sha256_file;
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
    /// .NET packages installed with NuGet.
    Nuget,
    /// Arbitrary files grouped into versions, downloaded through the generic feed.
    Generic,
}

impl Ecosystem {
    /// All supported ecosystems.
    pub const ALL: [Ecosystem; 5] = [
        Ecosystem::Composer,
        Ecosystem::Npm,
        Ecosystem::Cargo,
        Ecosystem::Nuget,
        Ecosystem::Generic,
    ];

    /// The identifier used in the API and on the command line.
    pub const fn as_str(self) -> &'static str {
        match self {
            Ecosystem::Composer => "composer",
            Ecosystem::Npm => "npm",
            Ecosystem::Cargo => "cargo",
            Ecosystem::Nuget => "nuget",
            Ecosystem::Generic => "generic",
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
                    "unknown ecosystem `{s}`; expected one of: composer, npm, cargo, nuget, generic"
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
    /// Prefix of the release tag on `HEAD` the version is taken from when `version` is
    /// not given (`foo/v` for `foo/v1.2.3`); `None` means `v`. See
    /// [`select_release_tag`](crate::git::select_release_tag).
    pub tag_prefix: Option<String>,
    /// Distribution tag (npm only).
    pub tag: Option<String>,
    /// Extra arguments passed verbatim to the native packaging tool (the arguments after
    /// `--` on the command line).
    pub extra_args: Vec<String>,
}

/// One file of a version, ready to be uploaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageFile {
    /// Local path of the file.
    pub path: PathBuf,
    /// File name in the registry.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Lower-case hex SHA-256 digest.
    pub sha256: String,
    /// Role of the file, `archive` for the single archive of composer, npm, cargo and
    /// nuget.
    pub role: Option<String>,
    /// Free-form labels (generic packages).
    pub labels: BTreeMap<String, String>,
}

impl PackageFile {
    /// Role of the archive of a composer, npm or cargo version.
    pub const ARCHIVE_ROLE: &'static str = "archive";

    /// Describes the file at `path`, computing its size and digest.
    ///
    /// `name` defaults to the file name of `path`.
    pub fn from_path(
        path: impl Into<PathBuf>,
        name: Option<String>,
        role: Option<String>,
        labels: BTreeMap<String, String>,
    ) -> Result<Self, Error> {
        let path = path.into();
        let name = match name {
            Some(name) => name,
            None => path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
                .ok_or_else(|| {
                    Error::InvalidInput(format!("`{}` has no usable file name", path.display()))
                })?,
        };
        let (sha256, size) = sha256_file(&path)?;
        Ok(PackageFile {
            path,
            name,
            size,
            sha256,
            role,
            labels,
        })
    }

    /// Describes the single archive of a composer, npm or cargo version.
    pub fn archive(path: impl Into<PathBuf>, name: String) -> Result<Self, Error> {
        Self::from_path(
            path,
            Some(name),
            Some(Self::ARCHIVE_ROLE.to_owned()),
            BTreeMap::new(),
        )
    }
}

/// A version ready to be published: its files and its manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    /// Ecosystem the package belongs to.
    pub eco: Ecosystem,
    /// Package name as understood by the ecosystem.
    pub name: String,
    /// Semantic version of the package.
    pub version: String,
    /// Files of the version; exactly one archive for composer, npm and cargo.
    pub files: Vec<PackageFile>,
    /// Native manifest (composer, npm, cargo) or free-form metadata (generic).
    pub manifest: serde_json::Value,
    /// Temporary directory holding files the packager produced, kept alive as long as
    /// the package (or a clone of it) exists; `None` when the files belong to the user.
    pub workdir: Option<Workdir>,
}

/// A temporary directory for the files a packager produces (archives built from the
/// project). It is deleted when the last clone is dropped.
#[derive(Debug, Clone)]
pub struct Workdir(Arc<tempfile::TempDir>);

impl Workdir {
    /// Creates a new, empty temporary directory.
    pub fn new() -> Result<Self, Error> {
        let dir = tempfile::Builder::new().prefix("sreg-").tempdir()?;
        Ok(Workdir(Arc::new(dir)))
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        self.0.path()
    }
}

impl PartialEq for Workdir {
    fn eq(&self, other: &Self) -> bool {
        self.path() == other.path()
    }
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
    fn archive_file_carries_digest_size_and_role() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sdk-2.3.1.tgz");
        std::fs::write(&path, b"hello").unwrap();

        let file = PackageFile::archive(&path, "sdk-2.3.1.tgz".into()).unwrap();

        assert_eq!(file.size, 5);
        assert_eq!(
            file.sha256,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(file.role.as_deref(), Some("archive"));
        assert_eq!(
            PackageFile::from_path(&path, None, None, BTreeMap::new())
                .unwrap()
                .name,
            "sdk-2.3.1.tgz"
        );
    }

    #[test]
    fn workdir_is_removed_with_its_last_clone() {
        let workdir = Workdir::new().unwrap();
        let clone = workdir.clone();
        let path = workdir.path().to_path_buf();
        assert!(path.is_dir());
        assert_eq!(workdir, clone);

        drop(workdir);
        assert!(path.is_dir());
        drop(clone);
        assert!(!path.exists());
    }

    #[test]
    fn ecosystem_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Ecosystem::Npm).unwrap(), "\"npm\"");
        assert_eq!(
            serde_json::to_string(&Ecosystem::Nuget).unwrap(),
            "\"nuget\""
        );
        assert_eq!(
            serde_json::from_str::<Ecosystem>("\"cargo\"").unwrap(),
            Ecosystem::Cargo
        );
    }
}
