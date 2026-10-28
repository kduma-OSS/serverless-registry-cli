//! Process exit codes of `sreg`.
//!
//! Pipelines rely on these values to tell a failed publication apart from one that
//! was skipped because the version already exists.

use sreg_core::models::GenericCandidate;
use sreg_core::Error;

/// The command completed successfully.
pub const SUCCESS: u8 = 0;

/// Generic error: invalid usage, network failure, rejected request and so on.
pub const FAILURE: u8 = 1;

/// The version (or, when appending, one of its files) is already published
/// (HTTP `409 version_exists` or `409 file_exists`).
pub const VERSION_EXISTS: u8 = 2;

/// This client is older than the minimum version the registry accepts.
pub const CLIENT_TOO_OLD: u8 = 3;

/// Where newer releases of `sreg` are published.
pub const RELEASES_URL: &str = "https://github.com/kduma-OSS/serverless-registry-cli/releases";

/// Finds the library error inside an error chain.
fn core_error(err: &anyhow::Error) -> Option<&Error> {
    err.chain().find_map(|cause| cause.downcast_ref::<Error>())
}

/// An extra line printed after the error message, if any.
pub fn hint_for(err: &anyhow::Error) -> Option<String> {
    match core_error(err) {
        Some(Error::ClientTooOld { min }) => {
            Some(format!("download sreg {min} or newer from {RELEASES_URL}"))
        }
        Some(Error::VersionExists) => {
            Some("published versions are immutable; bump the version to publish again".into())
        }
        Some(Error::FileExists { .. }) => {
            Some("published files are immutable; use another file name or a new version".into())
        }
        Some(Error::Ambiguous { version, files, .. }) => {
            Some(ambiguous_hint(version.as_deref(), files))
        }
        _ => None,
    }
}

/// Lists the files a `--sole` label filter selects, so that the user can narrow it.
fn ambiguous_hint(version: Option<&str>, files: &[GenericCandidate]) -> String {
    let mut hint = match version {
        Some(version) => format!("add --label filters to select one of these files of {version}:"),
        None => "add --label filters to select one of these files:".to_owned(),
    };
    for file in files {
        hint.push_str("\n  ");
        hint.push_str(&file.name);
        if !file.labels.is_empty() {
            let labels: Vec<String> = file
                .labels
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            hint.push_str(&format!(" ({})", labels.join(", ")));
        }
    }
    hint
}

/// Maps an error to the exit code the process should terminate with.
pub fn code_for(err: &anyhow::Error) -> u8 {
    match core_error(err) {
        Some(Error::VersionExists | Error::FileExists { .. }) => VERSION_EXISTS,
        Some(Error::ClientTooOld { .. }) => CLIENT_TOO_OLD,
        _ => FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::*;

    #[test]
    fn version_exists_maps_to_its_own_code() {
        let err = anyhow::Error::new(Error::VersionExists);
        assert_eq!(code_for(&err), VERSION_EXISTS);
    }

    #[test]
    fn file_exists_maps_to_the_version_exists_code() {
        let err = anyhow::Error::new(Error::FileExists {
            message: "x".into(),
        });
        assert_eq!(code_for(&err), VERSION_EXISTS);
    }

    #[test]
    fn ambiguous_labels_fail_with_one_and_list_the_candidates() {
        let err = anyhow::Error::new(Error::Ambiguous {
            message: "2 files match".into(),
            version: None,
            files: vec![
                GenericCandidate {
                    name: "a.tgz".into(),
                    labels: [("os".to_owned(), "linux".to_owned())].into(),
                },
                GenericCandidate {
                    name: "b.tgz".into(),
                    labels: Default::default(),
                },
            ],
        });
        assert_eq!(code_for(&err), FAILURE);
        assert_eq!(
            hint_for(&err).unwrap(),
            "add --label filters to select one of these files:\n  a.tgz (os=linux)\n  b.tgz"
        );
    }

    #[test]
    fn client_too_old_maps_to_its_own_code() {
        let err = anyhow::Error::new(Error::ClientTooOld {
            min: "1.2.0".into(),
        });
        assert_eq!(code_for(&err), CLIENT_TOO_OLD);
    }

    #[test]
    fn wrapped_errors_are_recognised() {
        let err = Err::<(), _>(Error::VersionExists)
            .context("publishing acme/sdk 1.0.0")
            .unwrap_err();
        assert_eq!(code_for(&err), VERSION_EXISTS);
    }

    #[test]
    fn client_too_old_hint_names_the_minimum_and_the_releases() {
        let err = anyhow::Error::new(Error::ClientTooOld {
            min: "1.2.0".into(),
        });
        let hint = hint_for(&err).unwrap();
        assert!(hint.contains("1.2.0"));
        assert!(hint.contains(RELEASES_URL));
        assert_eq!(hint_for(&anyhow::anyhow!("boom")), None);
    }

    #[test]
    fn other_errors_are_generic_failures() {
        assert_eq!(code_for(&anyhow::anyhow!("boom")), FAILURE);
        let err = anyhow::Error::new(Error::InvalidInput("bad".into()));
        assert_eq!(code_for(&err), FAILURE);
    }
}
