//! Reading a `.pom` into the version's manifest.
//!
//! Only effective POMs are accepted: the coordinates, the packaging and the
//! dependencies must be written out in the POM itself, without a `<parent>` to inherit
//! them from and without `${…}` properties to resolve.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use serde_json::{json, Value};
use sreg_core::Error;

/// Packaging of a POM without `<packaging>`.
pub const DEFAULT_PACKAGING: &str = "jar";

/// Scope of a dependency without `<scope>`.
pub const DEFAULT_SCOPE: &str = "compile";

/// What to do about a POM that is not an effective POM.
const EFFECTIVE_POM_HINT: &str = "sreg needs an effective POM with every value written out: \
     with Maven, deploy a flattened POM (flatten-maven-plugin) or check the result with \
     `mvn help:effective-pom`; Gradle's maven-publish already writes resolved POMs";

/// The metadata of a `.pom`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pom {
    /// The complete XML, as found in the file (without a byte order mark).
    pub xml: String,
    /// `<groupId>`.
    pub group_id: String,
    /// `<artifactId>`.
    pub artifact_id: String,
    /// `<version>`.
    pub version: String,
    /// `<packaging>`, [`DEFAULT_PACKAGING`] when missing.
    pub packaging: String,
    /// `<dependencies>` of the project (not those of `<dependencyManagement>`, plugins
    /// or profiles).
    pub dependencies: Vec<Dependency>,
}

/// One `<dependency>` of the project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dependency {
    /// `<groupId>`.
    pub group_id: String,
    /// `<artifactId>`.
    pub artifact_id: String,
    /// `<version>`; `None` when the POM leaves it to an imported BOM.
    pub version: Option<String>,
    /// `<scope>`, [`DEFAULT_SCOPE`] when missing.
    pub scope: String,
    /// `<optional>true</optional>`.
    pub optional: bool,
}

impl Pom {
    /// The manifest sent to the registry.
    pub fn manifest(&self) -> Value {
        let dependencies: Vec<Value> = self
            .dependencies
            .iter()
            .map(|dependency| {
                json!({
                    "groupId": dependency.group_id,
                    "artifactId": dependency.artifact_id,
                    "version": dependency.version,
                    "scope": dependency.scope,
                    "optional": dependency.optional,
                })
            })
            .collect();
        json!({
            "groupId": self.group_id,
            "artifactId": self.artifact_id,
            "version": self.version,
            "packaging": self.packaging,
            "pom": self.xml,
            "dependencies": dependencies,
        })
    }
}

/// A dependency while its elements are being read.
#[derive(Default)]
struct PartialDependency {
    group_id: Option<String>,
    artifact_id: Option<String>,
    version: Option<String>,
    scope: Option<String>,
    optional: Option<String>,
}

/// Parses the text of a `.pom`.
pub fn parse(xml: &str) -> Result<Pom, Error> {
    let xml = xml.strip_prefix('\u{feff}').unwrap_or(xml);
    let invalid = |detail: String| Error::InvalidInput(format!("invalid POM: {detail}"));
    let mut reader = Reader::from_str(xml);
    let mut path: Vec<String> = Vec::new();
    let mut text = String::new();
    let mut group_id = None;
    let mut artifact_id = None;
    let mut version = None;
    let mut packaging = None;
    let mut dependencies: Vec<PartialDependency> = Vec::new();

    loop {
        let event = reader
            .read_event()
            .map_err(|err| invalid(format!("not valid XML: {err}")))?;
        match event {
            Event::Start(start) | Event::Empty(start)
                if path.is_empty() && local_name(&start)? != "project" =>
            {
                return Err(invalid(format!(
                    "the root element is `{}`, expected `project`",
                    local_name(&start)?
                )));
            }
            Event::Start(start) => {
                let name = local_name(&start)?;
                check_element(&path, &name)?;
                if is_dependency(&path, &name) {
                    dependencies.push(PartialDependency::default());
                }
                path.push(name);
                text.clear();
            }
            Event::Empty(start) => {
                let name = local_name(&start)?;
                check_element(&path, &name)?;
                if is_dependency(&path, &name) {
                    dependencies.push(PartialDependency::default());
                }
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
                let parents: Vec<&str> = path.iter().map(String::as_str).collect();
                match parents.as_slice() {
                    ["project"] => match name.as_str() {
                        "groupId" => group_id = Some(value),
                        "artifactId" => artifact_id = Some(value),
                        "version" => version = Some(value),
                        "packaging" => packaging = Some(value),
                        _ => {}
                    },
                    ["project", "dependencies", "dependency"] => {
                        if let Some(dependency) = dependencies.last_mut() {
                            match name.as_str() {
                                "groupId" => dependency.group_id = Some(value),
                                "artifactId" => dependency.artifact_id = Some(value),
                                "version" => dependency.version = Some(value),
                                "scope" => dependency.scope = Some(value),
                                "optional" => dependency.optional = Some(value),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() {
        return Err(invalid(format!("<{}> is not closed", path.join("/"))));
    }

    let required = |value: Option<String>, element: &str| {
        let value = value.filter(|value| !value.is_empty()).ok_or_else(|| {
            Error::InvalidInput(format!("the POM has no <{element}>; {EFFECTIVE_POM_HINT}"))
        })?;
        check_resolved(&value, element)?;
        Ok::<_, Error>(value)
    };
    let group_id = required(group_id, "groupId")?;
    let artifact_id = required(artifact_id, "artifactId")?;
    let version = required(version, "version")?;
    let packaging = match packaging.filter(|packaging| !packaging.is_empty()) {
        Some(packaging) => {
            check_resolved(&packaging, "packaging")?;
            packaging
        }
        None => DEFAULT_PACKAGING.to_owned(),
    };
    let dependencies = dependencies
        .into_iter()
        .map(|dependency| {
            let field = |value: Option<String>, element: &str| -> Result<Option<String>, Error> {
                let value = value.filter(|value| !value.is_empty());
                if let Some(value) = &value {
                    check_resolved(value, &format!("dependency/{element}"))?;
                }
                Ok(value)
            };
            let group_id = field(dependency.group_id, "groupId")?;
            let artifact_id = field(dependency.artifact_id, "artifactId")?;
            let (Some(group_id), Some(artifact_id)) = (group_id, artifact_id) else {
                return Err(invalid(
                    "a <dependency> has no <groupId> or <artifactId>".into(),
                ));
            };
            let version = field(dependency.version, "version")?;
            let scope = field(dependency.scope, "scope")?.unwrap_or_else(|| DEFAULT_SCOPE.into());
            let optional = match field(dependency.optional, "optional")?.as_deref() {
                None | Some("false") => false,
                Some("true") => true,
                Some(other) => {
                    return Err(invalid(format!(
                        "<optional> of {group_id}:{artifact_id} is `{other}`, expected true or \
                         false"
                    )))
                }
            };
            Ok(Dependency {
                group_id,
                artifact_id,
                version,
                scope,
                optional,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;

    Ok(Pom {
        xml: xml.to_owned(),
        group_id,
        artifact_id,
        version,
        packaging,
        dependencies,
    })
}

/// Rejects a `<parent>` of the project.
fn check_element(path: &[String], name: &str) -> Result<(), Error> {
    if name == "parent" && path.len() == 1 && path[0] == "project" {
        return Err(Error::InvalidInput(format!(
            "the POM inherits from a <parent>, so its coordinates and dependencies are not \
             complete; {EFFECTIVE_POM_HINT}"
        )));
    }
    Ok(())
}

/// Whether `<name>` opened under `path` is a dependency of the project.
fn is_dependency(path: &[String], name: &str) -> bool {
    name == "dependency" && path.len() == 2 && path[0] == "project" && path[1] == "dependencies"
}

/// Rejects a value with a `${…}` property.
fn check_resolved(value: &str, element: &str) -> Result<(), Error> {
    if value.contains("${") {
        return Err(Error::InvalidInput(format!(
            "<{element}> of the POM is `{value}`, which needs a property to be resolved; \
             {EFFECTIVE_POM_HINT}"
        )));
    }
    Ok(())
}

fn local_name(start: &BytesStart<'_>) -> Result<String, Error> {
    std::str::from_utf8(start.local_name().as_ref())
        .map(str::to_owned)
        .map_err(|_| Error::InvalidInput("invalid POM: an element name is not UTF-8".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const POM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>com.acme</groupId>
  <artifactId>sdk</artifactId>
  <version>1.4.0</version>
  <name>Acme &amp; Co SDK</name>
  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>com.fasterxml.jackson</groupId>
        <artifactId>jackson-bom</artifactId>
        <version>2.17.0</version>
        <type>pom</type>
        <scope>import</scope>
      </dependency>
    </dependencies>
  </dependencyManagement>
  <dependencies>
    <dependency>
      <groupId>com.squareup.okhttp3</groupId>
      <artifactId>okhttp</artifactId>
      <version>4.12.0</version>
    </dependency>
    <dependency>
      <groupId>com.fasterxml.jackson.core</groupId>
      <artifactId>jackson-databind</artifactId>
      <scope>runtime</scope>
    </dependency>
    <dependency>
      <groupId>org.slf4j</groupId>
      <artifactId>slf4j-api</artifactId>
      <version><![CDATA[2.0.13]]></version>
      <optional>true</optional>
    </dependency>
  </dependencies>
  <build>
    <plugins>
      <plugin>
        <artifactId>maven-shade-plugin</artifactId>
        <dependencies>
          <dependency>
            <groupId>org.ow2.asm</groupId>
            <artifactId>asm</artifactId>
            <version>${asm.version}</version>
          </dependency>
        </dependencies>
      </plugin>
    </plugins>
  </build>
  <profiles>
    <profile>
      <dependencies>
        <dependency><groupId>x</groupId><artifactId>y</artifactId></dependency>
      </dependencies>
    </profile>
  </profiles>
</project>
"#;

    fn dependency(
        group_id: &str,
        artifact_id: &str,
        version: Option<&str>,
        scope: &str,
        optional: bool,
    ) -> Dependency {
        Dependency {
            group_id: group_id.into(),
            artifact_id: artifact_id.into(),
            version: version.map(str::to_owned),
            scope: scope.into(),
            optional,
        }
    }

    #[test]
    fn parses_coordinates_defaults_and_project_dependencies_only() {
        let pom = parse(POM).unwrap();
        assert_eq!(pom.group_id, "com.acme");
        assert_eq!(pom.artifact_id, "sdk");
        assert_eq!(pom.version, "1.4.0");
        assert_eq!(pom.packaging, "jar");
        assert_eq!(
            pom.dependencies,
            [
                dependency(
                    "com.squareup.okhttp3",
                    "okhttp",
                    Some("4.12.0"),
                    "compile",
                    false
                ),
                dependency(
                    "com.fasterxml.jackson.core",
                    "jackson-databind",
                    None,
                    "runtime",
                    false
                ),
                dependency("org.slf4j", "slf4j-api", Some("2.0.13"), "compile", true),
            ]
        );
        assert_eq!(pom.xml, POM);
        assert_eq!(parse(&format!("\u{feff}{POM}")).unwrap().xml, POM);
    }

    #[test]
    fn manifest_has_the_contract_shape() {
        let pom = parse(
            "<project><groupId>g</groupId><artifactId>a</artifactId><version>1</version>\
             <packaging>aar</packaging><dependencies><dependency><groupId>d</groupId>\
             <artifactId>e</artifactId><version>2</version><scope>test</scope></dependency>\
             </dependencies></project>",
        )
        .unwrap();
        assert_eq!(
            pom.manifest(),
            json!({
                "groupId": "g",
                "artifactId": "a",
                "version": "1",
                "packaging": "aar",
                "pom": pom.xml,
                "dependencies": [{
                    "groupId": "d", "artifactId": "e", "version": "2",
                    "scope": "test", "optional": false
                }]
            })
        );
    }

    #[test]
    fn rejects_poms_that_are_not_effective() {
        let parent = "<project><parent><groupId>g</groupId><artifactId>p</artifactId>\
                      <version>1</version></parent><groupId>g</groupId><artifactId>a</artifactId>\
                      <version>1</version></project>";
        let err = parse(parent).unwrap_err();
        assert!(err.to_string().contains("<parent>"), "{err}");
        assert!(err.to_string().contains("mvn help:effective-pom"), "{err}");

        for xml in [
            "<project><groupId>g</groupId><artifactId>a</artifactId>\
             <version>${revision}</version></project>",
            "<project><groupId>${g}</groupId><artifactId>a</artifactId>\
             <version>1</version></project>",
            "<project><groupId>g</groupId><artifactId>a</artifactId><version>1</version>\
             <dependencies><dependency><groupId>d</groupId><artifactId>e</artifactId>\
             <version>${project.version}</version></dependency></dependencies></project>",
            "<project><groupId>g</groupId><artifactId>a</artifactId><version>1</version>\
             <dependencies><dependency><groupId>d</groupId><artifactId>e</artifactId>\
             <scope>${scope}</scope></dependency></dependencies></project>",
        ] {
            let err = parse(xml).unwrap_err();
            assert!(err.to_string().contains("property"), "{err}");
        }
        let err =
            parse("<project><artifactId>a</artifactId><version>1</version></project>").unwrap_err();
        assert!(err.to_string().contains("no <groupId>"), "{err}");
    }

    #[test]
    fn rejects_documents_that_are_not_poms() {
        for xml in [
            "",
            "<settings><groupId>g</groupId></settings>",
            "<project><groupId>g</groupId><artifactId>a</artifactId><version>1</version>",
            "<project><groupId>g&bogus;</groupId><artifactId>a</artifactId><version>1</version></project>",
            "<project><groupId>g</groupId><artifactId>a</artifactId><version>1</version>\
             <dependencies><dependency><version>1</version></dependency></dependencies></project>",
            "<project><groupId>g</groupId><artifactId>a</artifactId><version>1</version>\
             <dependencies><dependency><groupId>d</groupId><artifactId>e</artifactId>\
             <optional>yes</optional></dependency></dependencies></project>",
        ] {
            assert!(parse(xml).is_err(), "{xml}");
        }
    }
}
