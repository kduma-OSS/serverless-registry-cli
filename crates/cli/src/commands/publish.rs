//! The `publish` commands: package, then `init`, upload and `commit`.

use serde::Serialize;
use sreg_core::models::PublishCommitResponse;
use sreg_core::publish::{publish, PublishOptions, PublishOutcome};
use sreg_core::{Ecosystem, Error, Package};
#[cfg(any(
    feature = "composer",
    feature = "npm",
    feature = "cargo",
    feature = "nuget",
    feature = "swift",
    feature = "maven"
))]
use sreg_core::{PackageOptions, Packager};

#[cfg(any(
    feature = "composer",
    feature = "npm",
    feature = "cargo",
    feature = "nuget",
    feature = "swift",
    feature = "maven"
))]
use crate::cli::PublishArgs;
use crate::cli::{GlobalArgs, OidcArgs};
use crate::context;
use crate::output::{table, Output};

/// Packages the project in `path` with `packager` and publishes it.
#[cfg(any(
    feature = "composer",
    feature = "npm",
    feature = "cargo",
    feature = "nuget",
    feature = "swift",
    feature = "maven"
))]
pub async fn run_packager(
    global: &GlobalArgs,
    out: &Output,
    packager: &dyn Packager,
    args: &PublishArgs,
    path: &std::path::Path,
    tag: Option<String>,
    extra_args: &[std::ffi::OsString],
) -> anyhow::Result<()> {
    use anyhow::Context;

    let eco = packager.ecosystem();
    let extra_args = extra_args
        .iter()
        .map(|arg| {
            arg.to_str().map(str::to_owned).ok_or_else(|| {
                anyhow::anyhow!("the argument {arg:?} after `--` is not valid UTF-8")
            })
        })
        .collect::<anyhow::Result<Vec<String>>>()?;
    let opts = PackageOptions {
        path: path.to_path_buf(),
        version: args.version.clone(),
        tag_prefix: args.tag_prefix.clone(),
        tag: tag.clone(),
        extra_args,
    };
    let package = packager.package(&opts).with_context(|| {
        let dir = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        format!("failed to package the {eco} project in {}", dir.display())
    })?;
    // Notes go to stderr, so that `--json` output stays a single document.
    for note in &package.notes {
        eprintln!("note: {note}");
    }
    let options = PublishOptions {
        dry_run: args.dry_run,
        tag,
        append: None,
    };
    publish_package(
        global,
        out,
        &package,
        options,
        &args.oidc,
        args.skip_existing,
    )
    .await
}

/// Publishes the archive at `archive`, built earlier, with `packager`.
///
/// The archive is described from the manifest inside it and uploaded as it is; no
/// packaging tool runs and no git checkout is needed.
#[cfg(any(
    feature = "composer",
    feature = "npm",
    feature = "cargo",
    feature = "nuget"
))]
pub async fn run_archive(
    global: &GlobalArgs,
    out: &Output,
    packager: &dyn Packager,
    args: &PublishArgs,
    archive: &std::path::Path,
    tag: Option<String>,
) -> anyhow::Result<()> {
    use anyhow::Context;

    let eco = packager.ecosystem();
    let opts = sreg_core::ArchiveOptions {
        path: archive.to_path_buf(),
        version: args.version.clone(),
    };
    let package = packager
        .package_archive(&opts)
        .with_context(|| format!("failed to read the {eco} archive {}", archive.display()))?;
    for note in &package.notes {
        eprintln!("note: {note}");
    }
    let options = PublishOptions {
        dry_run: args.dry_run,
        tag,
        append: None,
    };
    publish_package(
        global,
        out,
        &package,
        options,
        &args.oidc,
        args.skip_existing,
    )
    .await
}

/// Publishes an already built `package`.
///
/// The token is resolved only now, after packaging, so that a short-lived OIDC token
/// is as fresh as possible when the upload starts. With `skip_existing`, a version (or,
/// when appending, a file) that is already published is reported as skipped and the
/// command succeeds instead of exiting with code 2.
pub async fn publish_package(
    global: &GlobalArgs,
    out: &Output,
    package: &Package,
    options: PublishOptions,
    oidc: &OidcArgs,
    skip_existing: bool,
) -> anyhow::Result<()> {
    let client = context::client_with(global, oidc.oidc, oidc.project.as_deref()).await?;
    let outcome = match publish(&client, package, options).await {
        Ok(outcome) => outcome,
        Err(err @ (Error::VersionExists | Error::FileExists { .. })) if skip_existing => {
            tracing::debug!(error = %err, "already published; skipping");
            let report = SkippedReport {
                skipped: true,
                eco: package.eco,
                name: package.name.clone(),
                version: package.version.clone(),
            };
            return out.emit(&report, || {
                println!(
                    "{} {} {}: version already published, skipping",
                    report.eco, report.name, report.version
                )
            });
        }
        Err(err) => {
            return Err(anyhow::Error::new(err).context(format!(
                "failed to publish {} {} {}",
                package.eco, package.name, package.version
            )))
        }
    };

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

/// `--skip-existing --json` output when the version is already published.
#[derive(Serialize)]
struct SkippedReport {
    skipped: bool,
    eco: Ecosystem,
    name: String,
    version: String,
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
            .map(|file| {
                let url = file.url.as_deref().unwrap_or("(no download URL)");
                vec![file.name.clone(), url.to_owned()]
            })
            .collect();
        println!("{}", table(&["FILE", "URL"], &rows));
    }
}
