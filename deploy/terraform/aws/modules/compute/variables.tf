variable "project" {
  description = "Project name prefix."
  type        = string
}

variable "region" {
  description = "AWS region (for CloudWatch log configuration)."
  type        = string
}

variable "vpc_id" {
  description = "ID of the VPC."
  type        = string
}

variable "public_subnet_ids" {
  description = "Public subnet IDs for the ALB."
  type        = list(string)
}

variable "private_subnet_ids" {
  description = "Private subnet IDs for ECS tasks."
  type        = list(string)
}

variable "container_image" {
  description = "Full container image URI for xiaoguai-core."
  type        = string
}

variable "instance_count" {
  description = "Desired number of ECS tasks."
  type        = number
  default     = 2
}

variable "task_cpu" {
  description = "ECS task CPU units."
  type        = number
  default     = 512
}

variable "task_memory_mb" {
  description = "ECS task memory in MiB."
  type        = number
  default     = 1024
}

variable "log_retention_days" {
  description = "CloudWatch log retention in days."
  type        = number
  default     = 30
}

variable "db_host" {
  description = "RDS writer hostname."
  type        = string
}

variable "db_port" {
  description = "RDS port."
  type        = number
  default     = 5432
}

variable "db_name" {
  description = "Postgres database name."
  type        = string
}

variable "db_secret_arn" {
  description = "ARN of the Secrets Manager secret holding Postgres credentials."
  type        = string
}

variable "redis_endpoint" {
  description = "ElastiCache configuration endpoint (host:port)."
  type        = string
}

variable "llm_secrets_arn" {
  description = "ARN of the Secrets Manager secret holding LLM API keys."
  type        = string
}
