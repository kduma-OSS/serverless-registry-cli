//! Construction of the registry client from global options.

use anyhow::Context;
use sreg_core::token::{resolve_token, TokenFlags};
use sreg_core::{Client, RegistryUrl};
use url::Url;

use crate::cli::GlobalArgs;

/// Version of this binary, sent as `X-Client-Version`.
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The registry URL from `--registry` or `REGISTRY_URL`, split into the base URL (safe to
/// print) and the credentials of its userinfo, if any.
pub fn registry(global: &GlobalArgs) -> anyhow::Result<RegistryUrl> {
    let input = global
        .registry
        .as_deref()
        .filter(|input| !input.trim().is_empty())
        .context("no registry: pass --registry <URL> or set REGISTRY_URL")?;
    Ok(RegistryUrl::parse(input)?)
}

/// The token from GitHub Actions with `oidc`, otherwise from `--token`/`REGISTRY_TOKEN`
/// or the credentials of the registry URL.
pub async fn token(
    global: &GlobalArgs,
    registry: &RegistryUrl,
    oidc: bool,
    project: Option<&str>,
) -> anyhow::Result<String> {
    Ok(resolve_token(TokenFlags {
        token: global.token.as_deref(),
        url_token: registry.token.as_deref(),
        oidc,
        project,
        registry: &registry.base,
    })
    .await?)
}

/// An authenticated client using `--token`/`REGISTRY_TOKEN` or the registry URL's
/// credentials.
pub async fn client(global: &GlobalArgs) -> anyhow::Result<Client> {
    client_with(global, false, None).await
}

/// An authenticated client, with the token built from GitHub Actions OIDC when `oidc`.
///
/// The client gets the registry URL without its credentials.
pub async fn client_with(
    global: &GlobalArgs,
    oidc: bool,
    project: Option<&str>,
) -> anyhow::Result<Client> {
    let registry = registry(global)?;
    let token = token(global, &registry, oidc, project).await?;
    Ok(Client::new(registry.base, Some(token), CLIENT_VERSION)?)
}

/// A client without credentials, for `GET /api/v1/version`.
pub fn anonymous_client(registry: Url) -> anyhow::Result<Client> {
    Ok(Client::new(registry, None, CLIENT_VERSION)?)
}
