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
