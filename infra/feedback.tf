# Feedback notifications: the app saves each message to DynamoDB, then publishes it to this
# topic, which emails the owner. AWS sends a confirmation email for the subscription once;
# nothing is delivered until the link in it is clicked.
#
# Not encrypted with KMS: messages are short-lived in SNS, the content is user feedback (not
# secrets), and an AWS-managed key would add KMS permissions for little benefit.
resource "aws_sns_topic" "feedback" {
  name = "${var.app_name}-feedback"
}

resource "aws_sns_topic_subscription" "feedback_email" {
  topic_arn = aws_sns_topic.feedback.arn
  protocol  = "email"
  endpoint  = var.alert_email
}

# Cloudflare Turnstile: a free, usually invisible "are you a person?" check on the feedback form,
# added after bots got past the honeypot field. The app sends each token to Cloudflare with the
# secret key to verify it (see crates/web/src/captcha.rs). Needs the Cloudflare API token to have
# the "Account > Turnstile > Edit" permission.
resource "cloudflare_turnstile_widget" "feedback" {
  account_id = data.cloudflare_zone.this.account.id
  name       = "${var.app_name} feedback"
  domains    = [var.domain, local.www_domain]
  # Cloudflare decides whether to show a checkbox; most people never see one.
  mode = "managed"
}
