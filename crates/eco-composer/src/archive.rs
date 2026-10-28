//! Building the zip archive of a Composer package.

use std::fs::File;
use std::io::{BufWriter, ErrorKind, Read};
use std::path::{Path, PathBuf};

use serde_json::Value;
use sreg_core::{CommandRunner, Error};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::validate::version_from_tag;

/// Archives larger than this need ZIP64 entries.
const ZIP64_THRESHOLD: u64 = u32::MAX as u64;

/// `git` run in the package directory.
pub(crate) struct Git<'a, R> {
    runner: &'a R,
    dir: &'a Path,
}

impl<'a, R: CommandRunner> Git<'a, R> {
    pub(crate) fn new(runner: &'a R, dir: &'a Path) -> Self {
        Git { runner, dir }
    }

    /// Whether the directory is inside a git work tree. A missing `git` counts as "no".
    pub(crate) fn is_repository(&self) -> Result<bool, Error> {
        match self
            .runner
            .run("git", &["rev-parse", "--is-inside-work-tree"], self.dir)
        {
            Ok(output) => Ok(output.is_success() && output.stdout_text() == "true"),
            Err(Error::Io(err)) if err.kind() == ErrorKind::NotFound => {
                tracing::debug!("git is not installed");
                Ok(false)
            }
            Err(err) => Err(err),
        }
    }

    /// The version named by the tag pointing at `HEAD` (`v1.2.3` → `1.2.3`).
    pub(crate) fn head_tag(&self) -> Result<String, Error> {
        let output = self.runner.run(
            "git",
            &["describe", "--exact-match", "--tags", "HEAD"],
            self.dir,
        )?;
        if !output.is_success() {
            tracing::debug!(stderr = %output.stderr_text(), "git describe failed");
            return Err(Error::InvalidInput(
                "no version given and HEAD has no tag; tag the release (`git tag v1.2.3`) or \
                 pass --version"
                    .into(),
            ));
        }
        let tag = output.stdout_text();
        Ok(version_from_tag(tag.trim()).to_owned())
    }

    /// Writes `git archive HEAD` as a zip to `destination`; honours `export-ignore`.
    pub(crate) fn archive(&self, destination: &Path) -> Result<(), Error> {
        let destination = destination.to_str().ok_or_else(|| {
            Error::InvalidInput(format!(
                "the temporary path `{}` is not valid UTF-8",
                destination.display()
            ))
        })?;
        let output = format!("--output={destination}");
        self.runner
            .run(
                "git",
                &["archive", "--format=zip", "--prefix=", &output, "HEAD"],
                self.dir,
            )?
            .into_success("git archive --format=zip HEAD")?;
        if !Path::new(destination).is_file() {
            return Err(Error::Command {
                command: "git archive --format=zip HEAD".into(),
                message: "no archive was written".into(),
            });
        }
        Ok(())
    }
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
        output: Result<CommandOutput, ErrorKind>,
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl Fake {
        fn new(output: Result<CommandOutput, ErrorKind>) -> Self {
            Fake {
                output,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl CommandRunner for Fake {
        fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, Error> {
            let mut call = vec![program.to_owned()];
            call.extend(args.iter().map(|arg| (*arg).to_owned()));
            self.calls.borrow_mut().push(call);
            self.output
                .clone()
                .map_err(|kind| Error::Io(std::io::Error::from(kind)))
        }
    }

    #[test]
    fn missing_git_means_no_repository() {
        let fake = Fake::new(Err(ErrorKind::NotFound));
        assert!(!Git::new(&fake, Path::new(".")).is_repository().unwrap());

        let fake = Fake::new(Err(ErrorKind::PermissionDenied));
        assert!(Git::new(&fake, Path::new(".")).is_repository().is_err());
    }

    #[test]
    fn repository_detection_reads_rev_parse() {
        let fake = Fake::new(Ok(CommandOutput::success("true\n")));
        assert!(Git::new(&fake, Path::new(".")).is_repository().unwrap());
        assert_eq!(
            fake.calls.borrow()[0],
            ["git", "rev-parse", "--is-inside-work-tree"]
        );

        let fake = Fake::new(Ok(CommandOutput::failure(
            128,
            "fatal: not a git repository",
        )));
        assert!(!Git::new(&fake, Path::new(".")).is_repository().unwrap());
    }

    #[test]
    fn head_tag_strips_the_v_prefix() {
        let fake = Fake::new(Ok(CommandOutput::success("v2.0.0-rc1\n")));
        assert_eq!(
            Git::new(&fake, Path::new(".")).head_tag().unwrap(),
            "2.0.0-rc1"
        );
        assert_eq!(
            fake.calls.borrow()[0],
            ["git", "describe", "--exact-match", "--tags", "HEAD"]
        );
    }

    #[test]
    fn untagged_head_is_a_clear_error() {
        let fake = Fake::new(Ok(CommandOutput::failure(
            128,
            "fatal: no tag exactly matches",
        )));
        let err = Git::new(&fake, Path::new(".")).head_tag().unwrap_err();
        assert!(err.to_string().contains("--version"), "{err}");
        assert!(err.to_string().contains("git tag v1.2.3"), "{err}");
    }

    #[test]
    fn failed_archive_reports_git_stderr() {
        let fake = Fake::new(Ok(CommandOutput::failure(
            128,
            "fatal: not a valid object name: 'HEAD'",
        )));
        let err = Git::new(&fake, Path::new("."))
            .archive(Path::new("/tmp/x.zip"))
            .unwrap_err();
        assert!(
            matches!(err, Error::Command { ref message, .. } if message.contains("HEAD")),
            "{err}"
        );
        assert_eq!(
            fake.calls.borrow()[0],
            [
                "git",
                "archive",
                "--format=zip",
                "--prefix=",
                "--output=/tmp/x.zip",
                "HEAD"
            ]
        );
    }
}
