# Infrastructure

Terraform for Choosey. Run by hand (`terraform apply`); CI only deploys app code.

| Piece | Where |
|---|---|
| Lambda (arm64, `provided.al2023`, 256 MB, reserved concurrency 5) + Function URL | ap-southeast-2 |
| DynamoDB `choosey` (on-demand, PITR, deletion protection) | ap-southeast-2 |
| CloudFront + ACM cert | global / us-east-1 |
| DNS (apex + www CNAME → CloudFront, ACM validation, CAA) | Cloudflare, DNS-only |
| GitHub OIDC provider + deploy role (UpdateFunctionCode on this function only) | IAM |
| $5/month budget alert | AWS Budgets |

## Origin protection
The Function URL is public (`NONE` auth) but CloudFront adds a random `x-origin-verify`
header and the app rejects requests without it (constant-time compare; refuses to start
in Lambda without it). OAC with `AWS_IAM` auth was rejected because it needs browsers to
send a SHA-256 of every POST body, which plain HTML forms can't do. Direct hits on the
Function URL get a 403, and reserved concurrency caps what they can cost.

The secret lives in Terraform state and the Lambda environment, never in git.
Rotate: `terraform apply -replace=random_password.origin_verify`.

## One-time setup
1. **State bucket**: `choosey-tfstate-<account-id>` in ap-southeast-2, versioned, SSE-S3,
   public access blocked, TLS-only bucket policy. Create it with:
   ```sh
   B=choosey-tfstate-<account-id>; R=ap-southeast-2
   aws s3api create-bucket --bucket $B --region $R --create-bucket-configuration LocationConstraint=$R --object-ownership BucketOwnerEnforced
   aws s3api put-public-access-block --bucket $B --public-access-block-configuration BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true
   aws s3api put-bucket-versioning --bucket $B --versioning-configuration Status=Enabled
   aws s3api put-bucket-encryption --bucket $B --server-side-encryption-configuration '{"Rules":[{"ApplyServerSideEncryptionByDefault":{"SSEAlgorithm":"AES256"},"BucketKeyEnabled":true}]}'
   aws s3api put-bucket-policy --bucket $B --policy "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"DenyInsecureTransport\",\"Effect\":\"Deny\",\"Principal\":\"*\",\"Action\":\"s3:*\",\"Resource\":[\"arn:aws:s3:::$B\",\"arn:aws:s3:::$B/*\"],\"Condition\":{\"Bool\":{\"aws:SecureTransport\":\"false\"}}}]}"
   cp backend.hcl.example backend.hcl   # then set bucket = "$B"
   ```
2. **Domain**: register `choosey.app` in Cloudflare Registrar (creates the zone).
3. **Cloudflare API token**: My Profile → API Tokens → Create Token → "Edit zone DNS"
   template, zone resources limited to `choosey.app` only. Export it; never commit it:
   `export CLOUDFLARE_API_TOKEN=...`
4. `cp terraform.tfvars.example terraform.tfvars` and set `budget_alert_email`.

## Apply
```sh
cd infra
terraform init -backend-config=backend.hcl
terraform plan -out tfplan
terraform apply tfplan
```
Then deploy the real build once by hand (CI does it after that):
```sh
cargo lambda build --release --arm64 --output-format zip -p choosey-web
aws lambda update-function-code --function-name choosey --zip-file fileb://target/lambda/choosey/bootstrap.zip --publish
```
And wire up CI:
```sh
gh api -X PUT repos/phyxphysio/choosey/environments/production \
  --input - <<<'{"deployment_branch_policy":{"protected_branches":false,"custom_branch_policies":true}}'
gh api -X POST repos/phyxphysio/choosey/environments/production/deployment-branch-policies -f name=main -f type=branch
gh variable set AWS_DEPLOY_ROLE_ARN --body "$(terraform output -raw deploy_role_arn)"
gh variable set AWS_REGION --body "$(terraform output -raw aws_region)"
gh variable set LAMBDA_FUNCTION_NAME --body "$(terraform output -raw lambda_function_name)"
```
The deploy role only trusts the repo's immutable OIDC subject (`repo:phyxphysio@<owner-id>/choosey@<repo-id>:environment:production`), and that
environment only accepts deployments from `main`.

## Not included (cost)
No WAF (≈$6+/month). Rate limiting is effectively reserved concurrency + the budget alert.
