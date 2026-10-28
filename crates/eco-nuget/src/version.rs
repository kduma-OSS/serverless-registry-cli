//! NuGet versions: `MAJOR.MINOR[.PATCH[.REVISION]][-PRERELEASE][+METADATA]`.

use sreg_core::Error;

/// The parts of a NuGet version.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Parsed<'a> {
    numbers: Vec<u64>,
    prerelease: Option<&'a str>,
}

fn parse(version: &str) -> Option<Parsed<'_>> {
    let (rest, metadata) = match version.split_once('+') {
        Some((rest, metadata)) => (rest, Some(metadata)),
        None => (version, None),
    };
    let (numbers, prerelease) = match rest.split_once('-') {
        Some((numbers, prerelease)) => (numbers, Some(prerelease)),
        None => (rest, None),
    };
    let labels_ok = |labels: &str| {
        labels.split('.').all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
    };
    if !prerelease.is_none_or(labels_ok) || !metadata.is_none_or(labels_ok) {
        return None;
    }
    let numbers: Vec<u64> = numbers
        .split('.')
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                None
            } else {
                part.parse().ok()
            }
        })
        .collect::<Option<_>>()?;
    if numbers.is_empty() || numbers.len() > 4 {
        return None;
    }
    Some(Parsed {
        numbers,
        prerelease,
    })
}

/// Checks a NuGet version: one to four numbers separated by dots, optionally followed
/// by `-<prerelease>` and `+<metadata>` made of dot-separated alphanumeric labels.
pub fn validate_version(version: &str) -> Result<(), Error> {
    parse(version).map(|_| ()).ok_or_else(|| {
        Error::InvalidInput(format!(
            "invalid NuGet version `{version}`; expected MAJOR.MINOR.PATCH with an optional \
             fourth number, -prerelease and +metadata, e.g. 1.2.0 or 2.0.0-beta.1"
        ))
    })
}

/// The normalised form NuGet compares versions by: three numbers (four when the
/// fourth is not zero), without leading zeros and build metadata, the prerelease in
/// lower case. `None` for an invalid version.
pub fn normalize_version(version: &str) -> Option<String> {
    let parsed = parse(version)?;
    let mut numbers = parsed.numbers;
    numbers.resize(numbers.len().max(3), 0);
    if numbers.len() == 4 && numbers[3] == 0 {
        numbers.truncate(3);
    }
    let mut normalized = numbers
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(".");
    if let Some(prerelease) = parsed.prerelease {
        normalized.push('-');
        normalized.push_str(&prerelease.to_ascii_lowercase());
    }
    Some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_nuget_versions() {
        for version in [
            "1",
            "1.2",
            "1.2.0",
            "1.2.3.4",
            "2.0.0-beta.1",
            "2.0.0-Beta-2",
            "1.0.0+git.abc",
        ] {
            assert!(validate_version(version).is_ok(), "{version}");
        }
    }

    #[test]
    fn rejects_other_versions() {
        for version in [
            "",
            "v1.2.0",
            "1.2.3.4.5",
            "1..2",
            "1.2.0-",
            "1.2.0-a..b",
            "1.2.0;Foo=bar",
            "$(Version)",
            "1.2.0-beta_1",
        ] {
            assert!(validate_version(version).is_err(), "{version}");
        }
    }

    #[test]
    fn normalizes_like_nuget() {
        assert_eq!(normalize_version("1.2").as_deref(), Some("1.2.0"));
        assert_eq!(normalize_version("01.002.3.0").as_deref(), Some("1.2.3"));
        assert_eq!(normalize_version("1.2.3.4").as_deref(), Some("1.2.3.4"));
        assert_eq!(
            normalize_version("2.0.0-Beta.1+abc").as_deref(),
            Some("2.0.0-beta.1")
        );
        assert_eq!(normalize_version("x"), None);
    }
}
