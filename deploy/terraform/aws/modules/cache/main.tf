# ---------------------------------------------------------------------------
# Cache module — ElastiCache Valkey (Redis-compatible) cluster mode.
#
# Mirrors the HA scaffold's Valkey 6-node cluster (3 shards × 2 replicas).
# Default here is 1 shard + 1 replica (2 nodes) — adjust via variables for
# production HA. Cluster mode gives online resharding without downtime.
#
# ElastiCache currently supports Valkey 7.2 + Redis-compatible protocol;
# the engine version "valkey8" is used where available; regions that do not
# yet support Valkey fall back to redis7.x — set engine variable accordingly.
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# Security group — only ECS app tasks may connect on 6379.
# ---------------------------------------------------------------------------

resource "aws_security_group" "cache" {
  name        = "${var.project}-cache-sg"
  description = "Allow Valkey/Redis access from xiaoguai-core ECS tasks only."
  vpc_id      = var.vpc_id

  ingress {
    description     = "Redis/Valkey from app"
    from_port       = 6379
    to_port         = 6379
    protocol        = "tcp"
    security_groups = [var.app_sg_id]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name = "${var.project}-cache-sg"
  }
}

# ---------------------------------------------------------------------------
# Subnet group — spans both private subnets.
# ---------------------------------------------------------------------------

resource "aws_elasticache_subnet_group" "main" {
  name       = "${var.project}-cache-subnet-group"
  subnet_ids = var.subnet_ids

  tags = {
    Name = "${var.project}-cache-subnet-group"
  }
}

# ---------------------------------------------------------------------------
# ElastiCache replication group (cluster mode enabled).
#
# `cluster_mode` + `num_node_groups` + `replicas_per_node_group` define
# the cluster topology. With cluster_mode enabled the configuration endpoint
# is used — xiaoguai-core must connect via the configuration endpoint, not
# individual node endpoints.
# ---------------------------------------------------------------------------

resource "aws_elasticache_replication_group" "main" {
  replication_group_id = "${var.project}-valkey"
  description          = "Xiaoguai Valkey/Redis cluster-mode cache."

  # Engine — use "valkey" if your region supports ElastiCache for Valkey,
  # otherwise set to "redis". Both use the same Redis protocol.
  engine         = var.engine
  engine_version = var.engine_version
  node_type      = var.redis_node_type
  port           = 6379

  # Cluster mode
  num_node_groups         = var.redis_num_shards
  replicas_per_node_group = var.redis_replicas_per_shard

  subnet_group_name  = aws_elasticache_subnet_group.main.name
  security_group_ids = [aws_security_group.cache.id]

  # Encryption
  at_rest_encryption_enabled = true
  transit_encryption_enabled = true
  # auth_token is required when transit_encryption_enabled = true.
  # The token is generated once and stored; rotation requires a cluster update.
  auth_token = random_password.auth_token.result

  # Automatic failover is required with cluster mode.
  automatic_failover_enabled = true
  multi_az_enabled           = var.redis_replicas_per_shard > 0

  # Maintenance
  maintenance_window       = "sun:05:00-sun:06:00"
  snapshot_retention_limit = 3
  snapshot_window          = "04:00-05:00"

  # Logging (slow log to CloudWatch)
  log_delivery_configuration {
    destination      = aws_cloudwatch_log_group.cache_slow.name
    destination_type = "cloudwatch-logs"
    log_format       = "json"
    log_type         = "slow-log"
  }

  tags = {
    Name = "${var.project}-valkey"
  }

  lifecycle {
    # auth_token changes force cluster recreation — accept after careful planning.
    ignore_changes = [auth_token]
  }
}

resource "random_password" "auth_token" {
  length           = 32
  special          = false # ElastiCache auth tokens must be alphanumeric.
  override_special = ""
}

resource "aws_cloudwatch_log_group" "cache_slow" {
  name              = "/aws/elasticache/${var.project}/slow-log"
  retention_in_days = 14

  tags = {
    Name = "${var.project}-cache-slow-log"
  }
}
