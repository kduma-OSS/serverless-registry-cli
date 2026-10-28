//! Packaging with a fake `dotnet` that writes a `.nupkg` (and `.snupkg`) built from the
//! fixture `.nuspec` files; the .NET SDK is not needed.

use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sreg_core::{CommandOutput, CommandRunner, Ecosystem, Error, PackageOptions, Packager};
use sreg_eco_nuget::NugetPackager;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

/// Writes a zip with the given entries.
fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    for (name, data) in entries {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

/// A fake `dotnet` (and a `git` outside any repository): `dotnet pack` writes
/// `<Id>.<Version>.nupkg` holding the fixture `.nuspec` (with the version from
/// `-p:PackageVersion`, like the real one) into the output directory.
struct FakeDotnet {
    nuspec: &'static str,
    symbols: bool,
    nuspec_in_package: bool,
    second_package: bool,
    calls: RefCell<Vec<Vec<String>>>,
}

impl FakeDotnet {
    fn new(nuspec: &'static str) -> Self {
        FakeDotnet {
            nuspec,
            symbols: false,
            nuspec_in_package: true,
            second_package: false,
            calls: RefCell::new(Vec::new()),
        }
    }

    fn with_symbols(mut self) -> Self {
        self.symbols = true;
        self
    }

    /// The `dotnet` calls, with the temporary output directory replaced by `<tmp>`.
    fn dotnet_calls(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .filter(|call| call[0] == "dotnet")
            .map(|call| {
                let mut call = call.clone();
                if let Some(index) = call.iter().position(|arg| arg == "-o") {
                    call[index + 1] = "<tmp>".to_owned();
                }
                call.join(" ")
            })
            .collect()
    }
}

/// The text of the element `name` in `xml`.
fn element<'a>(xml: &'a str, name: &str) -> &'a str {
    let start = xml.find(&format!("<{name}>")).unwrap() + name.len() + 2;
    let end = xml.find(&format!("</{name}>")).unwrap();
    &xml[start..end]
}

impl CommandRunner for FakeDotnet {
    fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, Error> {
        let mut call = vec![program.to_owned()];
        call.extend(args.iter().map(|arg| (*arg).to_owned()));
        self.calls.borrow_mut().push(call);
        if program == "git" {
            return Ok(CommandOutput::failure(128, "fatal: not a git repository"));
        }
        assert_eq!((program, args[0]), ("dotnet", "pack"));
        let output = {
            let index = args.iter().position(|arg| *arg == "-o").unwrap();
            PathBuf::from(args[index + 1])
        };
        let mut nuspec = fixture(self.nuspec);
        if let Some(version) = args
            .iter()
            .find_map(|arg| arg.strip_prefix("-p:PackageVersion="))
        {
            let current = format!("<version>{}</version>", element(&nuspec, "version"));
            nuspec = nuspec.replace(&current, &format!("<version>{version}</version>"));
        }
        let id = element(&nuspec, "id").to_owned();
        let version = element(&nuspec, "version").to_owned();
        let nuspec_name = format!("{id}.nuspec");
        let mut entries: Vec<(&str, &[u8])> = vec![
            ("[Content_Types].xml", b"<Types />"),
            ("lib/net8.0/Acme.dll", b"MZ"),
        ];
        if self.nuspec_in_package {
            entries.push((&nuspec_name, nuspec.as_bytes()));
        }
        write_zip(&output.join(format!("{id}.{version}.nupkg")), &entries);
        if self.symbols {
            write_zip(
                &output.join(format!("{id}.{version}.snupkg")),
                &[("lib/net8.0/Acme.pdb", b"pdb")],
            );
        }
        if self.second_package {
            write_zip(&output.join("Other.1.0.0.nupkg"), &entries);
        }
        Ok(CommandOutput::success("Successfully created package."))
    }
}

/// A directory with one project file.
fn project_dir(projects: &[&str]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for project in projects {
        let path = dir.path().join(project);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "<Project Sdk=\"Microsoft.NET.Sdk\" />").unwrap();
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

fn invalid_input(err: &Error, needle: &str) -> bool {
    matches!(err, Error::InvalidInput(message) if message.contains(needle))
}

fn zip_entry(path: &Path, name: &str) -> String {
    let mut archive = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
    let mut text = String::new();
    archive
        .by_name(name)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}

#[test]
fn packs_the_project_with_its_symbols() {
    let dir = project_dir(&["Acme.Sdk.csproj"]);
    let fake = FakeDotnet::new("Acme.Sdk.nuspec").with_symbols();
    let mut options = opts(dir.path(), None);
    options.extra_args = vec!["-c".into(), "Release".into()];

    let package = NugetPackager::with_runner(&fake).package(&options).unwrap();

    assert_eq!(
        fake.dotnet_calls(),
        ["dotnet pack Acme.Sdk.csproj -o <tmp> -c Release"]
    );
    assert_eq!(package.eco, Ecosystem::Nuget);
    assert_eq!(package.name, "Acme.Sdk");
    assert_eq!(package.version, "1.2.0");
    assert!(package.notes.is_empty());
    assert_eq!(
        package.manifest,
        json!({
            "id": "Acme.Sdk",
            "version": "1.2.0",
            "nuspec": fixture("Acme.Sdk.nuspec"),
            "description": "Acme SDK for .NET",
            "authors": "Acme",
            "projectUrl": "https://github.com/acme/sdk-dotnet",
            "licenseExpression": "MIT",
            "tags": ["acme", "sdk"],
            "dependencyGroups": [
                {
                    "targetFramework": "net8.0",
                    "dependencies": [{ "id": "Newtonsoft.Json", "range": "[13.0.1, )" }]
                },
                {
                    "targetFramework": ".NETStandard2.0",
                    "dependencies": [
                        { "id": "Newtonsoft.Json", "range": "[13.0.1, )" },
                        { "id": "System.Text.Json", "range": "[8.0.0, 9.0.0)" }
                    ]
                },
                { "targetFramework": "net6.0", "dependencies": [] },
                {
                    "targetFramework": null,
                    "dependencies": [{ "id": "Acme.Core", "range": "2.0.0" }]
                }
            ]
        })
    );

    // Renamed to the lower-case names the registry requires.
    let names: Vec<(&str, Option<&str>)> = package
        .files
        .iter()
        .map(|file| (file.name.as_str(), file.role.as_deref()))
        .collect();
    assert_eq!(
        names,
        [
            ("acme.sdk.1.2.0.nupkg", Some("archive")),
            ("acme.sdk.1.2.0.snupkg", Some("symbols")),
        ]
    );
    for file in &package.files {
        assert_eq!(
            file.path.file_name().unwrap().to_str(),
            Some(file.name.as_str())
        );
        let (sha256, size) = sreg_core::digest::sha256_file(&file.path).unwrap();
        assert_eq!((file.sha256.as_str(), file.size), (sha256.as_str(), size));
    }
    assert_eq!(
        zip_entry(&package.files[0].path, "Acme.Sdk.nuspec"),
        fixture("Acme.Sdk.nuspec")
    );

    let path = package.files[0].path.clone();
    drop(package);
    assert!(!path.exists());
}

#[test]
fn version_flag_becomes_the_package_version_property() {
    let dir = project_dir(&["src/Acme.Sdk/Acme.Sdk.csproj", "README.md"]);
    let fake = FakeDotnet::new("Acme.Sdk.nuspec").with_symbols();

    let package = NugetPackager::with_runner(&fake)
        .project(Some(PathBuf::from("src/Acme.Sdk/Acme.Sdk.csproj")))
        .package(&opts(dir.path(), Some("2.0.0-RC.1")))
        .unwrap();

    assert_eq!(
        fake.dotnet_calls(),
        ["dotnet pack src/Acme.Sdk/Acme.Sdk.csproj -o <tmp> -p:PackageVersion=2.0.0-RC.1"]
    );
    assert_eq!(package.version, "2.0.0-RC.1");
    assert_eq!(package.manifest["version"], "2.0.0-RC.1");
    assert_eq!(package.files[0].name, "acme.sdk.2.0.0-rc.1.nupkg");
    assert_eq!(package.files[1].name, "acme.sdk.2.0.0-rc.1.snupkg");
}

#[test]
fn no_symbols_leaves_the_snupkg_out() {
    let dir = project_dir(&["Acme.Sdk.csproj"]);
    let fake = FakeDotnet::new("Acme.Sdk.nuspec").with_symbols();

    let package = NugetPackager::with_runner(&fake)
        .symbols(false)
        .package(&opts(dir.path(), None))
        .unwrap();

    assert_eq!(package.files.len(), 1);
    assert_eq!(package.files[0].name, "acme.sdk.1.2.0.nupkg");
}

#[test]
fn dependencies_outside_a_group_form_a_group_without_framework() {
    let dir = project_dir(&["Acme.Legacy.csproj"]);
    let fake = FakeDotnet::new("Acme.Legacy.nuspec");

    let package = NugetPackager::with_runner(&fake)
        .package(&opts(dir.path(), None))
        .unwrap();

    assert_eq!(package.name, "Acme.Legacy");
    assert_eq!(package.version, "0.9.1-Beta.2");
    assert_eq!(package.files.len(), 1);
    assert_eq!(package.files[0].name, "acme.legacy.0.9.1-beta.2.nupkg");
    assert_eq!(
        package.manifest["dependencyGroups"],
        json!([{
            "targetFramework": null,
            "dependencies": [
                { "id": "Acme.Core", "range": "[1.0.0, 2.0.0)" },
                { "id": "Acme.Logging", "range": "(, )" }
            ]
        }])
    );
    for key in ["projectUrl", "licenseExpression", "tags"] {
        assert!(package.manifest.get(key).is_none(), "{key}");
    }
    assert_eq!(
        package.manifest["nuspec"],
        Value::from(fixture("Acme.Legacy.nuspec"))
    );
}

#[test]
fn solutions_are_rejected() {
    let dir = project_dir(&["Acme.sln", "Acme.Sdk.csproj"]);
    let fake = FakeDotnet::new("Acme.Sdk.nuspec");

    let err = NugetPackager::with_runner(&fake)
        .project(Some(PathBuf::from("Acme.sln")))
        .package(&opts(dir.path(), None))
        .unwrap_err();

    assert!(invalid_input(&err, "multi-package output"), "{err}");
    assert!(fake.calls.borrow().is_empty());
}

#[test]
fn the_project_must_be_unambiguous() {
    let fake = FakeDotnet::new("Acme.Sdk.nuspec");
    let packager = NugetPackager::with_runner(&fake);

    let none = project_dir(&["README.md"]);
    let err = packager.package(&opts(none.path(), None)).unwrap_err();
    assert!(invalid_input(&err, "has no .csproj"), "{err}");

    let several = project_dir(&["Acme.Sdk.csproj", "Acme.Tests.csproj"]);
    let err = packager.package(&opts(several.path(), None)).unwrap_err();
    assert!(
        invalid_input(
            &err,
            "several project files (Acme.Sdk.csproj, Acme.Tests.csproj)"
        ),
        "{err}"
    );

    let err = NugetPackager::with_runner(&fake)
        .project(Some(PathBuf::from("Missing.csproj")))
        .package(&opts(several.path(), None))
        .unwrap_err();
    assert!(invalid_input(&err, "does not exist"), "{err}");

    assert!(fake.calls.borrow().is_empty());
}

#[test]
fn invalid_input_is_rejected_before_running_dotnet() {
    let dir = project_dir(&["Acme.Sdk.csproj"]);
    let fake = FakeDotnet::new("Acme.Sdk.nuspec");
    let packager = NugetPackager::with_runner(&fake);

    let mut reserved = opts(dir.path(), None);
    reserved.extra_args = vec!["-p:PackageVersion=9.9.9".into()];
    let err = packager.package(&reserved).unwrap_err();
    assert!(
        invalid_input(&err, "cannot be passed to `dotnet pack`"),
        "{err}"
    );

    let mut output = opts(dir.path(), None);
    output.extra_args = vec!["-o".into(), "dist".into()];
    assert!(packager.package(&output).is_err());

    let err = packager
        .package(&opts(dir.path(), Some("1.0;Foo=bar")))
        .unwrap_err();
    assert!(invalid_input(&err, "invalid NuGet version"), "{err}");

    assert!(fake.dotnet_calls().is_empty());
}

#[test]
fn unexpected_dotnet_output_is_an_error() {
    let dir = project_dir(&["Acme.Sdk.csproj"]);

    let mut fake = FakeDotnet::new("Acme.Sdk.nuspec");
    fake.nuspec_in_package = false;
    let err = NugetPackager::with_runner(&fake)
        .package(&opts(dir.path(), None))
        .unwrap_err();
    assert!(invalid_input(&err, "no .nuspec"), "{err}");

    let mut fake = FakeDotnet::new("Acme.Sdk.nuspec");
    fake.second_package = true;
    let err = NugetPackager::with_runner(&fake)
        .package(&opts(dir.path(), None))
        .unwrap_err();
    assert!(invalid_input(&err, "several packages"), "{err}");
}

#[test]
fn a_missing_dotnet_is_reported() {
    struct NoDotnet;
    impl CommandRunner for NoDotnet {
        fn run(&self, program: &str, _: &[&str], _: &Path) -> Result<CommandOutput, Error> {
            match program {
                "git" => Ok(CommandOutput::failure(128, "not a repository")),
                _ => Err(Error::Io(std::io::ErrorKind::NotFound.into())),
            }
        }
    }
    let dir = project_dir(&["Acme.Sdk.csproj"]);
    let err = NugetPackager::with_runner(NoDotnet)
        .package(&opts(dir.path(), Some("1.0.0")))
        .unwrap_err();
    assert!(invalid_input(&err, "install the .NET SDK"), "{err}");
}

#[test]
fn dotnet_failures_are_reported() {
    struct Failing;
    impl CommandRunner for Failing {
        fn run(&self, program: &str, _: &[&str], _: &Path) -> Result<CommandOutput, Error> {
            Ok(match program {
                "git" => CommandOutput::failure(128, "not a repository"),
                _ => CommandOutput::failure(1, "error NU5026: the file to package was not found"),
            })
        }
    }
    let dir = project_dir(&["Acme.Sdk.csproj"]);
    let err = NugetPackager::with_runner(Failing)
        .package(&opts(dir.path(), None))
        .unwrap_err();
    assert!(
        matches!(&err, Error::Command { command, message }
            if command.starts_with("dotnet pack Acme.Sdk.csproj -o ")
                && message.contains("NU5026")),
        "{err:?}"
    );
}
