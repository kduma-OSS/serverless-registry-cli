//! Swift packaging for `sreg`.
//!
//! Will archive a Swift package (`Package.swift`) for the registry; not implemented yet.

#![deny(unsafe_code)]

use sreg_core::{Ecosystem, Error, Package, PackageOptions, Packager};

/// Packages Swift packages for publication.
#[derive(Debug, Clone, Copy, Default)]
pub struct SwiftPackager;

impl Packager for SwiftPackager {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Swift
    }

    fn package(&self, _opts: &PackageOptions) -> Result<Package, Error> {
        Err(Error::InvalidInput("not implemented yet".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_swift_ecosystem() {
        assert_eq!(SwiftPackager.ecosystem(), Ecosystem::Swift);
    }

    #[test]
    fn packaging_is_not_implemented_yet() {
        let err = SwiftPackager
            .package(&PackageOptions::default())
            .unwrap_err();
        assert!(err.to_string().contains("not implemented yet"), "{err}");
    }
}
