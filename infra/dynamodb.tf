# One item per decision, keyed by pk = "DEC#<view_id>".
resource "aws_dynamodb_table" "decisions" {
  name         = var.app_name
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "pk"

  attribute {
    name = "pk"
    type = "S"
  }

  # Encryption at rest is always on (AWS-owned key, free).
  point_in_time_recovery {
    enabled = true
  }

  deletion_protection_enabled = true
}
