terraform {
  required_version = ">= 1.10"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
    cloudflare = {
      source  = "cloudflare/cloudflare"
      version = "~> 5.0"
    }
    random = {
      source  = "hashicorp/random"
      version = "~> 3.6"
    }
    archive = {
      source  = "hashicorp/archive"
      version = "~> 2.4"
    }
  }

  # State bucket is created once by hand (see infra/README.md). Its name
  # contains the AWS account ID, so it is passed at init time from a
  # gitignored backend.hcl: terraform init -backend-config=backend.hcl
  # Native S3 locking (use_lockfile) means no DynamoDB lock table is needed.
  backend "s3" {
    key          = "choosey/prod.tfstate"
    encrypt      = true
    use_lockfile = true
  }
}
