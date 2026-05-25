output "alb_dns_name" {
  description = "DNS name of the Application Load Balancer."
  value       = aws_lb.main.dns_name
}

output "alb_zone_id" {
  description = "Hosted-zone ID of the ALB (for Route 53 A-alias records)."
  value       = aws_lb.main.zone_id
}

output "ecs_cluster_name" {
  description = "Name of the ECS cluster."
  value       = aws_ecs_cluster.main.name
}

output "ecs_service_name" {
  description = "Name of the ECS service."
  value       = aws_ecs_service.app.name
}

output "log_group_name" {
  description = "CloudWatch log group for xiaoguai-core container logs."
  value       = aws_cloudwatch_log_group.app.name
}

output "app_sg_id" {
  description = "Security group ID of the ECS tasks (used by DB and cache modules for ingress rules)."
  value       = aws_security_group.app.id
}
