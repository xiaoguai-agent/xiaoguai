variable "region" {
  description = "AWS region to deploy into (e.g. us-east-1, ap-southeast-1)."
  type        = string
  default     = "us-east-1"
}

variable "project" {
  description = "Short project name used as a name prefix for all resources."
  type        = string
  default     = "xiaoguai"

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,18}[a-z0-9]$", var.project))
    error_message = "project must be lowercase alphanumeric + hyphens, 3-20 characters."
  }
}

variable "vpc_cidr" {
  description = "CIDR block for the VPC (must be /16 or smaller)."
  type        = string
  default     = "10.0.0.0/16"
}

variable "db_instance_class" {
  description = "RDS instance class for the Postgres primary and standby."
  type        = string
  default     = "db.t4g.medium"
}

variable "db_name" {
  description = "Postgres database name created at provision time."
  type        = string
  default     = "xiaoguai"
}

variable "db_username" {
  description = "Postgres master username (stored in Secrets Manager, not in tfstate as plaintext)."
  type        = string
  default     = "xiaoguai"
}

variable "redis_node_type" {
  description = "ElastiCache node type for the Valkey/Redis cluster."
  type        = string
  default     = "cache.t4g.medium"
}

variable "redis_num_shards" {
  description = "Number of shards in the ElastiCache cluster-mode cluster (1–90)."
  type        = number
  default     = 1

  validation {
    condition     = var.redis_num_shards >= 1 && var.redis_num_shards <= 90
    error_message = "redis_num_shards must be between 1 and 90."
  }
}

variable "redis_replicas_per_shard" {
  description = "Read replicas per shard (0–5). Use ≥1 for HA."
  type        = number
  default     = 1

  validation {
    condition     = var.redis_replicas_per_shard >= 0 && var.redis_replicas_per_shard <= 5
    error_message = "redis_replicas_per_shard must be between 0 and 5."
  }
}

variable "instance_count" {
  description = "Desired number of xiaoguai-core ECS Fargate tasks."
  type        = number
  default     = 2

  validation {
    condition     = var.instance_count >= 1
    error_message = "instance_count must be at least 1."
  }
}

variable "task_cpu" {
  description = "ECS task CPU units (256/512/1024/2048/4096)."
  type        = number
  default     = 512
}

variable "task_memory_mb" {
  description = "ECS task memory in MiB."
  type        = number
  default     = 1024
}

variable "container_image" {
  description = "Full container image URI for xiaoguai-core (e.g. 123456789.dkr.ecr.us-east-1.amazonaws.com/xiaoguai-core:v1.1.4)."
  type        = string
}

variable "llm_secrets_arn" {
  description = "ARN of an existing Secrets Manager secret that holds LLM API keys (e.g. {\"OPENAI_API_KEY\":\"sk-...\"}). Leave empty to provision a placeholder."
  type        = string
  default     = ""
}

variable "log_retention_days" {
  description = "CloudWatch log retention in days."
  type        = number
  default     = 30
}

# =============================================================================
# Wave-3 — HotL (policy store + enforcement engine)
# =============================================================================

variable "hotl_enabled" {
  description = "Enable the HotL enforcement engine (HOTL_ENABLED env var)."
  type        = bool
  default     = false
}

variable "hotl_policy_store_backend" {
  description = "HotL policy store backend: 'pg' (Postgres, recommended) or 'in-mem'."
  type        = string
  default     = "pg"

  validation {
    condition     = contains(["pg", "in-mem"], var.hotl_policy_store_backend)
    error_message = "hotl_policy_store_backend must be 'pg' or 'in-mem'."
  }
}

variable "hotl_enforcement_enabled" {
  description = "When true, HotL decisions are enforced; false = audit-only mode."
  type        = bool
  default     = false
}

variable "hotl_slack_escalation_webhook_url" {
  description = "Slack channel webhook URL for HotL policy violation escalation (non-secret URL; leave empty to disable)."
  type        = string
  default     = ""
}

variable "hotl_email_escalation_address" {
  description = "E-mail address to receive HotL escalation alerts (leave empty to disable)."
  type        = string
  default     = ""
}

variable "hotl_webhook_escalation_url" {
  description = "Generic webhook URL for HotL policy violation events (leave empty to disable)."
  type        = string
  default     = ""
}

# =============================================================================
# Wave-3 — Outcomes (agent run recorder)
# =============================================================================

variable "outcomes_backend" {
  description = "Outcomes recorder backend: 'pg' (Postgres, durable) or 'in-mem' (ephemeral)."
  type        = string
  default     = "in-mem"

  validation {
    condition     = contains(["pg", "in-mem"], var.outcomes_backend)
    error_message = "outcomes_backend must be 'pg' or 'in-mem'."
  }
}

variable "outcomes_retention_days" {
  description = "Number of days to retain outcome records before expiry."
  type        = number
  default     = 30
}

variable "outcomes_timeseries_bucket_seconds" {
  description = "Bucket size in seconds for outcomes time-series aggregation windows."
  type        = number
  default     = 300
}

# =============================================================================
# Wave-3 — Skills / packs
# =============================================================================

variable "skills_packs_config_path" {
  description = "Absolute path inside the container to the packs config file. Mount a ConfigMap or EFS volume there for custom packs. Leave empty to use defaults."
  type        = string
  default     = ""
}

variable "skills_install_allowlist" {
  description = "Comma-separated list of pack IDs allowed to be installed. Empty string = allow all packs in packs_config_path."
  type        = string
  default     = ""
}

variable "skills_auto_install_on_startup" {
  description = "When true, packs in the allow-list are auto-installed on container startup."
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
  description = "Rate-limit backend: 'in-mem' (single-replica only) or 'redis' (uses the cache endpoint)."
  type        = string
  default     = "in-mem"

  validation {
    condition     = contains(["in-mem", "redis"], var.rate_limit_backend)
    error_message = "rate_limit_backend must be 'in-mem' or 'redis'."
  }
}

variable "rate_limit_per_tenant_requests" {
  description = "Maximum requests per tenant per rate-limit window."
  type        = number
  default     = 100
}

variable "rate_limit_per_tenant_window_seconds" {
  description = "Duration in seconds of the per-tenant rate-limit window."
  type        = number
  default     = 60
}

variable "rate_limit_per_route_requests" {
  description = "Per-route request limit per window (overrides per-tenant for matching routes)."
  type        = number
  default     = 30
}

variable "rate_limit_per_route_window_seconds" {
  description = "Duration in seconds of the per-route rate-limit window."
  type        = number
  default     = 10
}

# =============================================================================
# Wave-3 — Cloud LLM v2 providers (non-secret config)
# Credentials are stored in Secrets Manager ARNs below.
# =============================================================================

variable "bedrock_region" {
  description = "AWS region for Amazon Bedrock API calls (BEDROCK_REGION)."
  type        = string
  default     = "us-east-1"
}

variable "bedrock_auth_mode" {
  description = "Bedrock auth mode: 'irsa' (pod IAM role, recommended) or 'env-keys' (explicit access key from bedrock_secrets_arn)."
  type        = string
  default     = "irsa"

  validation {
    condition     = contains(["irsa", "env-keys"], var.bedrock_auth_mode)
    error_message = "bedrock_auth_mode must be 'irsa' or 'env-keys'."
  }
}

variable "bedrock_secrets_arn" {
  description = "ARN of a Secrets Manager secret with keys BEDROCK_AWS_ACCESS_KEY_ID and BEDROCK_AWS_SECRET_ACCESS_KEY. Required when bedrock_auth_mode='env-keys'; ignored for 'irsa'."
  type        = string
  default     = ""
  sensitive   = true
}

variable "azure_openai_endpoint" {
  description = "Azure OpenAI resource endpoint, e.g. https://<resource>.openai.azure.com/ (AZURE_OPENAI_ENDPOINT)."
  type        = string
  default     = ""
}

variable "azure_openai_deployment_id" {
  description = "Azure OpenAI deployment / model name (AZURE_OPENAI_DEPLOYMENT_ID)."
  type        = string
  default     = ""
}

variable "azure_openai_api_version" {
  description = "Azure OpenAI API version string, e.g. '2024-02-01' (AZURE_OPENAI_API_VERSION)."
  type        = string
  default     = "2024-02-01"
}

variable "azure_openai_secrets_arn" {
  description = "ARN of a Secrets Manager secret with key AZURE_OPENAI_API_KEY. Leave empty to skip Azure OpenAI."
  type        = string
  default     = ""
  sensitive   = true
}

variable "mistral_api_base" {
  description = "Mistral API base URL (MISTRAL_API_BASE). Leave empty to use the default public endpoint."
  type        = string
  default     = ""
}

variable "mistral_secrets_arn" {
  description = "ARN of a Secrets Manager secret with key MISTRAL_API_KEY. Leave empty to skip Mistral."
  type        = string
  default     = ""
  sensitive   = true
}

variable "groq_api_base" {
  description = "Groq API base URL (GROQ_API_BASE). Leave empty to use the default public endpoint."
  type        = string
  default     = ""
}

variable "groq_secrets_arn" {
  description = "ARN of a Secrets Manager secret with key GROQ_API_KEY. Leave empty to skip Groq."
  type        = string
  default     = ""
  sensitive   = true
}

# =============================================================================
# Wave-3 — Observability (OpenTelemetry + Prometheus)
# =============================================================================

variable "otel_enabled" {
  description = "Enable OTLP trace/metric export (OTEL_ENABLED)."
  type        = bool
  default     = false
}

variable "otel_endpoint" {
  description = "OTLP gRPC collector endpoint, e.g. http://otel-collector:4317 (OTEL_EXPORTER_OTLP_ENDPOINT)."
  type        = string
  default     = ""
}

variable "otel_traces_sampling_ratio" {
  description = "Fraction of traces to sample, between 0.0 and 1.0 (OTEL_TRACES_SAMPLER_ARG)."
  type        = string
  default     = "0.1"
}

variable "otel_service_name" {
  description = "Service name reported in OTLP spans (OTEL_SERVICE_NAME)."
  type        = string
  default     = "xiaoguai"
}

variable "prometheus_enabled" {
  description = "Expose a /metrics Prometheus scrape endpoint (PROMETHEUS_ENABLED)."
  type        = bool
  default     = false
}

variable "prometheus_listen_addr" {
  description = "Bind address for the Prometheus scrape listener (PROMETHEUS_LISTEN_ADDR). Exposes port 9090 by default."
  type        = string
  default     = "0.0.0.0:9090"
}

# =============================================================================
# Wave-3 — IM adapters (Discord, Telegram, Mattermost, Slack)
# Credentials are always stored in Secrets Manager; supply ARNs here.
# =============================================================================

variable "im_discord_enabled" {
  description = "Enable the Discord bot adapter."
  type        = bool
  default     = false
}

variable "im_discord_channel_allowlist" {
  description = "Comma-separated Discord channel IDs the bot is allowed to post in. Empty = all accessible channels."
  type        = string
  default     = ""
}

variable "im_discord_webhook_signing_enabled" {
  description = "Enable Discord interaction webhook signature verification (DISCORD_WEBHOOK_SIGNING_ENABLED)."
  type        = bool
  default     = false
}

variable "im_discord_secrets_arn" {
  description = "ARN of a Secrets Manager secret with key DISCORD_BOT_TOKEN (and optionally DISCORD_WEBHOOK_SECRET when webhook_signing_enabled=true). Leave empty when discord is disabled."
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
  description = "Comma-separated Telegram chat IDs (groups or users) the bot is allowed to respond in. Empty = all."
  type        = string
  default     = ""
}

variable "im_telegram_secrets_arn" {
  description = "ARN of a Secrets Manager secret with key TELEGRAM_BOT_TOKEN. Leave empty when telegram is disabled."
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
  description = "Mattermost server URL, e.g. https://mattermost.example.com (MATTERMOST_SERVER_URL)."
  type        = string
  default     = ""
}

variable "im_mattermost_channel_allowlist" {
  description = "Comma-separated Mattermost channel IDs the bot is allowed to post in. Empty = all."
  type        = string
  default     = ""
}

variable "im_mattermost_secrets_arn" {
  description = "ARN of a Secrets Manager secret with key MATTERMOST_BOT_TOKEN. Leave empty when mattermost is disabled."
  type        = string
  default     = ""
  sensitive   = true
}

variable "im_slack_enabled" {
  description = "Enable the Slack bot adapter (wave-3 bot-token flow)."
  type        = bool
  default     = false
}

variable "im_slack_channel_allowlist" {
  description = "Comma-separated Slack channel IDs the bot is allowed to post in. Empty = all."
  type        = string
  default     = ""
}

variable "im_slack_secrets_arn" {
  description = "ARN of a Secrets Manager secret with keys SLACK_BOT_TOKEN and SLACK_SIGNING_SECRET. Leave empty when slack is disabled."
  type        = string
  default     = ""
  sensitive   = true
}
