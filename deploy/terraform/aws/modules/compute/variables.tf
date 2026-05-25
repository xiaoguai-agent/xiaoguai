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

# =============================================================================
# Wave-3 — HotL
# =============================================================================

variable "hotl_enabled" {
  description = "Enable the HotL enforcement engine."
  type        = bool
  default     = false
}

variable "hotl_policy_store_backend" {
  description = "HotL policy store backend: 'pg' or 'in-mem'."
  type        = string
  default     = "pg"
}

variable "hotl_enforcement_enabled" {
  description = "HotL enforcement mode (true) vs audit-only (false)."
  type        = bool
  default     = false
}

variable "hotl_slack_escalation_webhook_url" {
  description = "Slack channel webhook URL for HotL escalation."
  type        = string
  default     = ""
}

variable "hotl_email_escalation_address" {
  description = "E-mail address for HotL escalation alerts."
  type        = string
  default     = ""
}

variable "hotl_webhook_escalation_url" {
  description = "Generic webhook URL for HotL policy violation events."
  type        = string
  default     = ""
}

# =============================================================================
# Wave-3 — Outcomes
# =============================================================================

variable "outcomes_backend" {
  description = "Outcomes recorder backend: 'pg' or 'in-mem'."
  type        = string
  default     = "in-mem"
}

variable "outcomes_retention_days" {
  description = "Days to retain outcome records before expiry."
  type        = number
  default     = 30
}

variable "outcomes_timeseries_bucket_seconds" {
  description = "Bucket size in seconds for outcomes time-series aggregation."
  type        = number
  default     = 300
}

# =============================================================================
# Wave-3 — Skills / packs
# =============================================================================

variable "skills_packs_config_path" {
  description = "Container path to the packs config file."
  type        = string
  default     = ""
}

variable "skills_install_allowlist" {
  description = "Comma-separated pack IDs allowed to install."
  type        = string
  default     = ""
}

variable "skills_auto_install_on_startup" {
  description = "Auto-install allow-listed packs on startup."
  type        = bool
  default     = false
}

# =============================================================================
# Wave-3 — Rate limiting
# =============================================================================

variable "rate_limit_enabled" {
  description = "Enable the rate-limit middleware."
  type        = bool
  default     = false
}

variable "rate_limit_backend" {
  description = "Rate-limit backend: 'in-mem' or 'redis'."
  type        = string
  default     = "in-mem"
}

variable "rate_limit_per_tenant_requests" {
  description = "Max requests per tenant per window."
  type        = number
  default     = 100
}

variable "rate_limit_per_tenant_window_seconds" {
  description = "Per-tenant window duration in seconds."
  type        = number
  default     = 60
}

variable "rate_limit_per_route_requests" {
  description = "Per-route request limit per window."
  type        = number
  default     = 30
}

variable "rate_limit_per_route_window_seconds" {
  description = "Per-route window duration in seconds."
  type        = number
  default     = 10
}

# =============================================================================
# Wave-3 — Cloud LLM v2
# =============================================================================

variable "bedrock_region" {
  description = "AWS region for Bedrock API calls."
  type        = string
  default     = "us-east-1"
}

variable "bedrock_auth_mode" {
  description = "Bedrock auth mode: 'irsa' or 'env-keys'."
  type        = string
  default     = "irsa"
}

variable "bedrock_secrets_arn" {
  description = "Secrets Manager ARN with BEDROCK_AWS_ACCESS_KEY_ID and BEDROCK_AWS_SECRET_ACCESS_KEY. Required when bedrock_auth_mode='env-keys'."
  type        = string
  default     = ""
  sensitive   = true
}

variable "azure_openai_endpoint" {
  description = "Azure OpenAI resource endpoint URL."
  type        = string
  default     = ""
}

variable "azure_openai_deployment_id" {
  description = "Azure OpenAI deployment / model name."
  type        = string
  default     = ""
}

variable "azure_openai_api_version" {
  description = "Azure OpenAI API version string."
  type        = string
  default     = "2024-02-01"
}

variable "azure_openai_secrets_arn" {
  description = "Secrets Manager ARN with AZURE_OPENAI_API_KEY."
  type        = string
  default     = ""
  sensitive   = true
}

variable "mistral_api_base" {
  description = "Mistral API base URL (empty = public default)."
  type        = string
  default     = ""
}

variable "mistral_secrets_arn" {
  description = "Secrets Manager ARN with MISTRAL_API_KEY."
  type        = string
  default     = ""
  sensitive   = true
}

variable "groq_api_base" {
  description = "Groq API base URL (empty = public default)."
  type        = string
  default     = ""
}

variable "groq_secrets_arn" {
  description = "Secrets Manager ARN with GROQ_API_KEY."
  type        = string
  default     = ""
  sensitive   = true
}

# =============================================================================
# Wave-3 — Observability
# =============================================================================

variable "otel_enabled" {
  description = "Enable OTLP trace/metric export."
  type        = bool
  default     = false
}

variable "otel_endpoint" {
  description = "OTLP gRPC collector endpoint, e.g. http://otel-collector:4317."
  type        = string
  default     = ""
}

variable "otel_traces_sampling_ratio" {
  description = "Fraction of traces to sample (0.0–1.0)."
  type        = string
  default     = "0.1"
}

variable "otel_service_name" {
  description = "Service name reported in OTLP spans."
  type        = string
  default     = "xiaoguai"
}

variable "prometheus_enabled" {
  description = "Expose a /metrics Prometheus scrape endpoint."
  type        = bool
  default     = false
}

variable "prometheus_listen_addr" {
  description = "Bind address for the Prometheus scrape listener."
  type        = string
  default     = "0.0.0.0:9090"
}

# =============================================================================
# Wave-3 — IM adapters
# =============================================================================

variable "im_discord_enabled" {
  description = "Enable the Discord bot adapter."
  type        = bool
  default     = false
}

variable "im_discord_channel_allowlist" {
  description = "Comma-separated Discord channel IDs allowed."
  type        = string
  default     = ""
}

variable "im_discord_webhook_signing_enabled" {
  description = "Enable Discord interaction webhook signature verification."
  type        = bool
  default     = false
}

variable "im_discord_secrets_arn" {
  description = "Secrets Manager ARN with DISCORD_BOT_TOKEN (and optionally DISCORD_WEBHOOK_SECRET)."
  type        = string
  default     = ""
  sensitive   = true
}

variable "im_telegram_enabled" {
  description = "Enable the Telegram bot adapter."
  type        = bool
  default     = false
}

variable "im_telegram_chat_allowlist" {
  description = "Comma-separated Telegram chat IDs allowed."
  type        = string
  default     = ""
}

variable "im_telegram_secrets_arn" {
  description = "Secrets Manager ARN with TELEGRAM_BOT_TOKEN."
  type        = string
  default     = ""
  sensitive   = true
}

variable "im_mattermost_enabled" {
  description = "Enable the Mattermost bot adapter."
  type        = bool
  default     = false
}

variable "im_mattermost_server_url" {
  description = "Mattermost server URL, e.g. https://mattermost.example.com."
  type        = string
  default     = ""
}

variable "im_mattermost_channel_allowlist" {
  description = "Comma-separated Mattermost channel IDs allowed."
  type        = string
  default     = ""
}

variable "im_mattermost_secrets_arn" {
  description = "Secrets Manager ARN with MATTERMOST_BOT_TOKEN."
  type        = string
  default     = ""
  sensitive   = true
}

variable "im_slack_enabled" {
  description = "Enable the Slack bot adapter."
  type        = bool
  default     = false
}

variable "im_slack_channel_allowlist" {
  description = "Comma-separated Slack channel IDs allowed."
  type        = string
  default     = ""
}

variable "im_slack_secrets_arn" {
  description = "Secrets Manager ARN with SLACK_BOT_TOKEN and SLACK_SIGNING_SECRET."
  type        = string
  default     = ""
  sensitive   = true
}
