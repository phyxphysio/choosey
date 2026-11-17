//! Checking that feedback comes from a person, not a bot.
//!
//! [`Captcha`] is a trait so the feedback handler doesn't care how the check is done:
//! [`Turnstile`] asks Cloudflare Turnstile in production, and [`NoCaptcha`] lets everything
//! through locally and in tests. It follows the same pattern as [`crate::notify::Notifier`].
//!
//! How Turnstile works: the feedback page loads Cloudflare's script, which (usually invisibly)
//! decides whether the visitor is a person and puts a one-time token in a hidden form field named
//! `cf-turnstile-response`. The server then sends that token, with our secret key, to Cloudflare's
//! `siteverify` endpoint, which says whether it is genuine. Tokens expire after 5 minutes and can
//! only be checked once.
//! Docs: <https://developers.cloudflare.com/turnstile/get-started/server-side-validation/>

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

/// The form field Turnstile's script adds to the form.
pub const TOKEN_FIELD: &str = "cf-turnstile-response";

/// What the check decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Looks like a person: go ahead.
    Human,
    /// Missing, expired, reused or fake token.
    NotHuman,
    /// We couldn't ask Cloudflare (network error, timeout, odd reply). Fail closed, but tell the
    /// person it's our problem so they try again rather than give up.
    Unavailable,
}

#[async_trait]
pub trait Captcha: Send + Sync {
    /// The public site key for the page's widget, or `None` when no check is configured (then the
    /// page shows no widget and [`Captcha::check`] passes everything).
    fn site_key(&self) -> Option<&str>;

    /// Check the token the browser submitted.
    async fn check(&self, token: &str) -> Verdict;
}

/// Used when Turnstile isn't configured (local development, tests): everyone is a person.
pub struct NoCaptcha;

#[async_trait]
impl Captcha for NoCaptcha {
    fn site_key(&self) -> Option<&str> {
        None
    }

    async fn check(&self, _token: &str) -> Verdict {
        Verdict::Human
    }
}

const SITEVERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

/// Longest a token can be, per Cloudflare's docs. Anything longer is fake, so don't send it.
const MAX_TOKEN_LEN: usize = 2048;

/// Cloudflare Turnstile.
pub struct Turnstile {
    client: reqwest::Client,
    site_key: String,
    secret_key: String,
    /// When set, the token must have been issued on this hostname (e.g. `choosey.app`), so a
    /// token solved on some other site that uses our public site key is refused. Unset locally,
    /// because Cloudflare's test keys report `example.com`.
    expected_hostname: Option<String>,
}

/// The parts of Cloudflare's reply we use. Unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct SiteverifyReply {
    success: bool,
    #[serde(default)]
    hostname: Option<String>,
    #[serde(default, rename = "error-codes")]
    error_codes: Vec<String>,
}

impl Turnstile {
    pub fn new(
        site_key: String,
        secret_key: String,
        expected_hostname: Option<String>,
    ) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            // Lambda's own timeout is 10 s; leave room to render the page afterwards.
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| format!("could not build HTTP client: {e}"))?;
        Ok(Self {
            client,
            site_key,
            secret_key,
            expected_hostname,
        })
    }

    /// Turn Cloudflare's reply into a verdict.
    fn judge(&self, reply: &SiteverifyReply) -> Verdict {
        if !reply.success {
            // Codes like "timeout-or-duplicate" or "invalid-input-response" mean a bad token.
            // "internal-error" is Cloudflare's own problem.
            if reply.error_codes.iter().any(|c| c == "internal-error") {
                return Verdict::Unavailable;
            }
            tracing::info!(codes = ?reply.error_codes, "turnstile rejected a token");
            return Verdict::NotHuman;
        }
        match (&self.expected_hostname, &reply.hostname) {
            (Some(want), Some(got)) if !got.eq_ignore_ascii_case(want) => {
                tracing::warn!(hostname = %got, "turnstile token was issued for another hostname");
                Verdict::NotHuman
            }
            (Some(_), None) => Verdict::NotHuman,
            _ => Verdict::Human,
        }
    }
}

#[async_trait]
impl Captcha for Turnstile {
    fn site_key(&self) -> Option<&str> {
        Some(&self.site_key)
    }

    async fn check(&self, token: &str) -> Verdict {
        let token = token.trim();
        if token.is_empty() || token.len() > MAX_TOKEN_LEN {
            return Verdict::NotHuman;
        }
        let sent = self
            .client
            .post(SITEVERIFY_URL)
            .form(&[("secret", self.secret_key.as_str()), ("response", token)])
            .send()
            .await
            .and_then(|r| r.error_for_status());
        let reply = match sent {
            Ok(r) => r.json::<SiteverifyReply>().await,
            Err(e) => Err(e),
        };
        match reply {
            Ok(reply) => self.judge(&reply),
            Err(e) => {
                // reqwest errors include the URL but never the form body, so the secret is safe.
                tracing::error!(error = %e, "turnstile siteverify failed");
                Verdict::Unavailable
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turnstile(host: Option<&str>) -> Turnstile {
        Turnstile::new("site".into(), "secret".into(), host.map(String::from)).unwrap()
    }

    fn reply(json: &str) -> SiteverifyReply {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn success_passes() {
        let t = turnstile(Some("choosey.app"));
        let r = reply(r#"{"success":true,"hostname":"choosey.app","error-codes":[]}"#);
        assert_eq!(t.judge(&r), Verdict::Human);
    }

    #[test]
    fn wrong_or_missing_hostname_fails_when_checked() {
        let t = turnstile(Some("choosey.app"));
        let other = reply(r#"{"success":true,"hostname":"evil.example"}"#);
        assert_eq!(t.judge(&other), Verdict::NotHuman);
        let missing = reply(r#"{"success":true}"#);
        assert_eq!(t.judge(&missing), Verdict::NotHuman);
        // Locally (test keys say "example.com") the hostname isn't checked.
        assert_eq!(turnstile(None).judge(&other), Verdict::Human);
    }

    #[test]
    fn bad_tokens_fail_and_cloudflare_errors_are_unavailable() {
        let t = turnstile(None);
        let dup = reply(r#"{"success":false,"error-codes":["timeout-or-duplicate"]}"#);
        assert_eq!(t.judge(&dup), Verdict::NotHuman);
        let internal = reply(r#"{"success":false,"error-codes":["internal-error"]}"#);
        assert_eq!(t.judge(&internal), Verdict::Unavailable);
    }

    #[tokio::test]
    async fn empty_or_huge_tokens_never_reach_cloudflare() {
        let t = turnstile(None);
        assert_eq!(t.check("  ").await, Verdict::NotHuman);
        assert_eq!(
            t.check(&"x".repeat(MAX_TOKEN_LEN + 1)).await,
            Verdict::NotHuman
        );
    }
}
