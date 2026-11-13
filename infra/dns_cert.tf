# The zone is created automatically when the domain is registered
# through Cloudflare Registrar.
data "cloudflare_zone" "this" {
  filter = {
    name = var.domain
  }
}

resource "aws_acm_certificate" "this" {
  provider                  = aws.us_east_1
  domain_name               = var.domain
  subject_alternative_names = [local.www_domain]
  validation_method         = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

resource "cloudflare_dns_record" "acm_validation" {
  for_each = {
    for dvo in aws_acm_certificate.this.domain_validation_options : dvo.domain_name => {
      name    = trimsuffix(dvo.resource_record_name, ".")
      content = trimsuffix(dvo.resource_record_value, ".")
      type    = dvo.resource_record_type
    }
  }

  zone_id = data.cloudflare_zone.this.id
  name    = each.value.name
  type    = each.value.type
  content = each.value.content
  ttl     = 300
  proxied = false
  comment = "ACM validation (terraform)"
}

resource "aws_acm_certificate_validation" "this" {
  provider                = aws.us_east_1
  certificate_arn         = aws_acm_certificate.this.arn
  validation_record_fqdns = [for r in cloudflare_dns_record.acm_validation : r.name]
}

# DNS only (grey cloud): CloudFront terminates TLS with the ACM cert.
# Cloudflare flattens the apex CNAME automatically.
resource "cloudflare_dns_record" "apex" {
  zone_id = data.cloudflare_zone.this.id
  name    = var.domain
  type    = "CNAME"
  content = aws_cloudfront_distribution.app.domain_name
  ttl     = 300
  proxied = false
  comment = "CloudFront (terraform)"
}

resource "cloudflare_dns_record" "www" {
  zone_id = data.cloudflare_zone.this.id
  name    = local.www_domain
  type    = "CNAME"
  content = aws_cloudfront_distribution.app.domain_name
  ttl     = 300
  proxied = false
  comment = "CloudFront (terraform)"
}

# Only Amazon may issue certificates for this domain.
resource "cloudflare_dns_record" "caa" {
  zone_id = data.cloudflare_zone.this.id
  name    = var.domain
  type    = "CAA"
  ttl     = 3600
  proxied = false
  data = {
    flags = 0
    tag   = "issue"
    value = "amazon.com"
  }
}
