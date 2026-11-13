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

variable "github_repo" {
  description = "owner/name of the repo allowed to deploy via OIDC."
  type        = string
  default     = "phyxphysio/choosey"
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
