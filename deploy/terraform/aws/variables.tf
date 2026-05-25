variable "region" {
  description = "AWS region to deploy into (e.g. us-east-1, ap-southeast-1)."
  type        = string
  default     = "us-east-1"
}

variable "project" {
  description = "Short project name used as a name prefix for all resources."
  type        = string
  default     = "xiaoguai"

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,18}[a-z0-9]$", var.project))
    error_message = "project must be lowercase alphanumeric + hyphens, 3-20 characters."
  }
}

variable "vpc_cidr" {
  description = "CIDR block for the VPC (must be /16 or smaller)."
  type        = string
  default     = "10.0.0.0/16"
}

variable "db_instance_class" {
  description = "RDS instance class for the Postgres primary and standby."
  type        = string
  default     = "db.t4g.medium"
}

variable "db_name" {
  description = "Postgres database name created at provision time."
  type        = string
  default     = "xiaoguai"
}

variable "db_username" {
  description = "Postgres master username (stored in Secrets Manager, not in tfstate as plaintext)."
  type        = string
  default     = "xiaoguai"
}

variable "redis_node_type" {
  description = "ElastiCache node type for the Valkey/Redis cluster."
  type        = string
  default     = "cache.t4g.medium"
}

variable "redis_num_shards" {
  description = "Number of shards in the ElastiCache cluster-mode cluster (1–90)."
  type        = number
  default     = 1

  validation {
    condition     = var.redis_num_shards >= 1 && var.redis_num_shards <= 90
    error_message = "redis_num_shards must be between 1 and 90."
  }
}

variable "redis_replicas_per_shard" {
  description = "Read replicas per shard (0–5). Use ≥1 for HA."
  type        = number
  default     = 1

  validation {
    condition     = var.redis_replicas_per_shard >= 0 && var.redis_replicas_per_shard <= 5
    error_message = "redis_replicas_per_shard must be between 0 and 5."
  }
}

variable "instance_count" {
  description = "Desired number of xiaoguai-core ECS Fargate tasks."
  type        = number
  default     = 2

  validation {
    condition     = var.instance_count >= 1
    error_message = "instance_count must be at least 1."
  }
}

variable "task_cpu" {
  description = "ECS task CPU units (256/512/1024/2048/4096)."
  type        = number
  default     = 512
}

variable "task_memory_mb" {
  description = "ECS task memory in MiB."
  type        = number
  default     = 1024
}

variable "container_image" {
  description = "Full container image URI for xiaoguai-core (e.g. 123456789.dkr.ecr.us-east-1.amazonaws.com/xiaoguai-core:v1.1.4)."
  type        = string
}

variable "llm_secrets_arn" {
  description = "ARN of an existing Secrets Manager secret that holds LLM API keys (e.g. {\"OPENAI_API_KEY\":\"sk-...\"}). Leave empty to provision a placeholder."
  type        = string
  default     = ""
}

variable "log_retention_days" {
  description = "CloudWatch log retention in days."
  type        = number
  default     = 30
}
