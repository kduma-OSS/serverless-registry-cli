//! Merging a `<server>` with credentials into a Maven `settings.xml`, and the snippets
//! `sreg auth maven` prints.
//!
//! The file is streamed event by event; a `<server>` is buffered until its `<id>` is
//! known, so everything except the previous entries of our id (comments, other servers,
//! formatting) is written back unchanged.

use anyhow::{bail, Context};
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};

/// A `<server>` entry of `settings.xml`.
pub struct MavenServer<'a> {
    /// `<id>`, which the repository declarations of the build refer to.
    pub id: &'a str,
    /// Token login.
    pub username: &'a str,
    /// Token password.
    pub password: &'a str,
}

const ROOT: &str = "settings";
const SERVERS: &str = "servers";
const SERVER: &str = "server";
const NAMESPACE: &str = "http://maven.apache.org/SETTINGS/1.0.0";

type XmlWriter = Writer<Vec<u8>>;

/// A `<server>` being read, kept back until its `<id>` is known.
struct Pending {
    /// The whitespace before `<server>`, dropped together with a duplicate.
    leading: Option<Event<'static>>,
    events: Vec<Event<'static>>,
    /// Depth of the stack when `<server>` was opened.
    depth: usize,
    /// Indentation of the `<server>` tag.
    indent: String,
    id: String,
}

/// Returns `existing` (or a new file) with `server` in `<servers>`: the first `<server>`
/// with the same `<id>` is replaced in place, further ones are removed, and without one
/// the entry is added at the end of `<servers>` (created when missing).
pub fn merge(existing: Option<&str>, server: &MavenServer<'_>) -> anyhow::Result<String> {
    let existing = existing.filter(|content| !content.trim().is_empty());
    let Some(existing) = existing else {
        return fresh(server);
    };

    let mut reader = Reader::from_str(existing);
    let mut writer = Writer::new(Vec::new());
    let mut stack: Vec<String> = Vec::new();
    let mut pending: Option<Pending> = None;
    let mut written = false;
    let mut seen_root = false;
    let mut seen_servers = false;
    let mut indent = String::new();
    // Whitespace inside `<servers>`, held back until it is known whether the element
    // after it is kept.
    let mut held: Option<Event<'static>> = None;

    loop {
        let event = reader.read_event().context("the file is not valid XML")?;
        if let Some(server_events) = pending.as_mut() {
            match &event {
                Event::Start(start) => stack.push(local_name(start)?),
                Event::End(_) => {
                    stack.pop();
                }
                Event::Text(text) if is_id(&stack, server_events.depth) => {
                    let text = text.xml10_content().context("invalid text")?;
                    server_events.id.push_str(&text);
                }
                Event::CData(text) if is_id(&stack, server_events.depth) => {
                    server_events
                        .id
                        .push_str(&String::from_utf8_lossy(text.as_ref()));
                }
                Event::Eof => bail!("the file is not valid XML: a <server> is not closed"),
                _ => {}
            }
            server_events.events.push(event.into_owned());
            if stack.len() == server_events.depth {
                let done = pending.take();
                if let Some(done) = done {
                    let ours = done.id.trim() == server.id;
                    if !ours || !written {
                        if let Some(leading) = done.leading {
                            writer.write_event(leading)?;
                        }
                    }
                    if !ours {
                        for event in done.events {
                            writer.write_event(event)?;
                        }
                    } else if !written {
                        write_server(&mut writer, server, &done.indent)?;
                        written = true;
                    }
                }
            }
            continue;
        }

        let opens_server = match &event {
            Event::Start(start) => {
                path_of(&stack) == [ROOT, SERVERS] && local_name(start)? == SERVER
            }
            _ => false,
        };
        if !opens_server {
            if let Some(text) = held.take() {
                writer.write_event(text)?;
            }
        }

        match event {
            Event::Start(start) => {
                let name = local_name(&start)?;
                let path = path_of(&stack);
                if path.is_empty() && name == ROOT {
                    seen_root = true;
                } else if path == [ROOT] && name == SERVERS {
                    seen_servers = true;
                } else if path == [ROOT, SERVERS] && name == SERVER {
                    pending = Some(Pending {
                        leading: held.take(),
                        events: vec![Event::Start(start.into_owned())],
                        depth: stack.len(),
                        indent: indent.clone(),
                        id: String::new(),
                    });
                    stack.push(name);
                    continue;
                }
                stack.push(name);
                writer.write_event(Event::Start(start))?;
            }
            Event::Empty(start) => {
                let name = local_name(&start)?;
                let path = path_of(&stack);
                if path.is_empty() && name == ROOT {
                    seen_root = true;
                    let end = BytesEnd::new(String::from_utf8(start.name().as_ref().to_vec())?);
                    writer.write_event(Event::Start(start))?;
                    write_text(&mut writer, "\n  ")?;
                    write_servers(&mut writer, server, "  ")?;
                    write_text(&mut writer, "\n")?;
                    writer.write_event(Event::End(end))?;
                    written = true;
                } else if path == [ROOT] && name == SERVERS {
                    seen_servers = true;
                    write_servers(&mut writer, server, &indent)?;
                    written = true;
                } else {
                    writer.write_event(Event::Empty(start))?;
                }
            }
            Event::End(end) => {
                let path = path_of(&stack);
                if path == [ROOT, SERVERS] && !written {
                    write_text(&mut writer, "  ")?;
                    write_server(&mut writer, server, &format!("{indent}  "))?;
                    write_text(&mut writer, &format!("\n{indent}"))?;
                    written = true;
                } else if path == [ROOT] && !seen_servers {
                    write_text(&mut writer, "  ")?;
                    write_servers(&mut writer, server, &format!("{indent}  "))?;
                    write_text(&mut writer, &format!("\n{indent}"))?;
                    written = true;
                }
                stack.pop();
                writer.write_event(Event::End(end))?;
            }
            Event::Text(text) => {
                let raw = String::from_utf8_lossy(text.as_ref()).into_owned();
                if raw.trim().is_empty() {
                    indent = raw.rsplit('\n').next().unwrap_or_default().to_owned();
                    if path_of(&stack) == [ROOT, SERVERS] {
                        held = Some(Event::Text(text.into_owned()));
                        continue;
                    }
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

/// The `<server>` element alone, as printed by `sreg auth maven`.
pub fn server_snippet(server: &MavenServer<'_>) -> anyhow::Result<String> {
    let mut writer = Writer::new(Vec::new());
    write_server(&mut writer, server, "")?;
    Ok(String::from_utf8(writer.into_inner())?)
}

/// A Gradle `credentials { … }` block (valid in the Kotlin and the Groovy DSL).
pub fn gradle_snippet(server: &MavenServer<'_>) -> String {
    format!(
        "credentials {{\n    username = \"{}\"\n    password = \"{}\"\n}}",
        gradle_string(server.username),
        gradle_string(server.password)
    )
}

/// Escapes a value for a double-quoted Kotlin or Groovy string.
fn gradle_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
}

fn fresh(server: &MavenServer<'_>) -> anyhow::Result<String> {
    let mut writer = Writer::new(Vec::new());
    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
    write_text(&mut writer, "\n")?;
    let mut root = BytesStart::new(ROOT);
    root.push_attribute(("xmlns", NAMESPACE));
    writer.write_event(Event::Start(root))?;
    write_text(&mut writer, "\n  ")?;
    write_servers(&mut writer, server, "  ")?;
    write_text(&mut writer, "\n")?;
    writer.write_event(Event::End(BytesEnd::new(ROOT)))?;
    write_text(&mut writer, "\n")?;
    Ok(String::from_utf8(writer.into_inner())?)
}

/// Whether the innermost open element is the `<id>` of the pending server.
fn is_id(stack: &[String], depth: usize) -> bool {
    stack.len() == depth + 2 && stack.last().is_some_and(|name| name == "id")
}

fn path_of(stack: &[String]) -> Vec<&str> {
    stack.iter().map(String::as_str).collect()
}

fn local_name(start: &BytesStart<'_>) -> anyhow::Result<String> {
    Ok(std::str::from_utf8(start.local_name().as_ref())
        .context("invalid element name")?
        .to_owned())
}

fn write_text(writer: &mut XmlWriter, text: &str) -> anyhow::Result<()> {
    writer.write_event(Event::Text(BytesText::new(text)))?;
    Ok(())
}

fn write_element(writer: &mut XmlWriter, name: &str, value: &str) -> anyhow::Result<()> {
    writer.write_event(Event::Start(BytesStart::new(name)))?;
    write_text(writer, value)?;
    writer.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}

/// `<servers>` with our server; `indent` is that of the `<servers>` tag.
fn write_servers(
    writer: &mut XmlWriter,
    server: &MavenServer<'_>,
    indent: &str,
) -> anyhow::Result<()> {
    writer.write_event(Event::Start(BytesStart::new(SERVERS)))?;
    write_text(writer, &format!("\n{indent}  "))?;
    write_server(writer, server, &format!("{indent}  "))?;
    write_text(writer, &format!("\n{indent}"))?;
    writer.write_event(Event::End(BytesEnd::new(SERVERS)))?;
    Ok(())
}

/// `<server>` with `id`, `username` and `password`; `indent` is that of `<server>`.
fn write_server(
    writer: &mut XmlWriter,
    server: &MavenServer<'_>,
    indent: &str,
) -> anyhow::Result<()> {
    writer.write_event(Event::Start(BytesStart::new(SERVER)))?;
    for (name, value) in [
        ("id", server.id),
        ("username", server.username),
        ("password", server.password),
    ] {
        write_text(writer, &format!("\n{indent}  "))?;
        write_element(writer, name, value)?;
    }
    write_text(writer, &format!("\n{indent}"))?;
    writer.write_event(Event::End(BytesEnd::new(SERVER)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER_ENTRY: MavenServer<'static> = MavenServer {
        id: "sreg",
        username: "shop-frontend",
        password: "k7Qx&2mN9",
    };

    #[test]
    fn creates_a_new_file() {
        assert_eq!(
            merge(None, &SERVER_ENTRY).unwrap(),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<settings xmlns="http://maven.apache.org/SETTINGS/1.0.0">
  <servers>
    <server>
      <id>sreg</id>
      <username>shop-frontend</username>
      <password>k7Qx&amp;2mN9</password>
    </server>
  </servers>
</settings>
"#
        );
    }

    #[test]
    fn replaces_the_server_in_place_and_keeps_everything_else() {
        let existing = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- team settings -->
<settings xmlns="http://maven.apache.org/SETTINGS/1.0.0">
  <servers>
    <server>
      <id>github</id>
      <username>octocat</username>
      <password>ghp_x</password>
    </server>
    <server>
      <!-- the registry -->
      <id> sreg </id>
      <username>old</username>
      <password>old</password>
    </server>
    <server>
      <id>nexus</id>
    </server>
    <server><id>sreg</id><password>older</password></server>
  </servers>
  <mirrors/>
</settings>
"#;

        let merged = merge(Some(existing), &SERVER_ENTRY).unwrap();

        assert_eq!(
            merged,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- team settings -->
<settings xmlns="http://maven.apache.org/SETTINGS/1.0.0">
  <servers>
    <server>
      <id>github</id>
      <username>octocat</username>
      <password>ghp_x</password>
    </server>
    <server>
      <id>sreg</id>
      <username>shop-frontend</username>
      <password>k7Qx&amp;2mN9</password>
    </server>
    <server>
      <id>nexus</id>
    </server>
  </servers>
  <mirrors/>
</settings>
"#
        );
    }

    #[test]
    fn appends_to_servers_or_adds_the_section() {
        let existing = "<settings>\n  <servers>\n    <server>\n      <id>github</id>\n    </server>\n  </servers>\n</settings>\n";
        let merged = merge(Some(existing), &SERVER_ENTRY).unwrap();
        assert!(merged
            .contains("      <id>github</id>\n    </server>\n    <server>\n      <id>sreg</id>\n"));
        assert!(merged.ends_with("    </server>\n  </servers>\n</settings>\n"));

        let existing = "<settings>\n  <localRepository>/m2</localRepository>\n</settings>\n";
        let merged = merge(Some(existing), &SERVER_ENTRY).unwrap();
        assert_eq!(
            merged,
            "<settings>\n  <localRepository>/m2</localRepository>\n  <servers>\n    <server>\n      \
             <id>sreg</id>\n      <username>shop-frontend</username>\n      \
             <password>k7Qx&amp;2mN9</password>\n    </server>\n  </servers>\n</settings>\n"
        );

        for existing in ["<settings/>", "<settings>\n  <servers/>\n</settings>"] {
            let merged = merge(Some(existing), &SERVER_ENTRY).unwrap();
            assert_eq!(merged.matches("<servers>").count(), 1, "{merged}");
            assert_eq!(merged.matches("<id>sreg</id>").count(), 1, "{merged}");
            let mut reader = Reader::from_str(&merged);
            while !matches!(reader.read_event().unwrap(), Event::Eof) {}
        }
    }

    #[test]
    fn rejects_files_that_are_not_settings() {
        assert!(merge(Some("<configuration/>"), &SERVER_ENTRY).is_err());
        assert!(merge(Some("<settings><servers>"), &SERVER_ENTRY).is_err());
        assert!(merge(Some("<settings><servers><server><id>x"), &SERVER_ENTRY).is_err());
    }

    #[test]
    fn snippets_escape_their_values() {
        assert_eq!(
            server_snippet(&SERVER_ENTRY).unwrap(),
            "<server>\n  <id>sreg</id>\n  <username>shop-frontend</username>\n  \
             <password>k7Qx&amp;2mN9</password>\n</server>"
        );
        let server = MavenServer {
            id: "sreg",
            username: "a\"b",
            password: "${x}\\",
        };
        assert_eq!(
            gradle_snippet(&server),
            "credentials {\n    username = \"a\\\"b\"\n    password = \"\\${x}\\\\\"\n}"
        );
    }
}
