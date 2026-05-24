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
}
