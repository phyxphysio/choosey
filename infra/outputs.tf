output "site_url" {
  value = "https://${var.domain}"
}

output "cloudfront_domain" {
  value = aws_cloudfront_distribution.app.domain_name
}

output "lambda_function_name" {
  value = aws_lambda_function.app.function_name
}

output "deploy_role_arn" {
  description = "Set as the GitHub Actions variable AWS_DEPLOY_ROLE_ARN."
  value       = aws_iam_role.deploy.arn
}

output "aws_region" {
  value = var.aws_region
}
