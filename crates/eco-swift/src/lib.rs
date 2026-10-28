//! Swift packaging for `sreg`.
//!
//! [`SwiftPackager`] describes a Swift package with `swift package dump-package`,
//! archives it with `swift package archive-source -o <tmp>/<name>-<version>.zip` and
//! publishes the zip as the `archive`, with `Package.swift` and every
//! `Package@swift-*.swift` next to it as `manifest` files. The registry name of the
//! package is `<scope>.<name>`: Swift packages declare no scope, so it comes from
//! `--scope`; the name is the manifest's `name`. Package.swift declares no version
//! either: it comes from `--version` or the release tag on `HEAD`.

#![deny(unsafe_code)]

use std::io::ErrorKind;
use std::path::Path;

use serde_json::{Map, Value};
use sreg_core::git::Git;
use sreg_core::{
    CommandOutput, CommandRunner, Ecosystem, Error, Package, PackageFile, PackageOptions, Packager,
    SystemRunner, Workdir,
};

/// Role of `Package.swift` and its `Package@swift-*.swift` variants.
pub const MANIFEST_ROLE: &str = "manifest";

/// The package manifest every Swift package has at its root.
pub const MANIFEST: &str = "Package.swift";

/// Prefix and suffix of the version-specific manifests (`Package@swift-5.8.swift`).
const VARIANT_PREFIX: &str = "Package@swift-";
const VARIANT_SUFFIX: &str = ".swift";

/// Packages Swift packages for publication.
#[derive(Debug, Clone, Default)]
pub struct SwiftPackager<R = SystemRunner> {
    runner: R,
    scope: Option<String>,
    repository_url: Option<String>,
}

impl SwiftPackager<SystemRunner> {
    /// A packager running the system's `swift` (and `git`, for the release tag and the
    /// `origin` remote).
    pub fn new() -> Self {
        SwiftPackager::with_runner(SystemRunner)
    }
}

impl<R: CommandRunner> SwiftPackager<R> {
    /// A packager running `swift` and `git` through `runner`.
    pub fn with_runner(runner: R) -> Self {
        SwiftPackager {
            runner,
            scope: None,
            repository_url: None,
        }
    }

    /// The registry scope the package is published under (`--scope`); required.
    pub fn scope(mut self, scope: Option<String>) -> Self {
        self.scope = scope;
        self
    }

    /// The repository URL recorded with the version (`--repository-url`); by default
    /// the URL of the `origin` remote, when there is one.
    pub fn repository_url(mut self, url: Option<String>) -> Self {
        self.repository_url = url;
        self
    }

    /// Runs `swift <args>` in `dir` and requires it to succeed.
    fn swift(&self, args: &[&str], dir: &Path) -> Result<CommandOutput, Error> {
        let command = format!("swift {}", args.join(" "));
        self.runner
            .run("swift", args, dir)
            .map_err(|err| match err {
                Error::Io(io) if io.kind() == ErrorKind::NotFound => Error::InvalidInput(
                    "`swift` was not found; install a Swift toolchain (5.7 or later) to \
                     publish Swift packages"
                        .into(),
                ),
                other => other,
            })?
            .into_success(&command)
    }

    /// The URL of the `origin` remote without credentials, or `None` when there is no
    /// such remote (or no git).
    fn origin_url(&self, dir: &Path) -> Option<String> {
        match self
            .runner
            .run("git", &["remote", "get-url", "origin"], dir)
        {
            Ok(output) if output.is_success() => {
                let url = output.stdout_text();
                let url = url.trim();
                (!url.is_empty()).then(|| strip_credentials(url))
            }
            Ok(output) => {
                tracing::debug!(stderr = %output.stderr_text(), "no origin remote");
                None
            }
            Err(err) => {
                tracing::debug!(error = %err, "cannot read the origin remote");
                None
            }
        }
    }
}

impl<R: CommandRunner> Packager for SwiftPackager<R> {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Swift
    }

    /// Describes and archives the package in `opts.path` into a temporary directory
    /// owned by the package.
    fn package(&self, opts: &PackageOptions) -> Result<Package, Error> {
        if opts.tag.is_some() {
            return Err(Error::InvalidInput(
                "dist-tags exist only for npm packages".into(),
            ));
        }
        check_extra_args(&opts.extra_args)?;
        let scope = self.scope.as_deref().ok_or_else(|| {
            Error::InvalidInput(
                "Swift packages are published as <scope>.<name> and Package.swift has no \
                 scope; pass --scope <scope>"
                    .into(),
            )
        })?;
        validate_scope(scope)?;
        let dir = opts.path.as_path();
        if !dir.join(MANIFEST).is_file() {
            return Err(Error::InvalidInput(format!(
                "`{}` does not exist; run sreg in the package directory or pass --path",
                dir.join(MANIFEST).display()
            )));
        }

        let version = Git::new(&self.runner, dir)
            .requested_version(opts.version.as_deref(), opts.tag_prefix.as_deref())?
            .ok_or_else(|| {
                Error::InvalidInput(
                    "no version given and no release tag (v<semver>) points at HEAD; \
                     Package.swift declares no version, so tag the release, pass --version, \
                     or pass --tag-prefix for tags of another form"
                        .into(),
                )
            })?;
        validate_version(&version)?;

        let dump = self.swift(&["package", "dump-package"], dir)?;
        let dump: Value = serde_json::from_slice(&dump.stdout).map_err(|err| {
            Error::InvalidInput(format!(
                "`swift package dump-package` did not print valid JSON: {err}"
            ))
        })?;
        let name = dump
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::InvalidInput(
                    "the output of `swift package dump-package` has no `name`".into(),
                )
            })?
            .to_owned();
        validate_name(&name)?;
        let tools_version = tools_version(&dump)?;

        let workdir = Workdir::new()?;
        let file_name = format!("{name}-{version}.zip");
        let archive = workdir.path().join(&file_name);
        let archive_arg = archive.to_str().ok_or_else(|| {
            Error::InvalidInput(format!(
                "the path `{}` is not valid UTF-8",
                archive.display()
            ))
        })?;
        let mut args = vec!["package", "archive-source", "-o", archive_arg];
        args.extend(opts.extra_args.iter().map(String::as_str));
        self.swift(&args, dir)?;
        if !archive.is_file() {
            return Err(Error::InvalidInput(format!(
                "`swift package archive-source` did not write {file_name}"
            )));
        }

        let mut files = vec![PackageFile::archive(&archive, file_name)?];
        for manifest in manifest_files(dir)? {
            files.push(PackageFile::from_path(
                dir.join(&manifest),
                Some(manifest),
                Some(MANIFEST_ROLE.to_owned()),
                Default::default(),
            )?);
        }

        let repository_url = match &self.repository_url {
            Some(url) => Some(url.clone()),
            None => self.origin_url(dir),
        };
        let mut manifest = Map::new();
        manifest.insert("toolsVersion".into(), tools_version.into());
        if let Some(url) = repository_url {
            manifest.insert("repositoryURL".into(), url.into());
        }
        manifest.insert("dumpPackage".into(), dump);

        Ok(Package {
            eco: Ecosystem::Swift,
            name: format!("{scope}.{name}"),
            version,
            files,
            manifest: Value::Object(manifest),
            workdir: Some(workdir),
            notes: Vec::new(),
        })
    }
}

/// `Package.swift` followed by the `Package@swift-*.swift` variants in `dir`, sorted.
fn manifest_files(dir: &Path) -> Result<Vec<String>, Error> {
    let mut variants = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let is_variant = name
            .strip_prefix(VARIANT_PREFIX)
            .and_then(|rest| rest.strip_suffix(VARIANT_SUFFIX))
            .is_some_and(|version| !version.is_empty());
        if is_variant {
            variants.push(name);
        }
    }
    variants.sort();
    let mut names = vec![MANIFEST.to_owned()];
    names.extend(variants);
    Ok(names)
}

/// The tools version of the dump (`toolsVersion._version`), `X.Y` or `X.Y.Z`.
fn tools_version(dump: &Value) -> Result<String, Error> {
    let version = dump
        .get("toolsVersion")
        .and_then(|tools| tools.get("_version"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            Error::InvalidInput(
                "the output of `swift package dump-package` has no `toolsVersion._version`".into(),
            )
        })?;
    let parts: Vec<&str> = version.split('.').collect();
    let numeric = parts
        .iter()
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    if !numeric || !(2..=3).contains(&parts.len()) {
        return Err(Error::InvalidInput(format!(
            "unexpected Swift tools version `{version}`; expected X.Y or X.Y.Z"
        )));
    }
    Ok(version.to_owned())
}

/// Removes the user name and password from a URL with a scheme
/// (`https://user:secret@host/x` → `https://host/x`); other remotes, such as
/// `git@github.com:acme/sdk.git`, are kept as they are.
fn strip_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(authority_end);
    match authority.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://{host}{path}"),
        None => url.to_owned(),
    }
}

/// Checks a registry scope: `[a-zA-Z0-9-]{1,39}`.
pub fn validate_scope(scope: &str) -> Result<(), Error> {
    let valid = (1..=39).contains(&scope.len())
        && scope.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "invalid Swift registry scope `{scope}`; expected 1 to 39 characters of A-Z, a-z, \
             0-9 and `-`"
        )))
    }
}

/// Checks a package name: `[a-zA-Z0-9_-]{1,100}`.
pub fn validate_name(name: &str) -> Result<(), Error> {
    let valid = (1..=100).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "invalid Swift package name `{name}` in Package.swift; the registry accepts 1 to \
             100 characters of A-Z, a-z, 0-9, `_` and `-`"
        )))
    }
}

/// Checks a version: semver 2, without a leading `v`.
pub fn validate_version(version: &str) -> Result<(), Error> {
    semver::Version::parse(version).map(|_| ()).map_err(|_| {
        Error::InvalidInput(format!(
            "invalid version `{version}`; Swift package versions are semver 2 without a \
             leading `v`, e.g. 1.2.3 or 2.0.0-beta.1"
        ))
    })
}

/// Rejects the flags of `swift package archive-source` that `sreg` sets itself: the
/// output file (`-o`, `--output`) and the package directory (`--package-path`, which
/// `--path` selects, so that the archive and the dumped manifest describe one package).
fn check_extra_args(args: &[String]) -> Result<(), Error> {
    let is = |arg: &str, flag: &str| {
        arg == flag
            || arg
                .strip_prefix(flag)
                .is_some_and(|rest| rest.starts_with('='))
    };
    let reserved = args.iter().find(|arg| {
        is(arg, "--output")
            || is(arg, "--package-path")
            || (arg.starts_with("-o") && !arg.starts_with("--"))
    });
    match reserved {
        Some(arg) => Err(Error::InvalidInput(format!(
            "`{arg}` cannot be passed to `swift package archive-source` after `--`: sreg sets \
             the output file itself and selects the package directory with --path"
        ))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn reports_swift_ecosystem() {
        assert_eq!(SwiftPackager::new().ecosystem(), Ecosystem::Swift);
    }

    #[test]
    fn scopes_and_names_follow_the_contract() {
        assert!(validate_scope("acme-corp42").is_ok());
        assert!(validate_scope(&"a".repeat(39)).is_ok());
        for scope in ["", "acme_corp", "acme.corp", "acme corp", &"a".repeat(40)] {
            assert!(validate_scope(scope).is_err(), "{scope}");
        }
        assert!(validate_name("Demo_Kit-2").is_ok());
        assert!(validate_name(&"a".repeat(100)).is_ok());
        for name in ["", "Demo Kit", "Demo.Kit", "acme/demo", &"a".repeat(101)] {
            assert!(validate_name(name).is_err(), "{name}");
        }
    }

    #[test]
    fn versions_are_semver_without_a_prefix() {
        assert!(validate_version("1.2.3").is_ok());
        assert!(validate_version("2.0.0-beta.1+build.5").is_ok());
        for version in ["v1.2.3", "1.2", "1", "latest", ""] {
            assert!(validate_version(version).is_err(), "{version}");
        }
    }

    #[test]
    fn tools_version_comes_from_the_dump() {
        let dump = |version: &str| json!({ "toolsVersion": { "_version": version } });
        assert_eq!(tools_version(&dump("5.9.0")).unwrap(), "5.9.0");
        assert_eq!(tools_version(&dump("6.0")).unwrap(), "6.0");
        for version in ["5", "5.9.0.1", "5.x", "5..0", ""] {
            assert!(tools_version(&dump(version)).is_err(), "{version}");
        }
        assert!(tools_version(&json!({ "toolsVersion": "5.9" })).is_err());
        assert!(tools_version(&json!({})).is_err());
    }

    #[test]
    fn credentials_are_stripped_from_remote_urls() {
        assert_eq!(
            strip_credentials("https://x-access-token:ghs_secret@github.com/acme/kit.git"),
            "https://github.com/acme/kit.git"
        );
        assert_eq!(
            strip_credentials("https://github.com/acme/kit.git"),
            "https://github.com/acme/kit.git"
        );
        assert_eq!(
            strip_credentials("ssh://git@example.com:2222/acme/kit.git"),
            "ssh://example.com:2222/acme/kit.git"
        );
        assert_eq!(
            strip_credentials("git@github.com:acme/kit.git"),
            "git@github.com:acme/kit.git"
        );
        assert_eq!(
            strip_credentials("https://host/path@x"),
            "https://host/path@x"
        );
    }

    #[test]
    fn the_output_flag_is_reserved() {
        for list in [
            &["-o", "x.zip"][..],
            &["--output", "x.zip"],
            &["--output=x.zip"],
            &["-ox.zip"],
            &["--package-path", "."],
            &["--package-path=."],
        ] {
            assert!(check_extra_args(&args(list)).is_err(), "{list:?}");
        }
        assert!(
            check_extra_args(&args(&["--verbose", "--outputs", "--scratch-path", "x"])).is_ok()
        );
    }
}
