#!/bin/bash
# The queue's role (decisions 0018, 0023): workers connect as `qd_queue` to
# take, renew and finish jobs, and to nothing else; the quality module's
# migrations grant it that (crates/quality/migrations/0002_queue.sql) and
# need it to exist first. Runs once, on a new database; on an existing one
# run tools/dev/queue-role.sh.
#
# QD_QUEUE_AUTH says how workers authenticate:
# - `password` (the default, development): QD_QUEUE_PASSWORD.
# - `certificate` (infra/postgres/tls.yml, decision 0066): the machine's
#   certificate (pg_hba.conf, `cert`), so the role has no password, and a
#   password it had is removed. The application's role ($APP_DB_USER) gets
#   `pg_read_all_stats` and `pg_signal_backend`, to see which machine each
#   queue session is (`pg_stat_ssl.client_dn`) and end a retired machine's
#   sessions at once (0066, point 4).
set -euo pipefail
case "${QD_QUEUE_AUTH:-password}" in
password)
	: "${QD_QUEUE_PASSWORD:?QD_QUEUE_PASSWORD must be set (or QD_QUEUE_AUTH=certificate)}"
	psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" \
		--set queue_password="$QD_QUEUE_PASSWORD" --set app_db="$POSTGRES_DB" <<-'SQL'
		SELECT format('CREATE ROLE qd_queue LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS PASSWORD %L', :'queue_password')
		WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'qd_queue')
		\gexec
		GRANT CONNECT ON DATABASE :"app_db" TO qd_queue;
		SQL
	echo "init: created the queue's role qd_queue, with a password"
	;;
certificate)
	: "${APP_DB_USER:?APP_DB_USER must be set}"
	psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" \
		--set app_user="$APP_DB_USER" --set app_db="$POSTGRES_DB" <<-'SQL'
		SELECT 'CREATE ROLE qd_queue LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS'
		WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'qd_queue')
		\gexec
		ALTER ROLE qd_queue PASSWORD NULL;
		GRANT CONNECT ON DATABASE :"app_db" TO qd_queue;
		GRANT pg_read_all_stats, pg_signal_backend TO :"app_user";
		SQL
	echo "init: created the queue's role qd_queue, without a password (machine certificates)"
	;;
*)
	echo "init: QD_QUEUE_AUTH must be password or certificate" >&2
	exit 1
	;;
esac
