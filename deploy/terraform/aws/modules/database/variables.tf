variable "project" {
  description = "Project name prefix."
  type        = string
}

variable "vpc_id" {
  description = "ID of the VPC."
  type        = string
}

variable "subnet_ids" {
  description = "Private subnet IDs for the DB subnet group (must span ≥2 AZs for Multi-AZ)."
  type        = list(string)
}

variable "app_sg_id" {
  description = "Security group ID of the ECS application tasks (granted Postgres ingress)."
  type        = string
}

variable "db_instance_class" {
  description = "RDS instance class."
  type        = string
}

variable "db_name" {
  description = "Postgres database name."
  type        = string
}

variable "db_username" {
  description = "Postgres master username."
  type        = string
}

variable "db_password_secret" {
  description = "ARN of the Secrets Manager secret containing the Postgres password (key: 'password')."
  type        = string
}
