#!/bin/sh
# Queues an evaluate job of the main tenant for a dataset's cut, as the
# application's role, as the scheduler will (decision 0018):
#   sh tools/dev/evaluate.sh clientes_naturales 2021-03-10
# Prints the job's id; `tools/dev/worker.sh` takes it.
set -eu
cd "$(dirname "$0")/../.."
set -a; . ./.env; set +a
dataset=${1:?dataset} cut=${2:?cut YYYY-MM-DD}
case $dataset in *[!a-z0-9_]*) echo "dataset: a-z 0-9 _" >&2; exit 2 ;; esac
case $cut in [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]) ;; *) echo "cut: YYYY-MM-DD" >&2; exit 2 ;; esac
docker exec -e PGPASSWORD="$APP_DB_PASSWORD" "${POSTGRES_CONTAINER:-data-quality-postgres-1}" \
  psql -qtA -h 127.0.0.1 -U "${APP_DB_USER:-knime_app}" -d "${POSTGRES_DB:-knime}" -c \
  "INSERT INTO quality.jobs (tenant_id, kind, dataset, parameters) VALUES ('00000000-0000-0000-0000-000000000001', 'evaluate', '$dataset', '{\"cut\": \"$cut\"}') RETURNING id"
