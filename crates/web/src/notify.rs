//! Telling the owner about new feedback.
//!
//! [`Notifier`] is a trait so the web app doesn't care how the owner hears about feedback:
//! [`SnsNotifier`] publishes to an SNS topic (which emails the owner) in production, and
//! [`NoopNotifier`] does nothing locally and in tests. Notifying is best effort: the feedback is
//! already saved before we get here, so a failure is logged and the user still sees "thanks".

use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use aws_sdk_sns::Client;
use aws_sdk_sns::error::DisplayErrorContext;
use choosey_core::model::Feedback;

#[async_trait]
pub trait Notifier: Send + Sync {
    /// Tell the owner about a new feedback message. Never fails: errors are logged.
    async fn feedback(&self, feedback: &Feedback);
}

/// Used when no topic is configured (local development, tests).
pub struct NoopNotifier;

#[async_trait]
impl Notifier for NoopNotifier {
    async fn feedback(&self, _feedback: &Feedback) {}
}

/// Most notifications sent per hour by one running instance. Feedback over the limit is still
/// saved (and visible in DynamoDB); it just doesn't email. This stops a bot from flooding the
/// owner's inbox. Lambda runs at most a handful of instances, so the real cap is a small multiple.
const MAX_PER_HOUR: u32 = 30;

/// Publishes to an SNS topic. The topic's email subscription delivers it.
pub struct SnsNotifier {
    client: Client,
    topic_arn: String,
    limit: HourlyLimit,
}

impl SnsNotifier {
    /// Build a client from the standard AWS environment (region, credentials).
    pub async fn from_env(topic_arn: String) -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self {
            client: Client::new(&config),
            topic_arn,
            limit: HourlyLimit::new(MAX_PER_HOUR),
        }
    }
}

#[async_trait]
impl Notifier for SnsNotifier {
    async fn feedback(&self, feedback: &Feedback) {
        if !self.limit.allow(Instant::now()) {
            tracing::warn!("feedback notification skipped: hourly limit reached");
            return;
        }
        let result = self
            .client
            .publish()
            .topic_arn(&self.topic_arn)
            // Fixed subject: SNS requires ASCII with no line breaks, and user text must not
            // be able to inject anything into it.
            .subject("New Choosey feedback")
            .message(email_body(feedback))
            .send()
            .await;
        if let Err(e) = result {
            tracing::error!(error = %DisplayErrorContext(&e), "feedback notification failed");
        }
    }
}

/// The plain-text email. SNS email is plain text, so user input can't become markup.
pub fn email_body(f: &Feedback) -> String {
    let contact = if f.contact.is_empty() {
        "(not given)"
    } else {
        f.contact.as_str()
    };
    format!(
        "Someone sent feedback on Choosey.\n\n\
         Reply to: {contact}\n\
         Feedback id: {id}\n\n\
         ----\n{message}\n----\n",
        id = f.id,
        message = f.message,
    )
}

/// A fixed-window counter: at most `max` events per hour.
///
/// A `std::sync::Mutex` is fine because the lock is never held across an `.await`.
struct HourlyLimit {
    max: u32,
    window: Mutex<(Instant, u32)>,
}

impl HourlyLimit {
    fn new(max: u32) -> Self {
        Self {
            max,
            window: Mutex::new((Instant::now(), 0)),
        }
    }

    /// Count one event at `now`; false if this hour's allowance is used up.
    fn allow(&self, now: Instant) -> bool {
        let mut w = self.window.lock().unwrap_or_else(|e| e.into_inner());
        if now.duration_since(w.0) >= Duration::from_secs(3600) {
            *w = (now, 0);
        }
        if w.1 >= self.max {
            return false;
        }
        w.1 += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hourly_limit_resets_after_an_hour() {
        let limit = HourlyLimit::new(2);
        let start = Instant::now();
        assert!(limit.allow(start));
        assert!(limit.allow(start));
        assert!(!limit.allow(start + Duration::from_secs(10)));
        assert!(limit.allow(start + Duration::from_secs(3600)));
    }

    #[test]
    fn email_body_includes_message_and_contact() {
        let f = Feedback {
            id: "abc".into(),
            created_at: 0,
            message: "The score page is confusing".into(),
            contact: String::new(),
        };
        let body = email_body(&f);
        assert!(body.contains("The score page is confusing"));
        assert!(body.contains("Reply to: (not given)"));
        assert!(body.contains("Feedback id: abc"));
    }
}
