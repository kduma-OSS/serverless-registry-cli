//! Validation of npm package names, versions and dist-tags.

use sreg_core::Error;

/// Longest package name npm accepts.
const MAX_NAME_LENGTH: usize = 214;

/// Checks a package name: the registry serves scoped packages only, `@scope/name`, in
/// lower case, with URL-safe characters (`a-z`, `0-9`, `-`, `.`, `_`, `~`), neither part
/// starting with `.` or `_`, at most 214 characters in total.
pub fn validate_name(name: &str) -> Result<(), Error> {
    let Some((scope, package)) = name.strip_prefix('@').and_then(|rest| rest.split_once('/'))
    else {
        return Err(Error::InvalidInput(format!(
            "`{name}` is not a scoped package name; the registry serves scoped packages only \
             (`@scope/name`), so rename the package in package.json"
        )));
    };
    let part_valid = |part: &str| {
        !part.is_empty()
            && !part.starts_with(['.', '_'])
            && part.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '_' | '~')
            })
    };
    if name.len() <= MAX_NAME_LENGTH && part_valid(scope) && part_valid(package) {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "invalid package name `{name}`; expected `@scope/name` in lower case with the \
             characters a-z, 0-9, `-`, `.`, `_` and `~`, neither part starting with `.` or \
             `_`, at most {MAX_NAME_LENGTH} characters"
        )))
    }
}

/// Checks a version: semver without a leading `v` (`1.2.3`, `2.0.0-rc.1`).
pub fn validate_version(version: &str) -> Result<(), Error> {
    semver::Version::parse(version).map(|_| ()).map_err(|err| {
        Error::InvalidInput(format!(
            "invalid version `{version}`: {err}; expected semver such as 1.2.3 or 2.0.0-rc.1, \
             without a leading `v`"
        ))
    })
}

/// Checks a dist-tag: `^[a-z][a-z0-9._-]{0,63}$` and not itself a version range (such
/// as `x` or `v1`), which npm would confuse with a version.
pub fn validate_tag(tag: &str) -> Result<(), Error> {
    let mut chars = tag.chars();
    let shape_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        && tag.len() <= 64;
    let unversioned = tag.strip_prefix('v').unwrap_or(tag);
    let looks_like_range =
        semver::VersionReq::parse(tag).is_ok() || semver::VersionReq::parse(unversioned).is_ok();
    if shape_ok && !looks_like_range {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "invalid dist-tag `{tag}`; expected a lower-case word such as `latest`, `next` or \
             `beta` (a-z, 0-9, `.`, `_`, `-`, at most 64 characters) that is not a version range"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_scoped_names() {
        for name in ["@acme/demo", "@acme-corp/sdk.js", "@a/b_c~d", "@0/1"] {
            assert!(validate_name(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn rejects_unscoped_and_invalid_names() {
        let err = validate_name("demo").unwrap_err();
        assert!(err.to_string().contains("scoped packages only"), "{err}");
        for name in [
            "",
            "@acme",
            "@acme/",
            "@/demo",
            "@Acme/demo",
            "@acme/Demo",
            "@acme/.demo",
            "@acme/_demo",
            "@acme/de mo",
            "@acme/a/b",
        ] {
            assert!(validate_name(name).is_err(), "{name}");
        }
        let long = format!("@acme/{}", "a".repeat(MAX_NAME_LENGTH));
        assert!(validate_name(&long).is_err());
    }

    #[test]
    fn versions_are_semver() {
        assert!(validate_version("1.2.3").is_ok());
        assert!(validate_version("2.0.0-rc.1+build.5").is_ok());
        for version in ["v1.2.3", "1.2", "", "latest"] {
            assert!(validate_version(version).is_err(), "{version}");
        }
    }

    #[test]
    fn tags_are_words_but_not_ranges() {
        for tag in ["latest", "next", "beta", "release-1.x", "lts_20"] {
            assert!(validate_tag(tag).is_ok(), "{tag}");
        }
        for tag in [
            "",
            "Latest",
            "1.0",
            "x",
            "v1",
            "v1.2",
            "-a",
            &"a".repeat(65),
        ] {
            assert!(validate_tag(tag).is_err(), "{tag}");
        }
    }
}
