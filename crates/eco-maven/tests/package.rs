//! Publishing from the fixture repositories under `tests/fixtures`, with a fake `git`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use serde_json::json;
use sreg_core::{CommandOutput, CommandRunner, Ecosystem, Error, PackageOptions, Packager};
use sreg_eco_maven::MavenPackager;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn repo() -> PathBuf {
    fixture("repo")
}

/// A fake `git`: a repository with `tags` on `HEAD`, or none at all.
struct FakeGit {
    tags: Option<&'static str>,
    calls: RefCell<usize>,
}

impl FakeGit {
    fn untagged() -> Self {
        FakeGit::tagged("")
    }

    fn tagged(tags: &'static str) -> Self {
        FakeGit {
            tags: Some(tags),
            calls: RefCell::new(0),
        }
    }

    fn outside() -> Self {
        FakeGit {
            tags: None,
            calls: RefCell::new(0),
        }
    }
}

impl CommandRunner for FakeGit {
    fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, Error> {
        assert_eq!(program, "git", "only git may run");
        *self.calls.borrow_mut() += 1;
        Ok(match (self.tags, args[0]) {
            (None, _) => CommandOutput::failure(128, "fatal: not a git repository"),
            (Some(_), "rev-parse") => CommandOutput::success("/work"),
            (Some(tags), "tag") => CommandOutput::success(tags),
            (_, other) => panic!("unexpected git {other}"),
        })
    }
}

fn opts(dir: &Path, version: Option<&str>) -> PackageOptions {
    PackageOptions {
        path: dir.to_path_buf(),
        version: version.map(str::to_owned),
        ..PackageOptions::default()
    }
}

fn packager<'a>(git: &'a FakeGit, coords: Option<&str>) -> MavenPackager<&'a FakeGit> {
    MavenPackager::with_runner(git).coords(coords.map(str::to_owned))
}

fn invalid_input(err: &Error, needle: &str) -> bool {
    matches!(err, Error::InvalidInput(message) if message.contains(needle))
}

fn roles(package: &sreg_core::Package) -> Vec<(&str, &str)> {
    package
        .files
        .iter()
        .map(|file| (file.name.as_str(), file.role.as_deref().unwrap_or_default()))
        .collect()
}

#[test]
fn publishes_a_jar_with_sources_javadoc_and_module() {
    let git = FakeGit::untagged();

    let package = packager(&git, Some("com.acme:sdk"))
        .package(&opts(&repo(), Some("1.4.0")))
        .unwrap();

    assert_eq!(package.eco, Ecosystem::Maven);
    assert_eq!(package.name, "com.acme:sdk");
    assert_eq!(package.version, "1.4.0");
    assert!(package.workdir.is_none());
    assert!(package.notes.is_empty());
    assert_eq!(
        roles(&package),
        [
            ("sdk-1.4.0.pom", "pom"),
            ("sdk-1.4.0.jar", "archive"),
            ("sdk-1.4.0.module", "module"),
            ("sdk-1.4.0-sources.jar", "sources"),
            ("sdk-1.4.0-javadoc.jar", "javadoc"),
            ("sdk-1.4.0-all.jar", "extra"),
        ]
    );
    let dir = repo().join("com/acme/sdk/1.4.0");
    let jar = &package.files[1];
    assert_eq!(jar.path, dir.join("sdk-1.4.0.jar"));
    assert_eq!(
        jar.size,
        std::fs::metadata(dir.join("sdk-1.4.0.jar")).unwrap().len()
    );
    let pom = std::fs::read_to_string(dir.join("sdk-1.4.0.pom")).unwrap();
    assert_eq!(
        package.manifest,
        json!({
            "groupId": "com.acme",
            "artifactId": "sdk",
            "version": "1.4.0",
            "packaging": "jar",
            "pom": pom,
            "dependencies": [
                {
                    "groupId": "com.squareup.okhttp3", "artifactId": "okhttp",
                    "version": "4.12.0", "scope": "compile", "optional": false
                },
                {
                    "groupId": "com.fasterxml.jackson.core", "artifactId": "jackson-databind",
                    "version": null, "scope": "runtime", "optional": false
                },
                {
                    "groupId": "org.slf4j", "artifactId": "slf4j-api",
                    "version": "2.0.13", "scope": "runtime", "optional": true
                }
            ]
        })
    );
}

#[test]
fn coords_select_an_aar_with_its_only_version() {
    let git = FakeGit::outside();

    let package = packager(&git, Some("com.acme.android:widgets"))
        .package(&opts(&repo(), None))
        .unwrap();

    assert_eq!(package.name, "com.acme.android:widgets");
    assert_eq!(package.version, "2.0.0");
    assert_eq!(package.manifest["packaging"], "aar");
    assert_eq!(
        package.manifest["dependencies"][0],
        json!({
            "groupId": "com.acme", "artifactId": "sdk", "version": "1.4.0",
            "scope": "compile", "optional": false
        })
    );
    assert_eq!(
        roles(&package),
        [
            ("widgets-2.0.0.pom", "pom"),
            ("widgets-2.0.0.aar", "archive")
        ]
    );
}

#[test]
fn several_artifacts_need_coords() {
    let git = FakeGit::untagged();
    let err = packager(&git, None)
        .package(&opts(&repo(), Some("1.4.0")))
        .unwrap_err();
    assert!(
        invalid_input(
            &err,
            "several artifacts (com.acme.android:widgets, com.acme:sdk)"
        ),
        "{err}"
    );
    assert!(invalid_input(&err, "--coords"), "{err}");

    let err = packager(&git, Some("com.acme:client"))
        .package(&opts(&repo(), Some("1.4.0")))
        .unwrap_err();
    assert!(invalid_input(&err, "no artifact com.acme:client"), "{err}");

    let err = packager(&git, Some("com.acme"))
        .package(&opts(&repo(), Some("1.4.0")))
        .unwrap_err();
    assert!(invalid_input(&err, "invalid --coords"), "{err}");
}

#[test]
fn the_version_selects_the_version_directory() {
    // Several versions and nothing to choose with.
    let git = FakeGit::untagged();
    let err = packager(&git, Some("com.acme:sdk"))
        .package(&opts(&repo(), None))
        .unwrap_err();
    assert!(
        invalid_input(&err, "several versions of com.acme:sdk (1.3.0, 1.4.0)"),
        "{err}"
    );

    // The release tag on HEAD.
    let git = FakeGit::tagged("v1.3.0\n");
    let package = packager(&git, Some("com.acme:sdk"))
        .package(&opts(&repo(), None))
        .unwrap();
    assert_eq!(package.version, "1.3.0");
    assert_eq!(*git.calls.borrow(), 2);
    assert_eq!(
        package.manifest["dependencies"],
        json!([
            {
                "groupId": "com.squareup.okhttp3", "artifactId": "okhttp",
                "version": "4.11.0", "scope": "compile", "optional": false
            },
            {
                "groupId": "junit", "artifactId": "junit",
                "version": "4.13.2", "scope": "test", "optional": false
            }
        ])
    );
    assert_eq!(
        roles(&package),
        [("sdk-1.3.0.pom", "pom"), ("sdk-1.3.0.jar", "archive")]
    );

    // A tag with a prefix.
    let git = FakeGit::tagged("v9.0.0\nsdk/v1.4.0\n");
    let mut options = opts(&repo(), None);
    options.tag_prefix = Some("sdk/v".into());
    let package = packager(&git, Some("com.acme:sdk"))
        .package(&options)
        .unwrap();
    assert_eq!(package.version, "1.4.0");

    // A version that was not built.
    let git = FakeGit::untagged();
    let err = packager(&git, Some("com.acme:sdk"))
        .package(&opts(&repo(), Some("1.5.0")))
        .unwrap_err();
    assert!(
        invalid_input(
            &err,
            "has no version 1.5.0 of com.acme:sdk; it has: 1.3.0, 1.4.0"
        ),
        "{err}"
    );
}

#[test]
fn a_pom_with_a_parent_is_rejected() {
    let git = FakeGit::outside();
    let err = packager(&git, None)
        .package(&opts(&fixture("parent"), None))
        .unwrap_err();
    assert!(
        invalid_input(&err, "child-1.0.0.pom: the POM inherits from a <parent>"),
        "{err}"
    );
    assert!(invalid_input(&err, "mvn help:effective-pom"), "{err}");
}

#[test]
fn a_pom_with_a_placeholder_is_rejected() {
    let git = FakeGit::outside();
    let err = packager(&git, None)
        .package(&opts(&fixture("placeholder"), None))
        .unwrap_err();
    assert!(
        invalid_input(
            &err,
            "<dependency/version> of the POM is `${project.version}`"
        ),
        "{err}"
    );
    assert!(invalid_input(&err, "effective POM"), "{err}");
}

#[test]
fn snapshots_are_rejected() {
    let git = FakeGit::outside();
    let err = packager(&git, None)
        .package(&opts(&fixture("snapshot"), None))
        .unwrap_err();
    assert!(invalid_input(&err, "2.0.0-SNAPSHOT is a snapshot"), "{err}");

    let err = packager(&git, Some("com.acme:sdk"))
        .package(&opts(&repo(), Some("1.5.0-SNAPSHOT")))
        .unwrap_err();
    assert!(invalid_input(&err, "is a snapshot"), "{err}");
    assert_eq!(*git.calls.borrow(), 1);
}

#[test]
fn passthrough_arguments_and_dist_tags_are_rejected() {
    let git = FakeGit::untagged();
    let mut options = opts(&repo(), Some("1.4.0"));
    options.extra_args = vec!["-DskipTests".into()];
    let err = packager(&git, Some("com.acme:sdk"))
        .package(&options)
        .unwrap_err();
    assert!(invalid_input(&err, "arguments after `--`"), "{err}");

    let mut options = opts(&repo(), Some("1.4.0"));
    options.tag = Some("next".into());
    assert!(packager(&git, Some("com.acme:sdk"))
        .package(&options)
        .is_err());
    assert_eq!(*git.calls.borrow(), 0);
}

#[test]
fn an_empty_or_missing_directory_explains_how_to_deploy() {
    let git = FakeGit::outside();
    let empty = tempfile::tempdir().unwrap();
    let err = packager(&git, None)
        .package(&opts(empty.path(), None))
        .unwrap_err();
    assert!(invalid_input(&err, "holds no Maven artifact"), "{err}");
    assert!(
        invalid_input(&err, "-DaltDeploymentRepository=local::file:./out"),
        "{err}"
    );

    let err = packager(&git, None)
        .package(&opts(&empty.path().join("missing"), None))
        .unwrap_err();
    assert!(invalid_input(&err, "is not a directory"), "{err}");
}

/// Copies one version directory of the fixture repository into a new repository.
fn single_version(version_dir: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let from = repo().join(version_dir);
    let to = root.path().join(version_dir);
    std::fs::create_dir_all(&to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
    root
}

#[test]
fn the_archive_is_required_unless_the_packaging_is_pom() {
    let git = FakeGit::outside();
    let root = single_version("com/acme/sdk/1.3.0");
    let dir = root.path().join("com/acme/sdk/1.3.0");
    std::fs::remove_file(dir.join("sdk-1.3.0.jar")).unwrap();

    let err = packager(&git, None)
        .package(&opts(root.path(), None))
        .unwrap_err();
    assert!(invalid_input(&err, "has no sdk-1.3.0.jar"), "{err}");

    let pom = std::fs::read_to_string(dir.join("sdk-1.3.0.pom")).unwrap();
    std::fs::write(
        dir.join("sdk-1.3.0.pom"),
        pom.replace("<packaging>jar</packaging>", "<packaging>pom</packaging>"),
    )
    .unwrap();
    let package = packager(&git, None)
        .package(&opts(root.path(), None))
        .unwrap();
    assert_eq!(package.manifest["packaging"], "pom");
    assert_eq!(roles(&package), [("sdk-1.3.0.pom", "pom")]);
}

#[test]
fn the_pom_must_describe_its_location() {
    let git = FakeGit::outside();
    let root = single_version("com/acme/sdk/1.3.0");
    let pom = root.path().join("com/acme/sdk/1.3.0/sdk-1.3.0.pom");
    let text = std::fs::read_to_string(&pom).unwrap();
    std::fs::write(
        &pom,
        text.replace(
            "<groupId>com.acme</groupId>\n  <artifactId>sdk",
            "<groupId>org.acme</groupId>\n  <artifactId>sdk",
        ),
    )
    .unwrap();

    let err = packager(&git, None)
        .package(&opts(root.path(), None))
        .unwrap_err();
    assert!(
        invalid_input(
            &err,
            "declares org.acme:sdk:1.3.0, but it is stored as com.acme:sdk:1.3.0"
        ),
        "{err}"
    );
}
