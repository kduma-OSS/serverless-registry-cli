//! Running external programs (`git`, `npm`, `cargo`, `dotnet`) behind a trait, so that
//! packagers can be tested with a fake instead of the real tools.

use std::path::Path;
use std::process::Command;

use crate::error::Result;

/// What a finished program produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandOutput {
    /// Exit code; `None` when the program was terminated by a signal.
    pub code: Option<i32>,
    /// Everything the program wrote to standard output.
    pub stdout: Vec<u8>,
    /// Everything the program wrote to standard error.
    pub stderr: Vec<u8>,
}

impl CommandOutput {
    /// A successful run with the given standard output.
    pub fn success(stdout: impl Into<Vec<u8>>) -> Self {
        CommandOutput {
            code: Some(0),
            stdout: stdout.into(),
            stderr: Vec::new(),
        }
    }

    /// A failed run with the given exit code and standard error.
    pub fn failure(code: i32, stderr: impl Into<Vec<u8>>) -> Self {
        CommandOutput {
            code: Some(code),
            stdout: Vec::new(),
            stderr: stderr.into(),
        }
    }

    /// Whether the program exited with code `0`.
    pub fn is_success(&self) -> bool {
        self.code == Some(0)
    }

    /// Standard output as text, without the trailing line break.
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim_end().to_owned()
    }

    /// Standard error as text, without surrounding whitespace.
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_owned()
    }
}

/// Runs external programs.
pub trait CommandRunner {
    /// Runs `program` with `args` in the directory `cwd` and waits for it to finish.
    ///
    /// A program that runs and fails is an `Ok` with a non-zero [`CommandOutput::code`];
    /// a program that cannot be started at all (for example because it is not installed)
    /// is an [`Error::Io`](crate::Error::Io).
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput>;
}

impl<R: CommandRunner + ?Sized> CommandRunner for &R {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput> {
        (**self).run(program, args, cwd)
    }
}

/// Runs programs on the local system.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput> {
        tracing::debug!(program, ?args, cwd = %cwd.display(), "running");
        let output = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .output()?;
        tracing::debug!(program, code = ?output.status.code(), "finished");
        Ok(CommandOutput {
            code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_programs_are_io_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = SystemRunner
            .run("sreg-test-no-such-program", &[], dir.path())
            .unwrap_err();
        assert!(
            matches!(err, crate::Error::Io(ref io) if io.kind() == std::io::ErrorKind::NotFound),
            "{err:?}"
        );
    }

    #[test]
    fn captures_output_and_exit_codes() {
        let dir = tempfile::tempdir().unwrap();
        let ok = SystemRunner.run("git", &["--version"], dir.path()).unwrap();
        assert!(ok.is_success());
        assert!(ok.stdout_text().starts_with("git version"));

        let failed = SystemRunner
            .run("git", &["no-such-subcommand"], dir.path())
            .unwrap();
        assert!(!failed.is_success());
        assert!(!failed.stderr_text().is_empty());
    }

    #[test]
    fn output_helpers() {
        assert!(CommandOutput::success("v1.0.0\n").is_success());
        assert_eq!(CommandOutput::success("v1.0.0\n").stdout_text(), "v1.0.0");
        let failed = CommandOutput::failure(128, " fatal: no tag \n");
        assert!(!failed.is_success());
        assert_eq!(failed.stderr_text(), "fatal: no tag");
        assert!(!CommandOutput::default().is_success());
    }
}
