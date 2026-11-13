provider "aws" {
  region = var.aws_region

  default_tags {
    tags = {
      app       = var.app_name
      managedby = "terraform"
    }
  }
}

# CloudFront only accepts ACM certificates from us-east-1.
provider "aws" {
  alias  = "us_east_1"
  region = "us-east-1"

  default_tags {
    tags = {
      app       = var.app_name
      managedby = "terraform"
    }
  }
}

# Reads the API token from the CLOUDFLARE_API_TOKEN environment variable.
# Never put the token in a .tf or committed .tfvars file.
provider "cloudflare" {}
