//! Maven packaging for `sreg`.
//!
//! Will publish the artifacts of a Maven build (taken from a directory, under
//! `group:artifact` coordinates) to the registry; not implemented yet.

#![deny(unsafe_code)]

use sreg_core::{Ecosystem, Error, Package, PackageOptions, Packager};

/// Packages Maven artifacts for publication.
#[derive(Debug, Clone, Copy, Default)]
pub struct MavenPackager;

impl Packager for MavenPackager {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Maven
    }

    fn package(&self, _opts: &PackageOptions) -> Result<Package, Error> {
        Err(Error::InvalidInput("not implemented yet".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_maven_ecosystem() {
        assert_eq!(MavenPackager.ecosystem(), Ecosystem::Maven);
    }

    #[test]
    fn packaging_is_not_implemented_yet() {
        let err = MavenPackager
            .package(&PackageOptions::default())
            .unwrap_err();
        assert!(err.to_string().contains("not implemented yet"), "{err}");
    }
}
