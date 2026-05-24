# ---------------------------------------------------------------------------
# Compute module — ECS Fargate + ALB.
#
# Creates:
#   - ECS cluster
#   - CloudWatch log group
#   - IAM roles (task execution + task)
#   - Security groups (ALB + app)
#   - ALB + listener + target group (healthcheck /healthz)
#   - ECS task definition
#   - ECS Fargate service
#
# The app container exposes port 8080 (see deploy/Dockerfile EXPOSE 8080).
# Environment variables are injected via ECS secrets (Secrets Manager).
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# CloudWatch log group
# ---------------------------------------------------------------------------

resource "aws_cloudwatch_log_group" "app" {
  name              = "/ecs/${var.project}/core"
  retention_in_days = var.log_retention_days

  tags = {
    Name = "${var.project}-core-logs"
  }
}

# ---------------------------------------------------------------------------
# IAM — Task Execution Role (pull image, write logs, read secrets)
# ---------------------------------------------------------------------------

resource "aws_iam_role" "task_execution" {
  name = "${var.project}-ecs-task-execution"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })

  tags = {
    Name = "${var.project}-ecs-task-execution"
  }
}

resource "aws_iam_role_policy_attachment" "task_execution_managed" {
  role       = aws_iam_role.task_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

resource "aws_iam_role_policy" "task_execution_secrets" {
  name = "${var.project}-ecs-secrets-access"
  role = aws_iam_role.task_execution.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = ["secretsmanager:GetSecretValue"]
        Resource = [
          var.db_secret_arn,
          var.llm_secrets_arn,
        ]
      }
    ]
  })
}

# ---------------------------------------------------------------------------
# IAM — Task Role (runtime permissions for the container itself)
# ---------------------------------------------------------------------------

resource "aws_iam_role" "task" {
  name = "${var.project}-ecs-task"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })

  tags = {
    Name = "${var.project}-ecs-task"
  }
}

# Allow the task to write its own logs (belt-and-suspenders alongside execution role).
resource "aws_iam_role_policy" "task_logs" {
  name = "${var.project}-ecs-task-logs"
  role = aws_iam_role.task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = ["logs:CreateLogStream", "logs:PutLogEvents"]
      Resource = "${aws_cloudwatch_log_group.app.arn}:*"
    }]
  })
}

# ---------------------------------------------------------------------------
# Security groups
# ---------------------------------------------------------------------------

resource "aws_security_group" "alb" {
  name        = "${var.project}-alb-sg"
  description = "ALB — allow HTTP/HTTPS from anywhere."
  vpc_id      = var.vpc_id

  ingress {
    description = "HTTP"
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    description = "HTTPS"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name = "${var.project}-alb-sg"
  }
}

resource "aws_security_group" "app" {
  name        = "${var.project}-app-sg"
  description = "xiaoguai-core ECS tasks — accept traffic from ALB only."
  vpc_id      = var.vpc_id

  ingress {
    description     = "App port from ALB"
    from_port       = 8080
    to_port         = 8080
    protocol        = "tcp"
    security_groups = [aws_security_group.alb.id]
  }

  egress {
    description = "Allow all outbound (DB, Redis, Secrets Manager, ECR)"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name = "${var.project}-app-sg"
  }
}

# ---------------------------------------------------------------------------
# Application Load Balancer
# ---------------------------------------------------------------------------

resource "aws_lb" "main" {
  name               = "${var.project}-alb"
  internal           = false
  load_balancer_type = "application"
  security_groups    = [aws_security_group.alb.id]
  subnets            = var.public_subnet_ids

  # Enable access logs to S3 (bucket must pre-exist and have the correct
  # bucket policy; disabled here to keep the module self-contained).
  # access_logs { bucket = "..." enabled = true }

  tags = {
    Name = "${var.project}-alb"
  }
}

resource "aws_lb_target_group" "app" {
  name        = "${var.project}-tg"
  port        = 8080
  protocol    = "HTTP"
  vpc_id      = var.vpc_id
  target_type = "ip" # Required for Fargate.

  health_check {
    enabled             = true
    path                = "/healthz"
    port                = "traffic-port"
    protocol            = "HTTP"
    healthy_threshold   = 2
    unhealthy_threshold = 3
    timeout             = 5
    interval            = 30
    matcher             = "200"
  }

  deregistration_delay = 30

  tags = {
    Name = "${var.project}-tg"
  }
}

# HTTP listener — forwards everything to the target group.
# TLS (HTTPS/443) is the user's responsibility (see README: DNS + ACM).
resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.main.arn
  port              = 80
  protocol          = "HTTP"

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.app.arn
  }
}

# ---------------------------------------------------------------------------
# ECS Cluster
# ---------------------------------------------------------------------------

resource "aws_ecs_cluster" "main" {
  name = "${var.project}-cluster"

  setting {
    name  = "containerInsights"
    value = "enabled"
  }

  tags = {
    Name = "${var.project}-cluster"
  }
}

resource "aws_ecs_cluster_capacity_providers" "main" {
  cluster_name       = aws_ecs_cluster.main.name
  capacity_providers = ["FARGATE", "FARGATE_SPOT"]

  default_capacity_provider_strategy {
    capacity_provider = "FARGATE"
    weight            = 1
  }
}

# ---------------------------------------------------------------------------
# ECS Task Definition
# ---------------------------------------------------------------------------

resource "aws_ecs_task_definition" "app" {
  family                   = "${var.project}-core"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory_mb
  execution_role_arn       = aws_iam_role.task_execution.arn
  task_role_arn            = aws_iam_role.task.arn

  container_definitions = jsonencode([
    {
      name      = "xiaoguai-core"
      image     = var.container_image
      essential = true

      portMappings = [
        {
          containerPort = 8080
          protocol      = "tcp"
        }
      ]

      environment = [
        {
          name  = "XIAOGUAI_SERVER__HOST"
          value = "0.0.0.0"
        },
        {
          name  = "XIAOGUAI_SERVER__PORT"
          value = "8080"
        },
        {
          name = "XIAOGUAI_DATABASE__URL"
          # Assembled at task startup from secrets; placeholder used for
          # static task def. ECS secrets injection fills the real password.
          value = "postgres://${var.db_name}@${var.db_host}:${var.db_port}/${var.db_name}"
        },
        {
          name  = "XIAOGUAI_CACHE__URL"
          value = "rediss://${var.redis_endpoint}"
        },
        {
          name  = "RUST_LOG"
          value = "info,sqlx=warn"
        }
      ]

      # Inject secrets from Secrets Manager as environment variables.
      secrets = [
        {
          name      = "XIAOGUAI_DB_PASSWORD"
          valueFrom = "${var.db_secret_arn}:password::"
        },
        {
          name      = "OPENAI_API_KEY"
          valueFrom = "${var.llm_secrets_arn}:OPENAI_API_KEY::"
        },
        {
          name      = "ANTHROPIC_API_KEY"
          valueFrom = "${var.llm_secrets_arn}:ANTHROPIC_API_KEY::"
        }
      ]

      logConfiguration = {
        logDriver = "awslogs"
        options = {
          "awslogs-group"         = aws_cloudwatch_log_group.app.name
          "awslogs-region"        = var.region
          "awslogs-stream-prefix" = "core"
        }
      }

      healthCheck = {
        command     = ["CMD-SHELL", "wget -qO- http://localhost:8080/healthz || exit 1"]
        interval    = 30
        timeout     = 5
        retries     = 3
        startPeriod = 60
      }
    }
  ])

  tags = {
    Name = "${var.project}-core-task"
  }
}

# ---------------------------------------------------------------------------
# ECS Fargate Service
# ---------------------------------------------------------------------------

resource "aws_ecs_service" "app" {
  name            = "${var.project}-core"
  cluster         = aws_ecs_cluster.main.id
  task_definition = aws_ecs_task_definition.app.arn
  desired_count   = var.instance_count
  launch_type     = "FARGATE"

  network_configuration {
    subnets          = var.private_subnet_ids
    security_groups  = [aws_security_group.app.id]
    assign_public_ip = false
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.app.arn
    container_name   = "xiaoguai-core"
    container_port   = 8080
  }

  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }

  deployment_controller {
    type = "ECS"
  }

  # Wait for ALB listener to be ready before creating the service.
  depends_on = [aws_lb_listener.http]

  # Allow external tools (e.g., CI/CD) to change the task definition
  # without Terraform treating it as drift.
  lifecycle {
    ignore_changes = [task_definition, desired_count]
  }

  tags = {
    Name = "${var.project}-core-service"
  }
}
