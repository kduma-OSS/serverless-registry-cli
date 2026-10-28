//! Reading the `.nuspec` of a `.nupkg` into the version's manifest.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use serde_json::{json, Map, Value};
use sreg_core::Error;

/// The metadata of a `.nuspec`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Nuspec {
    /// The complete XML, as found in the package (without a byte order mark).
    pub xml: String,
    /// Package id.
    pub id: String,
    /// Version as written in the `.nuspec`.
    pub version: String,
    /// `<description>`.
    pub description: Option<String>,
    /// `<authors>`.
    pub authors: Option<String>,
    /// `<projectUrl>`.
    pub project_url: Option<String>,
    /// `<license type="expression">`.
    pub license_expression: Option<String>,
    /// `<tags>`, split on whitespace.
    pub tags: Option<Vec<String>>,
    /// `<dependencies>`: one entry per `<group>`, plus one without a target framework
    /// for `<dependency>` elements outside any group.
    pub dependency_groups: Vec<DependencyGroup>,
}

/// One `<group>` of `<dependencies>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyGroup {
    /// The `targetFramework` attribute; `None` for a group without one.
    pub target_framework: Option<String>,
    /// The dependencies of the group.
    pub dependencies: Vec<Dependency>,
}

/// One `<dependency id="…" version="…" />`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    /// Package id of the dependency.
    pub id: String,
    /// Version range as written; `(, )` (any version) when the attribute is missing.
    pub range: String,
}

/// Range of a dependency without a `version` attribute: any version.
pub const ANY_RANGE: &str = "(, )";

impl Nuspec {
    /// The manifest sent to the registry (`docs/api.md`, 6.4, nuget). Optional fields
    /// that the `.nuspec` does not have are left out.
    pub fn manifest(&self) -> Value {
        let mut manifest = Map::new();
        manifest.insert("id".into(), self.id.clone().into());
        manifest.insert("version".into(), self.version.clone().into());
        manifest.insert("nuspec".into(), self.xml.clone().into());
        let optional = [
            ("description", &self.description),
            ("authors", &self.authors),
            ("projectUrl", &self.project_url),
            ("licenseExpression", &self.license_expression),
        ];
        for (key, value) in optional {
            if let Some(value) = value {
                manifest.insert(key.into(), value.clone().into());
            }
        }
        if let Some(tags) = &self.tags {
            manifest.insert("tags".into(), tags.clone().into());
        }
        let groups: Vec<Value> = self
            .dependency_groups
            .iter()
            .map(|group| {
                let dependencies: Vec<Value> = group
                    .dependencies
                    .iter()
                    .map(|dependency| json!({ "id": dependency.id, "range": dependency.range }))
                    .collect();
                json!({
                    "targetFramework": group.target_framework,
                    "dependencies": dependencies,
                })
            })
            .collect();
        manifest.insert("dependencyGroups".into(), groups.into());
        Value::Object(manifest)
    }
}

/// Reads the single `*.nuspec` at the root of the `.nupkg` at `path`.
pub fn read_from_nupkg(path: &Path) -> Result<Nuspec, Error> {
    let invalid = |message: String| Error::InvalidInput(message);
    let mut archive = zip::ZipArchive::new(File::open(path)?).map_err(|err| {
        invalid(format!(
            "`{}` is not a valid .nupkg (zip): {err}",
            path.display()
        ))
    })?;
    let names: Vec<String> = archive
        .file_names()
        .filter(|name| !name.contains('/') && name.to_ascii_lowercase().ends_with(".nuspec"))
        .map(str::to_owned)
        .collect();
    let name = match names.as_slice() {
        [name] => name,
        [] => return Err(invalid("the .nupkg has no .nuspec at its root".into())),
        _ => {
            return Err(invalid(format!(
                "the .nupkg has several .nuspec files at its root: {}",
                names.join(", ")
            )))
        }
    };
    let mut bytes = Vec::new();
    archive
        .by_name(name)
        .map_err(|err| invalid(format!("cannot read {name} from the .nupkg: {err}")))?
        .read_to_end(&mut bytes)?;
    let xml = String::from_utf8(bytes)
        .map_err(|_| invalid(format!("{name} in the .nupkg is not UTF-8")))?;
    parse(&xml)
}

/// Parses the text of a `.nuspec`.
pub fn parse(xml: &str) -> Result<Nuspec, Error> {
    let xml = xml.strip_prefix('\u{feff}').unwrap_or(xml);
    let invalid = |detail: String| Error::InvalidInput(format!("invalid .nuspec: {detail}"));
    let mut nuspec = Nuspec {
        xml: xml.to_owned(),
        ..Nuspec::default()
    };
    let mut reader = Reader::from_str(xml);
    let mut path: Vec<String> = Vec::new();
    let mut text = String::new();
    let mut license_is_expression = false;
    let mut ungrouped: Option<usize> = None;

    loop {
        let event = reader
            .read_event()
            .map_err(|err| invalid(format!("not valid XML: {err}")))?;
        match event {
            Event::Start(start) | Event::Empty(start)
                if path.is_empty() && local_name(&start)? != "package" =>
            {
                return Err(invalid(format!(
                    "the root element is `{}`, expected `package`",
                    local_name(&start)?
                )));
            }
            Event::Start(start) => {
                let name = local_name(&start)?;
                element(&mut nuspec, &path, &name, &start, &mut ungrouped)?;
                if is_metadata_field(&path, &name, "license") {
                    license_is_expression =
                        attribute(&start, "type")?.as_deref() == Some("expression");
                }
                path.push(name);
                text.clear();
            }
            Event::Empty(start) => {
                let name = local_name(&start)?;
                element(&mut nuspec, &path, &name, &start, &mut ungrouped)?;
            }
            Event::Text(content) => text.push_str(
                &content
                    .xml10_content()
                    .map_err(|err| invalid(err.to_string()))?,
            ),
            Event::CData(content) => text.push_str(
                &content
                    .xml10_content()
                    .map_err(|err| invalid(err.to_string()))?,
            ),
            Event::GeneralRef(reference) => {
                let character = reference
                    .resolve_char_ref()
                    .map_err(|err| invalid(err.to_string()))?;
                match character {
                    Some(character) => text.push(character),
                    None => {
                        let entity = reference.decode().map_err(|err| invalid(err.to_string()))?;
                        let resolved = quick_xml::escape::resolve_predefined_entity(&entity)
                            .ok_or_else(|| invalid(format!("unknown entity `&{entity};`")))?;
                        text.push_str(resolved);
                    }
                }
            }
            Event::End(_) => {
                let Some(name) = path.pop() else {
                    return Err(invalid("unbalanced closing tag".into()));
                };
                let value = text.trim().to_owned();
                text.clear();
                if path.len() != 2 || path[0] != "package" || path[1] != "metadata" {
                    continue;
                }
                match name.as_str() {
                    "id" => nuspec.id = value,
                    "version" => nuspec.version = value,
                    "description" => nuspec.description = Some(value),
                    "authors" => nuspec.authors = Some(value),
                    "projectUrl" => nuspec.project_url = Some(value),
                    "license" if license_is_expression => {
                        nuspec.license_expression = Some(value);
                    }
                    "tags" => {
                        nuspec.tags = Some(value.split_whitespace().map(str::to_owned).collect());
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if nuspec.id.is_empty() {
        return Err(invalid("<id> is missing or empty".into()));
    }
    if nuspec.version.is_empty() {
        return Err(invalid("<version> is missing or empty".into()));
    }
    Ok(nuspec)
}

/// Whether `<name>` opened under `path` is the metadata field `field`.
fn is_metadata_field(path: &[String], name: &str, field: &str) -> bool {
    name == field && path.len() == 2 && path[0] == "package" && path[1] == "metadata"
}

/// Handles the elements that carry data in attributes: dependency groups and
/// dependencies.
fn element(
    nuspec: &mut Nuspec,
    path: &[String],
    name: &str,
    start: &BytesStart<'_>,
    ungrouped: &mut Option<usize>,
) -> Result<(), Error> {
    let in_dependencies = |depth: usize| {
        path.len() == depth
            && path[0] == "package"
            && path[1] == "metadata"
            && path[2] == "dependencies"
    };
    match name {
        "group" if in_dependencies(3) => {
            nuspec.dependency_groups.push(DependencyGroup {
                target_framework: attribute(start, "targetFramework")?
                    .filter(|framework| !framework.trim().is_empty()),
                dependencies: Vec::new(),
            });
        }
        "dependency" if in_dependencies(4) && path[3] == "group" => {
            let dependency = dependency(start)?;
            if let Some(group) = nuspec.dependency_groups.last_mut() {
                group.dependencies.push(dependency);
            }
        }
        "dependency" if in_dependencies(3) => {
            let dependency = dependency(start)?;
            let index = *ungrouped.get_or_insert_with(|| {
                nuspec.dependency_groups.push(DependencyGroup::default());
                nuspec.dependency_groups.len() - 1
            });
            nuspec.dependency_groups[index]
                .dependencies
                .push(dependency);
        }
        _ => {}
    }
    Ok(())
}

fn dependency(start: &BytesStart<'_>) -> Result<Dependency, Error> {
    let id = attribute(start, "id")?
        .ok_or_else(|| Error::InvalidInput("invalid .nuspec: a <dependency> has no `id`".into()))?;
    let range = attribute(start, "version")?
        .filter(|range| !range.trim().is_empty())
        .unwrap_or_else(|| ANY_RANGE.to_owned());
    Ok(Dependency { id, range })
}

fn local_name(start: &BytesStart<'_>) -> Result<String, Error> {
    std::str::from_utf8(start.local_name().as_ref())
        .map(str::to_owned)
        .map_err(|_| Error::InvalidInput("invalid .nuspec: an element name is not UTF-8".into()))
}

/// The value of the attribute `name` (without a namespace prefix), unescaped.
fn attribute(start: &BytesStart<'_>, name: &str) -> Result<Option<String>, Error> {
    for attribute in start.attributes() {
        let attribute = attribute
            .map_err(|err| Error::InvalidInput(format!("invalid .nuspec attribute: {err}")))?;
        if attribute.key.local_name().as_ref() == name.as_bytes() {
            let value = attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|err| Error::InvalidInput(format!("invalid .nuspec attribute: {err}")))?;
            return Ok(Some(value.trim().to_owned()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NUSPEC: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://schemas.microsoft.com/packaging/2013/05/nuspec.xsd">
  <metadata>
    <id>Acme.Sdk</id>
    <version>1.2.0</version>
    <authors>Acme &amp; Co</authors>
    <license type="file">LICENSE.txt</license>
    <description><![CDATA[The <Acme> SDK]]> &#x2014; for .NET</description>
    <tags>  acme   sdk
      http </tags>
    <dependencies>
      <dependency id="Newtonsoft.Json" version="[13.0.1, )" />
      <dependency id="Acme.Core" />
    </dependencies>
  </metadata>
  <files><file src="x" target="lib" /></files>
</package>"#;

    #[test]
    fn parses_text_entities_and_ungrouped_dependencies() {
        let nuspec = parse(NUSPEC).unwrap();
        assert_eq!(nuspec.id, "Acme.Sdk");
        assert_eq!(nuspec.version, "1.2.0");
        assert_eq!(nuspec.authors.as_deref(), Some("Acme & Co"));
        assert_eq!(
            nuspec.description.as_deref(),
            Some("The <Acme> SDK \u{2014} for .NET")
        );
        assert_eq!(nuspec.license_expression, None);
        assert_eq!(nuspec.project_url, None);
        assert_eq!(
            nuspec.tags,
            Some(vec!["acme".to_owned(), "sdk".to_owned(), "http".to_owned()])
        );
        assert_eq!(
            nuspec.dependency_groups,
            [DependencyGroup {
                target_framework: None,
                dependencies: vec![
                    Dependency {
                        id: "Newtonsoft.Json".into(),
                        range: "[13.0.1, )".into()
                    },
                    Dependency {
                        id: "Acme.Core".into(),
                        range: ANY_RANGE.into()
                    },
                ],
            }]
        );
        assert_eq!(nuspec.xml, NUSPEC);
    }

    #[test]
    fn strips_a_byte_order_mark() {
        let nuspec = parse(&format!("\u{feff}{NUSPEC}")).unwrap();
        assert_eq!(nuspec.xml, NUSPEC);
    }

    #[test]
    fn rejects_documents_that_are_not_nuspecs() {
        for xml in [
            "",
            "<package><metadata><id>A</id></metadata></package>",
            "<package><metadata><version>1.0.0</version></metadata></package>",
            "<nuspec><metadata><id>A</id><version>1</version></metadata></nuspec>",
            "<package><metadata><id>A</id><version>1</version></package>",
            "<package><metadata><id>A&bogus;</id><version>1</version></metadata></package>",
            "<package><metadata><id>A</id><version>1</version><dependencies>\
             <dependency version=\"1\"/></dependencies></metadata></package>",
        ] {
            assert!(parse(xml).is_err(), "{xml}");
        }
    }
}
