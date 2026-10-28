//! Validation of Composer package names and versions.

use sreg_core::Error;

/// Checks a Composer package name against Composer's own rule:
/// `^[a-z0-9]([_.-]?[a-z0-9]+)*/[a-z0-9](([_.]|-{1,2})?[a-z0-9]+)*$`.
pub fn validate_name(name: &str) -> Result<(), Error> {
    let valid = match name.split_once('/') {
        Some((vendor, package)) => segment_valid(vendor, false) && segment_valid(package, true),
        None => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "invalid package name `{name}` in composer.json; expected <vendor>/<package> in \
             lower case, with words separated by a single `.`, `_` or `-` (or `--` in the \
             package part)"
        )))
    }
}

/// Lower-case letters and digits, separated by single `_`, `.` or `-` (and `--` when
/// `double_dash` is allowed); it starts and ends with a letter or digit.
fn segment_valid(segment: &str, double_dash: bool) -> bool {
    let is_word = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let bytes_ok = segment
        .chars()
        .all(|c| is_word(c) || matches!(c, '_' | '.' | '-'));
    if !bytes_ok || !segment.starts_with(is_word) || !segment.ends_with(is_word) {
        return false;
    }
    segment
        .split(is_word)
        .filter(|separator| !separator.is_empty())
        .all(|separator| match separator {
            "_" | "." | "-" => true,
            "--" => double_dash,
            _ => false,
        })
}

/// Checks a version: semver `MAJOR.MINOR.PATCH` with an optional prerelease of `alpha`,
/// `beta` or `rc`, optionally followed by a number with or without a dot
/// (`1.0.0-rc1`, `1.0.0-beta.2`); no leading `v`, no build metadata, no `dev-*`.
pub fn validate_version(version: &str) -> Result<(), Error> {
    if version.starts_with("dev-") || version.ends_with("-dev") {
        return Err(Error::InvalidInput(format!(
            "`{version}` is a development version; the registry only accepts releases \
             (MAJOR.MINOR.PATCH, optionally -alpha, -beta or -rc with a number)"
        )));
    }
    let valid = match semver::Version::parse(version) {
        Ok(parsed) => parsed.build.is_empty() && prerelease_valid(parsed.pre.as_str()),
        Err(_) => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "invalid version `{version}`; expected MAJOR.MINOR.PATCH, optionally followed by \
             -alpha, -beta or -rc and a number (1.0.0-rc1, 1.0.0-beta.2), without a leading \
             `v` or build metadata"
        )))
    }
}

/// Empty, or `alpha`, `beta` or `rc` optionally followed by `[.]<digits>`.
fn prerelease_valid(pre: &str) -> bool {
    if pre.is_empty() {
        return true;
    }
    ["alpha", "beta", "rc"].iter().any(|label| {
        pre.strip_prefix(label).is_some_and(|rest| {
            let number = rest.strip_prefix('.').unwrap_or(rest);
            rest.is_empty() || (!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
        })
    })
}

/// The version named by a git tag: `v1.2.3` → `1.2.3`, `1.2.3` → `1.2.3`.
pub fn version_from_tag(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_composer_names() {
        for name in [
            "acme/sdk",
            "acme/http-client",
            "acme-corp/sdk",
            "a1/b2",
            "acme.io/my_lib",
            "acme/foo--bar",
            "0/0",
        ] {
            assert!(validate_name(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn rejects_invalid_composer_names() {
        for name in [
            "",
            "sdk",
            "Acme/sdk",
            "acme/SDK",
            "acme/sdk/extra",
            "/sdk",
            "acme/",
            "acme--corp/sdk",
            "acme/foo---bar",
            "acme/foo__bar",
            "acme/-sdk",
            "acme/sdk-",
            "acme/foo.-bar",
            "acme/s dk",
            "@acme/sdk",
        ] {
            assert!(validate_name(name).is_err(), "{name}");
        }
    }

    #[test]
    fn accepts_releases_and_composer_prereleases() {
        for version in [
            "1.2.3",
            "0.0.1",
            "10.20.30",
            "1.0.0-alpha",
            "1.0.0-alpha1",
            "1.0.0-alpha.1",
            "1.0.0-beta.2",
            "1.0.0-beta12",
            "1.0.0-rc",
            "1.0.0-rc1",
            "1.0.0-rc.3",
        ] {
            assert!(validate_version(version).is_ok(), "{version}");
        }
    }

    #[test]
    fn rejects_other_versions() {
        for version in [
            "",
            "1.2",
            "1",
            "v1.2.3",
            "1.2.3.4",
            "1.2.3+build.5",
            "1.0.0-preview",
            "1.0.0-RC1",
            "1.0.0-rc.1.2",
            "1.0.0-rc.x",
            "1.0.0-beta-2",
            "1.0.0-alpha.beta",
            "dev-main",
            "dev-feature/x",
            "1.0.x-dev",
            "1.2.3-dev",
            "latest",
        ] {
            assert!(validate_version(version).is_err(), "{version}");
        }
    }

    #[test]
    fn dev_versions_get_their_own_message() {
        let err = validate_version("dev-main").unwrap_err();
        assert!(err.to_string().contains("development version"), "{err}");
    }

    #[test]
    fn tags_lose_their_v_prefix() {
        assert_eq!(version_from_tag("v1.2.3"), "1.2.3");
        assert_eq!(version_from_tag("1.2.3"), "1.2.3");
        assert_eq!(version_from_tag("v1.0.0-rc1"), "1.0.0-rc1");
    }
}
