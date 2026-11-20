#!/bin/sh
# Creates or updates Grafana's read-only role on the dev database
# (infra/postgres/grafana-role.sh, decision 0026), with the password that
# infra/observability/setup-secrets.sh wrote to secrets/grafana_pg_password.
# `task obs:up` runs it; running it again changes nothing. Needs the API to
# have run its migrations once (`task dev`).
#
# The password reaches the container in the environment of `docker exec`
# (`-e NAME` with no value copies it from this process), so it is on no
# command line that `ps` shows; inside, psql reads it with \getenv.
set -eu
cd "$(dirname "$0")/../.."
set -a; . ./.env; set +a
file=${QD_SECRETS_DIR:-./secrets}/grafana_pg_password
[ -s "$file" ] || { echo "$file is missing: run infra/observability/setup-secrets.sh" >&2; exit 1; }
QD_GRAFANA_PG_PASSWORD=$(cat "$file")
QD_GRAFANA_TENANT=${QD_GRAFANA_TENANT:-}
export QD_GRAFANA_PG_PASSWORD QD_GRAFANA_TENANT
docker exec -i -e QD_GRAFANA_PG_PASSWORD -e QD_GRAFANA_TENANT \
  "${POSTGRES_CONTAINER:-${APP_STACK:-data-quality}-postgres-1}" bash <infra/postgres/grafana-role.sh
