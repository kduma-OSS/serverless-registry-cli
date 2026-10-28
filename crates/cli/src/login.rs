//! Signing in through the browser (`--login` and `sreg login`): the registry gives a
//! page to open, the user signs in and approves there, and the short-lived token comes
//! back by polling. Messages go to stderr, so stdout stays clean for the result.

use std::process::{Command, Stdio};

use sreg_core::login::{IssuedToken, LoginPurpose, LoginSession};

use crate::cli::GlobalArgs;
use crate::context;

/// Runs the whole login and returns the issued token.
pub async fn acquire(global: &GlobalArgs, purpose: &LoginPurpose) -> anyhow::Result<IssuedToken> {
    let registry = context::registry(global)?;
    let client = context::anonymous_client(registry.base)?;
    let issued = client
        .login(purpose, |session| announce(global, purpose, session))
        .await?;
    eprintln!(
        "Approved: signed in as {} ({}); the token is valid until {}.",
        issued.project, issued.kind, issued.expires_at
    );
    Ok(issued)
}

fn announce(global: &GlobalArgs, purpose: &LoginPurpose, session: &LoginSession) {
    eprintln!("{}", describe(purpose));
    eprintln!("Open this page, sign in and approve:");
    eprintln!("  {}", session.url);
    if !global.no_browser {
        open_browser(&session.url);
    }
    eprintln!("Waiting for the approval (Ctrl-C to cancel)...");
}

/// What the user is asked to approve, for the terminal.
fn describe(purpose: &LoginPurpose) -> String {
    match purpose {
        LoginPurpose::Pull { .. } => "Requesting a token to download packages.".to_owned(),
        LoginPurpose::Publish { scopes } => {
            let mut text = "Requesting a token to publish:".to_owned();
            for scope in scopes {
                text.push_str(&format!(
                    "\n  {} {} {}",
                    scope.eco, scope.package, scope.version
                ));
            }
            text
        }
    }
}

/// Opens `url` in the default browser. Best effort: the URL is printed anyway, and a
/// failure (no browser, no display) is only logged. The opener is started and not waited
/// for, because some of them stay in the foreground for as long as the browser runs.
fn open_browser(url: &str) {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    } else {
        Command::new("xdg-open")
    };
    let started = command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(err) = started {
        tracing::debug!(error = %err, "could not open a browser");
    }
}

#[cfg(test)]
mod tests {
    use sreg_core::login::PublishScope;
    use sreg_core::Ecosystem;

    use super::*;

    #[test]
    fn describes_a_pull_and_every_published_version() {
        assert_eq!(
            describe(&LoginPurpose::Pull { eco: None }),
            "Requesting a token to download packages."
        );
        let purpose = LoginPurpose::Publish {
            scopes: vec![
                PublishScope {
                    eco: Ecosystem::Composer,
                    package: "acme/sdk".into(),
                    version: "1.4.0".into(),
                },
                PublishScope {
                    eco: Ecosystem::Npm,
                    package: "@acme/sdk".into(),
                    version: "2.0.0".into(),
                },
            ],
        };
        assert_eq!(
            describe(&purpose),
            "Requesting a token to publish:\n  composer acme/sdk 1.4.0\n  npm @acme/sdk 2.0.0"
        );
    }
}
