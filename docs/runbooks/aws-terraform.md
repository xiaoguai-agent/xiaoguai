# Runbook — AWS Terraform deployment

Covers: initial deploy, rolling update, teardown, and common gotchas for
the `deploy/terraform/aws/` module.

---

## 0. Prerequisites

| Tool | Minimum version | Install |
|---|---|---|
| Terraform | 1.6.0 | `brew install terraform` or [tfenv](https://github.com/tfutils/tfenv) |
| AWS CLI | 2.x | `brew install awscli` |
| Docker | 24.x | For building and pushing the container image |

AWS credentials must have permissions for: EC2, ECS, RDS, ElastiCache,
Secrets Manager, IAM (role create/attach), CloudWatch Logs, ELB.

---

## 1. Initial deployment

### 1a. Build and push the container image

```bash
# From repo root.
docker build -f deploy/Dockerfile -t xiaoguai-core:$(git describe --tags --abbrev=0) .

# Create ECR repo (once per account/region).
aws ecr create-repository --repository-name xiaoguai-core --region us-east-1

# Authenticate Docker to ECR.
aws ecr get-login-password --region us-east-1 \
  | docker login --username AWS --password-stdin \
      123456789012.dkr.ecr.us-east-1.amazonaws.com

# Tag and push.
IMAGE=123456789012.dkr.ecr.us-east-1.amazonaws.com/xiaoguai-core:$(git describe --tags --abbrev=0)
docker tag xiaoguai-core:$(git describe --tags --abbrev=0) "$IMAGE"
docker push "$IMAGE"
```

### 1b. Terraform init

```bash
cd deploy/terraform/aws

# Without remote backend (local state for experimentation):
terraform init

# With S3 backend (recommended for teams):
terraform init \
  -backend-config="bucket=my-tfstate-bucket" \
  -backend-config="key=xiaoguai/prod/terraform.tfstate" \
  -backend-config="region=us-east-1" \
  -backend-config="dynamodb_table=terraform-locks"
```

### 1c. Plan and apply

```bash
terraform plan \
  -var="container_image=$IMAGE" \
  -var="project=xiaoguai-prod" \
  -out=tfplan

terraform apply tfplan
```

Expected duration: **10–18 minutes** (RDS Multi-AZ creation is the
bottleneck; ElastiCache cluster mode takes 5–8 min).

Expected outputs:

```
alb_dns_name    = "xiaoguai-prod-alb-123456789.us-east-1.elb.amazonaws.com"
db_endpoint     = "xiaoguai-prod-postgres.cxyz.us-east-1.rds.amazonaws.com:5432"
redis_endpoint  = "xiaoguai-prod-valkey.cfg.use1.cache.amazonaws.com:6379"
ecs_cluster_name = "xiaoguai-prod-cluster"
ecs_service_name = "xiaoguai-prod-core"
log_group_name   = "/ecs/xiaoguai-prod/core"
```

### 1d. Populate LLM API keys

The module provisions a placeholder secret. Populate it before ECS tasks
are healthy:

```bash
SECRET_ARN=$(terraform output -raw llm_secrets_arn)
aws secretsmanager put-secret-value \
  --secret-id "$SECRET_ARN" \
  --secret-string '{
    "OPENAI_API_KEY":    "sk-...",
    "ANTHROPIC_API_KEY": "sk-ant-..."
  }'
```

### 1e. Run database migrations

The `xiaoguai` CLI runs SQLx migrations. Execute it as a one-shot ECS task:

```bash
CLUSTER=$(terraform output -raw ecs_cluster_name)
SUBNETS=$(aws ecs describe-services \
  --cluster "$CLUSTER" \
  --services xiaoguai-prod-core \
  --query 'services[0].networkConfiguration.awsvpcConfiguration.subnets' \
  --output text | tr '\t' ',')
SG=$(aws ecs describe-services \
  --cluster "$CLUSTER" \
  --services xiaoguai-prod-core \
  --query 'services[0].networkConfiguration.awsvpcConfiguration.securityGroups[0]' \
  --output text)

aws ecs run-task \
  --cluster "$CLUSTER" \
  --task-definition xiaoguai-prod-core \
  --launch-type FARGATE \
  --network-configuration "awsvpcConfiguration={subnets=[$SUBNETS],securityGroups=[$SG]}" \
  --overrides '{"containerOverrides":[{"name":"xiaoguai-core","command":["migrate"]}]}'
```

Wait for the task to exit 0, then force a new service deployment:

```bash
aws ecs update-service \
  --cluster "$CLUSTER" \
  --service xiaoguai-prod-core \
  --force-new-deployment
```

### 1f. Verify

```bash
ALB=$(terraform output -raw alb_dns_name)

# Health check.
curl -sf "http://$ALB/healthz"
# Expected: 200 OK with body "ok"

# API smoke test.
curl -sf "http://$ALB/v1/sessions" -H "Content-Type: application/json" | jq .
```

---

## 2. Rolling update (new image version)

```bash
cd deploy/terraform/aws

IMAGE=123456789012.dkr.ecr.us-east-1.amazonaws.com/xiaoguai-core:v1.2.0

# Build + push new image (step 1a above), then:
terraform apply -var="container_image=$IMAGE" -var="project=xiaoguai-prod"

# ECS lifecycle.ignore_changes skips Terraform re-registering the task def.
# Force a new deployment manually:
aws ecs update-service \
  --cluster $(terraform output -raw ecs_cluster_name) \
  --service $(terraform output -raw ecs_service_name) \
  --task-definition xiaoguai-prod-core \
  --force-new-deployment
```

ECS performs a rolling update: new tasks start, healthcheck passes
(`/healthz` → 200), old tasks drain (30 s deregistration delay), then
stop. Zero downtime with `desired_count ≥ 2`.

---

## 3. Scaling

```bash
# Scale out ECS tasks (immediate, no Terraform required).
aws ecs update-service \
  --cluster $(terraform output -raw ecs_cluster_name) \
  --service $(terraform output -raw ecs_service_name) \
  --desired-count 4

# Scale RDS to a larger instance class (requires brief failover).
terraform apply \
  -var="db_instance_class=db.t4g.large" \
  -var="container_image=$IMAGE" \
  -var="project=xiaoguai-prod"
```

---

## 4. Teardown

> **Warning**: `terraform destroy` will delete all data including the
> RDS instance. The final snapshot (`xiaoguai-prod-postgres-final`) is
> preserved in RDS snapshots. Deletion protection is enabled — you must
> disable it first or use `-target` to remove it.

```bash
cd deploy/terraform/aws

# Step 1: Disable deletion protection on the RDS instance.
aws rds modify-db-instance \
  --db-instance-identifier xiaoguai-prod-postgres \
  --no-deletion-protection \
  --apply-immediately

# Step 2: Destroy all resources.
terraform destroy \
  -var="container_image=$IMAGE" \
  -var="project=xiaoguai-prod"

# Verify the final RDS snapshot was created.
aws rds describe-db-snapshots \
  --db-snapshot-identifier xiaoguai-prod-postgres-final \
  --query 'DBSnapshots[0].{Status:Status,AllocatedStorage:AllocatedStorage}'
```

---

## 5. Common gotchas

### "Error: waiting for ECS Service to reach steady state"

ECS deployment circuit breaker tripped. Check:

```bash
# View recent service events.
aws ecs describe-services \
  --cluster $(terraform output -raw ecs_cluster_name) \
  --services $(terraform output -raw ecs_service_name) \
  --query 'services[0].events[:5]'

# Stream container logs.
aws logs tail $(terraform output -raw log_group_name) --follow
```

Most common causes:
- Container healthcheck failing: the `/healthz` endpoint on port 8080
  must return 200. Check `XIAOGUAI_DATABASE__URL` is correct.
- LLM secret not populated: ECS task fails to inject secret → container
  exits. Populate the secret (step 1d) and force a new deployment.
- Missing DB migration: `xiaoguai-core` will exit if tables don't exist.
  Run the migration task (step 1e) first.

### "Error: InvalidParameterException: The provided target group does not have listener associated"

Happens when Terraform creates the ECS service before the ALB listener is
ready. The `depends_on` in the compute module handles this; if it recurs,
re-run `terraform apply`.

### RDS Multi-AZ failover testing

```bash
# Trigger a manual failover (takes ~60 s; connection errors expected).
aws rds reboot-db-instance \
  --db-instance-identifier xiaoguai-prod-postgres \
  --force-failover

# xiaoguai-core uses sqlx connection pooling with automatic reconnect;
# in-flight requests during failover will see connection errors.
# ECS health checks will restart any task that gets stuck.
```

### ElastiCache AUTH token rotation

The AUTH token is generated once by `random_password` and stored in
Terraform state. Rotation requires:

1. Update `random_password` (add a `keepers` map and change a key).
2. Apply — ElastiCache accepts token rotation without cluster recreation
   (two-token window): `aws elasticache modify-replication-group ... --auth-token-update-strategy ROTATE`.
3. Remove old token after verifying all clients use the new one.

### "Provider produced inconsistent result" on first apply

Rare: can happen if Secrets Manager secret version is not yet visible
immediately after creation. Re-run `terraform apply` — it is idempotent.

### NAT gateway costs

A single NAT gateway routes all private-subnet egress. For multi-AZ
egress HA, add a second NAT gateway in `public_subnet[1]` and a
per-AZ route table. This doubles the NAT gateway cost (~$35/mo each).

---

## 6. DNS and TLS (operator responsibility)

This module outputs `alb_dns_name`. To serve traffic on a custom domain
with HTTPS:

1. Request an ACM certificate for your domain (`aws acm request-certificate`).
2. Validate via DNS or email.
3. Add an `aws_lb_listener` for port 443 referencing the certificate ARN.
4. Add an `aws_route53_record` (A-alias pointing to `alb_dns_name` / `alb_zone_id`).

These steps are not included in the module to keep it domain-agnostic.

---

## 7. Monitoring

```bash
# Tail live container logs.
aws logs tail $(terraform output -raw log_group_name) --follow --format short

# ECS service CPU/memory.
aws cloudwatch get-metric-statistics \
  --namespace AWS/ECS \
  --metric-name CPUUtilization \
  --dimensions Name=ClusterName,Value=$(terraform output -raw ecs_cluster_name) \
               Name=ServiceName,Value=$(terraform output -raw ecs_service_name) \
  --period 60 --statistics Average \
  --start-time $(date -u -v-1H +%FT%TZ) \
  --end-time $(date -u +%FT%TZ)
```
