//! `sreg generic publish` and `sreg generic get`.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context};
use sreg_core::download::Downloader;
use sreg_core::publish::PublishOptions;
use sreg_core::{PackageOptions, Packager};
use sreg_eco_generic::{
    parse_key_value, parse_spec, validate_label, validate_name, validate_version, GenericFileSpec,
    GenericPackager,
};

use crate::cli::{GenericGetArgs, GenericPublishArgs, GlobalArgs};
use crate::commands::publish::publish_package;
use crate::context;
use crate::output::Output;

pub async fn publish(
    global: &GlobalArgs,
    out: &Output,
    args: &GenericPublishArgs,
) -> anyhow::Result<()> {
    let files = match &args.spec {
        Some(spec) => {
            let json = std::fs::read_to_string(spec)
                .with_context(|| format!("failed to read {}", spec.display()))?;
            // Paths in the spec are relative to the current directory, like positional files.
            parse_spec(&json, Path::new("")).with_context(|| format!("in {}", spec.display()))?
        }
        None => {
            let labels = parse_labels(&args.labels)?;
            args.files
                .iter()
                .map(|path| GenericFileSpec {
                    path: path.clone(),
                    labels: labels.iter().cloned().collect(),
                })
                .collect()
        }
    };

    let mut meta = match &args.meta_file {
        Some(path) => {
            let json = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            match serde_json::from_str::<serde_json::Value>(&json)
                .with_context(|| format!("{} is not valid JSON", path.display()))?
            {
                serde_json::Value::Object(map) => map,
                _ => bail!("{} must contain a JSON object", path.display()),
            }
        }
        None => serde_json::Map::new(),
    };
    for entry in &args.meta {
        let (key, value) = parse_key_value(entry)?;
        meta.insert(key, serde_json::Value::String(value));
    }

    let packager = GenericPackager {
        name: args.name.clone(),
        files,
        meta,
    };
    let package = packager.package(&PackageOptions {
        version: Some(args.version.clone()),
        ..PackageOptions::default()
    })?;

    let options = PublishOptions {
        dry_run: args.dry_run,
        tag: None,
        append: Some(!args.no_append),
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

pub async fn get(global: &GlobalArgs, out: &Output, args: &GenericGetArgs) -> anyhow::Result<()> {
    validate_name(&args.name)?;
    let latest = args.version == "latest";
    if !latest {
        validate_version(&args.version)?;
        if args.prerelease {
            bail!("--prerelease only applies to `latest`");
        }
    }
    let labels = parse_labels(&args.labels)?;

    let client = context::client_with(global, args.oidc.oidc, args.oidc.project.as_deref()).await?;
    let version = client
        .generic_version(&args.name, &args.version, &labels, args.prerelease)
        .await?;

    if out.is_json() {
        return out.json(&version);
    }
    if version.yanked {
        eprintln!("warning: {} {} is yanked", version.name, version.version);
    }
    if version.files.is_empty() {
        bail!(
            "{} {} has no files matching the requested labels",
            version.name,
            version.version
        );
    }

    std::fs::create_dir_all(&args.out)
        .with_context(|| format!("failed to create {}", args.out.display()))?;
    let downloader = Downloader::new(&client)?;
    for file in &version.files {
        let path = downloader
            .download(file, &args.out)
            .await
            .with_context(|| format!("failed to download {}", file.name))?;
        println!("{}", path.display());
    }
    Ok(())
}

/// Parses `--label key=value` arguments; a key may appear only once.
fn parse_labels(values: &[String]) -> anyhow::Result<Vec<(String, String)>> {
    let mut labels = BTreeMap::new();
    for value in values {
        let (key, label) = parse_key_value(value)?;
        validate_label(&key, &label)?;
        if labels.insert(key.clone(), label).is_some() {
            bail!("label `{key}` is given more than once");
        }
    }
    Ok(labels.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_parsed_once_per_key() {
        let labels = parse_labels(&["os=linux".into(), "arch=x86_64".into()]).unwrap();
        assert_eq!(
            labels,
            [
                ("arch".to_owned(), "x86_64".to_owned()),
                ("os".to_owned(), "linux".to_owned())
            ]
        );
        assert!(parse_labels(&["os=linux".into(), "os=macos".into()]).is_err());
        assert!(parse_labels(&["os:x=linux".into()]).is_err());
    }
}
