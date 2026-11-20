#!/bin/sh
# Back up the catalog (decision 0021, point 8): Dolt's own backup of every
# tenant database (all branches, tags and working sets) into the
# `dolt-backups` volume, one directory per database, then a copy of that
# volume to the lake's backup bucket. Run it from the repository root, with
# the stack up; until the application schedules it, a cron or systemd timer
# on the host:
#   infra/dolt/backup.sh
# Without DOLT_BACKUP_S3_ACCESS_KEY in .env only the local backup is made
# (infra/dolt/lake-backup-user.sh creates that user and its bucket).
#
# Restore one database (as a new name, then swap if it is the right one):
#   docker exec data-quality-dolt-dolt-1 dolt sql -q \
#     "CALL DOLT_BACKUP('restore', 'file:///backups/tenant_x', 'tenant_x_restored')"
set -eu
cd "$(dirname "$0")/../.."
env_file=${DOLT_ENV_FILE:-.env}
set -a; . "./$env_file"; set +a
# Outside development, without compose.dev.yml:
#   DOLT_COMPOSE_FILES="-f infra/dolt/docker-compose.yml"
# and with the machines' certificates (decision 0066):
#   DOLT_COMPOSE_FILES="-f infra/dolt/docker-compose.yml -f infra/dolt/certificate.yml"
files=${DOLT_COMPOSE_FILES:--f infra/dolt/docker-compose.yml -f infra/dolt/compose.dev.yml}
dc="docker compose --project-directory . --env-file $env_file -p ${DOLT_PROJECT:-data-quality-dolt} $files"

# Root, from inside the container's network: the only place it may connect
# from. With certificate.yml it presents the application VM's certificate
# through the one-shot `sql`, as Dolt refuses a connection without one.
case $files in
  *certificate.yml*)
    rows() { $dc --profile sql run --rm -T sql -N -B -e "$1"; } ;;
  *)
    rows() { $dc exec -T dolt dolt sql -r csv -q "$1" | tail -n +2; } ;;
esac
dbs=$(rows "SHOW DATABASES LIKE 'tenant\_%'")
for db in $dbs; do
  rows "USE \`$db\`; CALL DOLT_BACKUP('sync-url', 'file:///backups/$db')" >/dev/null
  echo "dolt: $db backed up"
done

if [ -n "${DOLT_BACKUP_S3_ACCESS_KEY:-}" ]; then
  $dc --profile backup run --rm backup-sync
  echo "dolt: backups copied to s3://${DOLT_BACKUP_BUCKET:-qd-backups}/dolt"
else
  echo "dolt: DOLT_BACKUP_S3_ACCESS_KEY not set, backups kept in the volume only"
fi
