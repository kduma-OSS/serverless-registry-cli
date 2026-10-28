//! Shared HTTP client, API models and packaging primitives for `sreg`.
//!
//! Every ecosystem crate implements the [`Packager`] trait defined here, and the
//! command-line tool talks to the registry exclusively through this crate.

#![deny(unsafe_code)]

pub mod client;
pub mod digest;
pub mod download;
pub mod error;
pub mod models;
pub mod oidc;
pub mod packager;
pub mod publish;
pub mod retry;
pub mod token;
pub mod upload;

#[cfg(test)]
mod test_env;

pub use client::Client;
pub use error::{Error, Result};
pub use packager::{Ecosystem, Package, PackageFile, PackageOptions, Packager, Workdir};
