# ---------------------------------------------------------------------------
# Xiaoguai — AWS Fargate + RDS Postgres + ElastiCache Valkey
# Root orchestration module.
#
# Dependency graph:
#   secrets → (none)
#   network → (none)
#   database → network, secrets
#   cache    → network
#   compute  → network, database, cache, secrets
# ---------------------------------------------------------------------------

module "network" {
  source   = "./modules/network"
  project  = var.project
  vpc_cidr = var.vpc_cidr
}

module "secrets" {
  source          = "./modules/secrets"
  project         = var.project
  db_username     = var.db_username
  db_name         = var.db_name
  llm_secrets_arn = var.llm_secrets_arn
}

module "database" {
  source             = "./modules/database"
  project            = var.project
  vpc_id             = module.network.vpc_id
  subnet_ids         = module.network.private_subnet_ids
  app_sg_id          = module.compute.app_sg_id
  db_instance_class  = var.db_instance_class
  db_name            = var.db_name
  db_username        = var.db_username
  db_password_secret = module.secrets.db_password_secret_arn
}

module "cache" {
  source                   = "./modules/cache"
  project                  = var.project
  vpc_id                   = module.network.vpc_id
  subnet_ids               = module.network.private_subnet_ids
  app_sg_id                = module.compute.app_sg_id
  redis_node_type          = var.redis_node_type
  redis_num_shards         = var.redis_num_shards
  redis_replicas_per_shard = var.redis_replicas_per_shard
}

module "compute" {
  source             = "./modules/compute"
  project            = var.project
  region             = var.region
  vpc_id             = module.network.vpc_id
  public_subnet_ids  = module.network.public_subnet_ids
  private_subnet_ids = module.network.private_subnet_ids
  container_image    = var.container_image
  instance_count     = var.instance_count
  task_cpu           = var.task_cpu
  task_memory_mb     = var.task_memory_mb
  log_retention_days = var.log_retention_days
  db_host            = module.database.db_endpoint
  db_port            = module.database.db_port
  db_name            = var.db_name
  db_secret_arn      = module.secrets.db_password_secret_arn
  redis_endpoint     = module.cache.configuration_endpoint
  llm_secrets_arn    = module.secrets.llm_secrets_arn

  # --- Wave-3: HotL ---
  hotl_enabled                      = var.hotl_enabled
  hotl_policy_store_backend         = var.hotl_policy_store_backend
  hotl_enforcement_enabled          = var.hotl_enforcement_enabled
  hotl_slack_escalation_webhook_url = var.hotl_slack_escalation_webhook_url
  hotl_email_escalation_address     = var.hotl_email_escalation_address
  hotl_webhook_escalation_url       = var.hotl_webhook_escalation_url

  # --- Wave-3: Outcomes ---
  outcomes_backend                   = var.outcomes_backend
  outcomes_retention_days            = var.outcomes_retention_days
  outcomes_timeseries_bucket_seconds = var.outcomes_timeseries_bucket_seconds

  # --- Wave-3: Skills / packs ---
  skills_packs_config_path       = var.skills_packs_config_path
  skills_install_allowlist       = var.skills_install_allowlist
  skills_auto_install_on_startup = var.skills_auto_install_on_startup

  # --- Wave-3: Rate limiting ---
  rate_limit_enabled                   = var.rate_limit_enabled
  rate_limit_backend                   = var.rate_limit_backend
  rate_limit_per_tenant_requests       = var.rate_limit_per_tenant_requests
  rate_limit_per_tenant_window_seconds = var.rate_limit_per_tenant_window_seconds
  rate_limit_per_route_requests        = var.rate_limit_per_route_requests
  rate_limit_per_route_window_seconds  = var.rate_limit_per_route_window_seconds

  # --- Wave-3: Cloud LLM v2 ---
  bedrock_region             = var.bedrock_region
  bedrock_auth_mode          = var.bedrock_auth_mode
  bedrock_secrets_arn        = var.bedrock_secrets_arn
  azure_openai_endpoint      = var.azure_openai_endpoint
  azure_openai_deployment_id = var.azure_openai_deployment_id
  azure_openai_api_version   = var.azure_openai_api_version
  azure_openai_secrets_arn   = var.azure_openai_secrets_arn
  mistral_api_base           = var.mistral_api_base
  mistral_secrets_arn        = var.mistral_secrets_arn
  groq_api_base              = var.groq_api_base
  groq_secrets_arn           = var.groq_secrets_arn

  # --- Wave-3: Observability ---
  otel_enabled               = var.otel_enabled
  otel_endpoint              = var.otel_endpoint
  otel_traces_sampling_ratio = var.otel_traces_sampling_ratio
  otel_service_name          = var.otel_service_name
  prometheus_enabled         = var.prometheus_enabled
  prometheus_listen_addr     = var.prometheus_listen_addr

  # --- Wave-3: IM adapters ---
  im_discord_enabled                 = var.im_discord_enabled
  im_discord_channel_allowlist       = var.im_discord_channel_allowlist
  im_discord_webhook_signing_enabled = var.im_discord_webhook_signing_enabled
  im_discord_secrets_arn             = var.im_discord_secrets_arn
  im_telegram_enabled                = var.im_telegram_enabled
  im_telegram_chat_allowlist         = var.im_telegram_chat_allowlist
  im_telegram_secrets_arn            = var.im_telegram_secrets_arn
  im_mattermost_enabled              = var.im_mattermost_enabled
  im_mattermost_server_url           = var.im_mattermost_server_url
  im_mattermost_channel_allowlist    = var.im_mattermost_channel_allowlist
  im_mattermost_secrets_arn          = var.im_mattermost_secrets_arn
  im_slack_enabled                   = var.im_slack_enabled
  im_slack_channel_allowlist         = var.im_slack_channel_allowlist
  im_slack_secrets_arn               = var.im_slack_secrets_arn
}
