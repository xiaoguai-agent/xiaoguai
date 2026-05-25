# Xiaoguai — AWS Terraform Module

Deploys **xiaoguai-core** on AWS using ECS Fargate, RDS Postgres 16
(Multi-AZ), and ElastiCache Valkey (cluster mode).

## Architecture

```
Internet
   │
   ▼
[ALB — public subnets] ─── HTTP :80 ──► [ECS Fargate tasks — private subnets]
                                                  │              │
                                         [RDS Postgres 16   [ElastiCache
                                          Multi-AZ — 5432]   Valkey — 6379]
```

All application resources run in private subnets. Egress to the internet
(ECR image pulls, Secrets Manager, CloudWatch) flows through a NAT gateway
in the first public subnet.

## Module tree

```
deploy/terraform/aws/
├── versions.tf          # provider pins (aws ~> 5.0, random ~> 3.0)
├── variables.tf         # all root variables
├── main.tf              # module orchestration
├── outputs.tf           # alb_dns_name, db_endpoint, redis_endpoint, …
├── modules/
│   ├── network/         # VPC, subnets, IGW, NAT GW, route tables
│   ├── database/        # RDS Postgres 16 Multi-AZ + parameter group
│   ├── cache/           # ElastiCache Valkey cluster mode
│   ├── compute/         # ECS cluster, task def, Fargate service, ALB
│   └── secrets/         # Secrets Manager — DB creds + LLM API keys
└── examples/
    └── minimal/         # smallest viable single-task dev deployment
```

## Variables

| Variable | Default | Description |
|---|---|---|
| `region` | `us-east-1` | AWS region |
| `project` | `xiaoguai` | Name prefix for all resources |
| `vpc_cidr` | `10.0.0.0/16` | VPC CIDR block |
| `container_image` | *(required)* | Full ECR image URI for xiaoguai-core |
| `db_instance_class` | `db.t4g.medium` | RDS instance class |
| `db_name` | `xiaoguai` | Postgres database name |
| `db_username` | `xiaoguai` | Postgres master username |
| `redis_node_type` | `cache.t4g.medium` | ElastiCache node type |
| `redis_num_shards` | `1` | Cluster shards (increase for throughput) |
| `redis_replicas_per_shard` | `1` | Replicas per shard (≥1 for HA) |
| `instance_count` | `2` | ECS Fargate task count |
| `task_cpu` | `512` | Task CPU units |
| `task_memory_mb` | `1024` | Task memory MiB |
| `llm_secrets_arn` | `""` | Existing Secrets Manager ARN for LLM keys (empty = provision placeholder) |
| `log_retention_days` | `30` | CloudWatch log retention |

## Quickstart

### Prerequisites

- AWS CLI configured (`aws configure` or IAM role)
- Terraform >= 1.6
- A container image pushed to ECR:
  ```bash
  docker build -f deploy/Dockerfile -t xiaoguai-core:latest .
  aws ecr create-repository --repository-name xiaoguai-core
  docker tag xiaoguai-core:latest 123456789012.dkr.ecr.us-east-1.amazonaws.com/xiaoguai-core:latest
  docker push 123456789012.dkr.ecr.us-east-1.amazonaws.com/xiaoguai-core:latest
  ```

### Deploy

```bash
cd deploy/terraform/aws

# 1. Initialise (downloads providers, no backend configured).
terraform init

# 2. Review plan.
terraform plan \
  -var="container_image=123456789012.dkr.ecr.us-east-1.amazonaws.com/xiaoguai-core:v1.1.4" \
  -var="project=xiaoguai-prod"

# 3. Apply (~15 min — RDS Multi-AZ is the slow step).
terraform apply \
  -var="container_image=123456789012.dkr.ecr.us-east-1.amazonaws.com/xiaoguai-core:v1.1.4" \
  -var="project=xiaoguai-prod"

# 4. Note the ALB DNS name from outputs.
terraform output alb_dns_name
```

### Populate LLM API keys

After first apply, populate the placeholder secret before ECS tasks will
start accepting LLM requests:

```bash
SECRET_ARN=$(terraform output -raw llm_secrets_arn)
aws secretsmanager put-secret-value \
  --secret-id "$SECRET_ARN" \
  --secret-string '{"OPENAI_API_KEY":"sk-...","ANTHROPIC_API_KEY":"sk-ant-..."}'
```

Then force a new ECS deployment to pick up the updated secrets:

```bash
aws ecs update-service \
  --cluster $(terraform output -raw ecs_cluster_name) \
  --service $(terraform output -raw ecs_service_name) \
  --force-new-deployment
```

## Cost estimate (rough, us-east-1, on-demand)

This is a ballpark for the **default configuration**. Actual costs depend on
traffic, data transfer, and whether Reserved Instances are used.

| Resource | Spec | ~USD/month |
|---|---|---|
| RDS Postgres | db.t4g.medium, Multi-AZ, 20 GB gp3 | ~$75 |
| ElastiCache | cache.t4g.medium × 2 (1 shard + 1 replica) | ~$50 |
| ECS Fargate | 512 CPU / 1024 MB × 2 tasks | ~$30 |
| ALB | 1 LCU baseline | ~$20 |
| NAT Gateway | 1 GW + data transfer | ~$35 |
| CloudWatch | Logs ingestion / storage | ~$5 |
| Secrets Manager | 2 secrets | ~$1 |
| **Total** | | **~$216/month** |

Savings levers: use Fargate Spot for tasks, Reserved DB instance (1yr),
disable Multi-AZ for non-prod.

## What this module does NOT include

The following are intentionally out of scope — operator responsibility:

| Capability | Why deferred |
|---|---|
| **DNS (Route 53)** | Domain names vary per deployment; add an `aws_route53_record` pointing to `alb_dns_name` |
| **TLS certificates (ACM)** | Requires domain validation; add `aws_acm_certificate` + HTTPS listener on port 443 |
| **WAF (AWS WAF v2)** | Sizing and rule sets are workload-specific |
| **S3 state backend** | Configured per team; pass `-backend-config` or add a `backend` block |
| **ECR repository** | Push your own image; this module references an existing URI |
| **Auto Scaling** | Add `aws_appautoscaling_*` resources wrapping the ECS service |
| **VPN / Direct Connect** | On-premises connectivity |
| **Cost Anomaly Detection** | Add `aws_ce_anomaly_*` resources separately |

## Remote state (recommended)

Add a `backend.tf` before first apply:

```hcl
terraform {
  backend "s3" {
    bucket         = "my-tfstate-bucket"
    key            = "xiaoguai/prod/terraform.tfstate"
    region         = "us-east-1"
    dynamodb_table = "terraform-locks"
    encrypt        = true
  }
}
```
