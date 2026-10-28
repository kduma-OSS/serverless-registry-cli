//! Writing the version into `Cargo.toml` without disturbing its formatting.

use sreg_core::Error;
use toml_edit::{DocumentMut, Item, Value};

/// Replaces `package.version` in the text of a `Cargo.toml`, keeping comments,
/// whitespace and the order of keys; a missing `version` is added.
///
/// A version inherited from the workspace (`version.workspace = true`) cannot be
/// changed per crate, so it is refused with a message pointing at the workspace.
pub fn set_package_version(text: &str, version: &str) -> Result<String, Error> {
    let mut document: DocumentMut = text
        .parse()
        .map_err(|err| Error::InvalidInput(format!("Cargo.toml is not valid TOML: {err}")))?;
    let package = document
        .get_mut("package")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| Error::InvalidInput("Cargo.toml has no [package] table".into()))?;
    match package.get_mut("version") {
        None => {
            package.insert("version", toml_edit::value(version));
        }
        Some(item) => {
            if let Some(current) = item.as_value_mut().filter(|value| value.is_str()) {
                let decor = current.decor().clone();
                *current = Value::from(version);
                *current.decor_mut() = decor;
            } else if item
                .as_table_like()
                .is_some_and(|table| table.contains_key("workspace"))
            {
                return Err(Error::InvalidInput(format!(
                    "the crate inherits its version from the workspace \
                     (`version.workspace = true`), so sreg cannot set it to {version} for this \
                     crate alone; set `version` in `[workspace.package]` of the workspace's \
                     Cargo.toml (and commit it) instead"
                )));
            } else {
                return Err(Error::InvalidInput(
                    "`package.version` in Cargo.toml is not a string".into(),
                ));
            }
        }
    }
    Ok(document.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"# The demo crate.
[package]
name = "acme-demo"   # the name
version = "0.1.0"    # bumped by the release workflow
edition = "2021"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
"#;

    #[test]
    fn replaces_only_the_version() {
        let edited = set_package_version(MANIFEST, "1.2.3").unwrap();
        assert_eq!(
            edited,
            MANIFEST.replace(
                r#"version = "0.1.0"    # bumped"#,
                r#"version = "1.2.3"    # bumped"#
            )
        );
    }

    #[test]
    fn adds_a_missing_version() {
        let edited =
            set_package_version("[package]\nname = \"x\"\nedition = \"2021\"\n", "0.2.0").unwrap();
        assert_eq!(
            edited,
            "[package]\nname = \"x\"\nedition = \"2021\"\nversion = \"0.2.0\"\n"
        );
    }

    #[test]
    fn refuses_a_version_inherited_from_the_workspace() {
        for manifest in [
            "[package]\nname = \"x\"\nversion.workspace = true\n",
            "[package]\nname = \"x\"\nversion = { workspace = true }\n",
        ] {
            let err = set_package_version(manifest, "1.0.0").unwrap_err();
            let message = err.to_string();
            assert!(message.contains("version.workspace = true"), "{message}");
            assert!(message.contains("[workspace.package]"), "{message}");
        }
    }

    #[test]
    fn rejects_manifests_without_a_package() {
        assert!(set_package_version("[workspace]\nmembers = []\n", "1.0.0").is_err());
        assert!(set_package_version("[package\n", "1.0.0").is_err());
        assert!(set_package_version("[package]\nversion = 1\n", "1.0.0").is_err());
    }
}
