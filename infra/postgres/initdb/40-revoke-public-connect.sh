#!/bin/bash
# Only the roles named here connect to the core's databases (quality-sdk
# decision 0009, step C5). PostgreSQL gives PUBLIC `CONNECT` on every new
# database, so without this a module's role (one per module,
# with its own databases on this server) could open the core's. Each core
# role that connects is granted it by name: the application's, the queue's
# and Grafana's on POSTGRES_DB (their own scripts), the metrics' on
# `postgres` (here). The superuser is not affected.
#
# Runs once on a new database (docker-entrypoint-initdb.d), as the
# superuser; on an existing installation tls/entrypoint.sh runs it at every
# start, once PostgreSQL answers. Running it again changes nothing.
set -euo pipefail
: "${POSTGRES_USER:?POSTGRES_USER must be set}"
: "${POSTGRES_DB:?POSTGRES_DB must be set}"

psql -v ON_ERROR_STOP=1 -X --username "$POSTGRES_USER" --dbname postgres \
  --set app_db="$POSTGRES_DB" >/dev/null <<-'SQL'
	REVOKE CONNECT ON DATABASE :"app_db" FROM PUBLIC;
	REVOKE CONNECT ON DATABASE postgres FROM PUBLIC;
	-- The metrics' role reads the statistics from `postgres` (30-metrics-role.sh).
	SELECT 'GRANT CONNECT ON DATABASE postgres TO qd_metrics'
	WHERE EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'qd_metrics')
	\gexec
	SQL
echo "init: CONNECT on $POSTGRES_DB and postgres revoked from PUBLIC"
