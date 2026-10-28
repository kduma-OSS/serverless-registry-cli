//! Packaging the fixture library with real `git` and with the directory fallback.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sreg_core::{CommandOutput, CommandRunner, Ecosystem, Error, PackageOptions, Packager};
use sreg_eco_composer::ComposerPackager;
use tempfile::TempDir;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A copy of the fixture library in a fresh temporary directory.
fn library() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), dir.path());
    dir
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=sreg tests",
            "-c",
            "user.email=tests@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Initialises a repository in `dir` and commits everything in it.
fn commit_all(dir: &Path) {
    git(dir, &["init", "-q"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "release"]);
}

fn opts(dir: &Path, version: Option<&str>) -> PackageOptions {
    PackageOptions {
        path: dir.to_path_buf(),
        version: version.map(str::to_owned),
        ..PackageOptions::default()
    }
}

fn zip_names(path: &Path) -> Vec<String> {
    let archive = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
    archive.file_names().map(str::to_owned).collect()
}

fn fixture_manifest() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture().join("composer.json")).unwrap())
        .unwrap()
}

/// A runner for which `git` is not installed.
struct NoGit;

impl CommandRunner for NoGit {
    fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, Error> {
        Err(Error::Io(std::io::ErrorKind::NotFound.into()))
    }
}

fn invalid_input(err: &Error, needle: &str) -> bool {
    matches!(err, Error::InvalidInput(message) if message.contains(needle))
}

#[test]
fn packages_the_tagged_head_with_git_archive() {
    let dir = library();
    commit_all(dir.path());
    git(dir.path(), &["tag", "v1.2.3"]);

    let package = ComposerPackager::new()
        .package(&opts(dir.path(), None))
        .unwrap();

    assert_eq!(package.eco, Ecosystem::Composer);
    assert_eq!(package.name, "acme/sdk");
    assert_eq!(package.version, "1.2.3");
    assert_eq!(package.manifest, fixture_manifest());
    assert_eq!(package.files.len(), 1);
    let file = &package.files[0];
    assert_eq!(file.name, "sdk-1.2.3.zip");
    assert_eq!(file.role.as_deref(), Some("archive"));
    assert!(file.labels.is_empty());
    let (sha256, size) = sreg_core::digest::sha256_file(&file.path).unwrap();
    assert_eq!((file.sha256.as_str(), file.size), (sha256.as_str(), size));

    let names = zip_names(&file.path);
    for expected in [
        "composer.json",
        "README.md",
        "src/Client.php",
        "src/Http/Request.php",
    ] {
        assert!(names.iter().any(|name| name == expected), "{names:?}");
    }
    assert!(
        !names.iter().any(|name| name.starts_with("tests")),
        "export-ignore was not honoured: {names:?}"
    );

    // The archive lives exactly as long as the package.
    let path = file.path.clone();
    drop(package);
    assert!(!path.exists());
}

#[test]
fn version_flag_overrides_the_tag() {
    let dir = library();
    commit_all(dir.path());
    git(dir.path(), &["tag", "v1.2.3"]);

    let package = ComposerPackager::new()
        .package(&opts(dir.path(), Some("2.0.0-rc1")))
        .unwrap();

    assert_eq!(package.version, "2.0.0-rc1");
    assert_eq!(package.files[0].name, "sdk-2.0.0-rc1.zip");
}

#[test]
fn bare_semver_tags_need_an_empty_tag_prefix() {
    let dir = library();
    commit_all(dir.path());
    git(dir.path(), &["tag", "-a", "1.4.0-beta.2", "-m", "beta"]);
    let packager = ComposerPackager::new();

    let err = packager.package(&opts(dir.path(), None)).unwrap_err();
    assert!(invalid_input(&err, "--tag-prefix"), "{err}");

    let package = packager
        .package(&PackageOptions {
            tag_prefix: Some(String::new()),
            ..opts(dir.path(), None)
        })
        .unwrap();
    assert_eq!(package.version, "1.4.0-beta.2");
}

#[test]
fn several_release_tags_on_head_are_ambiguous() {
    let dir = library();
    commit_all(dir.path());
    git(dir.path(), &["tag", "v1.2.3"]);
    git(dir.path(), &["tag", "v1.3.0"]);

    let err = ComposerPackager::new()
        .package(&opts(dir.path(), None))
        .unwrap_err();

    assert!(invalid_input(&err, "more than one release tag"), "{err}");
}

#[test]
fn version_and_tag_prefix_cannot_be_combined() {
    let dir = library();
    let err = ComposerPackager::with_runner(NoGit)
        .package(&PackageOptions {
            tag_prefix: Some("sdk/v".into()),
            ..opts(dir.path(), Some("1.0.0"))
        })
        .unwrap_err();
    assert!(invalid_input(&err, "cannot be combined"), "{err}");
}

#[test]
fn passthrough_arguments_are_rejected() {
    let dir = library();
    let err = ComposerPackager::with_runner(NoGit)
        .package(&PackageOptions {
            extra_args: vec!["--foo".into()],
            ..opts(dir.path(), Some("1.0.0"))
        })
        .unwrap_err();
    assert!(invalid_input(&err, "arguments after `--`"), "{err}");
}

#[test]
fn untagged_head_without_a_version_is_rejected() {
    let dir = library();
    commit_all(dir.path());

    let err = ComposerPackager::new()
        .package(&opts(dir.path(), None))
        .unwrap_err();

    assert!(invalid_input(&err, "HEAD has no tag"), "{err}");
}

#[test]
fn tags_that_are_not_composer_releases_are_rejected() {
    let dir = library();
    commit_all(dir.path());
    git(dir.path(), &["tag", "release-2026"]);

    let err = ComposerPackager::new()
        .package(&opts(dir.path(), None))
        .unwrap_err();

    assert!(invalid_input(&err, "release-2026"), "{err}");
}

#[test]
fn uncommitted_composer_json_changes_are_rejected() {
    let dir = library();
    commit_all(dir.path());
    let path = dir.path().join("composer.json");
    let edited = std::fs::read_to_string(&path)
        .unwrap()
        .replace("^10.5", "^11.0");
    std::fs::write(&path, edited).unwrap();

    let err = ComposerPackager::new()
        .package(&opts(dir.path(), Some("1.0.0")))
        .unwrap_err();

    assert!(invalid_input(&err, "commit the changes"), "{err}");
}

#[test]
fn a_package_in_a_subdirectory_becomes_the_archive_root() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join("MONOREPO.md"), "monorepo\n").unwrap();
    let package_dir = repo.path().join("packages/sdk");
    copy_dir(&fixture(), &package_dir);
    copy_dir(&fixture(), &repo.path().join("packages/other"));
    commit_all(repo.path());
    git(repo.path(), &["tag", "v9.0.0"]);
    git(repo.path(), &["tag", "sdk/v1.1.0"]);
    git(repo.path(), &["tag", "other/v2.0.0"]);

    let package = ComposerPackager::new()
        .package(&PackageOptions {
            tag_prefix: Some("sdk/v".into()),
            ..opts(&package_dir, None)
        })
        .unwrap();

    assert_eq!(package.version, "1.1.0");
    assert_eq!(package.files[0].name, "sdk-1.1.0.zip");
    let names = zip_names(&package.files[0].path);
    assert!(
        names.iter().any(|name| name == "composer.json"),
        "{names:?}"
    );
    assert!(
        names.iter().any(|name| name == "src/Client.php"),
        "{names:?}"
    );
    assert!(!names.iter().any(|name| name.starts_with("packages/")));
    assert!(!names
        .iter()
        .any(|name| name == "MONOREPO.md" || name.starts_with("other")));
    // The package's own .gitattributes applies.
    assert!(
        !names.iter().any(|name| name.starts_with("tests")),
        "{names:?}"
    );
}

#[test]
fn an_untracked_package_directory_is_an_error() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join("README.md"), "monorepo\n").unwrap();
    commit_all(repo.path());
    let package_dir = repo.path().join("packages/new");
    copy_dir(&fixture(), &package_dir);

    let err = ComposerPackager::new()
        .package(&opts(&package_dir, Some("1.0.0")))
        .unwrap_err();

    assert!(
        matches!(err, Error::Command { ref command, .. } if command.contains("HEAD:packages/new")),
        "{err}"
    );
}

#[test]
fn falls_back_to_zipping_the_directory_without_git() {
    let dir = library();
    std::fs::create_dir_all(dir.path().join("vendor/composer")).unwrap();
    std::fs::write(dir.path().join("vendor/autoload.php"), "<?php\n").unwrap();
    std::fs::create_dir_all(dir.path().join(".git/refs")).unwrap();
    std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::create_dir_all(dir.path().join("src/Resources/vendor")).unwrap();
    std::fs::write(dir.path().join("src/Resources/vendor/app.js"), "x\n").unwrap();
    std::fs::create_dir_all(dir.path().join("bin")).unwrap();
    std::fs::write(dir.path().join("bin/sdk"), "#!/usr/bin/env php\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            dir.path().join("bin/sdk"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }

    let packager = ComposerPackager::with_runner(NoGit);
    let package = packager.package(&opts(dir.path(), Some("1.2.3"))).unwrap();

    assert_eq!(package.version, "1.2.3");
    assert_eq!(package.manifest, fixture_manifest());
    let file = &package.files[0];
    assert_eq!(file.name, "sdk-1.2.3.zip");

    let names = zip_names(&file.path);
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "entries are not sorted");
    assert_eq!(
        names,
        [
            ".gitattributes",
            "README.md",
            "bin/",
            "bin/sdk",
            "composer.json",
            "src/",
            "src/Client.php",
            "src/Http/",
            "src/Http/Request.php",
            "src/Resources/",
            "src/Resources/vendor/",
            "src/Resources/vendor/app.js",
            "tests/",
            "tests/ClientTest.php",
        ]
    );

    let mut archive = zip::ZipArchive::new(File::open(&file.path).unwrap()).unwrap();
    let mut composer_json = String::new();
    archive
        .by_name("composer.json")
        .unwrap()
        .read_to_string(&mut composer_json)
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&composer_json).unwrap(),
        fixture_manifest()
    );
    #[cfg(unix)]
    {
        let mode = archive.by_name("bin/sdk").unwrap().unix_mode().unwrap();
        assert_eq!(mode & 0o777, 0o755);
    }

    // Packaging the same tree again gives a byte-identical archive.
    let again = packager.package(&opts(dir.path(), Some("1.2.3"))).unwrap();
    assert_eq!(again.files[0].sha256, file.sha256);
}

#[test]
fn without_git_a_version_is_required() {
    let dir = library();

    let err = ComposerPackager::with_runner(NoGit)
        .package(&opts(dir.path(), None))
        .unwrap_err();

    assert!(invalid_input(&err, "not inside a git repository"), "{err}");
}

#[test]
fn versions_are_validated() {
    let dir = library();
    let packager = ComposerPackager::with_runner(NoGit);
    for (version, ok) in [
        ("1.2.3", true),
        ("1.0.0-alpha", true),
        ("1.0.0-beta.2", true),
        ("1.0.0-rc1", true),
        ("v1.2.3", false),
        ("1.2", false),
        ("1.2.3+build", false),
        ("1.0.0-preview.1", false),
        ("dev-main", false),
        ("1.x-dev", false),
    ] {
        let result = packager.package(&opts(dir.path(), Some(version)));
        assert_eq!(result.is_ok(), ok, "{version}: {result:?}");
    }
}

#[test]
fn composer_json_version_must_match() {
    let dir = library();
    let path = dir.path().join("composer.json");
    let mut manifest = fixture_manifest();
    manifest["version"] = "1.2.3".into();
    std::fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
    let packager = ComposerPackager::with_runner(NoGit);

    let package = packager.package(&opts(dir.path(), Some("1.2.3"))).unwrap();
    assert_eq!(package.manifest["version"], "1.2.3");

    let err = packager
        .package(&opts(dir.path(), Some("1.2.4")))
        .unwrap_err();
    assert!(invalid_input(&err, "declares version"), "{err}");
}

#[test]
fn composer_json_name_must_be_a_valid_composer_name() {
    let dir = library();
    let path = dir.path().join("composer.json");
    let packager = ComposerPackager::with_runner(NoGit);
    for name in ["Acme/SDK", "sdk", "acme/sdk/extra", "acme/my__sdk"] {
        let mut manifest = fixture_manifest();
        manifest["name"] = name.into();
        std::fs::write(&path, serde_json::to_string(&manifest).unwrap()).unwrap();

        let err = packager
            .package(&opts(dir.path(), Some("1.0.0")))
            .unwrap_err();
        assert!(invalid_input(&err, "invalid package name"), "{name}: {err}");
    }

    let mut manifest = fixture_manifest();
    manifest.as_object_mut().unwrap().remove("name");
    std::fs::write(&path, serde_json::to_string(&manifest).unwrap()).unwrap();
    let err = packager
        .package(&opts(dir.path(), Some("1.0.0")))
        .unwrap_err();
    assert!(invalid_input(&err, "no `name`"), "{err}");
}

#[test]
fn a_missing_composer_json_is_reported() {
    let dir = tempfile::tempdir().unwrap();

    let err = ComposerPackager::with_runner(NoGit)
        .package(&opts(dir.path(), Some("1.0.0")))
        .unwrap_err();

    assert!(
        invalid_input(&err, "composer.json` does not exist"),
        "{err}"
    );
}
