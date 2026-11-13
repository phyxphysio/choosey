variable "app_name" {
  description = "Name used for every resource. Change here to rename."
  type        = string
  default     = "choosey"
}

variable "domain" {
  description = "Apex domain, registered via Cloudflare Registrar."
  type        = string
  default     = "choosey.app"
}

variable "aws_region" {
  description = "Region for Lambda, DynamoDB and logs."
  type        = string
  default     = "ap-southeast-2"
}

variable "github_oidc_subject_prefix" {
  description = <<-EOT
    The repo's OIDC `sub` prefix. This repo uses GitHub's immutable subject format, which embeds
    the owner and repo IDs so a renamed or re-created repo with the same name can't deploy.
    Check with: gh api repos/<owner>/<repo>/actions/oidc/customization/sub
  EOT
  type        = string
  default     = "repo:phyxphysio@64931554/choosey@1457136940"
}

variable "budget_alert_email" {
  description = "Where AWS Budgets sends cost alerts. Set in terraform.tfvars (gitignored)."
  type        = string
  sensitive   = true
}

variable "budget_limit_usd" {
  description = "Monthly cost alert threshold."
  type        = number
  default     = 5
}

variable "lambda_reserved_concurrency" {
  description = "Hard cap on concurrent executions, so a traffic spike can't run up a bill."
  type        = number
  default     = 5
}
