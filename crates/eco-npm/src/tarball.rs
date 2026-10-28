//! Reading what `npm pack` produced: its `--json` report and the tarball.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use serde_json::Value;
use sreg_core::Error;

/// Path of the manifest inside an npm tarball.
const PACKAGE_JSON: &str = "package/package.json";

/// The entry of `npm pack --json` describing the one packed tarball.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packed {
    /// File name of the tarball in the pack destination.
    pub filename: String,
    /// Package name as reported by npm.
    pub name: Option<String>,
    /// Package version as reported by npm.
    pub version: Option<String>,
}

/// Parses the standard output of `npm pack --json`: an array with one object per
/// packed tarball.
///
/// Lifecycle scripts (`prepack`, `prepare`) may print to standard output before the
/// report, so when the whole output is not JSON, the report is taken from the first
/// line starting with `[` from which the rest of the output parses.
pub fn parse_pack_output(stdout: &[u8]) -> Result<Packed, Error> {
    let text = String::from_utf8_lossy(stdout);
    let report: Value = match serde_json::from_str(&text) {
        Ok(report) => report,
        Err(err) => text
            .match_indices('[')
            .map(|(index, _)| index)
            .filter(|&index| index == 0 || text[..index].ends_with('\n'))
            .find_map(|start| serde_json::from_str(&text[start..]).ok())
            .ok_or_else(|| {
                Error::InvalidInput(format!(
                    "could not read the JSON report of `npm pack --json`: {err}"
                ))
            })?,
    };
    let entries = report.as_array().ok_or_else(|| {
        Error::InvalidInput("the JSON report of `npm pack` is not an array".into())
    })?;
    let entry = match entries.as_slice() {
        [entry] => entry,
        [] => return Err(Error::InvalidInput("`npm pack` packed nothing".into())),
        many => {
            return Err(Error::InvalidInput(format!(
                "`npm pack` packed {} packages; one call publishes one package, so select \
                 it with --workspace",
                many.len()
            )))
        }
    };
    let text_field = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
    let filename = text_field("filename").ok_or_else(|| {
        Error::InvalidInput("the JSON report of `npm pack` has no `filename`".into())
    })?;
    if filename.is_empty()
        || filename.contains(['/', '\\'])
        || filename.starts_with('.')
        || !filename.ends_with(".tgz")
    {
        return Err(Error::InvalidInput(format!(
            "`npm pack` reported an unexpected file name `{filename}`; expected <name>.tgz"
        )));
    }
    Ok(Packed {
        filename,
        name: text_field("name"),
        version: text_field("version"),
    })
}

/// Reads `package/package.json` from a gzipped npm tarball as a JSON object.
///
/// The whole tarball is read, so that a truncated or corrupt one is an error here and
/// not at install time.
pub fn read_package_json(path: &Path) -> Result<Value, Error> {
    let mut archive = tar::Archive::new(GzDecoder::new(File::open(path)?));
    let mut manifest = None;
    for entry in archive.entries().map_err(|err| not_a_tarball(path, err))? {
        let mut entry = entry.map_err(|err| not_a_tarball(path, err))?;
        let is_manifest = entry.path()?.to_str() == Some(PACKAGE_JSON);
        if !is_manifest || manifest.is_some() {
            // Skipping to the next header decompresses the data, which checks the CRC.
            continue;
        }
        let mut text = String::new();
        entry
            .read_to_string(&mut text)
            .map_err(|err| not_a_tarball(path, err))?;
        let parsed: Value = serde_json::from_str(&text).map_err(|err| {
            Error::InvalidInput(format!(
                "{PACKAGE_JSON} in the tarball is not valid JSON: {err}"
            ))
        })?;
        if !parsed.is_object() {
            return Err(Error::InvalidInput(format!(
                "{PACKAGE_JSON} in the tarball must contain a JSON object"
            )));
        }
        manifest = Some(parsed);
    }
    // The tar reader stops at its end marker; reading the gzip stream to its end checks
    // the trailer, so that a truncated file is rejected.
    std::io::copy(&mut archive.into_inner(), &mut std::io::sink())
        .map_err(|err| not_a_tarball(path, err))?;
    manifest.ok_or_else(|| {
        Error::InvalidInput(format!(
            "the tarball `{}` has no {PACKAGE_JSON}",
            path.display()
        ))
    })
}

fn not_a_tarball(path: &Path, err: std::io::Error) -> Error {
    Error::InvalidInput(format!(
        "`{}` is not a valid npm tarball (.tgz): {err}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &str = r#"[
  {
    "id": "@acme/demo@1.2.3",
    "name": "@acme/demo",
    "version": "1.2.3",
    "size": 242,
    "shasum": "fd75467840a475b5d3632a9b050d3b2d7859d428",
    "integrity": "sha512-AS2J",
    "filename": "acme-demo-1.2.3.tgz",
    "files": [{ "path": "index.js", "size": 20, "mode": 420 }],
    "entryCount": 1,
    "bundled": []
  }
]
"#;

    #[test]
    fn parses_the_pack_report() {
        let packed = parse_pack_output(REPORT.as_bytes()).unwrap();
        assert_eq!(packed.filename, "acme-demo-1.2.3.tgz");
        assert_eq!(packed.name.as_deref(), Some("@acme/demo"));
        assert_eq!(packed.version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn skips_script_output_before_the_report() {
        let stdout = format!("> @acme/demo@1.2.3 prepack\n> echo [build]\n[build]\n{REPORT}");
        let packed = parse_pack_output(stdout.as_bytes()).unwrap();
        assert_eq!(packed.filename, "acme-demo-1.2.3.tgz");
    }

    #[test]
    fn rejects_unexpected_reports() {
        for stdout in [
            "",
            "not json",
            "{}",
            "[]",
            r#"[{"filename":"a.tgz"},{"filename":"b.tgz"}]"#,
            r#"[{"name":"@a/b"}]"#,
            r#"[{"filename":"../a.tgz"}]"#,
            r#"[{"filename":"a.zip"}]"#,
        ] {
            assert!(parse_pack_output(stdout.as_bytes()).is_err(), "{stdout}");
        }
        let err = parse_pack_output(br#"[{"filename":"a.tgz"},{"filename":"b.tgz"}]"#).unwrap_err();
        assert!(err.to_string().contains("--workspace"), "{err}");
    }
}
