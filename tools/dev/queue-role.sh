#!/bin/sh
# Creates the queue's role on the existing dev database (a new one gets it
# from infra/postgres/initdb/20-queue-role.sh). The first time it generates
# QD_QUEUE_PASSWORD and appends it to .env, without printing it. Then run
# `server bootstrap-admin` (or `task dev`) to apply the quality module's
# migrations, which grant the role its functions.
set -eu
cd "$(dirname "$0")/../.."
set -a; . ./.env; set +a
if [ -z "${QD_QUEUE_PASSWORD:-}" ]; then
  QD_QUEUE_PASSWORD=$(openssl rand -hex 24)
  printf '\n# The queue'"'"'s PostgreSQL role (tools/dev/queue-role.sh)\nQD_QUEUE_PASSWORD=%s\n' "$QD_QUEUE_PASSWORD" >>.env
fi
docker exec -i -e QD_QUEUE_PASSWORD="$QD_QUEUE_PASSWORD" "${POSTGRES_CONTAINER:-data-quality-postgres-1}" \
  sh -c 'POSTGRES_USER="$POSTGRES_USER" POSTGRES_DB="$POSTGRES_DB" bash' <infra/postgres/initdb/20-queue-role.sh
