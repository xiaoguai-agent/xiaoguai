variable "project" {
  description = "Project name prefix."
  type        = string
}

variable "db_username" {
  description = "Postgres master username stored in the generated secret."
  type        = string
}

variable "db_name" {
  description = "Postgres database name stored in the generated secret."
  type        = string
}

variable "llm_secrets_arn" {
  description = "ARN of a pre-existing Secrets Manager secret holding LLM API keys. When empty, a placeholder secret is created."
  type        = string
  default     = ""
}
