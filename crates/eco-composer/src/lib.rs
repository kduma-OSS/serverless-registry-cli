//! Composer packaging for `sreg`.
//!
//! Archives the project with `git archive HEAD` (falling back to a zip of the directory) and reads `composer.json`.

#![deny(unsafe_code)]

use sreg_core::{Ecosystem, Error, Package, PackageOptions, Packager};

/// Packages Composer projects for publication.
#[derive(Debug, Clone, Copy, Default)]
pub struct ComposerPackager;

impl Packager for ComposerPackager {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Composer
    }

    fn package(&self, _opts: &PackageOptions) -> Result<Package, Error> {
        Err(Error::InvalidInput("not implemented yet".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_composer_ecosystem() {
        assert_eq!(ComposerPackager.ecosystem(), Ecosystem::Composer);
    }
}
