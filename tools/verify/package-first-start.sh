#!/bin/bash
# Verification of an installation's first start from the core's package
# (decision 0080, step 6; decision 0082, point 3), on this machine: the
# package tools/package.sh makes, extracted under a temporary /opt-like
# folder with a `current` link and a .env from infra/package/env.example,
# run with quality-sdk's agent's command line, the one-shots in between:
#
#   1. `server ca-init` into QD_MODULE_DIR/ca (as the API's user);
#      the one-shots below as quality-sdk's installer runs them
#      (packaging/install.sh): `run --rm --no-deps` of PostgreSQL's files
#      with the one-shot's overlay;
#   2. `up -d postgres`, without waiting: not healthy without TLS;
#   3. the app-machine one-shot: machine.pem into QD_MACHINE_DIR, and
#      PostgreSQL goes on with TLS (healthy);
#   4. the agent's `up --wait`: every service healthy, Traefik's copy of
#      machine.pem in QD_MODULE_DIR/internal, and every role of PostgreSQL's
#      first init made (the queue's and the metrics' too);
#   5. the updater-certificate one-shot into a stand-in for
#      /etc/quality-data/internal, then internal.yml and machines-ca.crt
#      copied into QD_MODULE_DIR/internal, aside then renamed.
#
# It also reports how long the first machine.pem lasts. Then, with no
# restart of Traefik: the internal entry point refuses a
# client without a certificate and, with the agent's, reaches the API on
# /api/updater/v1/version; and the node agent (`node`, decision 0084)
# renews machine.pem over the renew route (QD_NODE_RENEW_EVERY=60s), which
# Traefik and PostgreSQL then serve without a restart. The API is on a network only
# Traefik shares with it and trusts Traefik's certificate header (decision
# 0083): a container on data-quality-edge does not reach it, by name or by
# any of its addresses.
#
# The images are CI's (decision 0082, point 4), never built here:
#
#   gh run download <run> -n image-api -n image-web -D <dir>
#   docker load -i <dir>/qd-api.tar && docker load -i <dir>/qd-web.tar
#   QD_API_IMAGE=qd-api:<sha> QD_WEB_IMAGE=qd-web:<sha> \
#     bash tools/verify/package-first-start.sh
#
# PostgreSQL and Traefik are postgres:16-alpine and traefik:v3.6. It
# publishes 80, 443, 9443-9445, 9009 and 5432 (the last three on
# 127.0.0.1) and uses the fixed networks data-quality-edge,
# data-quality-backend and data-quality-api, so the dev stack must be down. Everything else is
# named qs-verify-first… and removed on exit, also on failure. Run alone
# (no build running), from the repository root. Needs docker, curl and
# openssl.
set -u
export LC_ALL=C
API=${QD_API_IMAGE:?set QD_API_IMAGE to the API image of a CI artifact}
WEB=${QD_WEB_IMAGE:?set QD_WEB_IMAGE to the web image of a CI artifact}
ALPINE=${ALPINE_IMAGE:-alpine:3}
CURL=${CURL_IMAGE:-curlimages/curl:8.11.1}
P=qs-verify-first
NAME=qd-app
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
D=$(mktemp -d)
chmod 755 "$D"
MOD=$D/opt/quality-data/quality-data
NODE=$D/node
UPD=$D/updater
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
found() { printf '[INFO] %s\n' "$*"; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }
dc() { # the agent's command line, then ARGS
  docker compose --project-name "$P" --project-directory "$MOD/current" \
    --file "$MOD/current/compose.yml" "${EXTRA[@]}" --env-file "$MOD/.env" "$@"
}
EXTRA=()
cleanup() {
  EXTRA=()
  dc down --volumes --remove-orphans --timeout 5 >/dev/null 2>&1
  docker rm -f "$P-ca" "$P-read" "$P-edge" >/dev/null 2>&1
  # Files the containers' users wrote (uid 65532, 70).
  docker run --rm -v "$D:/d" "$ALPINE" sh -c 'rm -rf /d/* /d/.[!.]*' >/dev/null 2>&1
  rm -rf -- "${D:?}"
}
trap cleanup EXIT
umask 022
echo "Docker $(docker version --format '{{.Server.Version}}'), $(docker compose version)"
echo "API $API, web $WEB"

# --- Before: the images, the ports and the networks --------------------------
if ! docker run --rm --entrypoint /usr/local/bin/server "$API" updater-certificate 2>&1 | grep -q usage; then
  ko "$API has no 'server updater-certificate'"
  exit 1
fi
if ! docker run --rm --entrypoint /usr/local/bin/qd-node "$API" 2>&1 | grep -q QD_AGENT_DIR; then
  ko "$API has no qd-node"
  exit 1
fi
for n in data-quality-edge data-quality-backend data-quality-api; do
  if docker network inspect "$n" >/dev/null 2>&1; then ko "the network $n exists: stop the dev stack"; exit 1; fi
done
for port in 80 443 9443 9444 9445 9009 5432; do
  if ss -Hltn "sport = :$port" | grep -q .; then ko "port $port is in use"; exit 1; fi
done

# --- The package, the .env and the files beside the version ------------------
VERSION=$(tr -d '[:space:]' <"$ROOT/version.txt")
OUT_DIR=$D "$ROOT/tools/package.sh" "$VERSION" >/dev/null || { ko "tools/package.sh"; exit 1; }
mkdir -p "$MOD/$VERSION" "$MOD/secrets" "$MOD/certs" "$MOD/internal" "$MOD/ca" "$NODE" "$UPD"
tar -xzf "$D/quality-data-$VERSION.tar.gz" -C "$MOD/$VERSION"
ln -s "$VERSION" "$MOD/current"
# As the installer's owners: the API's user (65532) owns the CA's, the node
# agent's and the agent's folders (the one-shots set their modes, which only
# the owner may: with a folder of another user they stop with "Operation not
# permitted").
asroot() { docker run --rm --name "$P-read" -v "$D:$D" "$ALPINE" sh -c "$1"; }
asroot "chown 65532:65532 '$MOD/ca' '$NODE' '$UPD'"
SUPER=$(openssl rand -hex 16)
APPPW=$(openssl rand -hex 16)
printf '%s' "$SUPER" >"$MOD/secrets/postgres_superuser_password"
printf 'postgres://knime_app:%s@%s:5432/knime?sslmode=verify-full&sslrootcert=/run/qd-ca/ca.crt' \
  "$APPPW" "$NAME" >"$MOD/secrets/database_url"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/CN=localhost" -addext "subjectAltName=DNS:localhost" \
  -keyout "$MOD/certs/local.key" -out "$MOD/certs/local.crt" 2>/dev/null
chmod 644 "$MOD"/secrets/* "$MOD"/certs/*
sed -e "s|^QD_MODULE_DIR=.*|QD_MODULE_DIR=$MOD|" -e "s|^QD_MACHINE_DIR=.*|QD_MACHINE_DIR=$NODE|" \
  -e "s|^QD_CA_DIR=.*|QD_CA_DIR=$MOD/ca|" -e "s|^QD_UPDATER_DIR=.*|QD_UPDATER_DIR=$UPD|" \
  -e "s|^IMAGE_API=.*|IMAGE_API=$API|" -e "s|^IMAGE_WEB=.*|IMAGE_WEB=$WEB|" \
  -e "s|^APP_DOMAIN=.*|APP_DOMAIN=localhost|" -e "s|^INTERNAL_DOMAIN=.*|INTERNAL_DOMAIN=$NAME|" \
  -e "s|^POSTGRES_SERVER_NAME=.*|POSTGRES_SERVER_NAME=$NAME|" -e "s|^OBS_VM=.*|OBS_VM=$P|" \
  -e "s|^ACME_EMAIL=.*|ACME_EMAIL=verify@example.invalid|" \
  -e "s|^POSTGRES_SUPERUSER_PASSWORD=.*|POSTGRES_SUPERUSER_PASSWORD=$SUPER|" \
  -e "s|^APP_DB_PASSWORD=.*|APP_DB_PASSWORD=$APPPW|" \
  "$ROOT/infra/package/env.example" >"$MOD/.env"
cat >>"$MOD/.env" <<EOF
QD_METRICS_VOLUME=$P-metrics
QD_TRAEFIK_CHECK_EVERY=5
QD_PG_TLS_CHECK_EVERY=5
QD_NODE_RENEW_EVERY=60s
POSTGRES_BIND=127.0.0.1
TRAEFIK_METRICS_BIND=127.0.0.1
MIMIR_BIND=127.0.0.1
EOF
if out=$(dc config --quiet 2>&1); then ok "the package's compose reads with the .env"; else ko "config: $out"; exit 1; fi
oneshot() { # NAME ARGS: an overlay of the package's PostgreSQL, as the installer runs it
  local name=$1
  shift
  docker compose --project-name "$P" --env-file "$MOD/.env" \
    --file "$MOD/current/infra/postgres/docker-compose.yml" --file "$MOD/current/infra/postgres/tls.yml" \
    --file "$MOD/current/infra/postgres/$name.yml" --profile "$name" run --rm --no-deps "$name" "$@"
}
health() { docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$(dc ps -q "$1")" 2>/dev/null; }

# --- 1. ca-init -----------------------------------------------------------------
# As quality-sdk's installer runs it (packaging/install.sh).
if out=$(docker run --rm --name "$P-ca" --user 65532:65532 --network none -v "$MOD/ca:/run/qd-ca" \
  --entrypoint /usr/local/bin/server "$API" ca-init --name "qs verify" --cert /run/qd-ca/ca.crt --key /run/qd-ca/ca.key 2>&1); then
  ok "1. ca-init (as the image's user)"
else
  ko "1. ca-init: $out"
fi

# --- 2. PostgreSQL alone, without waiting --------------------------------------
dc up --detach --no-build postgres >/dev/null 2>&1 || ko "2. up -d postgres"
sleep 15
case $(health postgres) in
  healthy) ko "2. PostgreSQL healthy without machine.pem" ;;
  *) ok "2. PostgreSQL up, not healthy without TLS: $(health postgres)" ;;
esac

# --- 3. app-machine --------------------------------------------------------------
if out=$(oneshot app-machine --name "$NAME" --dir /run/qd-node 2>&1); then
  ok "3. app-machine: $(printf '%s' "$out" | tail -n 1)"
else
  ko "3. app-machine: $(printf '%s' "$out" | tail -n 3)"
fi
for _ in $(seq 30); do [ "$(health postgres)" = healthy ] && break; sleep 2; done
expect "3. PostgreSQL with TLS, by itself" healthy "$(health postgres)"
FIRST=$(asroot "cat '$NODE/machine.pem'" | openssl x509 -noout -serial 2>/dev/null)

# --- 4. The agent's up -------------------------------------------------------------
if out=$(dc up --detach --no-build --remove-orphans --wait --wait-timeout 300 2>&1); then
  ok "4. the agent's up --wait"
else
  ko "4. up: $(printf '%s' "$out" | tail -n 3)"
  dc logs --no-log-prefix --tail 10 api 2>&1
fi
expect "4. running" "api node postgres traefik web" \
  "$(dc ps --services --status running 2>/dev/null | sort | tr '\n' ' ' | sed 's/ $//')"
expect "4. PostgreSQL's first init made every role" "knime_app,qd_metrics,qd_queue" \
  "$(docker exec "$(dc ps -q postgres)" psql -U postgres -d knime -XAtc \
    "SELECT string_agg(rolname, ',' ORDER BY rolname) FROM pg_roles WHERE rolname IN ('knime_app','qd_queue','qd_metrics')" 2>&1)"
expect "4. PostgreSQL never restarted" 0 "$(docker inspect --format '{{.RestartCount}}' "$(dc ps -q postgres)")"
for _ in $(seq 15); do [ -s "$MOD/internal/app-machine.pem" ] && break; sleep 1; done
expect "4. Traefik's copy of machine.pem in internal/" yes "$([ -s "$MOD/internal/app-machine.pem" ] && echo yes || echo no)"
found "4. the first machine.pem lasts until $(asroot "cat '$NODE/machine.pem'" |
  openssl x509 -noout -enddate 2>/dev/null | cut -d= -f2) (issued $(date -u '+%b %e %H:%M:%S %Y GMT'))"
TRAEFIK=$(dc ps -q traefik)
STARTED=$(docker inspect --format '{{.State.StartedAt}}' "$TRAEFIK")

# --- 5. updater-certificate, then internal.yml and the CA in internal/ ---------
if out=$(oneshot updater-certificate --name qs-verify-updater --dir /run/qd-updater 2>&1); then
  ok "5. updater-certificate: $(printf '%s' "$out" | tail -n 1)"
else
  ko "5. updater-certificate: $(printf '%s' "$out" | tail -n 3)"
fi
# As the installer does (root): aside then renamed, the CA first.
put() { asroot "cp '$1' '$MOD/internal/.$2.new' && chmod 644 '$MOD/internal/.$2.new' && mv -f '$MOD/internal/.$2.new' '$MOD/internal/$2'"; }
put "$MOD/ca/ca.crt" machines-ca.crt
put "$MOD/current/infra/traefik/internal.yml" internal.yml
# The agent's files, as the agent (root) reads them.
mkdir -p "$D/client"
asroot "cp '$UPD/ca.pem' '$UPD/certificate.pem' '$UPD/private-key.pem' '$D/client/' &&
  chown $(id -u) '$D'/client/* && chmod 600 '$D'/client/*"

# --- The internal entry point, Traefik not restarted ----------------------------
internal() { # PATH [curl args]: the status
  local path=$1
  shift
  curl -s -o /dev/null -w '%{http_code}' --max-time 5 --cacert "$D/client/ca.pem" \
    --resolve "$NAME:9443:127.0.0.1" "$@" "https://$NAME:9443$path"
}
agent() { internal "$1" --cert "$D/client/certificate.pem" --key "$D/client/private-key.pem"; }
for _ in $(seq 20); do [ "$(agent /api/updater/v1/version)" != 000 ] && break; sleep 1; done
expect "without a client certificate: no handshake" 000 "$(internal /api/updater/v1/version)"
for _ in $(seq 20); do [ "$(agent /api/updater/v1/version)" = 200 ] && break; sleep 1; done
expect "the agent's certificate reaches /api/updater/v1/version" 200 "$(agent /api/updater/v1/version)"
expect "Traefik was not restarted" "$STARTED" "$(docker inspect --format '{{.State.StartedAt}}' "$TRAEFIK")"

# --- The node agent renews machine.pem, taken without a restart (0084) ------------
# QD_NODE_RENEW_EVERY (60s here, 8h by default) is how often it renews.
served() { # PORT [openssl args]: the serial the port serves
  openssl s_client -connect "127.0.0.1:$1" -servername "$NAME" "${@:2}" </dev/null 2>/dev/null |
    openssl x509 -noout -serial 2>/dev/null; }
held() { asroot "cat '$NODE/machine.pem'" | openssl x509 -noout -serial 2>/dev/null; }
POSTGRES=$(dc ps -q postgres)
PG_STARTED=$(docker inspect --format '{{.State.StartedAt}}' "$POSTGRES")
for _ in $(seq 150); do [ "$(held)" != "$FIRST" ] && break; sleep 1; done
renewed=$(held)
[ "$renewed" != "$FIRST" ] && ok "the node agent renewed machine.pem: $renewed" ||
  ko "the node agent did not renew machine.pem in 150 s"
dc logs --no-log-prefix --tail 5 node 2>&1 | sed 's/^/  node: /'
traefik_ok=no pg_ok=no
for _ in $(seq 40); do
  now=$(held)
  [ "$now" != "$FIRST" ] && [ "$(served 9443)" = "$now" ] && traefik_ok=yes
  [ "$now" != "$FIRST" ] && [ "$(served 5432 -starttls postgres)" = "$now" ] && pg_ok=yes
  [ "$traefik_ok$pg_ok" = yesyes ] && break
  sleep 1
done
expect "the internal entry point serves the renewed certificate" yes "$traefik_ok"
expect "PostgreSQL serves the renewed certificate" yes "$pg_ok"
expect "Traefik was not restarted" "$STARTED" "$(docker inspect --format '{{.State.StartedAt}}' "$TRAEFIK")"
expect "PostgreSQL was not restarted" "$PG_STARTED" "$(docker inspect --format '{{.State.StartedAt}}' "$POSTGRES")"
expect "the node agent never restarted" 0 "$(docker inspect --format '{{.RestartCount}}' "$(dc ps -q node)")"
expect "the node agent's networks" "${P}_default" \
  "$(docker inspect --format '{{range $n, $_ := .NetworkSettings.Networks}}{{$n}} {{end}}' "$(dc ps -q node)" | sed 's/ $//')"

# --- Only Traefik reaches the API (0083) ------------------------------------------
edge() { docker run --rm --name "$P-edge" --network data-quality-edge "$CURL" \
  -s -o /dev/null -w '%{http_code}' --max-time 3 "$1" 2>/dev/null; }
expect "from data-quality-edge, the web (the probe works)" 200 "$(edge http://web:8080/)"
expect "from data-quality-edge, the API by name" 000 "$(edge http://api:8080/)"
for ip in $(docker inspect --format '{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}' "$(dc ps -q api)"); do
  expect "from data-quality-edge, the API at $ip" 000 "$(edge "http://$ip:8080/")"
done
expect "the API's networks" "data-quality-api data-quality-backend" \
  "$(docker inspect --format '{{range $n, $_ := .NetworkSettings.Networks}}{{$n}} {{end}}' "$(dc ps -q api)" | sed 's/ $//')"

[ "$FAILED" = 0 ] && echo "All checks passed." || echo "Some checks failed."
exit "$FAILED"
