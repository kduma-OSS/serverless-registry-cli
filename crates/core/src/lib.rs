//! Shared HTTP client, API models and packaging primitives for `sreg`.
//!
//! Every ecosystem crate implements the [`Packager`] trait defined here, and the
//! command-line tool talks to the registry exclusively through this crate.

#![deny(unsafe_code)]

pub mod client;
pub mod digest;
pub mod download;
pub mod error;
pub mod git;
pub mod models;
pub mod oidc;
pub mod packager;
pub mod process;
pub mod publish;
pub mod registry_url;
pub mod retry;
pub mod token;
pub mod upload;

#[cfg(test)]
mod test_env;

pub use client::{Client, GenericQuery};
pub use error::{Error, Result};
pub use packager::{Ecosystem, Package, PackageFile, PackageOptions, Packager, Workdir};
pub use process::{CommandOutput, CommandRunner, SystemRunner};
pub use registry_url::RegistryUrl;
