//! Text and JSON output.
//!
//! With `--json` the JSON document is the only thing written to stdout; human-readable
//! text is suppressed. Errors, prompts and logs always go to stderr.

use std::io::Write;

use serde::Serialize;

/// Where and how command results are printed.
#[derive(Debug, Clone, Copy)]
pub struct Output {
    json: bool,
}

impl Output {
    pub fn new(json: bool) -> Self {
        Output { json }
    }

    /// Prints `value` as pretty JSON on stdout.
    pub fn json<T: Serialize + ?Sized>(&self, value: &T) -> anyhow::Result<()> {
        let mut stdout = std::io::stdout().lock();
        serde_json::to_writer_pretty(&mut stdout, value)?;
        writeln!(stdout)?;
        Ok(())
    }

    /// Prints `value` as JSON with `--json`, otherwise calls `text`.
    pub fn emit<T: Serialize + ?Sized>(
        &self,
        value: &T,
        text: impl FnOnce(),
    ) -> anyhow::Result<()> {
        if self.json {
            self.json(value)
        } else {
            text();
            Ok(())
        }
    }
}

/// Formats rows as a left-aligned table with a header row.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|header| header.chars().count())
        .collect();
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }

    let format_row = |cells: Vec<&str>| {
        let line: Vec<String> = cells
            .iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        line.join("  ").trim_end().to_owned()
    };

    let mut lines = vec![format_row(headers.to_vec())];
    lines.extend(
        rows.iter()
            .map(|row| format_row(row.iter().map(String::as_str).collect())),
    );
    lines.join("\n")
}

/// Indents every line of `text` by `spaces`.
pub fn indent(text: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    text.lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_pads_columns() {
        let rendered = table(
            &["ECO", "PACKAGE"],
            &[
                vec!["npm".into(), "@acme/sdk".into()],
                vec!["composer".into(), "acme/*".into()],
            ],
        );
        assert_eq!(
            rendered,
            "ECO       PACKAGE\nnpm       @acme/sdk\ncomposer  acme/*"
        );
    }

    #[test]
    fn indent_prefixes_each_line() {
        assert_eq!(indent("a\nb", 2), "  a\n  b");
    }
}
