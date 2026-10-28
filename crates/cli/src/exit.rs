//! Process exit codes of `sreg`.
//!
//! Pipelines rely on these values to tell a failed publication apart from one that
//! was skipped because the version already exists.

use sreg_core::Error;

/// The command completed successfully.
pub const SUCCESS: u8 = 0;

/// Generic error: invalid usage, network failure, rejected request and so on.
pub const FAILURE: u8 = 1;

/// The version being published already exists in the registry (HTTP `409`).
pub const VERSION_EXISTS: u8 = 2;

/// This client is older than the minimum version the registry accepts.
pub const CLIENT_TOO_OLD: u8 = 3;

/// Maps an error to the exit code the process should terminate with.
pub fn code_for(err: &anyhow::Error) -> u8 {
    match err.chain().find_map(|cause| cause.downcast_ref::<Error>()) {
        Some(Error::VersionExists) => VERSION_EXISTS,
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
    fn other_errors_are_generic_failures() {
        assert_eq!(code_for(&anyhow::anyhow!("boom")), FAILURE);
        let err = anyhow::Error::new(Error::InvalidInput("bad".into()));
        assert_eq!(code_for(&err), FAILURE);
    }
}
