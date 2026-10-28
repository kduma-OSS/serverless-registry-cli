//! Packaging with a fake `swift` that prints the fixture `dump-package.json` and writes a
//! zip of the fixture package for `archive-source`; no Swift toolchain is needed.

use std::cell::RefCell;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sreg_core::{CommandOutput, CommandRunner, Ecosystem, Error, PackageOptions, Packager};
use sreg_eco_swift::SwiftPackager;
use zip::write::SimpleFileOptions;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture_package() -> PathBuf {
    fixtures().join("DemoKit")
}

fn dump_package() -> Value {
    let text = std::fs::read_to_string(fixtures().join("dump-package.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// A fake `swift` and `git`.
struct FakeSwift {
    /// `git rev-parse --show-toplevel`; `None` outside a repository.
    toplevel: Option<&'static str>,
    /// `git tag --points-at HEAD`.
    tags: &'static str,
    /// `git remote get-url origin`; `None` when there is no `origin`.
    origin: Option<&'static str>,
    /// The dump `swift package dump-package` prints.
    dump: Value,
    /// Whether `swift` is installed.
    installed: bool,
    calls: RefCell<Vec<Vec<String>>>,
}

impl FakeSwift {
    fn new() -> Self {
        FakeSwift {
            toplevel: Some("/work"),
            tags: "v1.2.0\n",
            origin: Some("https://github.com/acme/DemoKit.git"),
            dump: dump_package(),
            installed: true,
            calls: RefCell::new(Vec::new()),
        }
    }

    /// The calls of `program`, with the temporary output file replaced by `<tmp>/<name>`.
    fn calls_of(&self, program: &str) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .filter(|call| call[0] == program)
            .map(|call| {
                let mut call = call.clone();
                if let Some(index) = call.iter().position(|arg| arg == "-o") {
                    let name = Path::new(&call[index + 1])
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    call[index + 1] = format!("<tmp>/{name}");
                }
                call.join(" ")
            })
            .collect()
    }
}

impl CommandRunner for FakeSwift {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, Error> {
        let mut call = vec![program.to_owned()];
        call.extend(args.iter().map(|arg| (*arg).to_owned()));
        self.calls.borrow_mut().push(call);
        if program == "git" {
            return Ok(match (args[0], self.toplevel) {
                (_, None) => CommandOutput::failure(128, "fatal: not a git repository"),
                ("rev-parse", Some(top)) => CommandOutput::success(top),
                ("tag", _) => CommandOutput::success(self.tags),
                ("remote", _) => match self.origin {
                    Some(url) => CommandOutput::success(format!("{url}\n")),
                    None => CommandOutput::failure(2, "error: No such remote 'origin'"),
                },
                _ => panic!("unexpected git call {args:?}"),
            });
        }
        assert_eq!(program, "swift");
        if !self.installed {
            return Err(Error::Io(std::io::ErrorKind::NotFound.into()));
        }
        match args {
            ["package", "dump-package"] => Ok(CommandOutput::success(
                serde_json::to_vec_pretty(&self.dump).unwrap(),
            )),
            ["package", "archive-source", "-o", output, ..] => {
                write_archive(cwd, Path::new(output));
                Ok(CommandOutput::success("Created DemoKit.zip"))
            }
            _ => panic!("unexpected swift call {args:?}"),
        }
    }
}

/// Writes the package directory into a zip under a `DemoKit/` directory, like
/// `swift package archive-source` does.
fn write_archive(dir: &Path, output: &Path) {
    let mut zip = zip::ZipWriter::new(File::create(output).unwrap());
    for name in ["Package.swift", "Sources/DemoKit/DemoKit.swift"] {
        zip.start_file(format!("DemoKit/{name}"), SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&std::fs::read(dir.join(name)).unwrap())
            .unwrap();
    }
    zip.finish().unwrap();
}

fn opts(dir: &Path, version: Option<&str>) -> PackageOptions {
    PackageOptions {
        path: dir.to_path_buf(),
        version: version.map(str::to_owned),
        ..PackageOptions::default()
    }
}

fn packager(fake: &FakeSwift) -> SwiftPackager<&FakeSwift> {
    SwiftPackager::with_runner(fake).scope(Some("acme".into()))
}

fn invalid_input(err: &Error, needle: &str) -> bool {
    matches!(err, Error::InvalidInput(message) if message.contains(needle))
}

#[test]
fn archives_the_package_with_its_manifests() {
    let fake = FakeSwift::new();

    let package = packager(&fake)
        .package(&opts(&fixture_package(), None))
        .unwrap();

    assert_eq!(
        fake.calls_of("swift"),
        [
            "swift package dump-package",
            "swift package archive-source -o <tmp>/DemoKit-1.2.0.zip",
        ]
    );
    assert_eq!(package.eco, Ecosystem::Swift);
    assert_eq!(package.name, "acme.DemoKit");
    assert_eq!(package.version, "1.2.0");
    assert!(package.notes.is_empty());
    assert_eq!(
        package.manifest,
        json!({
            "toolsVersion": "5.9.0",
            "repositoryURL": "https://github.com/acme/DemoKit.git",
            "dumpPackage": dump_package(),
        })
    );
    // The dump is passed on verbatim, keys in their original order.
    assert_eq!(
        serde_json::to_string(&package.manifest["dumpPackage"]).unwrap(),
        serde_json::to_string(&dump_package()).unwrap()
    );

    let files: Vec<(&str, Option<&str>)> = package
        .files
        .iter()
        .map(|file| (file.name.as_str(), file.role.as_deref()))
        .collect();
    assert_eq!(
        files,
        [
            ("DemoKit-1.2.0.zip", Some("archive")),
            ("Package.swift", Some("manifest")),
            ("Package@swift-5.8.swift", Some("manifest")),
        ]
    );
    let archive = &package.files[0];
    assert!(archive
        .path
        .starts_with(package.workdir.as_ref().unwrap().path()));
    let mut zip = zip::ZipArchive::new(File::open(&archive.path).unwrap()).unwrap();
    assert!(zip.by_name("DemoKit/Package.swift").is_ok());
    let manifest = &package.files[1];
    assert_eq!(manifest.path, fixture_package().join("Package.swift"));
    assert_eq!(
        manifest.size,
        std::fs::metadata(fixture_package().join("Package.swift"))
            .unwrap()
            .len()
    );
}

#[test]
fn the_flags_win_over_the_tag_and_the_remote() {
    let fake = FakeSwift::new();
    let mut options = opts(&fixture_package(), Some("2.0.0-beta.1"));
    options.extra_args = vec!["--verbose".into()];

    let package = packager(&fake)
        .repository_url(Some("https://git.example.com/acme/demo-kit".into()))
        .package(&options)
        .unwrap();

    assert_eq!(package.version, "2.0.0-beta.1");
    assert_eq!(
        package.manifest["repositoryURL"],
        "https://git.example.com/acme/demo-kit"
    );
    assert_eq!(
        fake.calls_of("swift")[1],
        "swift package archive-source -o <tmp>/DemoKit-2.0.0-beta.1.zip --verbose"
    );
    assert!(!fake
        .calls_of("git")
        .iter()
        .any(|call| call.starts_with("git remote")));
}

#[test]
fn the_tag_prefix_selects_the_tag() {
    let fake = FakeSwift {
        tags: "v9.9.9\ndemo-kit/v0.3.1\n",
        ..FakeSwift::new()
    };
    let mut options = opts(&fixture_package(), None);
    options.tag_prefix = Some("demo-kit/v".into());

    let package = packager(&fake).package(&options).unwrap();

    assert_eq!(package.version, "0.3.1");
}

#[test]
fn credentials_in_the_origin_url_are_dropped_and_no_origin_is_fine() {
    let fake = FakeSwift {
        origin: Some("https://x-access-token:ghs_secret@github.com/acme/DemoKit.git"),
        ..FakeSwift::new()
    };
    let package = packager(&fake)
        .package(&opts(&fixture_package(), None))
        .unwrap();
    assert_eq!(
        package.manifest["repositoryURL"],
        "https://github.com/acme/DemoKit.git"
    );

    let fake = FakeSwift {
        origin: None,
        ..FakeSwift::new()
    };
    let package = packager(&fake)
        .package(&opts(&fixture_package(), None))
        .unwrap();
    assert!(package.manifest.get("repositoryURL").is_none());
    assert_eq!(
        package
            .manifest
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["toolsVersion", "dumpPackage"]
    );
}

#[test]
fn a_version_is_required() {
    let untagged = FakeSwift {
        tags: "release\n",
        ..FakeSwift::new()
    };
    let err = packager(&untagged)
        .package(&opts(&fixture_package(), None))
        .unwrap_err();
    assert!(
        invalid_input(&err, "Package.swift declares no version"),
        "{err}"
    );
    assert!(invalid_input(&err, "--version"), "{err}");

    let outside = FakeSwift {
        toplevel: None,
        ..FakeSwift::new()
    };
    let err = packager(&outside)
        .package(&opts(&fixture_package(), None))
        .unwrap_err();
    assert!(invalid_input(&err, "declares no version"), "{err}");
    // Nothing was run before the version was known.
    assert!(untagged.calls_of("swift").is_empty());
    assert!(outside.calls_of("swift").is_empty());

    let fake = FakeSwift::new();
    let err = packager(&fake)
        .package(&opts(&fixture_package(), Some("v1.2.0")))
        .unwrap_err();
    assert!(invalid_input(&err, "without a leading `v`"), "{err}");
}

#[test]
fn the_scope_is_required_and_validated() {
    let fake = FakeSwift::new();
    let err = SwiftPackager::with_runner(&fake)
        .package(&opts(&fixture_package(), Some("1.0.0")))
        .unwrap_err();
    assert!(invalid_input(&err, "pass --scope"), "{err}");

    for scope in ["acme_corp", "", "acme.corp", &"a".repeat(40)] {
        let err = SwiftPackager::with_runner(&fake)
            .scope(Some(scope.to_owned()))
            .package(&opts(&fixture_package(), Some("1.0.0")))
            .unwrap_err();
        assert!(invalid_input(&err, "invalid Swift registry scope"), "{err}");
    }
    assert!(fake.calls.borrow().is_empty());
}

#[test]
fn the_name_from_the_manifest_is_validated() {
    let mut dump = dump_package();
    dump["name"] = json!("Demo Kit");
    let fake = FakeSwift {
        dump,
        ..FakeSwift::new()
    };
    let err = packager(&fake)
        .package(&opts(&fixture_package(), None))
        .unwrap_err();
    assert!(
        invalid_input(&err, "invalid Swift package name `Demo Kit`"),
        "{err}"
    );
    assert_eq!(fake.calls_of("swift"), ["swift package dump-package"]);

    let mut dump = dump_package();
    dump.as_object_mut().unwrap().remove("toolsVersion");
    let fake = FakeSwift {
        dump,
        ..FakeSwift::new()
    };
    let err = packager(&fake)
        .package(&opts(&fixture_package(), None))
        .unwrap_err();
    assert!(invalid_input(&err, "toolsVersion"), "{err}");
}

#[test]
fn the_manifest_must_exist() {
    let empty = tempfile::tempdir().unwrap();
    let fake = FakeSwift::new();
    let err = packager(&fake)
        .package(&opts(empty.path(), Some("1.0.0")))
        .unwrap_err();
    assert!(
        invalid_input(&err, "Package.swift` does not exist"),
        "{err}"
    );
    assert!(fake.calls.borrow().is_empty());
}

#[test]
fn only_package_swift_variants_are_manifests() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "Package.swift",
        "Package@swift-5.swift",
        "Package@swift-6.0.swift",
        "Package@swift-.swift",
        "Package@swift-5.9.txt",
        "Package.resolved",
        "package@swift-5.9.swift",
    ] {
        std::fs::write(dir.path().join(name), "// swift-tools-version: 5.9\n").unwrap();
    }
    std::fs::create_dir_all(dir.path().join("Sources/DemoKit")).unwrap();
    std::fs::write(dir.path().join("Sources/DemoKit/DemoKit.swift"), "").unwrap();
    let fake = FakeSwift::new();

    let package = packager(&fake)
        .package(&opts(dir.path(), Some("1.0.0")))
        .unwrap();

    let manifests: Vec<&str> = package
        .files
        .iter()
        .filter(|file| file.role.as_deref() == Some("manifest"))
        .map(|file| file.name.as_str())
        .collect();
    assert_eq!(
        manifests,
        [
            "Package.swift",
            "Package@swift-5.swift",
            "Package@swift-6.0.swift"
        ]
    );
}

#[test]
fn reserved_arguments_and_dist_tags_are_rejected() {
    let fake = FakeSwift::new();
    let mut options = opts(&fixture_package(), Some("1.0.0"));
    options.extra_args = vec!["-o".into(), "x.zip".into()];
    let err = packager(&fake).package(&options).unwrap_err();
    assert!(invalid_input(&err, "`-o` cannot be passed"), "{err}");

    let mut options = opts(&fixture_package(), Some("1.0.0"));
    options.tag = Some("next".into());
    assert!(packager(&fake).package(&options).is_err());
    assert!(fake.calls.borrow().is_empty());
}

#[test]
fn a_missing_toolchain_is_explained() {
    let fake = FakeSwift {
        installed: false,
        ..FakeSwift::new()
    };
    let err = packager(&fake)
        .package(&opts(&fixture_package(), None))
        .unwrap_err();
    assert!(invalid_input(&err, "`swift` was not found"), "{err}");
}
