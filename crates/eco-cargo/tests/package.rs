//! Packaging with a fake `cargo` answering with canned `cargo metadata` output, and
//! with the real `cargo` on a crate without dependencies.

use std::cell::RefCell;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use sreg_core::{CommandOutput, CommandRunner, Ecosystem, Error, PackageOptions, Packager};
use sreg_eco_cargo::{CargoPackager, CRATES_IO_INDEX};
use tempfile::TempDir;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
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

/// A copy of the fixture `name` in a fresh temporary directory.
fn copy_fixture(name: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixtures().join(name), dir.path());
    dir
}

fn package_version(manifest: &Path) -> String {
    let document: toml_edit::DocumentMut =
        std::fs::read_to_string(manifest).unwrap().parse().unwrap();
    document["package"]["version"].as_str().unwrap().to_owned()
}

/// A fake `cargo` for the fixture workspace in `root` (and a `git` outside any
/// repository): `cargo metadata` answers with the canned output, reflecting the version
/// currently in `core/Cargo.toml`, and `cargo package` writes a dummy `.crate`.
struct FakeCargo {
    root: PathBuf,
    calls: RefCell<Vec<Vec<String>>>,
}

impl FakeCargo {
    fn new(root: &Path) -> Self {
        FakeCargo {
            root: root.to_path_buf(),
            calls: RefCell::new(Vec::new()),
        }
    }

    /// The `cargo` calls, with the temporary target directory replaced by `<tmp>`.
    fn cargo_calls(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .filter(|call| call[0] == "cargo")
            .map(|call| {
                let mut call = call.clone();
                if let Some(index) = call.iter().position(|arg| arg == "--target-dir") {
                    call[index + 1] = "<tmp>".to_owned();
                }
                call.join(" ")
            })
            .collect()
    }

    fn metadata(&self) -> String {
        std::fs::read_to_string(fixtures().join("workspace/metadata.json"))
            .unwrap()
            .replace("@ROOT@", self.root.to_str().unwrap())
            .replace(
                "@CORE_VERSION@",
                &package_version(&self.root.join("core/Cargo.toml")),
            )
    }
}

impl CommandRunner for FakeCargo {
    fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, Error> {
        let mut call = vec![program.to_owned()];
        call.extend(args.iter().map(|arg| (*arg).to_owned()));
        self.calls.borrow_mut().push(call);
        match (program, args.first()) {
            ("git", _) => Ok(CommandOutput::failure(128, "fatal: not a git repository")),
            ("cargo", Some(&"metadata")) => Ok(CommandOutput::success(self.metadata())),
            ("cargo", Some(&"package")) => {
                let value = |flag: &str| {
                    let index = args.iter().position(|arg| *arg == flag).unwrap();
                    args[index + 1]
                };
                let name = value("-p");
                let metadata: Value = serde_json::from_str(&self.metadata()).unwrap();
                let version = metadata["packages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|package| package["name"] == name)
                    .unwrap()["version"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let dir = Path::new(value("--target-dir")).join("package");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(format!("{name}-{version}.crate")), b"crate").unwrap();
                Ok(CommandOutput::success(""))
            }
            _ => Ok(CommandOutput::failure(1, "unexpected command")),
        }
    }
}

fn opts(dir: &Path, version: Option<&str>) -> PackageOptions {
    PackageOptions {
        path: dir.to_path_buf(),
        version: version.map(str::to_owned),
        ..PackageOptions::default()
    }
}

fn invalid_input(err: &Error, needle: &str) -> bool {
    matches!(err, Error::InvalidInput(message) if message.contains(needle))
}

fn core_index_line(version: &str) -> Value {
    let dep = |name: &str, req: &str| {
        json!({
            "name": name, "req": req, "features": [], "optional": false,
            "default_features": true, "target": null, "kind": "normal",
            "registry": CRATES_IO_INDEX, "package": null
        })
    };
    let mut util = dep("acme-util", "^0.2");
    util["registry"] = Value::Null;
    let mut json_dep = dep("json", "^1");
    json_dep["optional"] = json!(true);
    json_dep["default_features"] = json!(false);
    json_dep["package"] = json!("serde_json");
    let mut serde = dep("serde", "^1.0");
    serde["optional"] = json!(true);
    serde["features"] = json!(["derive"]);
    let mut cc = dep("cc", "^1");
    cc["kind"] = json!("build");
    let mut libc = dep("libc", "^0.2");
    libc["target"] = json!("cfg(unix)");
    json!({
        "name": "acme-core",
        "vers": version,
        "deps": [util, json_dep, serde, dep("tracing", "^0.1"), cc, libc],
        "features": { "default": ["std"], "full": ["std", "json"], "std": [] },
        "features2": { "json": ["dep:json", "serde?/std"] },
        "links": "acme",
        "rust_version": "1.75"
    })
}

#[test]
fn rewrites_the_version_and_builds_the_index_line() {
    let root = copy_fixture("workspace");
    let fake = FakeCargo::new(root.path());
    let mut options = opts(&root.path().join("core/src"), Some("1.5.0"));
    options.extra_args = vec!["--locked".into()];

    let package = CargoPackager::with_runner(&fake).package(&options).unwrap();

    assert_eq!(
        fake.cargo_calls(),
        [
            "cargo metadata --format-version 1 --no-deps",
            "cargo metadata --format-version 1 --no-deps",
            "cargo package --no-verify --allow-dirty -p acme-core --target-dir <tmp> --locked",
        ]
    );
    assert_eq!(package.eco, Ecosystem::Cargo);
    assert_eq!(package.name, "acme-core");
    assert_eq!(package.version, "1.5.0");
    assert_eq!(package.manifest, core_index_line("1.5.0"));
    assert_eq!(package.files.len(), 1);
    assert_eq!(package.files[0].name, "acme-core-1.5.0.crate");
    assert_eq!(package.files[0].role.as_deref(), Some("archive"));
    assert_eq!(package.notes.len(), 1);
    assert!(
        package.notes[0].contains("to 1.5.0 (was 1.4.0)"),
        "{:?}",
        package.notes
    );

    // Only the version changed, comments and all.
    let original = std::fs::read_to_string(fixtures().join("workspace/core/Cargo.toml")).unwrap();
    let edited = std::fs::read_to_string(root.path().join("core/Cargo.toml")).unwrap();
    assert_eq!(
        edited,
        original.replace(
            "version = \"1.4.0\" # bumped",
            "version = \"1.5.0\" # bumped"
        )
    );
}

#[test]
fn package_flag_selects_a_member_from_the_root() {
    let root = copy_fixture("workspace");
    let fake = FakeCargo::new(root.path());

    let package = CargoPackager::with_runner(&fake)
        .package_name(Some("acme-core".into()))
        .package(&opts(root.path(), None))
        .unwrap();

    assert_eq!(
        fake.cargo_calls(),
        [
            "cargo metadata --format-version 1 --no-deps",
            "cargo package --no-verify --allow-dirty -p acme-core --target-dir <tmp>",
        ]
    );
    assert_eq!(package.version, "1.4.0");
    assert_eq!(package.manifest, core_index_line("1.4.0"));
    assert!(package.notes.is_empty());
}

#[test]
fn manifest_path_is_passed_to_cargo_metadata() {
    let root = copy_fixture("workspace");
    let fake = FakeCargo::new(root.path());
    let mut options = opts(root.path(), None);
    options.extra_args = vec!["--manifest-path".into(), "core/Cargo.toml".into()];

    let package = CargoPackager::with_runner(&fake).package(&options).unwrap();

    assert_eq!(package.name, "acme-core");
    assert_eq!(
        fake.cargo_calls()[0],
        "cargo metadata --format-version 1 --no-deps --manifest-path core/Cargo.toml"
    );
}

#[test]
fn several_packages_need_the_package_flag() {
    let root = copy_fixture("workspace");
    let fake = FakeCargo::new(root.path());

    let err = CargoPackager::with_runner(&fake)
        .package(&opts(root.path(), Some("1.0.0")))
        .unwrap_err();

    assert!(
        invalid_input(&err, "several packages (acme-core, acme-util, acme-helper)"),
        "{err}"
    );
    assert!(invalid_input(&err, "--package"), "{err}");
}

#[test]
fn a_version_inherited_from_the_workspace_is_not_rewritten() {
    let root = copy_fixture("workspace");
    let fake = FakeCargo::new(root.path());
    let packager = CargoPackager::with_runner(&fake).package_name(Some("acme-util".into()));
    let manifest = root.path().join("util/Cargo.toml");
    let before = std::fs::read_to_string(&manifest).unwrap();

    let err = packager
        .package(&opts(root.path(), Some("0.3.0")))
        .unwrap_err();

    assert!(invalid_input(&err, "version.workspace = true"), "{err}");
    assert!(invalid_input(&err, "util/Cargo.toml"), "{err}");
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), before);
    assert!(!fake
        .cargo_calls()
        .iter()
        .any(|call| call.contains("package ")));

    // The version the workspace already sets needs no rewrite.
    let package = packager.package(&opts(root.path(), Some("0.2.0"))).unwrap();
    assert_eq!(package.version, "0.2.0");
    assert_eq!(package.manifest["features"], json!({}));
    assert!(package.manifest.get("features2").is_none());
}

#[test]
fn invalid_input_is_rejected_before_running_cargo() {
    let root = copy_fixture("workspace");
    let fake = FakeCargo::new(root.path());
    let packager = CargoPackager::with_runner(&fake);

    let mut reserved = opts(root.path(), None);
    reserved.extra_args = vec!["--target-dir=/elsewhere".into()];
    let err = packager.package(&reserved).unwrap_err();
    assert!(
        invalid_input(&err, "cannot be passed to `cargo package`"),
        "{err}"
    );

    let mut workspace = opts(root.path(), None);
    workspace.extra_args = vec!["--workspace".into()];
    assert!(packager.package(&workspace).is_err());

    assert!(fake.cargo_calls().is_empty());

    let err = packager
        .package(&opts(&root.path().join("core"), Some("v1.0.0")))
        .unwrap_err();
    assert!(invalid_input(&err, "invalid version"), "{err}");
    assert_eq!(
        package_version(&root.path().join("core/Cargo.toml")),
        "1.4.0"
    );
}

#[test]
fn cargo_failures_are_reported() {
    struct Failing;
    impl CommandRunner for Failing {
        fn run(&self, _: &str, _: &[&str], _: &Path) -> Result<CommandOutput, Error> {
            Ok(CommandOutput::failure(
                101,
                "error: could not find `Cargo.toml`",
            ))
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let err = CargoPackager::with_runner(Failing)
        .package(&opts(dir.path(), None))
        .unwrap_err();
    assert!(
        matches!(&err, Error::Command { command, message }
            if command.starts_with("cargo metadata") && message.contains("Cargo.toml")),
        "{err:?}"
    );
}

fn cargo_available() -> bool {
    Command::new("cargo")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn packages_the_fixture_with_real_cargo() {
    if !cargo_available() {
        eprintln!("skipping: cargo is not installed");
        return;
    }
    let dir = copy_fixture("demo");

    let package = CargoPackager::new()
        .package(&opts(dir.path(), Some("1.2.3")))
        .unwrap();

    assert_eq!(package.name, "acme-demo");
    assert_eq!(package.version, "1.2.3");
    assert_eq!(
        package.manifest,
        json!({
            "name": "acme-demo",
            "vers": "1.2.3",
            "deps": [],
            "features": { "default": ["std"], "extra": ["std"], "std": [] },
            "links": null,
            "rust_version": "1.70"
        })
    );
    let manifest = std::fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(
        manifest.contains("version = \"1.2.3\" # bumped by the release workflow"),
        "{manifest}"
    );

    let file = &package.files[0];
    assert_eq!(file.name, "acme-demo-1.2.3.crate");
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
    assert!(
        names.contains(&"acme-demo-1.2.3/src/lib.rs".to_owned()),
        "{names:?}"
    );

    let path = file.path.clone();
    drop(package);
    assert!(!path.exists());
}

#[test]
fn real_cargo_reports_an_unknown_package() {
    if !cargo_available() {
        eprintln!("skipping: cargo is not installed");
        return;
    }
    let dir = copy_fixture("demo");

    let err = CargoPackager::new()
        .package_name(Some("missing".into()))
        .package(&opts(dir.path(), None))
        .unwrap_err();

    assert!(invalid_input(&err, "no package `missing`"), "{err}");
}
