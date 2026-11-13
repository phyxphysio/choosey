# Shared secret that CloudFront adds to every origin request. The app rejects
# requests without it, so the public Function URL is useless if called directly.
# (OAC + AWS_IAM auth was not used: it requires browsers to send a SHA-256 of
# every POST body, which plain HTML forms can't do.)
resource "random_password" "origin_verify" {
  length  = 48
  special = false
}

resource "aws_cloudwatch_log_group" "lambda" {
  name              = "/aws/lambda/${var.app_name}"
  retention_in_days = 14
}

data "aws_iam_policy_document" "lambda_assume" {
  statement {
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["lambda.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "lambda" {
  name               = "${var.app_name}-lambda"
  assume_role_policy = data.aws_iam_policy_document.lambda_assume.json
}

# Least privilege: item-level access to the one table, and logs to the one group.
data "aws_iam_policy_document" "lambda" {
  statement {
    sid       = "DecisionsTable"
    actions   = ["dynamodb:GetItem", "dynamodb:PutItem", "dynamodb:DeleteItem"]
    resources = [aws_dynamodb_table.decisions.arn]
  }
  statement {
    sid       = "Logs"
    actions   = ["logs:CreateLogStream", "logs:PutLogEvents"]
    resources = ["${aws_cloudwatch_log_group.lambda.arn}:*"]
  }
}

resource "aws_iam_role_policy" "lambda" {
  name   = "${var.app_name}-lambda"
  role   = aws_iam_role.lambda.id
  policy = data.aws_iam_policy_document.lambda.json
}

# Placeholder code for the first apply. Real code is deployed by CI
# (aws lambda update-function-code), so Terraform ignores code changes.
data "archive_file" "placeholder" {
  type        = "zip"
  output_path = "${path.module}/.build/placeholder.zip"
  source {
    filename = "bootstrap"
    content  = "#!/bin/sh\necho 'placeholder: deploy the real build' >&2\nexit 1\n"
  }
}

resource "aws_lambda_function" "app" {
  function_name = var.app_name
  role          = aws_iam_role.lambda.arn
  runtime       = "provided.al2023"
  handler       = "bootstrap"
  architectures = ["arm64"]
  memory_size   = 256
  timeout       = 10

  filename         = data.archive_file.placeholder.output_path
  source_code_hash = data.archive_file.placeholder.output_base64sha256

  reserved_concurrent_executions = var.lambda_reserved_concurrency

  environment {
    variables = {
      STORE                = "dynamo"
      TABLE_NAME           = aws_dynamodb_table.decisions.name
      PUBLIC_BASE_URL      = "https://${var.domain}"
      ORIGIN_VERIFY_SECRET = random_password.origin_verify.result
      RUST_LOG             = "info"
    }
  }

  logging_config {
    log_format = "Text"
    log_group  = aws_cloudwatch_log_group.lambda.name
  }

  lifecycle {
    ignore_changes = [filename, source_code_hash]
  }

  depends_on = [aws_iam_role_policy.lambda]
}

resource "aws_lambda_function_url" "app" {
  function_name      = aws_lambda_function.app.function_name
  authorization_type = "NONE"
}

# Public Function URLs need both permissions (since Oct 2025). Requests without
# the CloudFront secret header are rejected by the app itself.
resource "aws_lambda_permission" "url_public" {
  statement_id           = "FunctionURLAllowPublicAccess"
  action                 = "lambda:InvokeFunctionUrl"
  function_name          = aws_lambda_function.app.function_name
  principal              = "*"
  function_url_auth_type = "NONE"
}

resource "aws_lambda_permission" "url_invoke" {
  statement_id             = "FunctionURLAllowInvokeAction"
  action                   = "lambda:InvokeFunction"
  function_name            = aws_lambda_function.app.function_name
  principal                = "*"
  invoked_via_function_url = true
}
