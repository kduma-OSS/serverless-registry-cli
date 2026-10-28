//! Packaging with a fake `npm` (command lines, error handling) and with the real one.

use std::cell::RefCell;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use sreg_core::{CommandOutput, CommandRunner, Ecosystem, Error, PackageOptions, Packager};
use sreg_eco_npm::NpmPackager;
use tempfile::TempDir;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo")
}

fn fixture_manifest() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture().join("package.json")).unwrap()).unwrap()
}

/// A copy of the fixture package in a fresh temporary directory.
fn project() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(fixture()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
    }
    dir
}

fn opts(dir: &Path, version: Option<&str>) -> PackageOptions {
    PackageOptions {
        path: dir.to_path_buf(),
        version: version.map(str::to_owned),
        ..PackageOptions::default()
    }
}

/// Writes a gzipped tarball with `package/package.json` (when given) and
/// `package/index.js`.
fn write_tarball(path: &Path, package_json: Option<&Value>) {
    let mut builder = tar::Builder::new(GzEncoder::new(
        File::create(path).unwrap(),
        Compression::default(),
    ));
    let mut add = |name: &str, data: &[u8]| {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, data).unwrap();
    };
    add("package/index.js", b"module.exports = 1;\n");
    if let Some(manifest) = package_json {
        add(
            "package/package.json",
            serde_json::to_string_pretty(manifest).unwrap().as_bytes(),
        );
    }
    builder.into_inner().unwrap().finish().unwrap();
}

/// A fake `npm` (and a `git` outside any repository): `npm pack` writes a tarball
/// holding `package_json` into the pack destination and reports it.
struct FakeNpm {
    package_json: Option<Value>,
    calls: RefCell<Vec<Vec<String>>>,
}

impl FakeNpm {
    fn new(package_json: Option<Value>) -> Self {
        FakeNpm {
            package_json,
            calls: RefCell::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.borrow().clone()
    }

    /// The `npm` calls with the temporary pack destination replaced by `<tmp>`.
    fn npm_calls(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| call[0] == "npm")
            .map(|call| {
                let mut call = call;
                if let Some(index) = call.iter().position(|arg| arg == "--pack-destination") {
                    call[index + 1] = "<tmp>".to_owned();
                }
                call.join(" ")
            })
            .collect()
    }
}

impl CommandRunner for FakeNpm {
    fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, Error> {
        let mut call = vec![program.to_owned()];
        call.extend(args.iter().map(|arg| (*arg).to_owned()));
        self.calls.borrow_mut().push(call);
        match (program, args.first()) {
            ("git", _) => Ok(CommandOutput::failure(128, "fatal: not a git repository")),
            ("npm", Some(&"pkg")) => Ok(CommandOutput::success("")),
            ("npm", Some(&"pack")) => {
                let index = args.iter().position(|arg| *arg == "--pack-destination");
                let destination = PathBuf::from(args[index.unwrap() + 1]);
                let manifest = self.package_json.clone().unwrap_or(json!({}));
                let name = manifest["name"].as_str().unwrap_or("x").to_owned();
                let version = manifest["version"].as_str().unwrap_or("0.0.0").to_owned();
                let filename = format!(
                    "{}-{version}.tgz",
                    name.trim_start_matches('@').replace('/', "-")
                );
                write_tarball(&destination.join(&filename), self.package_json.as_ref());
                let report = json!([{
                    "id": format!("{name}@{version}"),
                    "name": name,
                    "version": version,
                    "filename": filename,
                    "integrity": "sha512-x",
                    "shasum": "x",
                    "files": []
                }]);
                Ok(CommandOutput::success(report.to_string()))
            }
            _ => Ok(CommandOutput::failure(1, "unexpected command")),
        }
    }
}

fn invalid_input(err: &Error, needle: &str) -> bool {
    matches!(err, Error::InvalidInput(message) if message.contains(needle))
}

#[test]
fn sets_the_version_before_packing_the_workspace() {
    let dir = project();
    let mut manifest = fixture_manifest();
    manifest["version"] = json!("2.0.0");
    manifest["_id"] = json!("@acme/demo@2.0.0");
    let fake = FakeNpm::new(Some(manifest));
    let packager = NpmPackager::with_runner(&fake).workspace(Some("packages/demo".into()));

    let mut options = opts(dir.path(), Some("2.0.0"));
    options.extra_args = vec!["--ignore-scripts".into()];
    options.tag = Some("beta".into());
    let package = packager.package(&options).unwrap();

    assert_eq!(
        fake.npm_calls(),
        [
            "npm pkg set version=2.0.0 -w packages/demo",
            "npm pack --json --pack-destination <tmp> -w packages/demo --ignore-scripts",
        ]
    );
    // --version is used as is: git is not asked for a tag.
    assert!(fake.calls().iter().all(|call| call[0] == "npm"));
    assert_eq!(package.eco, Ecosystem::Npm);
    assert_eq!(package.name, "@acme/demo");
    assert_eq!(package.version, "2.0.0");
    assert!(package.manifest.get("_id").is_none());
    assert_eq!(
        package.manifest["dependencies"],
        fixture_manifest()["dependencies"]
    );
    assert_eq!(package.files.len(), 1);
    assert_eq!(package.files[0].name, "acme-demo-2.0.0.tgz");
    assert_eq!(package.files[0].role.as_deref(), Some("archive"));
    assert_eq!(package.notes.len(), 1);
    assert!(
        package.notes[0].contains("workspace `packages/demo`"),
        "{:?}",
        package.notes
    );
}

#[test]
fn keeps_the_manifest_version_without_flag_or_tag() {
    let dir = project();
    let fake = FakeNpm::new(Some(fixture_manifest()));

    let package = NpmPackager::with_runner(&fake)
        .package(&opts(dir.path(), None))
        .unwrap();

    assert_eq!(
        fake.npm_calls(),
        ["npm pack --json --pack-destination <tmp>"]
    );
    assert_eq!(package.version, "0.1.0");
    assert_eq!(package.manifest, fixture_manifest());
    assert!(package.notes.is_empty());
}

#[test]
fn a_tarball_without_package_json_is_an_error() {
    let dir = project();
    let fake = FakeNpm::new(None);

    let err = NpmPackager::with_runner(&fake)
        .package(&opts(dir.path(), None))
        .unwrap_err();

    assert!(invalid_input(&err, "has no package/package.json"), "{err}");
}

#[test]
fn unscoped_packages_are_rejected() {
    let dir = project();
    let mut manifest = fixture_manifest();
    manifest["name"] = json!("demo");
    let fake = FakeNpm::new(Some(manifest));

    let err = NpmPackager::with_runner(&fake)
        .package(&opts(dir.path(), None))
        .unwrap_err();

    assert!(invalid_input(&err, "scoped packages only"), "{err}");
}

#[test]
fn the_tarball_must_carry_the_requested_version() {
    let dir = project();
    let fake = FakeNpm::new(Some(fixture_manifest()));

    let err = NpmPackager::with_runner(&fake)
        .package(&opts(dir.path(), Some("1.0.0")))
        .unwrap_err();

    assert!(invalid_input(&err, "contains version 0.1.0"), "{err}");
}

#[test]
fn invalid_input_is_rejected_before_running_npm() {
    let dir = project();
    let fake = FakeNpm::new(Some(fixture_manifest()));
    let packager = NpmPackager::with_runner(&fake);

    let mut reserved = opts(dir.path(), Some("1.0.0"));
    reserved.extra_args = vec!["--pack-destination=/elsewhere".into()];
    let err = packager.package(&reserved).unwrap_err();
    assert!(
        invalid_input(&err, "cannot be passed to `npm pack`"),
        "{err}"
    );

    let mut workspace = opts(dir.path(), Some("1.0.0"));
    workspace.extra_args = vec!["-w".into(), "other".into()];
    assert!(packager.package(&workspace).is_err());

    let err = packager
        .package(&opts(dir.path(), Some("v1.0.0")))
        .unwrap_err();
    assert!(invalid_input(&err, "invalid version"), "{err}");

    let mut tag = opts(dir.path(), Some("1.0.0"));
    tag.tag = Some("1.x".into());
    assert!(invalid_input(
        &packager.package(&tag).unwrap_err(),
        "dist-tag"
    ));

    let empty = tempfile::tempdir().unwrap();
    let err = packager
        .package(&opts(empty.path(), Some("1.0.0")))
        .unwrap_err();
    assert!(invalid_input(&err, "has no package.json"), "{err}");

    assert!(fake.npm_calls().is_empty(), "{:?}", fake.calls());
}

#[test]
fn npm_failures_are_reported() {
    struct Failing;
    impl CommandRunner for Failing {
        fn run(&self, program: &str, _: &[&str], _: &Path) -> Result<CommandOutput, Error> {
            Ok(match program {
                "git" => CommandOutput::failure(128, "not a repository"),
                _ => CommandOutput::failure(1, "npm error code E404"),
            })
        }
    }
    let dir = project();
    let err = NpmPackager::with_runner(Failing)
        .package(&opts(dir.path(), Some("1.0.0")))
        .unwrap_err();
    assert!(
        matches!(&err, Error::Command { command, message }
            if command == "npm pkg set version=1.0.0" && message.contains("E404")),
        "{err:?}"
    );
}

fn npm_available() -> bool {
    Command::new("npm")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn packs_the_fixture_with_real_npm() {
    if !npm_available() {
        eprintln!("skipping: npm is not installed");
        return;
    }
    let dir = project();

    let package = NpmPackager::new()
        .package(&opts(dir.path(), Some("1.2.3")))
        .unwrap();

    assert_eq!(package.name, "@acme/demo");
    assert_eq!(package.version, "1.2.3");
    let mut expected = fixture_manifest();
    expected["version"] = json!("1.2.3");
    assert_eq!(package.manifest, expected);
    // package.json on disk was updated and the change is reported.
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("package.json")).unwrap())
            .unwrap();
    assert_eq!(on_disk["version"], "1.2.3");
    assert_eq!(package.notes.len(), 1);

    let file = &package.files[0];
    assert_eq!(file.name, "acme-demo-1.2.3.tgz");
    let (sha256, size) = sreg_core::digest::sha256_file(&file.path).unwrap();
    assert_eq!((file.sha256.as_str(), file.size), (sha256.as_str(), size));

    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
        File::open(&file.path).unwrap(),
    ));
    let names: Vec<String> = archive
        .entries()
        .unwrap()
        .map(|entry| entry.unwrap().path().unwrap().display().to_string())
        .collect();
    assert!(names.contains(&"package/index.js".to_owned()), "{names:?}");
    assert!(
        !names.contains(&"package/notes.txt".to_owned()),
        "{names:?}"
    );

    let path = file.path.clone();
    drop(package);
    assert!(!path.exists());
}

#[test]
fn real_npm_reports_an_unknown_workspace() {
    if !npm_available() {
        eprintln!("skipping: npm is not installed");
        return;
    }
    let dir = project();

    let err = NpmPackager::new()
        .workspace(Some("missing".into()))
        .package(&opts(dir.path(), None))
        .unwrap_err();

    assert!(
        matches!(&err, Error::Command { command, .. } if command.starts_with("npm pack")),
        "{err:?}"
    );
}

mod archive_mode {
    use sreg_core::ArchiveOptions;

    use super::*;

    fn archive_opts(path: &Path, version: Option<&str>) -> ArchiveOptions {
        ArchiveOptions {
            path: path.to_path_buf(),
            version: version.map(str::to_owned),
        }
    }

    fn manifest() -> Value {
        json!({
            "name": "@acme/demo", "version": "1.2.3",
            "dependencies": { "left-pad": "^1.0.0" }, "_id": "dropped"
        })
    }

    #[test]
    fn describes_the_tarball_without_touching_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.tgz");
        write_tarball(&path, Some(&manifest()));
        let before = std::fs::read(&path).unwrap();

        // A runner that panics proves that no tool (git, npm) is invoked.
        let package = NpmPackager::with_runner(Unreachable)
            .package_archive(&archive_opts(&path, Some("1.2.3")))
            .unwrap();

        assert_eq!(package.name, "@acme/demo");
        assert_eq!(package.version, "1.2.3");
        assert_eq!(
            package.manifest,
            json!({ "name": "@acme/demo", "version": "1.2.3",
                    "dependencies": { "left-pad": "^1.0.0" } })
        );
        let file = &package.files[0];
        assert_eq!(file.path, path);
        assert_eq!(file.name, "acme-demo-1.2.3.tgz");
        assert_eq!(file.role.as_deref(), Some("archive"));
        assert_eq!(file.size, before.len() as u64);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(package.workdir.is_none());
    }

    #[test]
    fn version_is_only_a_consistency_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.tgz");
        write_tarball(&path, Some(&manifest()));
        let err = NpmPackager::with_runner(Unreachable)
            .package_archive(&archive_opts(&path, Some("1.2.4")))
            .unwrap_err();
        assert!(err.to_string().contains("1.2.3"), "{err}");
        assert!(err.to_string().contains("1.2.4"), "{err}");
    }

    #[test]
    fn malformed_archives_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let packager = NpmPackager::with_runner(Unreachable);

        let no_manifest = dir.path().join("a.tgz");
        write_tarball(&no_manifest, None);
        let garbage = dir.path().join("b.tgz");
        std::fs::write(&garbage, b"not a tarball").unwrap();
        let unscoped = dir.path().join("c.tgz");
        write_tarball(
            &unscoped,
            Some(&json!({ "name": "demo", "version": "1.0.0" })),
        );
        let no_version = dir.path().join("d.tgz");
        write_tarball(&no_version, Some(&json!({ "name": "@acme/demo" })));
        let bad_version = dir.path().join("e.tgz");
        write_tarball(
            &bad_version,
            Some(&json!({ "name": "@acme/demo", "version": "one" })),
        );
        let truncated = dir.path().join("f.tgz");
        let mut bytes = Vec::new();
        {
            let full = dir.path().join("full.tgz");
            write_tarball(&full, Some(&manifest()));
            bytes.extend(std::fs::read(&full).unwrap());
        }
        bytes.truncate(bytes.len() - 12);
        std::fs::write(&truncated, bytes).unwrap();

        for path in [
            no_manifest,
            garbage,
            unscoped,
            no_version,
            bad_version,
            truncated,
            dir.path().join("missing.tgz"),
        ] {
            assert!(
                packager
                    .package_archive(&archive_opts(&path, None))
                    .is_err(),
                "{}",
                path.display()
            );
        }
    }

    /// A runner that fails the test when anything is run.
    struct Unreachable;

    impl CommandRunner for Unreachable {
        fn run(&self, program: &str, _: &[&str], _: &Path) -> Result<CommandOutput, Error> {
            panic!("archive mode must not run {program}");
        }
    }
}
