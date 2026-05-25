output "alb_dns_name" {
  description = "DNS name of the Application Load Balancer. Point your CNAME / A-alias here."
  value       = module.compute.alb_dns_name
}

output "alb_zone_id" {
  description = "Hosted-zone ID of the ALB — required when creating an Route 53 A-alias record."
  value       = module.compute.alb_zone_id
}

output "db_endpoint" {
  description = "RDS writer endpoint (host:port). Accessed only from within the VPC."
  value       = "${module.database.db_endpoint}:${module.database.db_port}"
  sensitive   = false
}

output "redis_endpoint" {
  description = "ElastiCache cluster configuration endpoint (host:port)."
  value       = module.cache.configuration_endpoint
}

output "ecs_cluster_name" {
  description = "Name of the ECS cluster."
  value       = module.compute.ecs_cluster_name
}

output "ecs_service_name" {
  description = "Name of the ECS service (use with `aws ecs update-service` for forced redeploys)."
  value       = module.compute.ecs_service_name
}

output "log_group_name" {
  description = "CloudWatch log group for xiaoguai-core container logs."
  value       = module.compute.log_group_name
}

output "db_secret_arn" {
  description = "ARN of the Secrets Manager secret holding Postgres credentials."
  value       = module.secrets.db_password_secret_arn
  sensitive   = true
}

output "llm_secrets_arn" {
  description = "ARN of the Secrets Manager secret holding LLM API keys."
  value       = module.secrets.llm_secrets_arn
  sensitive   = true
}

# ---------------------------------------------------------------------------
# Wave-3 observability outputs
# ---------------------------------------------------------------------------

output "prometheus_scrape_addr" {
  description = "Prometheus scrape address configured on the ECS tasks (PROMETHEUS_LISTEN_ADDR). This is a container-local bind address; expose it via a dedicated NLB target group or a Prometheus agent running as a sidecar. Empty string when prometheus_enabled=false."
  value       = var.prometheus_enabled ? var.prometheus_listen_addr : ""
}

output "otel_endpoint" {
  description = "OTLP gRPC endpoint the application is configured to export traces and metrics to. Empty string when otel_enabled=false."
  value       = var.otel_enabled ? var.otel_endpoint : ""
}
