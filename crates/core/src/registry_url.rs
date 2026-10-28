//! Registry URLs that may carry credentials in their userinfo.
//!
//! Following the Composer convention, `https://<login>:<password>@packages.example.com/`
//! passes the token `<login>:<password>` together with the registry URL. Both parts are
//! percent-decoded, so a login or password containing `@`, `:`, `/`, `?`, `#` or `%`
//! must be percent-encoded (`%40`, `%3A`, `%2F`, `%3F`, `%23`, `%25`).

use percent_encoding::percent_decode_str;
use url::Url;

use crate::error::{Error, Result};

/// A registry URL split into the base URL and the token found in its userinfo.
#[derive(Clone, PartialEq, Eq)]
pub struct RegistryUrl {
    /// The registry URL without userinfo, query and fragment; safe to print.
    pub base: Url,
    /// `<login>:<password>` from the URL's userinfo, percent-decoded. Secret.
    pub token: Option<String>,
}

impl std::fmt::Debug for RegistryUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryUrl")
            .field("base", &self.base.as_str())
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl RegistryUrl {
    /// Parses a registry URL, moving credentials found in its userinfo to `token`.
    ///
    /// Credentials need both a login and a password, and are only accepted over
    /// `https`, or over `http` to `localhost` and `127.0.0.1`. Error messages never
    /// contain the credentials.
    pub fn parse(input: &str) -> Result<Self> {
        let mut url = Url::parse(input).map_err(|err| {
            // The input may contain credentials: only echo it when it cannot.
            if input.contains('@') {
                Error::InvalidInput(format!("invalid registry URL: {err}"))
            } else {
                Error::InvalidInput(format!("invalid registry URL `{input}`: {err}"))
            }
        })?;
        let username = url.username().to_owned();
        let password = url.password().map(str::to_owned);
        // Neither of these fail for URLs that already had credentials.
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.set_query(None);
        url.set_fragment(None);

        if username.is_empty() && password.is_none() {
            return Ok(RegistryUrl {
                base: url,
                token: None,
            });
        }

        let (Some(password), false) = (password, username.is_empty()) else {
            return Err(Error::InvalidInput(
                "registry URL credentials need both a login and a password".into(),
            ));
        };
        let secure = match url.scheme() {
            "https" => true,
            "http" => matches!(url.host_str(), Some("localhost" | "127.0.0.1")),
            _ => false,
        };
        if !secure {
            return Err(Error::InvalidInput(format!(
                "refusing to send the credentials of the registry URL `{url}` in clear text: \
                 use https"
            )));
        }

        let login = decode(&username)?;
        let password = decode(&password)?;
        if login.is_empty() || password.is_empty() {
            return Err(Error::InvalidInput(
                "registry URL credentials need both a login and a password".into(),
            ));
        }
        Ok(RegistryUrl {
            base: url,
            token: Some(format!("{login}:{password}")),
        })
    }
}

fn decode(part: &str) -> Result<String> {
    percent_decode_str(part)
        .decode_utf8()
        .map(|decoded| decoded.into_owned())
        .map_err(|_| {
            Error::InvalidInput(
                "registry URL credentials are not valid UTF-8 after percent-decoding".into(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> RegistryUrl {
        RegistryUrl::parse(input).unwrap()
    }

    fn error(input: &str) -> String {
        match RegistryUrl::parse(input).unwrap_err() {
            Error::InvalidInput(message) => message,
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn plain_urls_have_no_token() {
        let url = parse("https://packages.example.com");
        assert_eq!(url.base.as_str(), "https://packages.example.com/");
        assert_eq!(url.token, None);
    }

    #[test]
    fn credentials_become_the_token() {
        let url = parse("https://shop-frontend:k7Qx2mN9@packages.example.com/");
        assert_eq!(url.base.as_str(), "https://packages.example.com/");
        assert_eq!(url.token.as_deref(), Some("shop-frontend:k7Qx2mN9"));
    }

    #[test]
    fn base_keeps_scheme_host_port_and_path() {
        let url = parse("https://okta.shop:pw@packages.example.com:8443/registry?x=1#frag");
        assert_eq!(
            url.base.as_str(),
            "https://packages.example.com:8443/registry"
        );
        assert_eq!(url.token.as_deref(), Some("okta.shop:pw"));

        let url = parse("https://shop:pw@packages.example.com/registry/");
        assert_eq!(url.base.as_str(), "https://packages.example.com/registry/");

        let url = parse("https://shop:pw@packages.example.com");
        assert_eq!(url.base.as_str(), "https://packages.example.com/");
    }

    #[test]
    fn credentials_are_percent_decoded() {
        let url = parse("https://shop%40eu:p%40ss%3Aword%2F%25@packages.example.com/");
        assert_eq!(url.token.as_deref(), Some("shop@eu:p@ss:word/%"));
        assert_eq!(url.base.as_str(), "https://packages.example.com/");

        // A type prefix and a `:` in the secret survive (only the first `:` splits).
        let url = parse("https://ci-acme:gh.a%3Ab@packages.example.com/");
        assert_eq!(url.token.as_deref(), Some("ci-acme:gh.a:b"));
    }

    #[test]
    fn credentials_need_a_login_and_a_password() {
        for input in [
            "https://shop@packages.example.com/",
            "https://shop:@packages.example.com/",
            "https://:secret@packages.example.com/",
        ] {
            assert_eq!(
                error(input),
                "registry URL credentials need both a login and a password",
                "{input}"
            );
        }
    }

    #[test]
    fn credentials_over_http_are_refused() {
        let message = error("http://shop:secret@packages.example.com/");
        assert!(message.contains("clear text"), "{message}");
        assert!(
            message.contains("http://packages.example.com/"),
            "{message}"
        );
        assert!(!message.contains("secret"), "{message}");
        assert!(RegistryUrl::parse("ftp://shop:secret@packages.example.com/").is_err());
    }

    #[test]
    fn credentials_over_http_to_localhost_are_allowed() {
        let url = parse("http://shop:secret@localhost:8787/registry/");
        assert_eq!(url.base.as_str(), "http://localhost:8787/registry/");
        assert_eq!(url.token.as_deref(), Some("shop:secret"));

        let url = parse("http://shop:secret@127.0.0.1:8787");
        assert_eq!(url.base.as_str(), "http://127.0.0.1:8787/");

        // Plain http without credentials is not this parser's business.
        assert_eq!(parse("http://packages.example.com/").token, None);
    }

    #[test]
    fn errors_and_debug_never_show_the_secret() {
        let message = error("https://shop:secret@packages.example.com:99999/");
        assert!(!message.contains("secret"), "{message}");
        let message = error("not a url");
        assert!(message.contains("`not a url`"), "{message}");

        let url = parse("https://shop:secret@packages.example.com/");
        let debug = format!("{url:?}");
        assert!(!debug.contains("secret"), "{debug}");
    }
}
