//! Merging a `machine` entry into a `.netrc` file.
//!
//! The file is tokenized only to find the entries of the machine; everything else
//! (other machines, `default`, `macdef` macros, comments, formatting) is kept as is.

/// One token of the file, with its byte range.
struct Token {
    start: usize,
    end: usize,
    text: String,
}

/// Returns `existing` (or a new file) with the entry `machine <host> login <login>
/// password <password>`: the first entry of `host` is replaced, further ones are
/// removed, and without one the entry is added before `default` (which must come last)
/// or at the end.
pub fn merge(existing: Option<&str>, host: &str, login: &str, password: &str) -> String {
    let entry = format!(
        "machine {} login {} password {}",
        quote(host),
        quote(login),
        quote(password)
    );
    let content = existing.unwrap_or_default();
    let tokens = tokenize(content);

    // Byte ranges of the entries of `host`: from `machine` to the end of the entry's
    // last token.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut default_start = None;
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        match token.text.as_str() {
            "machine" => {
                let start = token.start;
                let name = tokens.get(index + 1).map(|token| token.text.as_str());
                let mut last = index + 1;
                while last + 1 < tokens.len() && !is_keyword(&tokens[last + 1].text) {
                    last += 1;
                }
                if name == Some(host) {
                    let end = tokens.get(last).map_or(content.len(), |token| token.end);
                    ranges.push((start, end));
                }
                index = last + 1;
            }
            "default" => {
                default_start.get_or_insert(token.start);
                index += 1;
            }
            _ => index += 1,
        }
    }

    let mut result = String::with_capacity(content.len() + entry.len() + 1);
    match ranges.split_first() {
        Some((&(start, end), duplicates)) => {
            result.push_str(&content[..start]);
            result.push_str(&entry);
            let mut cursor = end;
            for &(start, end) in duplicates {
                result.push_str(&content[cursor..start]);
                // Drop the duplicate with the whitespace that follows it.
                let rest = &content[end..];
                cursor = end + (rest.len() - rest.trim_start().len());
            }
            result.push_str(&content[cursor..]);
        }
        None => match default_start {
            Some(start) => {
                result.push_str(&content[..start]);
                result.push_str(&entry);
                result.push('\n');
                result.push_str(&content[start..]);
            }
            None => {
                result.push_str(content);
                if !result.is_empty() && !result.ends_with('\n') {
                    result.push('\n');
                }
                result.push_str(&entry);
                result.push('\n');
            }
        },
    }
    result
}

/// Words that start a new entry.
fn is_keyword(text: &str) -> bool {
    matches!(text, "machine" | "default" | "macdef")
}

/// Splits the file into tokens: whitespace-separated words or `"quoted strings"`
/// (with `\` escapes), skipping `#` comments and the bodies of `macdef` macros, which
/// run until the next blank line.
fn tokenize(content: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = content.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '#' {
            while chars.next_if(|&(_, c)| c != '\n').is_some() {}
            continue;
        }
        let mut text = String::new();
        let mut end = content.len();
        if c == '"' {
            chars.next();
            while let Some((index, c)) = chars.next() {
                match c {
                    '"' => {
                        end = index + 1;
                        break;
                    }
                    '\\' => {
                        if let Some((_, escaped)) = chars.next() {
                            text.push(escaped);
                        }
                    }
                    other => text.push(other),
                }
            }
        } else {
            while let Some(&(index, c)) = chars.peek() {
                if c.is_whitespace() {
                    end = index;
                    break;
                }
                text.push(c);
                chars.next();
            }
        }
        let macro_definition = c != '"' && text == "macdef";
        tokens.push(Token { start, end, text });
        if macro_definition {
            // The macro's name ends its line; the body runs until a blank line.
            let line_end = content[end..]
                .find('\n')
                .map_or(content.len(), |offset| end + offset + 1);
            let line = &content[end..line_end];
            if let Some(name) = line.split_whitespace().next() {
                let start = end + line.find(name).unwrap_or_default();
                tokens.push(Token {
                    start,
                    end: start + name.len(),
                    text: name.to_owned(),
                });
            }
            let body_end = blank_line_after(content, line_end);
            while chars.next_if(|&(index, _)| index < body_end).is_some() {}
        }
    }
    tokens
}

/// The position after the first blank line at or after `from` (the end of a macro
/// body), or the end of the content.
fn blank_line_after(content: &str, from: usize) -> usize {
    let mut position = from;
    for line in content[from.min(content.len())..].split_inclusive('\n') {
        position += line.len();
        if line.trim().is_empty() {
            return position;
        }
    }
    content.len()
}

/// Quotes a value that would otherwise be split or read as a comment.
fn quote(value: &str) -> String {
    let plain = !value.is_empty()
        && !value.starts_with('#')
        && !value.contains(|c: char| c.is_whitespace() || c == '"' || c == '\\');
    if plain {
        value.to_owned()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRY: &str = "machine packages.example.com login root password k7Qx";

    fn merged(existing: Option<&str>) -> String {
        merge(existing, "packages.example.com", "root", "k7Qx")
    }

    #[test]
    fn creates_or_appends_an_entry() {
        assert_eq!(merged(None), format!("{ENTRY}\n"));
        assert_eq!(merged(Some("")), format!("{ENTRY}\n"));
        assert_eq!(
            merged(Some("machine github.com login x password y")),
            format!("machine github.com login x password y\n{ENTRY}\n")
        );
    }

    #[test]
    fn replaces_the_entry_of_the_machine_wherever_it_spans() {
        let existing = "# work\nmachine github.com\n  login x\n  password y\n\
                        machine packages.example.com\n  login old\n  password old\n\
                        machine other.example.com login o password p\n";
        assert_eq!(
            merged(Some(existing)),
            format!(
                "# work\nmachine github.com\n  login x\n  password y\n{ENTRY}\n\
                 machine other.example.com login o password p\n"
            )
        );
    }

    #[test]
    fn removes_duplicates_and_keeps_default_last() {
        let existing = "machine packages.example.com login a password a\n\
                        machine packages.example.com login b password b\n\
                        default login anonymous password me@example.com\n";
        assert_eq!(
            merged(Some(existing)),
            format!("{ENTRY}\ndefault login anonymous password me@example.com\n")
        );

        let existing = "machine a login a password a\ndefault login anonymous password x\n";
        assert_eq!(
            merged(Some(existing)),
            format!("machine a login a password a\n{ENTRY}\ndefault login anonymous password x\n")
        );
    }

    #[test]
    fn macro_bodies_and_quoted_tokens_are_not_entries() {
        let existing = "machine ftp.example.com login u password \"p w\"\n\
                        macdef init\nmachine packages.example.com\ncd /pub\n\n\
                        machine x login y password z\n";
        assert_eq!(
            merged(Some(existing)),
            format!("{existing}{ENTRY}\n"),
            "the machine line inside the macro must not be replaced"
        );
    }

    #[test]
    fn quotes_values_that_need_it() {
        assert_eq!(quote("k7Qx"), "k7Qx");
        assert_eq!(quote("a b"), "\"a b\"");
        assert_eq!(quote("#x"), "\"#x\"");
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
        let content = merge(None, "h", "l", "p w");
        let tokens: Vec<String> = tokenize(&content).into_iter().map(|t| t.text).collect();
        assert_eq!(tokens, ["machine", "h", "login", "l", "password", "p w"]);
    }
}
