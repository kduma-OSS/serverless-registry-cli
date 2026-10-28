//! Maven packaging for `sreg`.
//!
//! [`MavenPackager`] publishes what a build deployed into a local Maven-2 file
//! repository (`mvn deploy -DaltDeploymentRepository=local::file:./out`, or Gradle's
//! `maven-publish` with a `file://` repository): it finds
//! `<group/as/path>/<artifactId>/<version>/<artifactId>-<version>.pom`, reads the POM
//! (which must be an effective POM) into the manifest and publishes every file of the
//! version directory except checksums and `maven-metadata*.xml`, with a role derived
//! from its name (`.asc` signatures are published as `extra` files). Nothing is run
//! except `git`, for the release tag.

#![deny(unsafe_code)]

mod pom;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sreg_core::git::Git;
use sreg_core::{
    CommandRunner, Ecosystem, Error, Package, PackageFile, PackageOptions, Packager, SystemRunner,
};

pub use pom::{parse as parse_pom, Dependency, Pom, DEFAULT_PACKAGING, DEFAULT_SCOPE};

/// Role of the `.pom` of a version.
pub const POM_ROLE: &str = "pom";
/// Role of the `-sources.jar` of a version.
pub const SOURCES_ROLE: &str = "sources";
/// Role of the `-javadoc.jar` of a version.
pub const JAVADOC_ROLE: &str = "javadoc";
/// Role of the Gradle module metadata (`.module`) of a version.
pub const MODULE_ROLE: &str = "module";
/// Role of every other file of a version (classified jars, distributions, ...).
pub const EXTRA_ROLE: &str = "extra";

/// Suffixes of the files a repository keeps next to the artifacts, which are not
/// published: checksums, which the registry computes and serves itself.
const SKIPPED_SUFFIXES: [&str; 4] = [".sha1", ".md5", ".sha256", ".sha512"];

/// How a local repository is produced, for error messages.
const REPOSITORY_HINT: &str = "deploy the build into a local repository with `mvn deploy \
     -DaltDeploymentRepository=local::file:./out` or a Gradle maven-publish repository with a \
     file:// URL, and pass that directory with --from-dir";

/// Publishes Maven artifacts from a local file repository.
#[derive(Debug, Clone, Default)]
pub struct MavenPackager<R = SystemRunner> {
    runner: R,
    coords: Option<String>,
}

impl MavenPackager<SystemRunner> {
    /// A packager running the system's `git` (for the release tag).
    pub fn new() -> Self {
        MavenPackager::with_runner(SystemRunner)
    }
}

impl<R: CommandRunner> MavenPackager<R> {
    /// A packager running `git` through `runner`.
    pub fn with_runner(runner: R) -> Self {
        MavenPackager {
            runner,
            coords: None,
        }
    }

    /// Publishes the artifact `<groupId>:<artifactId>` (`--coords`) when the repository
    /// holds several.
    pub fn coords(mut self, coords: Option<String>) -> Self {
        self.coords = coords;
        self
    }
}

/// A version directory holding `<artifactId>-<version>.pom` (or, for a snapshot, any
/// `<artifactId>-*.pom`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Candidate {
    group_id: String,
    artifact_id: String,
    version: String,
    dir: PathBuf,
}

impl Candidate {
    fn coords(&self) -> String {
        format!("{}:{}", self.group_id, self.artifact_id)
    }
}

impl<R: CommandRunner> Packager for MavenPackager<R> {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Maven
    }

    /// Selects one artifact and version in the repository `opts.path` and describes its
    /// files. The version is `--version`, else the release tag on `HEAD` of the current
    /// directory's git repository, else the only version in the repository.
    fn package(&self, opts: &PackageOptions) -> Result<Package, Error> {
        if opts.tag.is_some() {
            return Err(Error::InvalidInput(
                "dist-tags exist only for npm packages".into(),
            ));
        }
        if !opts.extra_args.is_empty() {
            return Err(Error::InvalidInput(
                "maven publishing runs no external tool; remove the arguments after `--`".into(),
            ));
        }
        let wanted = self.coords.as_deref().map(parse_coords).transpose()?;
        if let Some(version) = &opts.version {
            validate_version(version)?;
        }
        let root = opts.path.as_path();
        if !root.is_dir() {
            return Err(Error::InvalidInput(format!(
                "`{}` is not a directory; {REPOSITORY_HINT}",
                root.display()
            )));
        }

        let candidates = scan(root)?;
        if candidates.is_empty() {
            return Err(Error::InvalidInput(format!(
                "`{}` holds no Maven artifact \
                 (<group>/<artifactId>/<version>/<artifactId>-<version>.pom); {REPOSITORY_HINT}",
                root.display()
            )));
        }
        let artifacts: BTreeSet<String> = candidates.iter().map(Candidate::coords).collect();
        let coords = match &wanted {
            Some((group_id, artifact_id)) => {
                let coords = format!("{group_id}:{artifact_id}");
                if !artifacts.contains(&coords) {
                    return Err(Error::InvalidInput(format!(
                        "`{}` holds no artifact {coords}; it holds: {}",
                        root.display(),
                        join(&artifacts)
                    )));
                }
                coords
            }
            None if artifacts.len() == 1 => artifacts.iter().next().cloned().unwrap_or_default(),
            None => {
                return Err(Error::InvalidInput(format!(
                    "`{}` holds several artifacts ({}); select one with --coords \
                     <groupId>:<artifactId>",
                    root.display(),
                    join(&artifacts)
                )))
            }
        };
        let versions: Vec<&Candidate> = candidates
            .iter()
            .filter(|candidate| candidate.coords() == coords)
            .collect();
        let names: BTreeSet<String> = versions.iter().map(|c| c.version.clone()).collect();

        let requested = Git::new(&self.runner, Path::new("."))
            .requested_version(opts.version.as_deref(), opts.tag_prefix.as_deref())?;
        let candidate = match &requested {
            Some(version) => versions
                .iter()
                .find(|candidate| &candidate.version == version)
                .ok_or_else(|| {
                    Error::InvalidInput(format!(
                        "`{}` has no version {version} of {coords}; it has: {}",
                        root.display(),
                        join(&names)
                    ))
                })?,
            None => match versions.as_slice() {
                [candidate] => candidate,
                _ => {
                    return Err(Error::InvalidInput(format!(
                        "`{}` has several versions of {coords} ({}); pass --version, or tag \
                         the release",
                        root.display(),
                        join(&names)
                    )))
                }
            },
        };
        validate_version(&candidate.version)?;

        let pom_name = format!("{}-{}.pom", candidate.artifact_id, candidate.version);
        let pom_path = candidate.dir.join(&pom_name);
        let xml = std::fs::read_to_string(&pom_path).map_err(|err| match err.kind() {
            std::io::ErrorKind::InvalidData => {
                Error::InvalidInput(format!("`{}` is not UTF-8", pom_path.display()))
            }
            _ => Error::Io(err),
        })?;
        let pom = pom::parse(&xml)
            .map_err(|err| Error::InvalidInput(format!("{pom_name}: {}", message(&err))))?;
        let declared = (&pom.group_id, &pom.artifact_id, &pom.version);
        let located = (
            &candidate.group_id,
            &candidate.artifact_id,
            &candidate.version,
        );
        if declared != located {
            return Err(Error::InvalidInput(format!(
                "{pom_name} declares {}:{}:{}, but it is stored as {}:{}:{}",
                pom.group_id,
                pom.artifact_id,
                pom.version,
                candidate.group_id,
                candidate.artifact_id,
                candidate.version
            )));
        }

        let files = version_files(&candidate.dir, &pom)?;
        Ok(Package {
            eco: Ecosystem::Maven,
            name: coords,
            version: pom.version.clone(),
            files,
            manifest: pom.manifest(),
            workdir: None,
            notes: Vec::new(),
        })
    }
}

/// The files of a version directory with their roles; the archive is required unless
/// the packaging is `pom`.
fn version_files(dir: &Path, pom: &Pom) -> Result<Vec<PackageFile>, Error> {
    let base = format!("{}-{}", pom.artifact_id, pom.version);
    let archive = archive_extension(&pom.packaging).map(|extension| format!("{base}.{extension}"));
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry
            .file_name()
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| {
                Error::InvalidInput(format!(
                    "the file name {:?} in `{}` is not valid UTF-8",
                    entry.file_name(),
                    dir.display()
                ))
            })?;
        if is_skipped(&name) {
            continue;
        }
        let role = if name == format!("{base}.pom") {
            POM_ROLE
        } else if Some(&name) == archive.as_ref() {
            PackageFile::ARCHIVE_ROLE
        } else if name == format!("{base}-sources.jar") {
            SOURCES_ROLE
        } else if name == format!("{base}-javadoc.jar") {
            JAVADOC_ROLE
        } else if name == format!("{base}.module") {
            MODULE_ROLE
        } else {
            EXTRA_ROLE
        };
        files.push((role_rank(role), name, role));
    }
    if let Some(archive) = &archive {
        if !files.iter().any(|(_, name, _)| name == archive) {
            return Err(Error::InvalidInput(format!(
                "`{}` has no {archive}, the archive of a `{}` artifact",
                dir.display(),
                pom.packaging
            )));
        }
    }
    files.sort();
    files
        .into_iter()
        .map(|(_, name, role)| {
            PackageFile::from_path(
                dir.join(&name),
                Some(name),
                Some(role.to_owned()),
                Default::default(),
            )
        })
        .collect()
}

/// The order of the files in `init`: the POM, the archive, then the rest.
fn role_rank(role: &str) -> u8 {
    match role {
        POM_ROLE => 0,
        PackageFile::ARCHIVE_ROLE => 1,
        MODULE_ROLE => 2,
        SOURCES_ROLE => 3,
        JAVADOC_ROLE => 4,
        _ => 5,
    }
}

/// The extension of the archive of an artifact with this packaging; `None` for `pom`.
pub fn archive_extension(packaging: &str) -> Option<&str> {
    match packaging {
        "pom" | "module" => None,
        "jar" | "maven-plugin" | "ejb" | "bundle" => Some("jar"),
        other => Some(other),
    }
}

/// Checksums and repository metadata are not published; signatures are.
fn is_skipped(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SKIPPED_SUFFIXES
        .iter()
        .any(|suffix| lower.ends_with(suffix))
        || (lower.starts_with("maven-metadata") && lower.ends_with(".xml"))
}

/// Finds the version directories under `root`.
fn scan(root: &Path) -> Result<Vec<Candidate>, Error> {
    let mut found = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() {
                if let Some(candidate) = candidate(root, &path) {
                    found.insert(candidate);
                }
            }
        }
    }
    Ok(found.into_iter().collect())
}

/// The candidate a `.pom` at `path` stands for:
/// `<root>/<group…>/<artifactId>/<version>/<artifactId>-<version>.pom`.
fn candidate(root: &Path, path: &Path) -> Option<Candidate> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Vec<&str> = relative
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<_>>()?;
    let [group @ .., artifact_id, version, file] = parts.as_slice() else {
        return None;
    };
    if group.is_empty() || !file.ends_with(".pom") {
        return None;
    }
    let expected = format!("{artifact_id}-{version}.pom");
    // A snapshot's files carry a timestamp instead of `-SNAPSHOT`; it is rejected later.
    let snapshot = is_snapshot(version) && file.starts_with(&format!("{artifact_id}-"));
    if *file != expected && !snapshot {
        return None;
    }
    Some(Candidate {
        group_id: group.join("."),
        artifact_id: (*artifact_id).to_owned(),
        version: (*version).to_owned(),
        dir: path.parent()?.to_path_buf(),
    })
}

fn is_snapshot(version: &str) -> bool {
    version.to_ascii_uppercase().ends_with("-SNAPSHOT")
}

/// Parses `--coords <groupId>:<artifactId>`.
pub fn parse_coords(coords: &str) -> Result<(String, String), Error> {
    let invalid = || {
        Error::InvalidInput(format!(
            "invalid --coords `{coords}`; expected <groupId>:<artifactId>, e.g. com.acme:sdk"
        ))
    };
    let (group_id, artifact_id) = coords.split_once(':').ok_or_else(invalid)?;
    if !is_identifier(group_id) || !is_identifier(artifact_id) {
        return Err(invalid());
    }
    Ok((group_id.to_owned(), artifact_id.to_owned()))
}

/// Maven coordinates: `[A-Za-z0-9_.-]+`.
fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Checks a version: `[A-Za-z0-9_.+-]{1,128}`, and not a snapshot.
pub fn validate_version(version: &str) -> Result<(), Error> {
    if is_snapshot(version) {
        return Err(Error::InvalidInput(format!(
            "{version} is a snapshot; published versions are immutable, so publish a release \
             version"
        )));
    }
    let valid = (1..=128).contains(&version.len())
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-'));
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "invalid Maven version `{version}`; expected 1 to 128 characters of A-Z, a-z, 0-9, \
             `.`, `_`, `+` and `-`"
        )))
    }
}

fn join(values: &BTreeSet<String>) -> String {
    values.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// The message of an error without the variant's prefix.
fn message(err: &Error) -> String {
    match err {
        Error::InvalidInput(message) => message.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_maven_ecosystem() {
        assert_eq!(MavenPackager::new().ecosystem(), Ecosystem::Maven);
    }

    #[test]
    fn packaging_maps_to_the_archive_extension() {
        for (packaging, extension) in [
            ("jar", Some("jar")),
            ("aar", Some("aar")),
            ("war", Some("war")),
            ("ear", Some("ear")),
            ("maven-plugin", Some("jar")),
            ("ejb", Some("jar")),
            ("bundle", Some("jar")),
            ("pom", None),
            ("module", None),
            ("nbm", Some("nbm")),
        ] {
            assert_eq!(archive_extension(packaging), extension, "{packaging}");
        }
    }

    #[test]
    fn checksums_and_metadata_are_skipped() {
        for name in [
            "sdk-1.0.0.jar.sha1",
            "sdk-1.0.0.pom.MD5",
            "sdk-1.0.0.jar.sha256",
            "sdk-1.0.0.jar.sha512",
            "sdk-1.0.0.jar.asc.sha1",
            "maven-metadata.xml",
            "maven-metadata-local.xml",
        ] {
            assert!(is_skipped(name), "{name}");
        }
        for name in [
            "sdk-1.0.0.jar",
            "sdk-1.0.0.jar.asc",
            "sdk-1.0.0.module",
            "sdk-1.0.0-all.zip",
            "metadata.xml",
        ] {
            assert!(!is_skipped(name), "{name}");
        }
    }

    #[test]
    fn candidates_follow_the_repository_layout() {
        let root = Path::new("/repo");
        let found = candidate(root, Path::new("/repo/com/acme/sdk/1.0.0/sdk-1.0.0.pom")).unwrap();
        assert_eq!(found.coords(), "com.acme:sdk");
        assert_eq!(found.version, "1.0.0");
        assert_eq!(found.dir, Path::new("/repo/com/acme/sdk/1.0.0"));
        for path in [
            "/repo/sdk/1.0.0/sdk-1.0.0.pom",
            "/repo/com/acme/sdk/1.0.0/sdk-1.0.1.pom",
            "/repo/com/acme/sdk/1.0.0/sdk-1.0.0.jar",
            "/repo/com/acme/sdk/1.0.0/other-1.0.0.pom",
        ] {
            assert_eq!(candidate(root, Path::new(path)), None, "{path}");
        }
        let snapshot = candidate(
            root,
            Path::new("/repo/com/acme/sdk/2.0.0-SNAPSHOT/sdk-2.0.0-20261028.101500-1.pom"),
        )
        .unwrap();
        assert_eq!(snapshot.version, "2.0.0-SNAPSHOT");
    }

    #[test]
    fn coords_and_versions_are_validated() {
        assert_eq!(
            parse_coords("com.acme:sdk-core_2").unwrap(),
            ("com.acme".to_owned(), "sdk-core_2".to_owned())
        );
        for coords in [
            "com.acme",
            "com.acme:",
            ":sdk",
            "com.acme:sdk:1.0",
            "com acme:sdk",
        ] {
            assert!(parse_coords(coords).is_err(), "{coords}");
        }
        for version in [
            "1.0",
            "1.0.0",
            "2.3.1.Final",
            "1.0.0-rc.1+build.5",
            "20261028",
        ] {
            assert!(validate_version(version).is_ok(), "{version}");
        }
        for version in ["", "1.0 beta", "1/0", "1.0.0-SNAPSHOT", "1.0-snapshot"] {
            assert!(validate_version(version).is_err(), "{version}");
        }
    }
}
