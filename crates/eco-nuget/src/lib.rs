//! NuGet packaging for `sreg`.
//!
//! Delegates packaging to `dotnet pack` and reads the `.nuspec` from the resulting
//! `.nupkg`.

#![deny(unsafe_code)]

use sreg_core::{Ecosystem, Error, Package, PackageOptions, Packager};

/// Packages NuGet projects for publication.
#[derive(Debug, Clone, Copy, Default)]
pub struct NugetPackager;

impl Packager for NugetPackager {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Nuget
    }

    fn package(&self, _opts: &PackageOptions) -> Result<Package, Error> {
        Err(Error::InvalidInput("not implemented yet".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_nuget_ecosystem() {
        assert_eq!(NugetPackager.ecosystem(), Ecosystem::Nuget);
    }
}
