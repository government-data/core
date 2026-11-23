#!/bin/bash
# Grafana's read-only role (decision 0026, point 6): Grafana reads the audit
# and the run logs as `qd_grafana`, which can SELECT the columns below and
# nothing else, and never writes. Its whole read surface is here, beside
# Grafana, not in a migration: the application's schema never learns of it.
#
# Run as the superuser, inside the PostgreSQL container, after the API has
# run its migrations once; running it again changes nothing, a rotated
# password file takes effect, and a column dropped from the list below is
# revoked. In development tools/dev/grafana-role.sh runs it.
#
# Environment:
#   POSTGRES_USER, POSTGRES_DB   the superuser and the application's database
#                                (set in the postgres image)
#   QD_GRAFANA_PG_PASSWORD_FILE  the role's password, a file (production), or
#   QD_GRAFANA_PG_PASSWORD       the password itself, in the environment (the
#                                wrapper's); never on a command line
#
# The grants are the real control: a Grafana Viewer can send any SELECT
# through Grafana's query API, so what is not granted here is what Grafana
# can never show. One installation is one organisation (decision 0076): the
# role sets no `app.tenant_id`, so the policies' `current_tenant()` is the
# main tenant until the tenants' contraction.
set -euo pipefail
: "${POSTGRES_USER:?POSTGRES_USER must be set}"
: "${POSTGRES_DB:?POSTGRES_DB must be set}"

fail() { echo "grafana-role: $*" >&2; exit 1; }
# A query's rows, unaligned; the statement on standard input.
query() { psql -v ON_ERROR_STOP=1 -X -At --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" "$@"; }

if [ -n "${QD_GRAFANA_PG_PASSWORD_FILE:-}" ]; then
  [ -r "$QD_GRAFANA_PG_PASSWORD_FILE" ] || fail "cannot read QD_GRAFANA_PG_PASSWORD_FILE"
  QD_GRAFANA_PG_PASSWORD=$(cat "$QD_GRAFANA_PG_PASSWORD_FILE")
fi
[ -n "${QD_GRAFANA_PG_PASSWORD:-}" ] || fail "set QD_GRAFANA_PG_PASSWORD_FILE (or QD_GRAFANA_PG_PASSWORD)"
# psql reads it with \getenv: never on its command line.
export QD_GRAFANA_PG_PASSWORD

migrated=$(query <<<"SELECT to_regclass('quality.jobs') IS NOT NULL")
[ "$migrated" = t ] || fail "quality.jobs does not exist: start the API once, so it runs its migrations, then run this again"

# The password is set only when the role does not sign in with it already
# (over TCP, as Grafana does): a second run leaves even its hash as it was.
rotate=true
if PGPASSWORD=$QD_GRAFANA_PG_PASSWORD psql -X -At -h 127.0.0.1 -U qd_grafana -d "$POSTGRES_DB" \
  -c 'SELECT 1' >/dev/null 2>&1; then
  rotate=false
fi

query --set app_db="$POSTGRES_DB" --set rotate="$rotate" >/dev/null <<-'SQL'
	\getenv grafana_password QD_GRAFANA_PG_PASSWORD
	BEGIN;
	SELECT 'CREATE ROLE qd_grafana LOGIN'
	WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'qd_grafana')
	\gexec
	-- Every time, so the attributes and a rotated password hold.
	ALTER ROLE qd_grafana LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT
	  NOBYPASSRLS NOREPLICATION CONNECTION LIMIT 10;
	\if :rotate
	ALTER ROLE qd_grafana PASSWORD :'grafana_password';
	\endif
	-- No tenant of its own any more (decision 0076): a setting an earlier
	-- run left goes, and current_tenant() is the main tenant.
	ALTER ROLE qd_grafana RESET app.tenant_id;
	-- Defence in depth: the grants already forbid every write.
	ALTER ROLE qd_grafana SET default_transaction_read_only = on;
	ALTER ROLE qd_grafana SET statement_timeout = '30s';
	ALTER ROLE qd_grafana SET idle_in_transaction_session_timeout = '60s';

	GRANT CONNECT ON DATABASE :"app_db" TO qd_grafana;
	-- Start from nothing in every schema of the database, so the grants
	-- below are always the exact surface (a table-level REVOKE also revokes
	-- its columns' privileges).
	SELECT format('REVOKE ALL ON ALL TABLES IN SCHEMA %1$I FROM qd_grafana;'
	              ' REVOKE ALL ON ALL SEQUENCES IN SCHEMA %1$I FROM qd_grafana;'
	              ' REVOKE ALL ON SCHEMA %1$I FROM qd_grafana', nspname)
	  FROM pg_namespace
	 WHERE nspname NOT LIKE 'pg\_%' AND nspname <> 'information_schema'
	\gexec
	GRANT USAGE ON SCHEMA quality TO qd_grafana;

	-- The audit log and who acted (staff only: what an auditor sees in the app),
	-- with a view's reason and shape (0069): never a value.
	GRANT SELECT (id, tenant_id, occurred_at, event_type, admin_id, action, resource,
	  service_account, job_id, ip, user_agent, email_attempted, purpose, reference,
	  rows_shown, fields_shown) ON public.admin_audit_events TO qd_grafana;
	GRANT SELECT (id, tenant_id, email, display_name) ON public.admin_users TO qd_grafana;
	GRANT SELECT (id, tenant_id, name, role, pool, state, version, cert_not_after,
	  created_at, approved_at, retired_at) ON public.machines TO qd_grafana;
	-- The run log: jobs without their parameters or errors, cuts, timings.
	GRANT SELECT (id, tenant_id, kind, dataset, engine, state, priority, attempts,
	  max_attempts, not_before, worker, lease_until, created_at, started_at, finished_at,
	  uses_trino) ON quality.jobs TO qd_grafana;
	GRANT SELECT (id, tenant_id, dataset, cut, schedule_id, requested_by, state, run,
	  extract_job, evaluate_job, due_at, fired_at) ON quality.cuts TO qd_grafana;
	GRANT SELECT (tenant_id, dataset, cut, job_id, records, duration_ms, rules_commit,
	  reported_at, igcd, entities, band_high, band_medium, band_low) ON quality.run_summary TO qd_grafana;
	GRANT SELECT (tenant_id, dataset, cut, stage, position, ms) ON quality.job_stats TO qd_grafana;
	GRANT SELECT (tenant_id, machine, held_at, expires_at) ON quality.trino_holds TO qd_grafana;
	-- The results the charts read (crates/quality/charts/*.sql): aggregates,
	-- and entities only by their keyed hash.
	GRANT SELECT (tenant_id, dataset, cut, rule, applies, failures, field, pattern,
	  dimension, severity) ON quality.rule_results TO qd_grafana;
	GRANT SELECT (tenant_id, dataset, cut, field, empty, distinct_values, pattern,
	  pattern_count, mean_icd, recency) ON quality.field_quality TO qd_grafana;
	GRANT SELECT (tenant_id, dataset, cut, entity, icd, band, segment) ON quality.entity_icd TO qd_grafana;
	GRANT SELECT (tenant_id, dataset, cut, dimension, grp, records, serious, mean_icd)
	  ON quality.group_metrics TO qd_grafana;
	GRANT SELECT (tenant_id, dataset, cut, variable, dimension, grp, percentiles, records,
	  low, high) ON quality.distribution_checks TO qd_grafana;
	GRANT SELECT (tenant_id, dataset, cut, dimension, grp, since, open, resolved, priority)
	  ON quality.queue_findings TO qd_grafana;
	GRANT SELECT (tenant_id, dataset, cut, cell, ok, total, passes, missing, margin)
	  ON quality.cells TO qd_grafana;
	GRANT SELECT (tenant_id, dataset, dimension, grp, days) ON quality.remediation_slas TO qd_grafana;
	COMMIT;
	SQL
echo "grafana-role: qd_grafana reads tenant $tenant, read-only"
