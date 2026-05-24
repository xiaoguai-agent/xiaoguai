# ---------------------------------------------------------------------------
# Database module — RDS Postgres 16 (Multi-AZ for HA).
#
# Features:
#   - Multi-AZ: automatic standby in a second AZ; failover < 60 s.
#   - Parameter group with logical replication enabled (matches HA scaffold).
#   - Encrypted at rest (AWS-managed KMS key).
#   - Automated backups, 7-day retention.
#   - Deletion protection enabled (override with `skip_final_snapshot = true`
#     and `deletion_protection = false` for dev environments).
# ---------------------------------------------------------------------------

data "aws_secretsmanager_secret_version" "db_password" {
  secret_id = var.db_password_secret
}

locals {
  db_password = jsondecode(data.aws_secretsmanager_secret_version.db_password.secret_string)["password"]
}

# ---------------------------------------------------------------------------
# Security group — only the ECS app SG may connect on 5432.
# ---------------------------------------------------------------------------

resource "aws_security_group" "db" {
  name        = "${var.project}-rds-sg"
  description = "Allow Postgres access from xiaoguai-core ECS tasks only."
  vpc_id      = var.vpc_id

  ingress {
    description     = "Postgres from app"
    from_port       = 5432
    to_port         = 5432
    protocol        = "tcp"
    security_groups = [var.app_sg_id]
  }

  egress {
    description = "Allow all outbound (required for RDS enhanced monitoring)"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name = "${var.project}-rds-sg"
  }
}

# ---------------------------------------------------------------------------
# Subnet group — spans both private subnets for Multi-AZ.
# ---------------------------------------------------------------------------

resource "aws_db_subnet_group" "main" {
  name       = "${var.project}-db-subnet-group"
  subnet_ids = var.subnet_ids

  tags = {
    Name = "${var.project}-db-subnet-group"
  }
}

# ---------------------------------------------------------------------------
# Parameter group — enables logical replication (required by HA scaffold).
# ---------------------------------------------------------------------------

resource "aws_db_parameter_group" "main" {
  name        = "${var.project}-pg16"
  family      = "postgres16"
  description = "Xiaoguai Postgres 16 — logical replication + performance tuning."

  parameter {
    name         = "rds.logical_replication"
    value        = "1"
    apply_method = "pending-reboot"
  }

  parameter {
    name         = "wal_level"
    value        = "logical"
    apply_method = "pending-reboot"
  }

  parameter {
    name         = "max_replication_slots"
    value        = "10"
    apply_method = "pending-reboot"
  }

  parameter {
    name         = "max_wal_senders"
    value        = "10"
    apply_method = "pending-reboot"
  }

  # Connection/memory tuning for t4g.medium baseline; tune for larger classes.
  parameter {
    name         = "shared_preload_libraries"
    value        = "pg_stat_statements"
    apply_method = "pending-reboot"
  }

  tags = {
    Name = "${var.project}-pg16-params"
  }
}

# ---------------------------------------------------------------------------
# RDS instance
# ---------------------------------------------------------------------------

resource "aws_db_instance" "main" {
  identifier = "${var.project}-postgres"

  engine         = "postgres"
  engine_version = "16"
  instance_class = var.db_instance_class

  db_name  = var.db_name
  username = var.db_username
  password = local.db_password

  db_subnet_group_name   = aws_db_subnet_group.main.name
  vpc_security_group_ids = [aws_security_group.db.id]
  parameter_group_name   = aws_db_parameter_group.main.name

  # HA: Multi-AZ provisions a synchronous standby in a second AZ.
  multi_az = true

  # Storage
  allocated_storage     = 20
  max_allocated_storage = 200
  storage_type          = "gp3"
  storage_encrypted     = true

  # Backups
  backup_retention_period = 7
  backup_window           = "03:00-04:00"
  maintenance_window      = "Mon:04:00-Mon:05:00"

  # Monitoring
  monitoring_interval = 60
  monitoring_role_arn = aws_iam_role.rds_monitoring.arn

  # Safety
  deletion_protection       = true
  skip_final_snapshot       = false
  final_snapshot_identifier = "${var.project}-postgres-final"
  copy_tags_to_snapshot     = true

  # Performance insights (no extra charge on t4g/m6g)
  performance_insights_enabled = true

  tags = {
    Name = "${var.project}-postgres"
  }
}

# ---------------------------------------------------------------------------
# IAM role for RDS Enhanced Monitoring
# ---------------------------------------------------------------------------

resource "aws_iam_role" "rds_monitoring" {
  name = "${var.project}-rds-monitoring-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "monitoring.rds.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })

  tags = {
    Name = "${var.project}-rds-monitoring-role"
  }
}

resource "aws_iam_role_policy_attachment" "rds_monitoring" {
  role       = aws_iam_role.rds_monitoring.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonRDSEnhancedMonitoringRole"
}
