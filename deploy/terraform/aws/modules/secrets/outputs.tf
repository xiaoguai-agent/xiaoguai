output "db_password_secret_arn" {
  description = "ARN of the Secrets Manager secret holding the Postgres password."
  value       = aws_secretsmanager_secret.db_password.arn
  sensitive   = true
}

output "llm_secrets_arn" {
  description = "ARN of the Secrets Manager secret holding LLM API keys. Returns the pre-existing ARN when llm_secrets_arn is provided, or the placeholder ARN when created by this module."
  value       = var.llm_secrets_arn != "" ? var.llm_secrets_arn : aws_secretsmanager_secret.llm_keys_placeholder[0].arn
  sensitive   = true
}
