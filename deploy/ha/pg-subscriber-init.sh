#!/bin/bash
# Run once on each pg-subscriber's first boot.
#
# Bootstraps the destination database to mirror pg-primary's
# schema, then creates a SUBSCRIPTION pointing at the primary's
# publication. The actual table DDL is sourced from a pg_dump
# --schema-only of the primary, so subscriber bring-up is gated on
# the primary already having shipped its sqlx migrations.
#
# We retry the schema dump for up to ~60s — the primary container
# is healthy by the time we run (compose dependency), but the
# xiaoguai-core migrator that creates the user tables runs on the
# core container, which boots later. The subscriber tolerates a
# late-arriving schema: SUBSCRIPTION creation just queues the
# initial copy. Tables added on the primary after the subscription
# is created are NOT auto-replicated under FOR ALL TABLES — see
# runbook §3 for the ALTER SUBSCRIPTION REFRESH PUBLICATION pattern
# this stack runs on every primary migration.

set -euo pipefail

: "${PRIMARY_HOST:?must be set in compose}"
: "${SUBSCRIPTION_NAME:?must be set in compose}"

# 1) Wait for the primary to actually have the publication tables
#    we care about. ~12 retries × 5s = 60s budget.
export PGPASSWORD="${POSTGRES_PASSWORD}"
for attempt in 1 2 3 4 5 6 7 8 9 10 11 12; do
  if pg_dump \
      --host="${PRIMARY_HOST}" \
      --username="${POSTGRES_USER}" \
      --dbname="${POSTGRES_DB}" \
      --schema-only \
      --no-owner \
      --no-privileges \
      > /tmp/primary-schema.sql 2>/dev/null
  then
    if [ -s /tmp/primary-schema.sql ]; then
      break
    fi
  fi
  echo "pg-subscriber-init: waiting for primary schema (attempt ${attempt}/12)…"
  sleep 5
done

if [ ! -s /tmp/primary-schema.sql ]; then
  echo "pg-subscriber-init: WARNING — primary schema still empty. Subscription will be created against empty target; first ALTER SUBSCRIPTION REFRESH after primary migrates will copy." >&2
fi

# 2) Apply the schema locally (ignore errors — re-runs are idempotent).
psql -v ON_ERROR_STOP=0 \
  --username "${POSTGRES_USER}" \
  --dbname "${POSTGRES_DB}" \
  --file /tmp/primary-schema.sql || true

# 3) Create the subscription. `copy_data = true` does the initial
#    sync; the slot lives on the primary under SUBSCRIPTION_NAME.
psql -v ON_ERROR_STOP=1 \
  --username "${POSTGRES_USER}" \
  --dbname "${POSTGRES_DB}" \
  <<-EOSQL
  DO \$\$
  BEGIN
    IF NOT EXISTS (
      SELECT 1 FROM pg_subscription WHERE subname = '${SUBSCRIPTION_NAME}'
    ) THEN
      CREATE SUBSCRIPTION ${SUBSCRIPTION_NAME}
        CONNECTION 'host=${PRIMARY_HOST} port=5432 dbname=${POSTGRES_DB} user=xiaoguai_repl password=xiaoguai'
        PUBLICATION xiaoguai_pub
        WITH (copy_data = true, create_slot = true, slot_name = '${SUBSCRIPTION_NAME}_slot');
    END IF;
  END
  \$\$;
EOSQL

echo "pg-subscriber-init: subscription ${SUBSCRIPTION_NAME} created."
