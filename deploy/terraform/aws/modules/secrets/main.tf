# ---------------------------------------------------------------------------
# Secrets module — Secrets Manager secrets for Postgres credentials and
# LLM API keys.
#
# Creates:
#   - A random Postgres password stored in Secrets Manager.
#   - A placeholder LLM API keys secret (if llm_secrets_arn is empty).
# ---------------------------------------------------------------------------

resource "random_password" "db" {
  length           = 32
  special          = true
  override_special = "!#$%&*()-_=+[]{}<>:?"
}

resource "aws_secretsmanager_secret" "db_password" {
  name                    = "${var.project}/db/password"
  recovery_window_in_days = 7

  tags = {
    Name = "${var.project}-db-password"
  }
}

resource "aws_secretsmanager_secret_version" "db_password" {
  secret_id = aws_secretsmanager_secret.db_password.id
  secret_string = jsonencode({
    username = var.db_username
    password = random_password.db.result
    dbname   = var.db_name
  })
}

# ---------------------------------------------------------------------------
# LLM API keys secret — created as a placeholder when no existing ARN is
# provided. Operators populate it after first apply via aws secretsmanager
# put-secret-value (see module README for the command).
# ---------------------------------------------------------------------------

resource "aws_secretsmanager_secret" "llm_keys_placeholder" {
  count = var.llm_secrets_arn == "" ? 1 : 0

  name                    = "${var.project}/llm/api-keys"
  recovery_window_in_days = 7

  tags = {
    Name = "${var.project}-llm-api-keys"
  }
}

resource "aws_secretsmanager_secret_version" "llm_keys_placeholder" {
  count = var.llm_secrets_arn == "" ? 1 : 0

  secret_id = aws_secretsmanager_secret.llm_keys_placeholder[0].id
  secret_string = jsonencode({
    OPENAI_API_KEY    = "PLACEHOLDER"
    ANTHROPIC_API_KEY = "PLACEHOLDER"
  })
}
