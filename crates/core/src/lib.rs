//! Shared HTTP client, API models and packaging primitives for `sreg`.
//!
//! Every ecosystem crate implements the [`Packager`] trait defined here, and the
//! command-line tool talks to the registry exclusively through this crate.

#![deny(unsafe_code)]

pub mod client;
pub mod digest;
pub mod error;
pub mod models;
pub mod packager;
pub mod retry;

pub use client::Client;
pub use error::{Error, Result};
pub use packager::{Ecosystem, Package, PackageOptions, Packager};
