# ---------------------------------------------------------------------------
# Minimal example — single-region, single-AZ-NAT, smallest viable instances.
#
# This is NOT a production configuration. It trades HA for cost:
#   - db.t4g.small instead of db.t4g.medium (no Multi-AZ override; Multi-AZ
#     is always on in the database module for safety — set it here if you add
#     a module variable to disable it).
#   - 1 ECS task instead of 2.
#   - cache.t4g.micro with 0 replicas (no standby).
#
# Run:
#   terraform init
#   terraform plan -var-file="minimal.tfvars"
#   terraform apply -var-file="minimal.tfvars"
# ---------------------------------------------------------------------------

terraform {
  required_version = ">= 1.6.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
    random = {
      source  = "hashicorp/random"
      version = "~> 3.0"
    }
  }
}

provider "aws" {
  region = var.region
}

module "xiaoguai" {
  source = "../../"

  region          = var.region
  project         = var.project
  vpc_cidr        = "10.1.0.0/16"
  container_image = var.container_image

  # Smallest viable compute.
  db_instance_class = "db.t4g.small"
  redis_node_type   = "cache.t4g.micro"

  # Single task — no redundancy.
  instance_count           = 1
  redis_replicas_per_shard = 0

  task_cpu       = 256
  task_memory_mb = 512
}

variable "region" {
  description = "AWS region."
  type        = string
  default     = "us-east-1"
}

variable "project" {
  description = "Project name prefix."
  type        = string
  default     = "xiaoguai-dev"
}

variable "container_image" {
  description = "Container image URI for xiaoguai-core."
  type        = string
}

output "alb_dns_name" {
  value = module.xiaoguai.alb_dns_name
}
