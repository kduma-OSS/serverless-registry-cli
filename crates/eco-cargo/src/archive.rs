//! Publishing a `.crate` built earlier: the sparse-index line comes from the
//! `Cargo.toml` inside the archive, and the archive itself is never touched.
//!
//! `cargo package` normalises the manifest it puts into the archive: workspace
//! inheritance is resolved, registries are spelled as `registry-index` URLs and path-only
//! dependencies are gone. That is the manifest `cargo publish` describes to a registry,
//! so the index line is built from it the same way.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use serde_json::{Map, Value};
use sreg_core::Error;
use toml::{Table, Value as Toml};

use crate::index::{split_features, IndexDependency, CRATES_IO_INDEX, CRATES_IO_SOURCES};

/// What `sreg` learned from a `.crate`.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchivedCrate {
    /// Crate name.
    pub name: String,
    /// Crate version.
    pub version: String,
    /// The sparse-index line, without the fields the registry computes.
    pub index_line: Value,
    /// How many dependencies were attributed to the source registry.
    pub source_registry_dependencies: usize,
}

/// Reads the `.crate` at `path`.
///
/// Dependencies from the registry whose index is `source_registry_index` become
/// dependencies of the registry being published to (`registry: null`); dependencies from
/// any other registry keep their index URL.
pub fn read_crate(
    path: &Path,
    source_registry_index: Option<&str>,
) -> Result<ArchivedCrate, Error> {
    let (top, text) = read_manifest(path)?;
    let manifest: Table = text.parse().map_err(|err| {
        Error::InvalidInput(format!(
            "the Cargo.toml in `{}` is not valid TOML: {err}",
            path.display()
        ))
    })?;
    let package = manifest
        .get("package")
        .and_then(Toml::as_table)
        .ok_or_else(|| Error::InvalidInput("the archived Cargo.toml has no [package]".into()))?;
    let field = |key: &str| {
        package.get(key).and_then(Toml::as_str).ok_or_else(|| {
            Error::InvalidInput(format!(
                "the archived Cargo.toml has no `package.{key}` string (a published crate \
                 carries a literal {key})"
            ))
        })
    };
    let name = field("name")?.to_owned();
    let version = field("version")?.to_owned();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err(Error::InvalidInput(format!(
            "invalid crate name `{name}` in the archived Cargo.toml"
        )));
    }
    crate::validate_version(&version)?;
    if top != format!("{name}-{version}") {
        return Err(Error::InvalidInput(format!(
            "the archive holds `{top}/`, but its Cargo.toml is for {name} {version}; expected \
             `{name}-{version}/`"
        )));
    }

    let mut matched = 0;
    let mut deps = Vec::new();
    let mut collect = |tables: &Table, target: Option<&str>| -> Result<(), Error> {
        for (section, kind) in [
            ("dependencies", "normal"),
            ("dev-dependencies", "dev"),
            ("dev_dependencies", "dev"),
            ("build-dependencies", "build"),
            ("build_dependencies", "build"),
        ] {
            let Some(table) = tables.get(section).and_then(Toml::as_table) else {
                continue;
            };
            for (key, spec) in table {
                let dep = dependency(key, spec, kind, target, source_registry_index)?;
                matched += usize::from(dep.1);
                deps.push(dep.0);
            }
        }
        Ok(())
    };
    collect(&manifest, None)?;
    if let Some(targets) = manifest.get("target").and_then(Toml::as_table) {
        for (target, tables) in targets {
            if let Some(tables) = tables.as_table() {
                collect(tables, Some(target))?;
            }
        }
    }

    let mut features = Map::new();
    if let Some(table) = manifest.get("features").and_then(Toml::as_table) {
        for (feature, values) in table {
            let values: Option<Vec<Value>> = values.as_array().map(|items| {
                items
                    .iter()
                    .filter_map(Toml::as_str)
                    .map(Value::from)
                    .collect()
            });
            let values = values.ok_or_else(|| {
                Error::InvalidInput(format!("feature `{feature}` must be an array of strings"))
            })?;
            features.insert(feature.clone(), Value::Array(values));
        }
    }
    let (features, features2) = split_features(&features);

    let optional = |key: &str| {
        package
            .get(key)
            .and_then(Toml::as_str)
            .map_or(Value::Null, Value::from)
    };
    let mut line = Map::new();
    line.insert("name".into(), Value::from(name.clone()));
    line.insert("vers".into(), Value::from(version.clone()));
    line.insert(
        "deps".into(),
        serde_json::to_value(deps).unwrap_or(Value::Array(Vec::new())),
    );
    line.insert("features".into(), Value::Object(features));
    if !features2.is_empty() {
        line.insert("features2".into(), Value::Object(features2));
    }
    line.insert("links".into(), optional("links"));
    line.insert("rust_version".into(), optional("rust-version"));

    Ok(ArchivedCrate {
        name,
        version,
        index_line: Value::Object(line),
        source_registry_dependencies: matched,
    })
}

/// One dependency of the archived manifest as an index entry, and whether it came from
/// the source registry.
fn dependency(
    key: &str,
    spec: &Toml,
    kind: &str,
    target: Option<&str>,
    source_registry_index: Option<&str>,
) -> Result<(IndexDependency, bool), Error> {
    let invalid = |detail: &str| {
        Error::InvalidInput(format!(
            "dependency `{key}` in the archived Cargo.toml: {detail}"
        ))
    };
    let detailed = match spec {
        Toml::String(req) => {
            let mut table = Table::new();
            table.insert("version".into(), Toml::String(req.clone()));
            table
        }
        Toml::Table(table) => table.clone(),
        _ => return Err(invalid("expected a version string or a table")),
    };
    let text = |name: &str| -> Result<Option<&str>, Error> {
        match detailed.get(name) {
            None => Ok(None),
            Some(value) => value
                .as_str()
                .map(Some)
                .ok_or_else(|| invalid(&format!("`{name}` must be a string"))),
        }
    };
    let flag = |names: &[&str], default: bool| -> Result<bool, Error> {
        for name in names {
            if let Some(value) = detailed.get(*name) {
                return value
                    .as_bool()
                    .ok_or_else(|| invalid(&format!("`{name}` must be a boolean")));
            }
        }
        Ok(default)
    };
    let features = match detailed.get("features") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .ok_or_else(|| invalid("`features` must be an array"))?
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid("`features` must hold strings"))
            })
            .collect::<Result<Vec<_>, _>>()?,
    };

    let (registry, from_source) = match (text("registry-index")?, text("registry")?) {
        (Some(index), _) => match source_registry_index {
            Some(source) if same_index(index, source) => (None, true),
            _ if CRATES_IO_SOURCES.contains(&index) => (Some(CRATES_IO_INDEX.to_owned()), false),
            _ => (Some(index.to_owned()), false),
        },
        (None, Some(name)) => {
            return Err(invalid(&format!(
                "it names the registry `{name}`, which an archive cannot resolve to an index \
                 URL (`cargo package` writes `registry-index`)"
            )))
        }
        (None, None) => (Some(CRATES_IO_INDEX.to_owned()), false),
    };
    let (name, package) = match text("package")? {
        Some(real) => (key.to_owned(), Some(real.to_owned())),
        None => (key.to_owned(), None),
    };
    Ok((
        IndexDependency {
            name,
            req: text("version")?.unwrap_or("*").to_owned(),
            features,
            optional: flag(&["optional"], false)?,
            default_features: flag(&["default-features", "default_features"], true)?,
            target: target.map(str::to_owned),
            kind: kind.to_owned(),
            registry,
            package,
        },
        from_source,
    ))
}

/// Whether two registry index URLs name the same registry: the `sparse+`/`registry+`
/// prefix, the case of scheme and host and trailing slashes are not significant.
pub(crate) fn same_index(a: &str, b: &str) -> bool {
    normalize_index(a) == normalize_index(b)
}

fn normalize_index(url: &str) -> String {
    let url = url.trim();
    let url = url
        .strip_prefix("sparse+")
        .or_else(|| url.strip_prefix("registry+"))
        .unwrap_or(url);
    let url = url.trim_end_matches('/');
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (host, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
            let host = host.to_ascii_lowercase();
            if path.is_empty() {
                format!("{}://{host}", scheme.to_ascii_lowercase())
            } else {
                format!("{}://{host}/{path}", scheme.to_ascii_lowercase())
            }
        }
        None => url.to_owned(),
    }
}

/// Reads the whole `.crate` (checking that it is a valid gzipped tarball) and returns
/// the name of its top-level directory and the text of `<top>/Cargo.toml`.
fn read_manifest(path: &Path) -> Result<(String, String), Error> {
    let invalid = |err: &dyn std::fmt::Display| {
        Error::InvalidInput(format!("`{}` is not a valid .crate: {err}", path.display()))
    };
    let mut archive = tar::Archive::new(GzDecoder::new(File::open(path)?));
    let mut found: Option<(String, String)> = None;
    for entry in archive.entries().map_err(|err| invalid(&err))? {
        let mut entry = entry.map_err(|err| invalid(&err))?;
        let entry_path = entry.path().map_err(|err| invalid(&err))?.into_owned();
        let parts: Vec<_> = entry_path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if let [top, file] = parts.as_slice() {
            if file == "Cargo.toml" && found.is_none() {
                let mut text = String::new();
                entry
                    .read_to_string(&mut text)
                    .map_err(|err| invalid(&err))?;
                found = Some((top.clone(), text));
            }
        }
    }
    // The tar reader stops at its end marker; reading the gzip stream to its end checks
    // the trailer, so that a truncated file is rejected.
    std::io::copy(&mut archive.into_inner(), &mut std::io::sink()).map_err(|err| invalid(&err))?;
    found.ok_or_else(|| invalid(&"it has no <name>-<version>/Cargo.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_urls_are_compared_loosely() {
        let source = "sparse+https://old.example.com/cargo/index/";
        for same in [
            "sparse+https://old.example.com/cargo/index/",
            "sparse+https://old.example.com/cargo/index",
            "sparse+HTTPS://OLD.example.com/cargo/index/",
            "registry+https://old.example.com/cargo/index/",
            "https://old.example.com/cargo/index",
        ] {
            assert!(same_index(same, source), "{same}");
        }
        for other in [
            "sparse+https://old.example.com/cargo/other/",
            "sparse+https://old.example.com/Cargo/index/",
            "sparse+https://new.example.com/cargo/index/",
            "sparse+https://index.crates.io/",
        ] {
            assert!(!same_index(other, source), "{other}");
        }
    }
}
