output "configuration_endpoint" {
  description = "ElastiCache cluster configuration endpoint (host:port). Use this with the Redis cluster-mode client."
  value       = "${aws_elasticache_replication_group.main.configuration_endpoint_address}:6379"
}

output "auth_token_secret" {
  description = "The ElastiCache AUTH token (stored in Terraform state; consider moving to Secrets Manager)."
  value       = random_password.auth_token.result
  sensitive   = true
}

output "cache_sg_id" {
  description = "Security group ID of the ElastiCache cluster."
  value       = aws_security_group.cache.id
}
