# Optional S3 bucket + lifecycle for contract-state backups.
# Apply from infrastructure/backup after configuring AWS credentials.

terraform {
  required_version = ">= 1.5.0"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = ">= 5.0"
    }
  }
}

variable "backup_bucket_name" {
  type        = string
  description = "S3 bucket that stores nebula_backup_*.tar.gz archives"
}

variable "retention_days" {
  type        = number
  description = "Expire backup objects after this many days"
  default     = 90
}

resource "aws_s3_bucket" "contract_backups" {
  bucket = var.backup_bucket_name
}

resource "aws_s3_bucket_versioning" "contract_backups" {
  bucket = aws_s3_bucket.contract_backups.id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "contract_backups" {
  bucket = aws_s3_bucket.contract_backups.id

  rule {
    id     = "expire-horizon-snapshots"
    status = "Enabled"

    filter {
      prefix = "backups/"
    }

    expiration {
      days = var.retention_days
    }

    noncurrent_version_expiration {
      noncurrent_days = var.retention_days
    }
  }
}

output "backup_bucket" {
  value = aws_s3_bucket.contract_backups.bucket
}
