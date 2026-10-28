//! `sreg version`.

use serde::Serialize;
use sreg_core::models::VersionInfo;

use crate::cli::GlobalArgs;
use crate::context::{self, CLIENT_VERSION};
use crate::exit::RELEASES_URL;
use crate::output::Output;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    client_version: &'static str,
    registry: Option<VersionInfo>,
    compatible: Option<bool>,
}

pub async fn run(global: &GlobalArgs, out: &Output) -> anyhow::Result<()> {
    // The version endpoint is anonymous: credentials in the URL are never sent.
    let registry = match &global.registry {
        Some(_) => {
            let registry = context::registry(global)?;
            Some(context::anonymous_client(registry.base)?.version().await?)
        }
        None => None,
    };
    let compatible = registry
        .as_ref()
        .and_then(|info| is_compatible(CLIENT_VERSION, &info.min_client_version));
    let report = Report {
        client_version: CLIENT_VERSION,
        registry,
        compatible,
    };

    out.emit(&report, || {
        println!("sreg {CLIENT_VERSION}");
        if let Some(info) = &report.registry {
            println!(
                "registry {} (schema {}, minimum client {})",
                info.version, info.schema_version, info.min_client_version
            );
        }
    })?;
    if report.compatible == Some(false) {
        eprintln!(
            "warning: this version of sreg is too old for the registry; download {} or newer \
             from {RELEASES_URL}",
            report
                .registry
                .as_ref()
                .map(|info| info.min_client_version.as_str())
                .unwrap_or_default()
        );
    }
    Ok(())
}

/// Whether `client` satisfies `min`; `None` when either is not valid semver.
fn is_compatible(client: &str, min: &str) -> Option<bool> {
    let client = semver::Version::parse(client).ok()?;
    let min = semver::Version::parse(min).ok()?;
    Some(client >= min)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_with_semver_precedence() {
        assert_eq!(is_compatible("0.2.0", "0.2.0-beta.1"), Some(true));
        assert_eq!(is_compatible("0.2.0-beta.1", "0.2.0"), Some(false));
        assert_eq!(is_compatible("0.10.0", "0.9.0"), Some(true));
        assert_eq!(is_compatible("0.1.0", "garbage"), None);
    }
}
