//! Construction of the registry client from global options.

use anyhow::Context;
use sreg_core::token::{resolve_token, TokenFlags};
use sreg_core::Client;
use url::Url;

use crate::cli::GlobalArgs;

/// Version of this binary, sent as `X-Client-Version`.
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The registry URL from `--registry` or `REGISTRY_URL`.
pub fn registry(global: &GlobalArgs) -> anyhow::Result<Url> {
    global
        .registry
        .clone()
        .context("no registry: pass --registry <URL> or set REGISTRY_URL")
}

/// The token from `--token`/`REGISTRY_TOKEN` or, with `oidc`, from GitHub Actions.
pub async fn token(
    global: &GlobalArgs,
    registry: &Url,
    oidc: bool,
    project: Option<&str>,
) -> anyhow::Result<String> {
    Ok(resolve_token(TokenFlags {
        token: global.token.as_deref(),
        oidc,
        project,
        registry,
    })
    .await?)
}

/// An authenticated client using `--token`/`REGISTRY_TOKEN`.
pub async fn client(global: &GlobalArgs) -> anyhow::Result<Client> {
    client_with(global, false, None).await
}

/// An authenticated client, with the token built from GitHub Actions OIDC when `oidc`.
pub async fn client_with(
    global: &GlobalArgs,
    oidc: bool,
    project: Option<&str>,
) -> anyhow::Result<Client> {
    let registry = registry(global)?;
    let token = token(global, &registry, oidc, project).await?;
    Ok(Client::new(registry, Some(token), CLIENT_VERSION)?)
}

/// A client without credentials, for `GET /api/v1/version`.
pub fn anonymous_client(registry: Url) -> anyhow::Result<Client> {
    Ok(Client::new(registry, None, CLIENT_VERSION)?)
}
