//! Browser login (docs/api.md 2.6, 6.20 and 6.21): the client asks the registry for a login
//! session, the user signs in and approves it on a page of the registry, and the client
//! collects a short-lived token by polling.

use std::fmt;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::client::Client;
use crate::error::{Error, Result};
use crate::models::Kind;
use crate::packager::Ecosystem;

/// The longest the client waits for an approval; the registry ends a session after 10
/// minutes.
const MAX_WAIT: Duration = Duration::from_secs(11 * 60);

/// One package version a publish login approves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishScope {
    /// Ecosystem of the package.
    pub eco: Ecosystem,
    /// Package name as the registry stores it, e.g. `acme/sdk` or `@acme/sdk`.
    pub package: String,
    /// The version to publish.
    pub version: String,
}

/// What a login asks the user to approve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginPurpose {
    /// A pull token. The ecosystem only labels the approval page.
    Pull {
        /// The ecosystem the token is for, if known.
        eco: Option<Ecosystem>,
    },
    /// A publish token for exactly these package versions.
    Publish {
        /// The versions to approve (at least 1; the registry caps the count with its
        /// `MAX_PUBLISH_SCOPES` setting, 20 by default).
        scopes: Vec<PublishScope>,
    },
}

#[derive(Serialize)]
struct CreateBody<'a> {
    purpose: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    eco: Option<Ecosystem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scopes: Option<&'a [PublishScope]>,
}

impl LoginPurpose {
    fn body(&self) -> CreateBody<'_> {
        match self {
            LoginPurpose::Pull { eco } => CreateBody {
                purpose: "pull",
                eco: *eco,
                scopes: None,
            },
            LoginPurpose::Publish { scopes } => CreateBody {
                purpose: "publish",
                eco: None,
                scopes: Some(scopes),
            },
        }
    }
}

/// A login session created by the registry (`POST /api/v1/auth/sessions`).
#[derive(Clone, Deserialize)]
pub struct LoginSession {
    /// Opaque session id.
    pub session: String,
    /// The page the user opens to sign in and approve.
    pub url: String,
    /// Proves that the poller is the client that created the session.
    pub poll_token: String,
    /// Seconds between polls.
    pub interval: u64,
    /// When the session ends, RFC 3339.
    pub expires_at: String,
}

impl fmt::Debug for LoginSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginSession")
            .field("url", &"<redacted>")
            .field("interval", &self.interval)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// The token the registry issued once the user approved.
#[derive(Clone, Deserialize)]
pub struct IssuedToken {
    /// The whole token, `<project>:st.<payload>.<signature>`.
    pub token: String,
    /// When it expires, RFC 3339.
    pub expires_at: String,
    /// The project it belongs to.
    pub project: String,
    /// The kind of the credential.
    pub kind: Kind,
}

impl fmt::Debug for IssuedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedToken")
            .field("token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("project", &self.project)
            .field("kind", &self.kind)
            .finish()
    }
}

#[derive(Serialize)]
struct PollBody<'a> {
    session: &'a str,
    poll_token: &'a str,
}

#[derive(Deserialize)]
struct PollResponse {
    status: String,
    token: Option<String>,
    expires_at: Option<String>,
    project: Option<String>,
    kind: Option<Kind>,
}

/// The state of a login session.
#[derive(Debug)]
pub enum Poll {
    /// Not decided yet.
    Pending,
    /// Approved; the token is delivered only once.
    Approved(IssuedToken),
}

impl Client {
    /// `POST /api/v1/auth/sessions`: starts a login. Needs no credential.
    pub async fn create_login_session(&self, purpose: &LoginPurpose) -> Result<LoginSession> {
        self.post(&["auth", "sessions"], Some(&purpose.body()))
            .await
    }

    /// `POST /api/v1/auth/sessions/poll`: asks for the decision. A refusal is
    /// `403 forbidden` and an expired session `404 not_found`, both as
    /// [`Error::Api`].
    pub async fn poll_login_session(&self, session: &LoginSession) -> Result<Poll> {
        let body = PollBody {
            session: &session.session,
            poll_token: &session.poll_token,
        };
        let response: PollResponse = self
            .post(&["auth", "sessions", "poll"], Some(&body))
            .await?;
        if response.status == "pending" {
            return Ok(Poll::Pending);
        }
        match response {
            PollResponse {
                status,
                token: Some(token),
                expires_at: Some(expires_at),
                project: Some(project),
                kind: Some(kind),
            } if status == "approved" => Ok(Poll::Approved(IssuedToken {
                token,
                expires_at,
                project,
                kind,
            })),
            _ => Err(Error::InvalidInput(
                "the registry sent an unexpected login response".into(),
            )),
        }
    }

    /// Runs a whole login: creates the session, hands it to `announce` (which shows the
    /// user where to go) and polls until the user approves, refuses or the session ends.
    pub async fn login(
        &self,
        purpose: &LoginPurpose,
        announce: impl FnOnce(&LoginSession),
    ) -> Result<IssuedToken> {
        let session = self.create_login_session(purpose).await?;
        announce(&session);
        let started = Instant::now();
        let interval = Duration::from_secs(session.interval.min(10));
        loop {
            match self.poll_login_session(&session).await? {
                Poll::Approved(token) => return Ok(token),
                Poll::Pending => {}
            }
            if started.elapsed() > MAX_WAIT {
                return Err(Error::InvalidInput(
                    "timed out waiting for the login to be approved".into(),
                ));
            }
            tokio::time::sleep(interval).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_asks_for_nothing_but_its_ecosystem() {
        let body = serde_json::to_value(LoginPurpose::Pull { eco: None }.body()).unwrap();
        assert_eq!(body, serde_json::json!({ "purpose": "pull" }));
        let body = serde_json::to_value(
            LoginPurpose::Pull {
                eco: Some(Ecosystem::Generic),
            }
            .body(),
        )
        .unwrap();
        assert_eq!(
            body,
            serde_json::json!({ "purpose": "pull", "eco": "generic" })
        );
    }

    #[test]
    fn a_publish_lists_every_version() {
        let purpose = LoginPurpose::Publish {
            scopes: vec![PublishScope {
                eco: Ecosystem::Composer,
                package: "acme/sdk".into(),
                version: "1.4.0".into(),
            }],
        };
        assert_eq!(
            serde_json::to_value(purpose.body()).unwrap(),
            serde_json::json!({
                "purpose": "publish",
                "scopes": [{ "eco": "composer", "package": "acme/sdk", "version": "1.4.0" }]
            })
        );
    }

    #[test]
    fn secrets_are_redacted_in_debug_output() {
        let issued = IssuedToken {
            token: "shop:st.secret.signature".into(),
            expires_at: "2026-10-28T10:00:00.000Z".into(),
            project: "shop".into(),
            kind: Kind::Read,
        };
        let session = LoginSession {
            session: "signed-session-id".into(),
            url: "https://registry.test/authorize/signed-session-id".into(),
            poll_token: "poll-secret".into(),
            interval: 2,
            expires_at: "2026-10-28T10:00:00.000Z".into(),
        };
        let shown = format!("{issued:?} {session:?}");
        for secret in ["secret.signature", "signed-session-id", "poll-secret"] {
            assert!(!shown.contains(secret), "{shown}");
        }
    }
}
