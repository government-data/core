#!/bin/sh
# The API on this machine for watch mode (`task dev:watch`): rebuilt and
# restarted by watchexec on every change under crates/, against the dev
# stack's Postgres (published on 127.0.0.1:5432) and with the same Settings
# key as the container, so the secrets already saved still open.
set -eu
cd "$(dirname "$0")/../.."
command -v watchexec >/dev/null || {
  echo "watchexec is missing: cargo install --locked watchexec-cli" >&2
  exit 1
}
[ -f .env ] || { echo "no .env: cp .env.example .env (see development/local-setup)" >&2; exit 1; }
set -a
. ./.env
set +a

mkdir -p .dev
# The container's Settings key, once (it lives in the settings-key volume).
if [ ! -s .dev/settings.key ]; then
  volume="${COMPOSE_PROJECT_NAME:-data-quality}_settings-key"
  if docker volume inspect "$volume" >/dev/null 2>&1; then
    docker run --rm -v "$volume:/v:ro" alpine cat /v/settings.key >.dev/settings.key 2>/dev/null || rm -f .dev/settings.key
  fi
  [ -s .dev/settings.key ] && chmod 600 .dev/settings.key
fi

export DATABASE_URL="postgres://${APP_DB_USER:-quality_data_app}:${APP_DB_PASSWORD}@127.0.0.1:5432/${POSTGRES_DB:-quality_data}?sslmode=disable"
# JSON lines, as in a container, so Alloy can read them (below).
export APP_HOST=0.0.0.0 APP_PORT=8080 LOG_FORMAT=json
export APP_PUBLIC_URL=https://localhost OAUTH_REDIRECT_BASE=https://localhost/api
export SETTINGS_KEY_FILE="$PWD/.dev/settings.key"
# The catalog on the dev Dolt (published on 127.0.0.1), as the application's user.
export DOLT_URL="mysql://qd_app:${DOLT_APP_PASSWORD}@127.0.0.1:${DOLT_PORT:-3307}"
# The lake through Traefik, for the jobs' credentials (infra/rustfs/sts-issuer.sh).
export LAKE_ENDPOINT=https://localhost LAKE_CA_FILE="$PWD/infra/traefik/certs/dev-ca.crt"
# Trino's protocol, published on 127.0.0.1 (compose.watch.yml): a source on
# Trino keeps its catalog there, as the machine client qd-api (.env.example).
export QUALITY_TRINO_URL=https://127.0.0.1:8490 QUALITY_TRINO_CA_FILE="$PWD/infra/traefik/certs/dev-ca.crt"
# The installation's CA, once infra/traefik/gen-dev-internal.sh made it: machines
# can join and renew. INTERNAL_CLIENT_CERT_HEADER stays off: here the API is
# reachable on :8080 without Traefik (set it in .env only to try renewing).
if [ -f .dev/ca/ca.crt ]; then
  export CA_CERT_FILE="$PWD/.dev/ca/ca.crt" CA_KEY_FILE="$PWD/.dev/ca/ca.key"
fi

# The API's lines also go to .dev/logs/api.log, which Alloy reads in
# development (infra/observability/alloy/dev-host.alloy) as the container's
# `api`; cargo's output (standard error) stays in the terminal only. The file
# starts empty at each start of the server, so it does not grow without end.
# watchexec signals the whole command's process group, tee included.
mkdir -p .dev/logs
chmod 755 .dev/logs
log=.dev/logs/api.log
: >"$log"
chmod 644 "$log"
exec watchexec --restart --stop-signal SIGTERM \
  --watch crates --watch Cargo.toml --exts rs,toml,sql,json \
  -- ": >$log; cargo run -p server -- serve | tee -a $log"
