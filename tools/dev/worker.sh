#!/bin/sh
# A qd-worker on this machine against the dev stack (decision 0018): the
# queue on the published Postgres as the queue's role, the API and the lake
# through Traefik with the development CA, as the dev machine client
# `qd-worker-dev` (allowed the audience quality-api, see .env.example).
#   sh tools/dev/worker.sh            # RUST_LOG=debug for more
set -eu
cd "$(dirname "$0")/../.."
set -a; . ./.env; set +a
: "${QD_QUEUE_PASSWORD:?run tools/dev/queue-role.sh first}"
: "${QD_WORKER_DEV_CLIENT_SECRET:?register qd-worker-dev (see .env.example)}"
export QD_QUEUE_DATABASE_URL="postgres://qd_queue:${QD_QUEUE_PASSWORD}@127.0.0.1:5432/${POSTGRES_DB:-knime}?sslmode=disable"
export QD_API_URL=https://localhost/api QD_CA_FILE="$PWD/infra/traefik/certs/dev-ca.crt"
export QD_WORKER_CLIENT_ID=qd-worker-dev QD_WORKER_CLIENT_SECRET="$QD_WORKER_DEV_CLIENT_SECRET"
# In release, as its image is built: a debug DuckDB asserts on the memory of
# an extension built for release (postgres, mysql), which an extraction loads
# (decision 0037, found 2026-10-31).
# JSON lines, as in its container, also to .dev/logs/worker.log, which Alloy
# reads in development (infra/observability/alloy/dev-host.alloy) as the
# container's `worker`; empty at each start. A second worker at the same
# time gets a file of its own, so two never write into one:
#   QD_DEV_WORKER=b sh tools/dev/worker.sh   # .dev/logs/worker-b.log
export LOG_FORMAT=json
case ${QD_DEV_WORKER:-} in *[!a-z0-9]*) echo "QD_DEV_WORKER: a-z 0-9" >&2; exit 2 ;; esac
log=.dev/logs/worker${QD_DEV_WORKER:+-$QD_DEV_WORKER}.log
mkdir -p .dev/logs
chmod 755 .dev/logs
: >"$log"
chmod 644 "$log"
cargo run -q --release --locked -p worker --bin qd-worker | tee -a "$log"
