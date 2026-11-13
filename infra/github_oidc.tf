# Lets GitHub Actions assume a role without any long-lived AWS keys.
resource "aws_iam_openid_connect_provider" "github" {
  url            = "https://token.actions.githubusercontent.com"
  client_id_list = ["sts.amazonaws.com"]
}

data "aws_iam_policy_document" "deploy_assume" {
  statement {
    actions = ["sts:AssumeRoleWithWebIdentity"]
    principals {
      type        = "Federated"
      identifiers = [aws_iam_openid_connect_provider.github.arn]
    }
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:aud"
      values   = ["sts.amazonaws.com"]
    }
    # Only jobs running in the "production" environment of this repo, which
    # is restricted to the main branch on the GitHub side.
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:sub"
      values   = ["${var.github_oidc_subject_prefix}:environment:production"]
    }
  }
}

resource "aws_iam_role" "deploy" {
  name                 = "${var.app_name}-github-deploy"
  assume_role_policy   = data.aws_iam_policy_document.deploy_assume.json
  max_session_duration = 3600
}

# Can update this one function's code and nothing else.
data "aws_iam_policy_document" "deploy" {
  statement {
    actions = [
      "lambda:UpdateFunctionCode",
      "lambda:GetFunction",
      "lambda:GetFunctionConfiguration",
      "lambda:PublishVersion",
    ]
    resources = [aws_lambda_function.app.arn]
  }
}

resource "aws_iam_role_policy" "deploy" {
  name   = "${var.app_name}-github-deploy"
  role   = aws_iam_role.deploy.id
  policy = data.aws_iam_policy_document.deploy.json
}
