//! Building the zip archive of a Composer package.

use std::fs::File;
use std::io::{BufWriter, Read};
use std::path::{Path, PathBuf};

use serde_json::Value;
use sreg_core::{CommandRunner, Error};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

/// Archives larger than this need ZIP64 entries.
const ZIP64_THRESHOLD: u64 = u32::MAX as u64;

/// The path of `dir` relative to the work tree's `toplevel`, `/`-separated; empty for
/// the top level itself.
pub(crate) fn relative_prefix(toplevel: &Path, dir: &Path) -> Result<String, Error> {
    let toplevel = std::fs::canonicalize(toplevel)?;
    let dir = std::fs::canonicalize(dir)?;
    let relative = dir.strip_prefix(&toplevel).map_err(|_| {
        Error::InvalidInput(format!(
            "`{}` is not inside the git work tree `{}`",
            dir.display(),
            toplevel.display()
        ))
    })?;
    let mut parts = Vec::new();
    for component in relative.components() {
        parts.push(
            component
                .as_os_str()
                .to_str()
                .ok_or_else(|| non_utf8(&dir))?,
        );
    }
    Ok(parts.join("/"))
}

/// Writes `git archive HEAD:<prefix>` (or `HEAD` for an empty prefix) as a zip to
/// `destination`: the package directory becomes the root of the archive and paths
/// marked `export-ignore` are left out.
pub(crate) fn git_archive<R: CommandRunner + ?Sized>(
    runner: &R,
    toplevel: &Path,
    prefix: &str,
    destination: &Path,
) -> Result<(), Error> {
    let destination_str = destination.to_str().ok_or_else(|| non_utf8(destination))?;
    let tree = if prefix.is_empty() {
        "HEAD".to_owned()
    } else {
        format!("HEAD:{prefix}")
    };
    let output = format!("--output={destination_str}");
    let command = format!("git archive --format=zip {tree}");
    runner
        .run(
            "git",
            &["archive", "--format=zip", "--prefix=", &output, &tree],
            toplevel,
        )?
        .into_success(&command)?;
    if !destination.is_file() {
        return Err(Error::Command {
            command,
            message: "no archive was written".into(),
        });
    }
    Ok(())
}

/// Parses `composer.json` at the root of the zip at `path`, if there is one.
pub(crate) fn read_composer_json(path: &Path) -> Result<Option<Value>, Error> {
    let mut archive = ZipArchive::new(File::open(path)?).map_err(zip_error)?;
    let mut entry = match archive.by_name("composer.json") {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(err) => return Err(zip_error(err)),
    };
    let mut text = String::new();
    entry.read_to_string(&mut text)?;
    let manifest = serde_json::from_str(&text).map_err(|err| {
        Error::InvalidInput(format!(
            "the archived composer.json is not valid JSON: {err}"
        ))
    })?;
    Ok(Some(manifest))
}

/// Reads the `composer.json` of a prebuilt zip: at the root of the archive or, as in the
/// dist archives of code hosts, inside its single top-level directory.
///
/// Every entry is read to the end, so that a truncated or corrupt archive is rejected
/// here (the CRC of each entry is checked) and not when Composer installs it.
pub(crate) fn read_prebuilt_composer_json(path: &Path) -> Result<Value, Error> {
    let invalid = |message: String| {
        Error::InvalidInput(format!(
            "`{}` is not a valid Composer zip: {message}",
            path.display()
        ))
    };
    let mut archive = ZipArchive::new(File::open(path)?).map_err(|err| invalid(err.to_string()))?;
    let mut names = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|err| invalid(err.to_string()))?;
        names.push(
            entry
                .name()
                .map_err(|err| invalid(err.to_string()))?
                .into_owned(),
        );
        std::io::copy(&mut entry, &mut std::io::sink()).map_err(|err| invalid(err.to_string()))?;
    }
    let manifest_name = if names.iter().any(|name| name == "composer.json") {
        "composer.json".to_owned()
    } else {
        let mut tops = names
            .iter()
            .filter_map(|name| name.split_once('/').map(|(top, _)| top))
            .collect::<Vec<_>>();
        tops.sort_unstable();
        tops.dedup();
        let nested = match tops.as_slice() {
            [top]
                if names
                    .iter()
                    .all(|name| name.starts_with(&format!("{top}/"))) =>
            {
                format!("{top}/composer.json")
            }
            _ => String::new(),
        };
        if !names.contains(&nested) {
            return Err(invalid("it has no composer.json at its root".into()));
        }
        nested
    };
    let mut text = String::new();
    archive
        .by_name(&manifest_name)
        .map_err(|err| invalid(err.to_string()))?
        .read_to_string(&mut text)
        .map_err(|err| invalid(format!("cannot read {manifest_name}: {err}")))?;
    let manifest: Value = serde_json::from_str(&text).map_err(|err| {
        Error::InvalidInput(format!(
            "the archived composer.json is not valid JSON: {err}"
        ))
    })?;
    if !manifest.is_object() {
        return Err(Error::InvalidInput(
            "the archived composer.json must contain a JSON object".into(),
        ));
    }
    Ok(manifest)
}

/// One entry of the directory being zipped.
struct Entry {
    /// Path inside the archive, `/`-separated, without a trailing `/`.
    name: String,
    path: PathBuf,
    kind: EntryKind,
}

enum EntryKind {
    Dir,
    File,
    Symlink,
}

/// Zips `dir` into `destination`, without `vendor/` (at the root), `.git` (anywhere) and
/// `destination` itself. Entries are sorted by name and sit at the root of the archive;
/// unix file modes are preserved and timestamps are fixed, so the same tree always
/// gives the same archive.
pub(crate) fn zip_directory(dir: &Path, destination: &Path) -> Result<(), Error> {
    let mut entries = Vec::new();
    collect(dir, "", destination, &mut entries)?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    let mut writer = ZipWriter::new(BufWriter::new(File::create(destination)?));
    let base = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::default());
    for entry in &entries {
        let metadata = std::fs::symlink_metadata(&entry.path)?;
        match entry.kind {
            EntryKind::Dir => {
                let options = base.unix_permissions(mode(&metadata, 0o755));
                writer
                    .add_directory(format!("{}/", entry.name), options)
                    .map_err(zip_error)?;
            }
            EntryKind::File => {
                let options = base
                    .unix_permissions(mode(&metadata, 0o644))
                    .large_file(metadata.len() >= ZIP64_THRESHOLD);
                writer
                    .start_file(entry.name.as_str(), options)
                    .map_err(zip_error)?;
                let mut file = File::open(&entry.path)?;
                std::io::copy(&mut file, &mut writer)?;
            }
            EntryKind::Symlink => {
                let target = std::fs::read_link(&entry.path)?;
                let target = target.to_str().ok_or_else(|| non_utf8(&entry.path))?;
                writer
                    .add_symlink(entry.name.as_str(), target.replace('\\', "/"), base)
                    .map_err(zip_error)?;
            }
        }
    }
    let mut inner = writer.finish().map_err(zip_error)?;
    std::io::Write::flush(&mut inner)?;
    Ok(())
}

fn collect(dir: &Path, prefix: &str, skip: &Path, entries: &mut Vec<Entry>) -> Result<(), Error> {
    for item in std::fs::read_dir(dir)? {
        let item = item?;
        let path = item.path();
        let file_name = item.file_name();
        let file_name = file_name.to_str().ok_or_else(|| non_utf8(&path))?;
        if file_name == ".git" || (prefix.is_empty() && file_name == "vendor") || path == skip {
            continue;
        }
        let name = format!("{prefix}{file_name}");
        let file_type = item.file_type()?;
        if file_type.is_symlink() {
            entries.push(Entry {
                name,
                path,
                kind: EntryKind::Symlink,
            });
        } else if file_type.is_dir() {
            collect(&path, &format!("{name}/"), skip, entries)?;
            entries.push(Entry {
                name,
                path,
                kind: EntryKind::Dir,
            });
        } else if file_type.is_file() {
            entries.push(Entry {
                name,
                path,
                kind: EntryKind::File,
            });
        } else {
            tracing::debug!(path = %path.display(), "skipping a special file");
        }
    }
    Ok(())
}

#[cfg(unix)]
fn mode(metadata: &std::fs::Metadata, _default: u32) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn mode(_metadata: &std::fs::Metadata, default: u32) -> u32 {
    default
}

fn non_utf8(path: &Path) -> Error {
    Error::InvalidInput(format!(
        "`{}` has a name that is not valid UTF-8 and cannot be archived",
        path.display()
    ))
}

fn zip_error(err: zip::result::ZipError) -> Error {
    match err {
        zip::result::ZipError::Io(err) => Error::Io(err),
        other => Error::InvalidInput(format!("zip archive: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use sreg_core::CommandOutput;

    use super::*;

    /// Answers every command with the same output and records the arguments.
    struct Fake {
        output: CommandOutput,
        calls: RefCell<Vec<(Vec<String>, PathBuf)>>,
    }

    impl CommandRunner for Fake {
        fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, Error> {
            let mut call = vec![program.to_owned()];
            call.extend(args.iter().map(|arg| (*arg).to_owned()));
            self.calls.borrow_mut().push((call, cwd.to_path_buf()));
            Ok(self.output.clone())
        }
    }

    #[test]
    fn prefix_is_the_path_below_the_top_level() {
        let top = tempfile::tempdir().unwrap();
        let sub = top.path().join("packages/sdk");
        std::fs::create_dir_all(&sub).unwrap();

        assert_eq!(relative_prefix(top.path(), &sub).unwrap(), "packages/sdk");
        assert_eq!(relative_prefix(top.path(), top.path()).unwrap(), "");
        assert_eq!(
            relative_prefix(top.path(), &sub.join("..")).unwrap(),
            "packages"
        );
        let elsewhere = tempfile::tempdir().unwrap();
        assert!(relative_prefix(top.path(), elsewhere.path()).is_err());
    }

    #[test]
    fn archives_the_subtree_from_the_top_level() {
        let fake = Fake {
            output: CommandOutput::success(""),
            calls: RefCell::new(Vec::new()),
        };
        // The fake writes nothing, so the missing archive is reported.
        let err = git_archive(
            &fake,
            Path::new("/work/repo"),
            "packages/sdk",
            Path::new("/tmp/sdk-1.0.0.zip"),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Command { .. }), "{err}");

        let calls = fake.calls.borrow();
        let (args, cwd) = &calls[0];
        assert_eq!(
            args,
            &[
                "git",
                "archive",
                "--format=zip",
                "--prefix=",
                "--output=/tmp/sdk-1.0.0.zip",
                "HEAD:packages/sdk"
            ]
        );
        assert_eq!(cwd, Path::new("/work/repo"));
    }

    #[test]
    fn the_top_level_is_archived_as_head() {
        let fake = Fake {
            output: CommandOutput::failure(128, "fatal: not a valid object name: 'HEAD'"),
            calls: RefCell::new(Vec::new()),
        };
        let err = git_archive(&fake, Path::new("/r"), "", Path::new("/tmp/x.zip")).unwrap_err();
        assert!(
            matches!(err, Error::Command { ref message, .. } if message.contains("HEAD")),
            "{err}"
        );
        assert_eq!(fake.calls.borrow()[0].0.last().unwrap(), "HEAD");
    }
}
