output "db_endpoint" {
  description = "RDS writer hostname (no port)."
  value       = aws_db_instance.main.address
}

output "db_port" {
  description = "RDS port (always 5432 for Postgres)."
  value       = aws_db_instance.main.port
}

output "db_sg_id" {
  description = "Security group ID of the RDS instance."
  value       = aws_security_group.db.id
}
