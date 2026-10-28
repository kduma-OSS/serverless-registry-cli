//! The git queries packagers share: the work tree a project lives in and the release tag
//! on `HEAD` that names the version to publish.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::process::CommandRunner;

/// Tag prefix used when none is given: release tags are `v<semver>`.
pub const DEFAULT_TAG_PREFIX: &str = "v";

/// `git` run in a project directory.
pub struct Git<'a, R: ?Sized> {
    runner: &'a R,
    dir: &'a Path,
}

impl<'a, R: CommandRunner + ?Sized> Git<'a, R> {
    /// Runs `git` through `runner` in `dir`.
    pub fn new(runner: &'a R, dir: &'a Path) -> Self {
        Git { runner, dir }
    }

    /// The top-level directory of the work tree containing the directory, or `None`
    /// when it is not inside a git work tree or `git` is not installed.
    pub fn toplevel(&self) -> Result<Option<PathBuf>> {
        match self
            .runner
            .run("git", &["rev-parse", "--show-toplevel"], self.dir)
        {
            Ok(output) if output.is_success() => {
                let top = output.stdout_text();
                Ok((!top.is_empty()).then(|| PathBuf::from(top)))
            }
            Ok(output) => {
                tracing::debug!(stderr = %output.stderr_text(), "not a git work tree");
                Ok(None)
            }
            Err(Error::Io(err)) if err.kind() == ErrorKind::NotFound => {
                tracing::debug!("git is not installed");
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    /// The tags pointing at `HEAD` (`git tag --points-at HEAD`).
    pub fn head_tags(&self) -> Result<Vec<String>> {
        let output = self
            .runner
            .run("git", &["tag", "--points-at", "HEAD"], self.dir)?
            .into_success("git tag --points-at HEAD")?;
        Ok(output
            .stdout_text()
            .lines()
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// The version named by the release tag on `HEAD`, see [`select_release_tag`].
    pub fn release_version(&self, tag_prefix: Option<&str>) -> Result<String> {
        let tags = self.head_tags()?;
        let (tag, version) = select_release_tag(&tags, tag_prefix)?;
        tracing::debug!(tag, version, "version from the tag on HEAD");
        Ok(version)
    }

    /// The version a packager that can rewrite its manifest (npm, cargo, nuget) should
    /// publish, or `None` to keep the version declared in the manifest.
    ///
    /// - `version` (the `--version` flag) wins;
    /// - with `tag_prefix`, the tag `<prefix><semver>` on `HEAD` is required;
    /// - otherwise a single `v<semver>` tag on `HEAD` is used when there is one, and
    ///   `None` is returned when there is none or the directory is not in a git work tree;
    ///   several such tags are an error.
    pub fn requested_version(
        &self,
        version: Option<&str>,
        tag_prefix: Option<&str>,
    ) -> Result<Option<String>> {
        match (version, tag_prefix) {
            (Some(_), Some(_)) => Err(Error::InvalidInput(
                "--version and --tag-prefix cannot be combined".into(),
            )),
            (Some(version), None) => Ok(Some(version.to_owned())),
            (None, Some(prefix)) => {
                if self.toplevel()?.is_none() {
                    return Err(Error::InvalidInput(format!(
                        "--tag-prefix needs a git repository, but `{}` is not inside one",
                        self.dir.display()
                    )));
                }
                self.release_version(Some(prefix)).map(Some)
            }
            (None, None) => {
                if self.toplevel()?.is_none() {
                    return Ok(None);
                }
                let tags = self.head_tags()?;
                let found = find_release_tag(&tags, None)?;
                if let Some((tag, version)) = &found {
                    tracing::debug!(tag, version, "version from the tag on HEAD");
                }
                Ok(found.map(|(_, version)| version))
            }
        }
    }
}

/// Like [`select_release_tag`], but `Ok(None)` when no tag on `HEAD` qualifies.
pub fn find_release_tag(
    tags: &[String],
    tag_prefix: Option<&str>,
) -> Result<Option<(String, String)>> {
    let prefix = tag_prefix.unwrap_or(DEFAULT_TAG_PREFIX);
    let qualifies = |tag: &String| {
        tag.strip_prefix(prefix)
            .is_some_and(|version| semver::Version::parse(version).is_ok())
    };
    if tags.iter().any(qualifies) {
        select_release_tag(tags, tag_prefix).map(Some)
    } else {
        Ok(None)
    }
}

/// Picks the release tag among the tags on `HEAD` and returns it with its version.
///
/// A release tag is `<prefix><semver>`; the prefix is `tag_prefix`, or `v` when `None`
/// (`v1.2.3` → `1.2.3`, `foo/v1.2.3` with the prefix `foo/v` → `1.2.3`). Exactly one
/// tag on `HEAD` must qualify: none, or more than one, is an error.
pub fn select_release_tag(tags: &[String], tag_prefix: Option<&str>) -> Result<(String, String)> {
    let prefix = tag_prefix.unwrap_or(DEFAULT_TAG_PREFIX);
    let candidates: Vec<(&String, &str)> = tags
        .iter()
        .filter_map(|tag| {
            let version = tag.strip_prefix(prefix)?;
            semver::Version::parse(version).ok()?;
            Some((tag, version))
        })
        .collect();
    let expected = format!("{prefix}<semver>, e.g. {prefix}1.2.3");
    match candidates.as_slice() {
        [(tag, version)] => Ok(((*tag).clone(), (*version).to_owned())),
        [] => {
            let found = if tags.is_empty() {
                "HEAD has no tags".to_owned()
            } else {
                format!("the tags on HEAD are: {}", tags.join(", "))
            };
            let hint = if tag_prefix.is_none() {
                "tag the release, pass --version, or pass --tag-prefix for tags of another form"
            } else {
                "tag the release or pass --version"
            };
            Err(Error::InvalidInput(format!(
                "no version given and no release tag ({expected}) points at HEAD ({found}); \
                 {hint}"
            )))
        }
        many => {
            let names: Vec<&str> = many.iter().map(|(tag, _)| tag.as_str()).collect();
            Err(Error::InvalidInput(format!(
                "HEAD carries more than one release tag ({}); pass --version to choose one",
                names.join(", ")
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::CommandOutput;

    fn tags(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    struct Fake(std::result::Result<CommandOutput, ErrorKind>);

    impl CommandRunner for Fake {
        fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput> {
            self.0.clone().map_err(|kind| Error::Io(kind.into()))
        }
    }

    #[test]
    fn picks_the_single_v_tag() {
        let (tag, version) =
            select_release_tag(&tags(&["latest", "v1.2.3", "1.2.4", "v1.2"]), None).unwrap();
        assert_eq!((tag.as_str(), version.as_str()), ("v1.2.3", "1.2.3"));
        let (_, version) = select_release_tag(&tags(&["v2.0.0-rc1"]), None).unwrap();
        assert_eq!(version, "2.0.0-rc1");
    }

    #[test]
    fn picks_the_tag_with_the_prefix() {
        let all = tags(&["v9.0.0", "foo/v1.2.3", "bar/v2.0.0", "foo-v3.0.0"]);
        assert_eq!(select_release_tag(&all, Some("foo/v")).unwrap().1, "1.2.3");
        assert_eq!(select_release_tag(&all, Some("foo-v")).unwrap().1, "3.0.0");
        assert_eq!(
            select_release_tag(&tags(&["1.4.0"]), Some("")).unwrap().1,
            "1.4.0"
        );
    }

    #[test]
    fn no_release_tag_is_an_error_naming_the_tags() {
        let err = select_release_tag(&tags(&["1.2.3", "release"]), None).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("v<semver>"), "{message}");
        assert!(message.contains("1.2.3, release"), "{message}");
        assert!(message.contains("--tag-prefix"), "{message}");

        let err = select_release_tag(&[], Some("foo/v")).unwrap_err();
        assert!(err.to_string().contains("HEAD has no tags"), "{err}");
        assert!(err.to_string().contains("foo/v<semver>"), "{err}");
    }

    #[test]
    fn several_release_tags_are_ambiguous() {
        let err = select_release_tag(&tags(&["v1.2.3", "v1.3.0", "x"]), None).unwrap_err();
        assert!(
            err.to_string()
                .contains("more than one release tag (v1.2.3, v1.3.0)"),
            "{err}"
        );
    }

    #[test]
    fn toplevel_is_none_outside_git_or_without_git() {
        let dir = Path::new(".");
        let missing = Fake(Err(ErrorKind::NotFound));
        assert_eq!(Git::new(&missing, dir).toplevel().unwrap(), None);
        let outside = Fake(Ok(CommandOutput::failure(
            128,
            "fatal: not a git repository",
        )));
        assert_eq!(Git::new(&outside, dir).toplevel().unwrap(), None);
        let inside = Fake(Ok(CommandOutput::success("/work/repo\n")));
        assert_eq!(
            Git::new(&inside, dir).toplevel().unwrap(),
            Some(PathBuf::from("/work/repo"))
        );
        let denied = Fake(Err(ErrorKind::PermissionDenied));
        assert!(Git::new(&denied, dir).toplevel().is_err());
    }

    #[test]
    fn head_tags_are_read_line_by_line() {
        let fake = Fake(Ok(CommandOutput::success("v1.0.0\nfoo/v2.0.0\n\n")));
        assert_eq!(
            Git::new(&fake, Path::new(".")).head_tags().unwrap(),
            ["v1.0.0", "foo/v2.0.0"]
        );
        let fake = Fake(Ok(CommandOutput::success("v1.0.0\nv1.0.1\n")));
        assert!(Git::new(&fake, Path::new("."))
            .release_version(None)
            .is_err());
    }

    #[test]
    fn find_release_tag_allows_none() {
        assert_eq!(
            find_release_tag(&tags(&["1.0.0", "x"]), None).unwrap(),
            None
        );
        assert_eq!(
            find_release_tag(&tags(&["v1.0.0"]), None).unwrap(),
            Some(("v1.0.0".to_owned(), "1.0.0".to_owned()))
        );
        assert!(find_release_tag(&tags(&["v1.0.0", "v2.0.0"]), None).is_err());
    }

    /// Answers `git rev-parse` with `toplevel` and `git tag` with `tags`.
    struct Repo {
        toplevel: Option<&'static str>,
        tags: &'static str,
    }

    impl CommandRunner for Repo {
        fn run(&self, _program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput> {
            Ok(match (args.first(), self.toplevel) {
                (Some(&"rev-parse"), Some(top)) => CommandOutput::success(top),
                (Some(&"rev-parse"), None) => CommandOutput::failure(128, "fatal: not a repo"),
                _ => CommandOutput::success(self.tags),
            })
        }
    }

    #[test]
    fn requested_version_prefers_the_flag_then_the_tag() {
        let dir = Path::new(".");
        let repo = Repo {
            toplevel: Some("/repo"),
            tags: "v1.2.3\nsdk/v2.0.0\n",
        };
        let git = Git::new(&repo, dir);
        assert_eq!(
            git.requested_version(Some("9.9.9"), None)
                .unwrap()
                .as_deref(),
            Some("9.9.9")
        );
        assert_eq!(
            git.requested_version(None, None).unwrap().as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            git.requested_version(None, Some("sdk/v"))
                .unwrap()
                .as_deref(),
            Some("2.0.0")
        );
        assert!(git.requested_version(None, Some("api/v")).is_err());
        assert!(git.requested_version(Some("1.0.0"), Some("sdk/v")).is_err());

        let untagged = Repo {
            toplevel: Some("/repo"),
            tags: "release\n",
        };
        assert_eq!(
            Git::new(&untagged, dir)
                .requested_version(None, None)
                .unwrap(),
            None
        );
        let ambiguous = Repo {
            toplevel: Some("/repo"),
            tags: "v1.0.0\nv1.0.1\n",
        };
        assert!(Git::new(&ambiguous, dir)
            .requested_version(None, None)
            .is_err());
        let outside = Repo {
            toplevel: None,
            tags: "",
        };
        assert_eq!(
            Git::new(&outside, dir)
                .requested_version(None, None)
                .unwrap(),
            None
        );
        let err = Git::new(&outside, dir)
            .requested_version(None, Some("sdk/v"))
            .unwrap_err();
        assert!(err.to_string().contains("needs a git repository"), "{err}");
    }

    #[test]
    fn real_git_reports_tags_on_head() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
                .args(args)
                .current_dir(dir.path())
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "x"]);
        git(&["tag", "v1.0.0"]);
        git(&["tag", "-a", "sdk/v2.0.0", "-m", "sdk"]);
        std::fs::create_dir(dir.path().join("sub")).unwrap();

        let runner = crate::process::SystemRunner;
        let sub = dir.path().join("sub");
        let repo = Git::new(&runner, &sub);
        let top = repo.toplevel().unwrap().unwrap();
        assert_eq!(
            std::fs::canonicalize(top).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
        assert_eq!(repo.release_version(None).unwrap(), "1.0.0");
        assert_eq!(repo.release_version(Some("sdk/v")).unwrap(), "2.0.0");
    }
}
