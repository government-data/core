#!/bin/bash
# The metrics' role (decision 0071, points 6 and 10): Alloy's PostgreSQL
# exporter on the application VM connects as `qd_metrics` to read the
# statistics views and nothing else. It is a member of `pg_monitor` (which
# reads pg_stat_activity and pg_stat_database, no table), has no password
# and signs in only with the application VM's machine certificate over TLS,
# to the `postgres` database alone (tls/pg_hba.conf, tls/pg_ident.conf).
#
# Runs once on a new database (docker-entrypoint-initdb.d), as the
# superuser; on an existing one the installer pipes it into the container
# the same way (`docker compose exec -T postgres bash <` this file), as
# Grafana's role. Running it again changes nothing.
set -euo pipefail
: "${POSTGRES_USER:?POSTGRES_USER must be set}"

psql -v ON_ERROR_STOP=1 -X --username "$POSTGRES_USER" --dbname postgres >/dev/null <<-'SQL'
	SELECT 'CREATE ROLE qd_metrics LOGIN'
	WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'qd_metrics')
	\gexec
	-- Every time, so the attributes hold. INHERIT: pg_monitor's rights apply.
	ALTER ROLE qd_metrics LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE INHERIT
	  NOBYPASSRLS NOREPLICATION CONNECTION LIMIT 3 PASSWORD NULL;
	GRANT pg_monitor TO qd_metrics;
	ALTER ROLE qd_metrics SET default_transaction_read_only = on;
	ALTER ROLE qd_metrics SET statement_timeout = '10s';
	SQL
echo "init: the metrics' role qd_metrics, in pg_monitor, without a password"
