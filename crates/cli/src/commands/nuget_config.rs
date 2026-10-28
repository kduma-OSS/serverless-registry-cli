//! Merging a package source and its credentials into a `nuget.config` file.
//!
//! The file is streamed event by event, so everything except the entries of our source
//! (comments, other sources, formatting) is written back unchanged.

use anyhow::{bail, Context};
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer, XmlVersion};

/// A package source with clear-text credentials.
pub struct NugetSource<'a> {
    /// Source key (`--registry-name`), also the element name under
    /// `packageSourceCredentials`.
    pub name: &'a str,
    /// Service index URL, `<registry>/nuget/index.json`.
    pub url: &'a str,
    /// Token login.
    pub username: &'a str,
    /// Token password.
    pub password: &'a str,
}

const SOURCES: &str = "packageSources";
const CREDENTIALS: &str = "packageSourceCredentials";
const ROOT: &str = "configuration";

/// Returns `existing` (or a new file) with `source` in `packageSources` and its
/// credentials in `packageSourceCredentials`, replacing previous entries of the same key.
pub fn merge(existing: Option<&str>, source: &NugetSource<'_>) -> anyhow::Result<String> {
    let existing = existing.filter(|content| !content.trim().is_empty());
    let Some(existing) = existing else {
        return fresh(source);
    };

    let mut reader = Reader::from_str(existing);
    let mut writer = Writer::new(Vec::new());
    let mut stack: Vec<String> = Vec::new();
    let mut skip_until: Option<usize> = None;
    let mut seen_root = false;
    let mut seen_sources = false;
    let mut seen_credentials = false;
    let mut indent = String::new();

    loop {
        let event = reader.read_event().context("the file is not valid XML")?;
        if let Some(depth) = skip_until {
            match &event {
                Event::Start(start) => stack.push(element_name(start)?),
                Event::End(_) => {
                    stack.pop();
                    if stack.len() == depth {
                        skip_until = None;
                    }
                }
                Event::Eof => bail!("the file is not valid XML"),
                _ => {}
            }
            continue;
        }

        match event {
            Event::Start(start) => {
                let name = element_name(&start)?;
                if is_ours(&stack, &start, &name, source)? {
                    skip_until = Some(stack.len());
                    stack.push(name);
                    continue;
                }
                note_section(
                    &stack,
                    &name,
                    &mut seen_root,
                    &mut seen_sources,
                    &mut seen_credentials,
                );
                stack.push(name);
                writer.write_event(Event::Start(start))?;
            }
            Event::Empty(start) => {
                let name = element_name(&start)?;
                if is_ours(&stack, &start, &name, source)? {
                    continue;
                }
                note_section(
                    &stack,
                    &name,
                    &mut seen_root,
                    &mut seen_sources,
                    &mut seen_credentials,
                );
                let path = path_of(&stack);
                if path.is_empty() && name == ROOT {
                    writer.write_event(Event::Start(BytesStart::new(ROOT)))?;
                    write_text(&mut writer, "\n")?;
                    write_sources(&mut writer, source, "  ")?;
                    write_credentials(&mut writer, source, "  ")?;
                    writer.write_event(Event::End(BytesEnd::new(ROOT)))?;
                } else if path == [ROOT] && name == SOURCES {
                    write_sources_section(&mut writer, source, &indent)?;
                } else if path == [ROOT] && name == CREDENTIALS {
                    write_credentials_section(&mut writer, source, &indent)?;
                } else {
                    writer.write_event(Event::Empty(start))?;
                }
            }
            Event::End(end) => {
                let path = path_of(&stack);
                if path == [ROOT, SOURCES] {
                    write_text(&mut writer, "  ")?;
                    write_add(&mut writer, source.name, source.url)?;
                    write_text(&mut writer, &format!("\n{indent}"))?;
                } else if path == [ROOT, CREDENTIALS] {
                    write_text(&mut writer, "  ")?;
                    write_credential_entry(&mut writer, source, &format!("{indent}  "))?;
                    write_text(&mut writer, &format!("\n{indent}"))?;
                } else if path == [ROOT] {
                    let child = format!("{indent}  ");
                    if !seen_sources {
                        write_text(&mut writer, "  ")?;
                        write_sources_section(&mut writer, source, &child)?;
                        write_text(&mut writer, &format!("\n{indent}"))?;
                    }
                    if !seen_credentials {
                        write_text(&mut writer, "  ")?;
                        write_credentials_section(&mut writer, source, &child)?;
                        write_text(&mut writer, &format!("\n{indent}"))?;
                    }
                }
                stack.pop();
                writer.write_event(Event::End(end))?;
            }
            Event::Text(text) => {
                let raw = String::from_utf8_lossy(text.as_ref()).into_owned();
                if raw.trim().is_empty() {
                    indent = raw.rsplit('\n').next().unwrap_or_default().to_owned();
                }
                writer.write_event(Event::Text(text))?;
            }
            Event::Eof => {
                if !stack.is_empty() {
                    bail!(
                        "the file is not valid XML: <{}> is not closed",
                        stack.join("/")
                    );
                }
                break;
            }
            other => writer.write_event(other)?,
        }
    }

    if !seen_root {
        bail!("the file has no <{ROOT}> root element");
    }
    Ok(String::from_utf8(writer.into_inner())?)
}

fn fresh(source: &NugetSource<'_>) -> anyhow::Result<String> {
    let mut writer = Writer::new(Vec::new());
    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)))?;
    write_text(&mut writer, "\n")?;
    writer.write_event(Event::Start(BytesStart::new(ROOT)))?;
    write_text(&mut writer, "\n  ")?;
    write_sources_section(&mut writer, source, "  ")?;
    write_text(&mut writer, "\n  ")?;
    write_credentials_section(&mut writer, source, "  ")?;
    write_text(&mut writer, "\n")?;
    writer.write_event(Event::End(BytesEnd::new(ROOT)))?;
    write_text(&mut writer, "\n")?;
    Ok(String::from_utf8(writer.into_inner())?)
}

/// Whether the element is a previous entry of our source that must be replaced.
fn is_ours(
    stack: &[String],
    start: &BytesStart<'_>,
    name: &str,
    source: &NugetSource<'_>,
) -> anyhow::Result<bool> {
    let path = path_of(stack);
    if path == [ROOT, SOURCES] && name == "add" {
        let key = start
            .try_get_attribute("key")
            .context("invalid attribute")?
            .map(|attribute| {
                attribute
                    .normalized_value(XmlVersion::Implicit1_0)
                    .map(|value| value.into_owned())
            })
            .transpose()
            .context("invalid attribute value")?;
        return Ok(key.as_deref() == Some(source.name));
    }
    Ok(path == [ROOT, CREDENTIALS] && name == source.name)
}

fn note_section(
    stack: &[String],
    name: &str,
    seen_root: &mut bool,
    seen_sources: &mut bool,
    seen_credentials: &mut bool,
) {
    let path = path_of(stack);
    if path.is_empty() && name == ROOT {
        *seen_root = true;
    } else if path == [ROOT] && name == SOURCES {
        *seen_sources = true;
    } else if path == [ROOT] && name == CREDENTIALS {
        *seen_credentials = true;
    }
}

fn path_of(stack: &[String]) -> Vec<&str> {
    stack.iter().map(String::as_str).collect()
}

fn element_name(start: &BytesStart<'_>) -> anyhow::Result<String> {
    Ok(std::str::from_utf8(start.name().as_ref())
        .context("invalid element name")?
        .to_owned())
}

type XmlWriter = Writer<Vec<u8>>;

fn write_text(writer: &mut XmlWriter, text: &str) -> anyhow::Result<()> {
    writer.write_event(Event::Text(BytesText::new(text)))?;
    Ok(())
}

fn write_add(writer: &mut XmlWriter, key: &str, value: &str) -> anyhow::Result<()> {
    let mut add = BytesStart::new("add");
    add.push_attribute(("key", key));
    add.push_attribute(("value", value));
    writer.write_event(Event::Empty(add))?;
    Ok(())
}

/// `<packageSources>` with our source; `indent` is the indentation of the section tag.
fn write_sources_section(
    writer: &mut XmlWriter,
    source: &NugetSource<'_>,
    indent: &str,
) -> anyhow::Result<()> {
    writer.write_event(Event::Start(BytesStart::new(SOURCES)))?;
    write_text(writer, &format!("\n{indent}  "))?;
    write_add(writer, source.name, source.url)?;
    write_text(writer, &format!("\n{indent}"))?;
    writer.write_event(Event::End(BytesEnd::new(SOURCES)))?;
    Ok(())
}

fn write_sources(
    writer: &mut XmlWriter,
    source: &NugetSource<'_>,
    indent: &str,
) -> anyhow::Result<()> {
    write_text(writer, indent)?;
    write_sources_section(writer, source, indent)?;
    write_text(writer, "\n")
}

fn write_credentials(
    writer: &mut XmlWriter,
    source: &NugetSource<'_>,
    indent: &str,
) -> anyhow::Result<()> {
    write_text(writer, indent)?;
    write_credentials_section(writer, source, indent)?;
    write_text(writer, "\n")
}

/// `<packageSourceCredentials>` with our entry; `indent` is that of the section tag.
fn write_credentials_section(
    writer: &mut XmlWriter,
    source: &NugetSource<'_>,
    indent: &str,
) -> anyhow::Result<()> {
    writer.write_event(Event::Start(BytesStart::new(CREDENTIALS)))?;
    write_text(writer, &format!("\n{indent}  "))?;
    write_credential_entry(writer, source, &format!("{indent}  "))?;
    write_text(writer, &format!("\n{indent}"))?;
    writer.write_event(Event::End(BytesEnd::new(CREDENTIALS)))?;
    Ok(())
}

/// `<name>` with `Username` and `ClearTextPassword`; `indent` is that of `<name>`.
fn write_credential_entry(
    writer: &mut XmlWriter,
    source: &NugetSource<'_>,
    indent: &str,
) -> anyhow::Result<()> {
    writer.write_event(Event::Start(BytesStart::new(source.name)))?;
    write_text(writer, &format!("\n{indent}  "))?;
    write_add(writer, "Username", source.username)?;
    write_text(writer, &format!("\n{indent}  "))?;
    write_add(writer, "ClearTextPassword", source.password)?;
    write_text(writer, &format!("\n{indent}"))?;
    writer.write_event(Event::End(BytesEnd::new(source.name)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: NugetSource<'static> = NugetSource {
        name: "sreg",
        url: "https://packages.example.com/nuget/index.json",
        username: "shop-frontend",
        password: "k7Qx&2mN9",
    };

    #[test]
    fn creates_a_new_file() {
        assert_eq!(
            merge(None, &SOURCE).unwrap(),
            r#"<?xml version="1.0" encoding="utf-8"?>
<configuration>
  <packageSources>
    <add key="sreg" value="https://packages.example.com/nuget/index.json"/>
  </packageSources>
  <packageSourceCredentials>
    <sreg>
      <add key="Username" value="shop-frontend"/>
      <add key="ClearTextPassword" value="k7Qx&amp;2mN9"/>
    </sreg>
  </packageSourceCredentials>
</configuration>
"#
        );
    }

    #[test]
    fn replaces_previous_entries_and_keeps_everything_else() {
        let existing = r#"<?xml version="1.0" encoding="utf-8"?>
<!-- team settings -->
<configuration>
  <packageSources>
    <clear />
    <add key="nuget.org" value="https://api.nuget.org/v3/index.json" />
    <add key="sreg" value="https://old.example.com/nuget/index.json" />
  </packageSources>
  <packageSourceCredentials>
    <other>
      <add key="Username" value="o" />
    </other>
    <sreg>
      <add key="Username" value="old" />
      <add key="ClearTextPassword" value="old" />
    </sreg>
  </packageSourceCredentials>
</configuration>
"#;

        let merged = merge(Some(existing), &SOURCE).unwrap();

        assert!(merged.contains("<!-- team settings -->"));
        assert!(merged.contains("<clear />"));
        assert!(merged
            .contains(r#"<add key="nuget.org" value="https://api.nuget.org/v3/index.json" />"#));
        assert!(merged.contains("<other>"));
        assert!(!merged.contains("old"));
        assert_eq!(merged.matches("<sreg>").count(), 1);
        assert_eq!(merged.matches(r#"key="sreg""#).count(), 1);
        assert!(merged.contains(
            r#"<add key="sreg" value="https://packages.example.com/nuget/index.json"/>"#
        ));
        assert!(merged.contains(r#"<add key="ClearTextPassword" value="k7Qx&amp;2mN9"/>"#));
        let mut reader = Reader::from_str(&merged);
        while !matches!(reader.read_event().unwrap(), Event::Eof) {}
    }

    #[test]
    fn adds_missing_sections() {
        let existing = "<configuration>\n  <config>\n    <add key=\"x\" value=\"y\" />\n  </config>\n</configuration>\n";

        let merged = merge(Some(existing), &SOURCE).unwrap();

        assert!(merged.contains("<config>"));
        assert!(merged.contains("\n  <packageSources>\n    <add key=\"sreg\""));
        assert!(merged
            .contains("\n  <packageSourceCredentials>\n    <sreg>\n      <add key=\"Username\""));
    }

    #[test]
    fn fills_empty_sections_and_roots() {
        let merged = merge(
            Some("<configuration>\n  <packageSources />\n  <packageSourceCredentials/>\n</configuration>"),
            &SOURCE,
        )
        .unwrap();
        assert_eq!(merged.matches("<packageSources>").count(), 1);
        assert_eq!(merged.matches("<sreg>").count(), 1);

        let merged = merge(Some("<configuration/>"), &SOURCE).unwrap();
        assert!(merged.contains("<packageSources>"));
        assert!(merged.contains("<sreg>"));
    }

    #[test]
    fn rejects_files_without_configuration() {
        assert!(merge(Some("<settings />"), &SOURCE).is_err());
        assert!(merge(Some("<configuration>"), &SOURCE).is_err());
    }
}
