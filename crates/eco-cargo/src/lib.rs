//! cargo packaging for `sreg`.
//!
//! Delegates packaging to `cargo package` and reads the crate metadata.

#![deny(unsafe_code)]

use sreg_core::{Ecosystem, Error, Package, PackageOptions, Packager};

/// Packages cargo projects for publication.
#[derive(Debug, Clone, Copy, Default)]
pub struct CargoPackager;

impl Packager for CargoPackager {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Cargo
    }

    fn package(&self, _opts: &PackageOptions) -> Result<Package, Error> {
        Err(Error::InvalidInput("not implemented yet".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_cargo_ecosystem() {
        assert_eq!(CargoPackager.ecosystem(), Ecosystem::Cargo);
    }
}
