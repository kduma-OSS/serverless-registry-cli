//! Resolution of the registry token shared by all commands.

use url::Url;

use crate::error::{Error, Result};
use crate::oidc::{compose_token, github_oidc_token};

/// The reserved login of an OIDC token without a project (`_:gh.<jwt>`): the Worker
/// resolves the project from the JWT's `repository` claim.
pub const RESOLVE_PROJECT_LOGIN: &str = "_";

/// Where the token comes from.
#[derive(Debug, Clone, Copy)]
pub struct TokenFlags<'a> {
    /// `--token` or `REGISTRY_TOKEN`.
    pub token: Option<&'a str>,
    /// The credentials from the registry URL's userinfo
    /// ([`RegistryUrl::token`](crate::RegistryUrl::token)).
    pub url_token: Option<&'a str>,
    /// `--oidc`: build the token from a GitHub Actions OIDC token.
    pub oidc: bool,
    /// `--project`: the project the OIDC token is issued for; `None` lets the registry
    /// resolve it from the repository the token was issued to.
    pub project: Option<&'a str>,
    /// The registry URL; its form without trailing slash is the OIDC audience.
    pub registry: &'a Url,
}

/// Resolves the token to authenticate with.
///
/// With `oidc`, requests a GitHub Actions OIDC token with the registry URL as audience
/// and returns `<project>:gh.<jwt>`, or `_:gh.<jwt>` without a project, which lets the
/// registry find the project from the token's `repository` claim; the other sources are
/// ignored. Otherwise returns `token` or, without it, `url_token`. When both are given
/// and differ, fails rather than guessing which one is meant.
pub async fn resolve_token(flags: TokenFlags<'_>) -> Result<String> {
    if flags.oidc {
        let login = match flags.project {
            Some(project) => {
                validate_login(project)?;
                project
            }
            None => RESOLVE_PROJECT_LOGIN,
        };
        let jwt = github_oidc_token(&audience(flags.registry)).await?;
        return Ok(compose_token(login, &jwt));
    }

    let explicit = flags.token.map(str::trim).filter(|token| !token.is_empty());
    let token = match (explicit, flags.url_token) {
        (Some(explicit), Some(url)) if explicit != url => {
            return Err(Error::InvalidInput(
                "--token/REGISTRY_TOKEN and the credentials in the registry URL differ: pass \
                 the token only once"
                    .into(),
            ))
        }
        (Some(token), _) | (None, Some(token)) => token,
        (None, None) => {
            return Err(Error::InvalidInput(
                "no token: pass --token, set REGISTRY_TOKEN or put the credentials in the \
                 registry URL (in GitHub Actions you can use --oidc instead)"
                    .into(),
            ))
        }
    };
    Token::login(token)?;
    Ok(token.to_owned())
}

/// The OIDC audience for a registry: its URL without query, fragment and trailing
/// slash, i.e. the Worker's `PUBLIC_URL`.
pub fn audience(registry: &Url) -> String {
    let mut url = registry.clone();
    url.set_query(None);
    url.set_fragment(None);
    url.as_str().trim_end_matches('/').to_owned()
}

/// Helpers for tokens of the form `[<source>.]<project>:[<type>.]<secret>`.
#[derive(Debug, Clone, Copy)]
pub struct Token;

impl Token {
    /// The login (`[<prefix>][<source>.]<project>`): everything before the first `:`.
    ///
    /// The login is not secret and may be shown to the user.
    pub fn login(token: &str) -> Result<&str> {
        Self::split(token).map(|(login, _)| login)
    }

    /// The password (`[<type>.]<secret>`): everything after the first `:`. Secret.
    pub fn password(token: &str) -> Result<&str> {
        Self::split(token).map(|(_, password)| password)
    }

    fn split(token: &str) -> Result<(&str, &str)> {
        match token.split_once(':') {
            Some((login, password)) if !login.is_empty() && !password.is_empty() => {
                Ok((login, password))
            }
            _ => Err(Error::InvalidInput(
                "malformed token: expected [<source>.]<project>:[<type>.]<secret>".into(),
            )),
        }
    }
}

fn validate_login(project: &str) -> Result<()> {
    let valid = !project.is_empty()
        && project
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.');
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "invalid project `{project}`: expected [<source>.]<project> in lower case"
        )))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{method, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::oidc::{REQUEST_TOKEN_ENV, REQUEST_URL_ENV};
    use crate::test_env::EnvGuard;

    fn registry() -> Url {
        Url::parse("https://packages.example.com/").unwrap()
    }

    fn flags<'a>(registry: &'a Url, token: Option<&'a str>) -> TokenFlags<'a> {
        TokenFlags {
            token,
            url_token: None,
            oidc: false,
            project: None,
            registry,
        }
    }

    #[test]
    fn audience_has_no_trailing_slash() {
        assert_eq!(audience(&registry()), "https://packages.example.com");
        let nested = Url::parse("https://example.com/registry/?x=1#y").unwrap();
        assert_eq!(audience(&nested), "https://example.com/registry");
    }

    #[test]
    fn token_splits_at_the_first_colon() {
        let token = "ci-acme-sdk:gh.eyJ:a.eyJ.sig";
        assert_eq!(Token::login(token).unwrap(), "ci-acme-sdk");
        assert_eq!(Token::password(token).unwrap(), "gh.eyJ:a.eyJ.sig");
        assert_eq!(Token::login("okta.shop:Zm9v").unwrap(), "okta.shop");
        for malformed in ["no-colon", ":secret", "project:"] {
            assert!(Token::login(malformed).is_err(), "{malformed}");
        }
    }

    #[tokio::test]
    async fn uses_the_given_token() {
        let registry = registry();
        let token = resolve_token(flags(&registry, Some(" shop:secret\n")))
            .await
            .unwrap();
        assert_eq!(token, "shop:secret");
    }

    #[tokio::test]
    async fn missing_or_malformed_token_is_rejected() {
        let registry = registry();
        for token in [None, Some(""), Some("no-colon")] {
            let err = resolve_token(flags(&registry, token)).await.unwrap_err();
            assert!(matches!(err, Error::InvalidInput(_)), "{token:?}");
        }
    }

    #[tokio::test]
    async fn url_credentials_are_used_without_an_explicit_token() {
        let registry = registry();
        let url_flags = |token| TokenFlags {
            url_token: Some("shop:from-url"),
            ..flags(&registry, token)
        };

        for token in [None, Some(""), Some(" \n")] {
            let resolved = resolve_token(url_flags(token)).await.unwrap();
            assert_eq!(resolved, "shop:from-url", "{token:?}");
        }
        // The same token twice is not a conflict.
        let resolved = resolve_token(url_flags(Some("shop:from-url\n")))
            .await
            .unwrap();
        assert_eq!(resolved, "shop:from-url");
    }

    #[tokio::test]
    async fn differing_explicit_and_url_tokens_are_rejected() {
        let registry = registry();
        let err = resolve_token(TokenFlags {
            url_token: Some("shop:from-url"),
            ..flags(&registry, Some("shop:explicit"))
        })
        .await
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("differ"), "{message}");
        assert!(!message.contains("shop:"), "{message}");
    }

    #[tokio::test]
    async fn oidc_rejects_an_invalid_project() {
        let registry = registry();
        for project in [Some("Shop:x"), Some(""), Some("_")] {
            let err = resolve_token(TokenFlags {
                token: None,
                url_token: None,
                oidc: true,
                project,
                registry: &registry,
            })
            .await
            .unwrap_err();
            assert!(matches!(err, Error::InvalidInput(_)), "{project:?}");
        }
    }

    #[test]
    fn oidc_builds_a_gh_token_for_the_registry_audience() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let server = runtime.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(query_param("audience", "https://packages.example.com"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({ "value": "eyJ.x.y" })),
                )
                .mount(&server)
                .await;
            server
        });
        let url = format!("{}/token?api-version=2.0", server.uri());
        let _env = EnvGuard::set(&[
            (REQUEST_URL_ENV, Some(url.as_str())),
            (REQUEST_TOKEN_ENV, Some("request-token")),
        ]);
        let registry = registry();

        let token = runtime
            .block_on(resolve_token(TokenFlags {
                token: Some("ignored:token"),
                url_token: Some("ignored:url"),
                oidc: true,
                project: Some("ci-acme-sdk"),
                registry: &registry,
            }))
            .unwrap();

        assert_eq!(token, "ci-acme-sdk:gh.eyJ.x.y");

        let token = runtime
            .block_on(resolve_token(TokenFlags {
                token: None,
                url_token: None,
                oidc: true,
                project: None,
                registry: &registry,
            }))
            .unwrap();

        assert_eq!(token, "_:gh.eyJ.x.y");
    }
}
