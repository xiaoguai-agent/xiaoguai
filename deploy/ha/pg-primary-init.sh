#!/bin/bash
# Run once on pg-primary's first boot (postgres entrypoint executes
# every .sh in /docker-entrypoint-initdb.d after initdb).
#
# Creates the logical-replication publication that subscribers latch
# onto. `FOR ALL TABLES` keeps us out of the business of enumerating
# tables here — the xiaoguai schema is owned by sqlx migrations on
# the primary, and subscribers replicate whatever the publication
# currently covers.
#
# NOTE: subscribers must have the destination schema in place BEFORE
# the subscription starts pulling data — see pg-subscriber-init.sh
# for how we bootstrap that via pg_dump --schema-only.

set -euo pipefail

psql -v ON_ERROR_STOP=1 \
  --username "${POSTGRES_USER}" \
  --dbname "${POSTGRES_DB}" \
  <<-EOSQL
  -- Replication role used by both subscribers. Password matches the
  -- compose-level POSTGRES_PASSWORD so we avoid a second secret in
  -- the demo stack. In production, give this its own credential
  -- and grant only REPLICATION + CONNECT.
  DO \$\$
  BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'xiaoguai_repl') THEN
      CREATE ROLE xiaoguai_repl WITH REPLICATION LOGIN PASSWORD 'xiaoguai';
    END IF;
  END
  \$\$;

  GRANT CONNECT ON DATABASE ${POSTGRES_DB} TO xiaoguai_repl;
  GRANT USAGE ON SCHEMA public TO xiaoguai_repl;
  GRANT SELECT ON ALL TABLES IN SCHEMA public TO xiaoguai_repl;
  ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO xiaoguai_repl;

  -- The publication is created empty here. xiaoguai-core's sqlx
  -- migrations create tables on the primary at first boot; once
  -- created they are automatically covered by FOR ALL TABLES.
  DO \$\$
  BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_publication WHERE pubname = 'xiaoguai_pub') THEN
      CREATE PUBLICATION xiaoguai_pub FOR ALL TABLES;
    END IF;
  END
  \$\$;
EOSQL

# pg_hba: allow the replication role from the compose network.
PG_HBA="${PGDATA}/pg_hba.conf"
if ! grep -q "xiaoguai_repl" "${PG_HBA}"; then
  echo "host    all             xiaoguai_repl   0.0.0.0/0               md5" >> "${PG_HBA}"
  echo "host    replication     xiaoguai_repl   0.0.0.0/0               md5" >> "${PG_HBA}"
fi
