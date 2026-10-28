//! npm packaging for `sreg`.
//!
//! Delegates packaging to `npm pack` and reads `package.json` from the resulting tarball.

#![deny(unsafe_code)]

use sreg_core::{Ecosystem, Error, Package, PackageOptions, Packager};

/// Packages npm projects for publication.
#[derive(Debug, Clone, Copy, Default)]
pub struct NpmPackager;

impl Packager for NpmPackager {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Npm
    }

    fn package(&self, _opts: &PackageOptions) -> Result<Package, Error> {
        Err(Error::InvalidInput("not implemented yet".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_npm_ecosystem() {
        assert_eq!(NpmPackager.ecosystem(), Ecosystem::Npm);
    }
}
