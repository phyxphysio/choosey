locals {
  # https://abc.lambda-url.region.on.aws/ -> abc.lambda-url.region.on.aws
  function_url_host = trimsuffix(trimprefix(aws_lambda_function_url.app.function_url, "https://"), "/")
}

data "aws_cloudfront_cache_policy" "disabled" {
  name = "Managed-CachingDisabled"
}

data "aws_cloudfront_cache_policy" "optimized" {
  name = "Managed-CachingOptimized"
}

# Forwards everything except Host (the Function URL needs its own Host).
data "aws_cloudfront_origin_request_policy" "all_except_host" {
  name = "Managed-AllViewerExceptHostHeader"
}

# Runs at the edge on every request, before anything reaches Lambda:
# 1. www.<domain> -> 301 to the apex, so there's one canonical origin.
# 2. Anything that isn't one of the app's routes gets a 404 right here. Scanners probing
#    for /.env, /wp-admin etc. then never invoke Lambda, so they can't use up its small
#    reserved concurrency and get real users throttled.
#    Keep this list in sync with the routes in crates/web/src/app.rs.
resource "aws_cloudfront_function" "viewer_request" {
  name    = "${var.app_name}-viewer-request"
  runtime = "cloudfront-js-2.0"
  publish = true
  comment = "www redirect + edge 404 for non-app paths"
  code    = <<-JS
    function handler(event) {
      var req = event.request;
      var host = req.headers.host && req.headers.host.value;
      if (host === "${local.www_domain}") {
        return {
          statusCode: 301,
          statusDescription: "Moved Permanently",
          headers: { location: { value: "https://${var.domain}" + req.uri } }
        };
      }
      var uri = req.uri;
      if (uri === "/" || uri === "/d" || uri.indexOf("/d/") === 0 || uri.indexOf("/static/") === 0) {
        return req;
      }
      return {
        statusCode: 404,
        statusDescription: "Not Found",
        headers: {
          "content-type": { value: "text/plain; charset=utf-8" },
          "cache-control": { value: "no-store" },
          "x-content-type-options": { value: "nosniff" }
        },
        body: { encoding: "text", data: "Not found" }
      };
    }
  JS

  # The distribution references this function, so the new one must exist before the old
  # one can be deleted.
  lifecycle {
    create_before_destroy = true
  }
}

moved {
  from = aws_cloudfront_function.www_redirect
  to   = aws_cloudfront_function.viewer_request
}

# Defence in depth: the app sets these too; override=false keeps the app's values.
resource "aws_cloudfront_response_headers_policy" "security" {
  name = "${var.app_name}-security-headers"

  security_headers_config {
    strict_transport_security {
      access_control_max_age_sec = 63072000
      include_subdomains         = true
      preload                    = true
      override                   = false
    }
    content_type_options {
      override = false
    }
    frame_options {
      frame_option = "DENY"
      override     = false
    }
    referrer_policy {
      referrer_policy = "no-referrer"
      override        = false
    }
  }

  # Don't advertise the origin.
  remove_headers_config {
    items {
      header = "server"
    }
  }
}

resource "aws_cloudfront_distribution" "app" {
  enabled         = true
  is_ipv6_enabled = true
  http_version    = "http2and3"
  price_class     = "PriceClass_All" # PriceClass_100 has no Australian edge locations
  aliases         = [var.domain, local.www_domain]
  comment         = var.app_name

  origin {
    origin_id   = "lambda"
    domain_name = local.function_url_host

    custom_origin_config {
      http_port              = 80
      https_port             = 443
      origin_protocol_policy = "https-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }

    custom_header {
      name  = "x-origin-verify"
      value = random_password.origin_verify.result
    }
  }

  # Dynamic pages: never cached.
  default_cache_behavior {
    target_origin_id           = "lambda"
    viewer_protocol_policy     = "redirect-to-https"
    allowed_methods            = ["GET", "HEAD", "OPTIONS", "PUT", "POST", "PATCH", "DELETE"]
    cached_methods             = ["GET", "HEAD"]
    compress                   = true
    cache_policy_id            = data.aws_cloudfront_cache_policy.disabled.id
    origin_request_policy_id   = data.aws_cloudfront_origin_request_policy.all_except_host.id
    response_headers_policy_id = aws_cloudfront_response_headers_policy.security.id

    function_association {
      event_type   = "viewer-request"
      function_arn = aws_cloudfront_function.viewer_request.arn
    }
  }

  # Static assets: cached at the edge.
  ordered_cache_behavior {
    path_pattern               = "/static/*"
    target_origin_id           = "lambda"
    viewer_protocol_policy     = "redirect-to-https"
    allowed_methods            = ["GET", "HEAD"]
    cached_methods             = ["GET", "HEAD"]
    compress                   = true
    cache_policy_id            = data.aws_cloudfront_cache_policy.optimized.id
    response_headers_policy_id = aws_cloudfront_response_headers_policy.security.id

    function_association {
      event_type   = "viewer-request"
      function_arn = aws_cloudfront_function.viewer_request.arn
    }
  }

  restrictions {
    geo_restriction {
      restriction_type = "none"
    }
  }

  viewer_certificate {
    acm_certificate_arn      = aws_acm_certificate_validation.this.certificate_arn
    ssl_support_method       = "sni-only"
    minimum_protocol_version = "TLSv1.2_2021"
  }
}
