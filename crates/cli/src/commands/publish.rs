//! The `publish` commands: package, then `init`, upload and `commit`.

use anyhow::Context;
use serde::Serialize;
use sreg_core::models::PublishCommitResponse;
use sreg_core::publish::{publish, PublishOptions, PublishOutcome};
use sreg_core::{Ecosystem, Package};
#[cfg(any(
    feature = "composer",
    feature = "npm",
    feature = "cargo",
    feature = "nuget"
))]
use sreg_core::{PackageOptions, Packager};

#[cfg(any(
    feature = "composer",
    feature = "npm",
    feature = "cargo",
    feature = "nuget"
))]
use crate::cli::PublishArgs;
use crate::cli::{GlobalArgs, OidcArgs};
use crate::context;
use crate::output::{table, Output};

/// Packages the project with `packager` and publishes it.
#[cfg(any(
    feature = "composer",
    feature = "npm",
    feature = "cargo",
    feature = "nuget"
))]
pub async fn run_packager(
    global: &GlobalArgs,
    out: &Output,
    packager: &dyn Packager,
    args: &PublishArgs,
    tag: Option<String>,
    extra_args: Vec<String>,
) -> anyhow::Result<()> {
    let eco = packager.ecosystem();
    let opts = PackageOptions {
        path: args.path.clone(),
        version: args.version.clone(),
        tag_prefix: None,
        tag: tag.clone(),
        extra_args,
    };
    let package = packager.package(&opts).with_context(|| {
        format!(
            "failed to package the {eco} project in {}",
            args.path.display()
        )
    })?;
    let options = PublishOptions {
        dry_run: args.dry_run,
        tag,
        append: None,
    };
    publish_package(global, out, &package, options, &args.oidc).await
}

/// Publishes an already built `package`.
///
/// The token is resolved only now, after packaging, so that a short-lived OIDC token
/// is as fresh as possible when the upload starts.
pub async fn publish_package(
    global: &GlobalArgs,
    out: &Output,
    package: &Package,
    options: PublishOptions,
    oidc: &OidcArgs,
) -> anyhow::Result<()> {
    let client = context::client_with(global, oidc.oidc, oidc.project.as_deref()).await?;
    let outcome = publish(&client, package, options).await.with_context(|| {
        format!(
            "failed to publish {} {} {}",
            package.eco, package.name, package.version
        )
    })?;

    match outcome {
        PublishOutcome::DryRun { init } => {
            let report = DryRunReport::new(package, init.upload_id, init.expires);
            out.emit(&report, || print_dry_run(&report))
        }
        PublishOutcome::Published { response } => {
            out.emit(&response, || print_published(&response))
        }
    }
}

/// `--dry-run --json` output. The presigned URLs are deliberately left out.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DryRunReport {
    dry_run: bool,
    eco: Ecosystem,
    name: String,
    version: String,
    upload_id: String,
    expires: String,
    files: Vec<DryRunFile>,
}

#[derive(Serialize)]
struct DryRunFile {
    name: String,
    size: u64,
    sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    labels: std::collections::BTreeMap<String, String>,
}

impl DryRunReport {
    fn new(package: &Package, upload_id: String, expires: String) -> Self {
        DryRunReport {
            dry_run: true,
            eco: package.eco,
            name: package.name.clone(),
            version: package.version.clone(),
            upload_id,
            expires,
            files: package
                .files
                .iter()
                .map(|file| DryRunFile {
                    name: file.name.clone(),
                    size: file.size,
                    sha256: file.sha256.clone(),
                    role: file.role.clone(),
                    labels: file.labels.clone(),
                })
                .collect(),
        }
    }
}

fn print_dry_run(report: &DryRunReport) {
    println!(
        "Dry run: the registry accepted {} {} {}; nothing was uploaded.",
        report.eco, report.name, report.version
    );
    let rows: Vec<Vec<String>> = report
        .files
        .iter()
        .map(|file| {
            let labels = file
                .labels
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(",");
            vec![
                file.name.clone(),
                file.size.to_string(),
                file.sha256.clone(),
                labels,
            ]
        })
        .collect();
    println!("{}", table(&["FILE", "SIZE", "SHA256", "LABELS"], &rows));
}

fn print_published(response: &PublishCommitResponse) {
    println!(
        "Published {} {} {}",
        response.eco, response.name, response.version
    );
    println!("{}", response.url);
    if response.files.len() > 1 {
        let rows: Vec<Vec<String>> = response
            .files
            .iter()
            .map(|file| vec![file.name.clone(), file.url.clone()])
            .collect();
        println!("{}", table(&["FILE", "URL"], &rows));
    }
}
