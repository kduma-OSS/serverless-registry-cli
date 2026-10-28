//! The version's manifest: one line of the cargo sparse index, built from
//! `cargo metadata`, without the fields the registry computes (`cksum`, `yanked`, `v`).

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::metadata::{MetadataDependency, MetadataPackage};

/// Index URL of crates.io, as cargo writes it into index lines.
pub const CRATES_IO_INDEX: &str = "https://github.com/rust-lang/crates.io-index";

/// Ways crates.io's index is spelled in `source` and `registry` of `cargo metadata`.
pub(crate) const CRATES_IO_SOURCES: [&str; 4] = [
    "registry+https://github.com/rust-lang/crates.io-index",
    "https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
    "https://index.crates.io/",
];

/// One dependency of an index line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexDependency {
    /// The name used in code: the rename when the dependency is renamed.
    pub name: String,
    /// Version requirement as cargo writes it.
    pub req: String,
    /// Features enabled on the dependency.
    pub features: Vec<String>,
    /// Whether the dependency is optional.
    pub optional: bool,
    /// Whether the dependency's default features are enabled.
    pub default_features: bool,
    /// `cfg(…)` or target triple the dependency is limited to.
    pub target: Option<String>,
    /// `normal`, `dev` or `build`.
    pub kind: String,
    /// Index URL of the dependency's registry; `None` for this registry.
    pub registry: Option<String>,
    /// The real crate name when the dependency is renamed.
    pub package: Option<String>,
}

/// Builds the index line of `package` published as `version`.
pub fn index_line(package: &MetadataPackage, version: &str) -> Value {
    let deps: Vec<IndexDependency> = package
        .dependencies
        .iter()
        .filter(|dep| !stripped_on_publish(dep))
        .map(index_dependency)
        .collect();
    let (features, features2) = split_features(&explicit_features(package));

    let mut line = Map::new();
    line.insert("name".into(), Value::from(package.name.clone()));
    line.insert("vers".into(), Value::from(version));
    line.insert(
        "deps".into(),
        serde_json::to_value(deps).unwrap_or(Value::Array(Vec::new())),
    );
    line.insert("features".into(), Value::Object(features));
    if !features2.is_empty() {
        line.insert("features2".into(), Value::Object(features2));
    }
    line.insert("links".into(), package.links.clone().into());
    line.insert("rust_version".into(), package.rust_version.clone().into());
    Value::Object(line)
}

/// `cargo publish` drops dev-dependencies that have only a path (no version): they
/// cannot be resolved from a registry.
fn stripped_on_publish(dep: &MetadataDependency) -> bool {
    dep.kind.as_deref() == Some("dev") && dep.path.is_some() && dep.req == "*"
}

fn index_dependency(dep: &MetadataDependency) -> IndexDependency {
    let (name, package) = match &dep.rename {
        Some(rename) => (rename.clone(), Some(dep.name.clone())),
        None => (dep.name.clone(), None),
    };
    IndexDependency {
        name,
        req: dep.req.clone(),
        features: dep.features.clone(),
        optional: dep.optional,
        default_features: dep.uses_default_features,
        target: dep.target.clone(),
        kind: dep.kind.clone().unwrap_or_else(|| "normal".to_owned()),
        registry: dependency_registry(dep),
        package,
    }
}

/// The registry a dependency is published from, as `cargo publish` declares it:
///
/// - `registry` set in `cargo metadata` (an alternative registry): `None`, i.e. this
///   registry, unless it is crates.io's index under another spelling. sreg cannot tell
///   one alternative registry from another, so every alternative registry is taken to
///   be the one being published to;
/// - otherwise a `source` of an alternative registry (`registry+`/`sparse+` other than
///   crates.io) is treated the same way;
/// - everything else (crates.io dependencies, and path or git dependencies with a
///   version, which cargo publishes as crates.io dependencies): crates.io.
fn dependency_registry(dep: &MetadataDependency) -> Option<String> {
    let alternative = |url: &str| !CRATES_IO_SOURCES.contains(&url);
    let from_alternative = match (&dep.registry, &dep.source) {
        (Some(registry), _) => alternative(registry),
        (None, Some(source))
            if source.starts_with("registry+") || source.starts_with("sparse+") =>
        {
            alternative(source)
        }
        _ => false,
    };
    (!from_alternative).then(|| CRATES_IO_INDEX.to_owned())
}

/// The features as written in the manifest: `cargo metadata` also lists the implicit
/// feature `x = ["dep:x"]` of every optional dependency `x` not referenced with `dep:`
/// elsewhere, which `cargo publish` does not send.
fn explicit_features(package: &MetadataPackage) -> Map<String, Value> {
    let values = |value: &Value| -> Vec<String> {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let optional: BTreeSet<&str> = package
        .dependencies
        .iter()
        .filter(|dep| dep.optional)
        .map(|dep| dep.rename.as_deref().unwrap_or(&dep.name))
        .collect();
    let referenced_elsewhere = |dep: &str, feature: &str| {
        let needle = format!("dep:{dep}");
        package
            .features
            .iter()
            .any(|(name, value)| name != feature && values(value).contains(&needle))
    };
    package
        .features
        .iter()
        .filter(|(name, value)| {
            let implicit = optional.contains(name.as_str())
                && values(value) == [format!("dep:{name}")]
                && !referenced_elsewhere(name, name);
            !implicit
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// Splits features like `cargo publish`: a feature using the `dep:` or `pkg?/feature`
/// syntax in any of its values goes to `features2`, every other one to `features`.
pub(crate) fn split_features(
    features: &Map<String, Value>,
) -> (Map<String, Value>, Map<String, Value>) {
    let mut plain = Map::new();
    let mut extended = Map::new();
    for (name, value) in features {
        let new_syntax = value.as_array().is_some_and(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .any(|item| item.starts_with("dep:") || item.contains("?/"))
        });
        let target = if new_syntax {
            &mut extended
        } else {
            &mut plain
        };
        target.insert(name.clone(), value.clone());
    }
    (plain, extended)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn dep(value: Value) -> MetadataDependency {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn registries_follow_cargo_publish() {
        let crates_io = dep(json!({
            "name": "serde", "req": "^1", "source": "registry+https://github.com/rust-lang/crates.io-index"
        }));
        assert_eq!(
            dependency_registry(&crates_io).as_deref(),
            Some(CRATES_IO_INDEX)
        );
        let path = dep(json!({ "name": "util", "req": "^0.2", "path": "/w/util" }));
        assert_eq!(dependency_registry(&path).as_deref(), Some(CRATES_IO_INDEX));
        let ours = dep(json!({
            "name": "mine", "req": "^0.3",
            "source": "sparse+https://packages.example.com/cargo/",
            "registry": "sparse+https://packages.example.com/cargo/"
        }));
        assert_eq!(dependency_registry(&ours), None);
        let ours_by_source = dep(json!({
            "name": "mine", "req": "^0.3", "source": "sparse+https://packages.example.com/cargo/"
        }));
        assert_eq!(dependency_registry(&ours_by_source), None);
        let sparse_crates_io = dep(json!({
            "name": "serde", "req": "^1", "registry": "sparse+https://index.crates.io/"
        }));
        assert_eq!(
            dependency_registry(&sparse_crates_io).as_deref(),
            Some(CRATES_IO_INDEX)
        );
    }

    #[test]
    fn features_split_on_the_new_syntax() {
        let features = json!({
            "default": ["std"],
            "std": [],
            "json": ["dep:serde_json"],
            "derive": ["serde?/derive", "std"],
            "full": ["json", "derive"]
        });
        let (plain, extended) = split_features(features.as_object().unwrap());
        assert_eq!(
            Value::Object(plain),
            json!({ "default": ["std"], "std": [], "full": ["json", "derive"] })
        );
        assert_eq!(
            Value::Object(extended),
            json!({ "json": ["dep:serde_json"], "derive": ["serde?/derive", "std"] })
        );
    }
}
