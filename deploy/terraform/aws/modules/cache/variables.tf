variable "project" {
  description = "Project name prefix."
  type        = string
}

variable "vpc_id" {
  description = "ID of the VPC."
  type        = string
}

variable "subnet_ids" {
  description = "Private subnet IDs for the cache subnet group."
  type        = list(string)
}

variable "app_sg_id" {
  description = "Security group ID of the ECS application tasks (granted Redis ingress)."
  type        = string
}

variable "redis_node_type" {
  description = "ElastiCache node type."
  type        = string
}

variable "redis_num_shards" {
  description = "Number of shards (num_node_groups) in cluster mode."
  type        = number
  default     = 1
}

variable "redis_replicas_per_shard" {
  description = "Replicas per shard (replicas_per_node_group). Use ≥1 for HA."
  type        = number
  default     = 1
}

variable "engine" {
  description = "Cache engine: 'valkey' (preferred) or 'redis'."
  type        = string
  default     = "valkey"
}

variable "engine_version" {
  description = "Engine version. For valkey: '8.0'; for redis: '7.1'."
  type        = string
  default     = "8.0"
}
