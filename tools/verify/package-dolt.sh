#!/bin/bash
# Verification of Dolt in the core's package (decision 0080's amendment of
# 2026-11-24), on this machine: the package tools/package.sh makes, laid
# out as an installation (a `current` link, the .env from
# infra/package/env.example, the files beside the version), started in
# quality-sdk's installer's order and with the agent's command line
# (decision 0082), then:
#
#   1. Dolt is healthy, on its own network with the API only, published
#      nowhere; its first start made root, qd_app and the catalog's
#      database, with the passwords from secrets/ (0400, the API's user's);
#   2. qd_app signs in with its password over TLS presenting the machine's
#      certificate and sees the catalog's database; a wrong password, no
#      certificate, are refused;
#   3. the API reaches Dolt: POST /internal/machines/dolt/hold with the
#      application machine's certificate answers free with Dolt's own
#      count of running statements (the API asked Dolt over its DOLT_URL,
#      VERIFY_IDENTITY by `dolt`, its DOLT_PASSWORD_FILE), then the hold is
#      released; the machine's certificate names `dolt`;
#   4. the agent's `up` again changes nothing: Dolt is not recreated nor
#      initialised again;
#   5. an installation from before Dolt: Dolt and its volume removed, the
#      agent's `up` adds it to the running stack, initialised as new, and
#      the API reaches it again.
#
# The images are CI's, never built here (an API image with DOLT_PASSWORD
# and the application machine's name `dolt`, decision 0080's amendment):
#
#   gh run download <run> -n image-api -n image-web -D <dir>
#   docker load -i <dir>/qd-api.tar && docker load -i <dir>/qd-web.tar
#   QD_API_IMAGE=qd-api:<sha> QD_WEB_IMAGE=qd-web:<sha> bash tools/verify/package-dolt.sh
#
# Publishes only 127.0.0.1:59443 (the internal entry point) and
# 127.0.0.1:55432 (PostgreSQL); not 80 nor 443. Uses the fixed networks
# data-quality-edge, -backend, -api, -db and -catalog, so the dev stack
# must be down. Its volumes are qs-verify-dolt_* (APP_STACK), never the dev
# stack's; everything is removed on exit, also on failure. Run alone (no
# build running), from the repository root. Needs docker, curl, openssl.
set -u
export LC_ALL=C
API=${QD_API_IMAGE:?set QD_API_IMAGE to the API image of a CI artifact}
WEB=${QD_WEB_IMAGE:?set QD_WEB_IMAGE to the web image of a CI artifact}
DOLT=${QD_DOLT_IMAGE:-dolthub/dolt-sql-server@sha256:24bdcff7a90bef6d15e8825be502f1c057842d079411390448dc13398c8bf619}
ALPINE=${ALPINE_IMAGE:-alpine:3}
MYSQL=${MYSQL_IMAGE:-mysql:8.4}
P=qs-verify-dolt
NAME=qd-app
DB=tenant_00000000000000000000000000000001
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
D=$(mktemp -d)
chmod 755 "$D"
MOD=$D/opt/quality-data/quality-data
NODE=$D/node
UPD=$D/updater
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }
dc() { # the agent's command line (with this script's ports overlay), then ARGS
  docker compose --project-name "$P" --project-directory "$MOD/current" \
    --file "$MOD/current/compose.yml" --file "$D/ports.yml" --env-file "$MOD/.env" "$@"
}
asroot() { docker run --rm --name "$P-root" -v "$D:$D" "$ALPINE" sh -c "$1"; }
cleanup() {
  dc down --volumes --remove-orphans --timeout 5 >/dev/null 2>&1
  docker rm -f "$P-root" "$P-ca" >/dev/null 2>&1
  docker run --rm -v "$D:/d" "$ALPINE" sh -c 'rm -rf /d/* /d/.[!.]*' >/dev/null 2>&1
  rm -rf -- "${D:?}"
}
trap cleanup EXIT
umask 022
echo "Docker $(docker version --format '{{.Server.Version}}'), $(docker compose version)"

for n in data-quality-edge data-quality-backend data-quality-api data-quality-db data-quality-catalog; do
  if docker network inspect "$n" >/dev/null 2>&1; then ko "the network $n exists: stop the dev stack"; exit 1; fi
done
for port in 59443 55432; do
  if ss -Hltn "sport = :$port" | grep -q .; then ko "port $port is in use"; exit 1; fi
done

# --- The package and the files beside the version, as the installer writes them
VERSION=$(tr -d '[:space:]' <"$ROOT/version.txt")
OUT_DIR=$D "$ROOT/tools/package.sh" "$VERSION" >/dev/null || { ko "tools/package.sh"; exit 1; }
mkdir -p "$MOD/$VERSION" "$MOD/secrets" "$MOD/certs" "$MOD/internal" "$MOD/ca" "$NODE" "$UPD" "$D/client"
tar -xzf "$D/quality-data-$VERSION.tar.gz" -C "$MOD/$VERSION"
ln -s "$VERSION" "$MOD/current"
SUPER=$(openssl rand -hex 16)
APPPW=$(openssl rand -hex 16)
printf '%s' "$SUPER" >"$MOD/secrets/postgres_superuser_password"
printf 'postgres://quality_data_app:%s@%s:5432/quality_data?sslmode=verify-full&sslrootcert=/run/qd-ca/ca.crt' \
  "$APPPW" "$NAME" >"$MOD/secrets/database_url"
# As the update agent makes them: random bytes, base64url, no newline; the
# OIDC keys PKCS#8 in base64 (module.yaml's secrets).
for s in dolt-root-password dolt-app-password rustfs-sts-issuer-secret-key; do
  openssl rand -base64 32 | tr '+/' '-_' | tr -d '=\n' >"$MOD/secrets/$s"
done
openssl rand -base64 16 | tr '+/' '-_' | tr -d '=\n' >"$MOD/secrets/rustfs-sts-issuer-access-key"
# The OIDC keys as the agent's (ring's PKCS#8), by the API's own command.
keygen() { docker run --rm --network none --entrypoint /usr/local/bin/server "$API" oidc-keygen "$@" | tr -d '
'; }
keygen >"$MOD/secrets/oidc-signing-key"
keygen --es256 >"$MOD/secrets/oidc-signing-key-es256"
GEN="dolt-root-password dolt-app-password rustfs-sts-issuer-access-key rustfs-sts-issuer-secret-key oidc-signing-key oidc-signing-key-es256"
DOLT_APP=$(cat "$MOD/secrets/dolt-app-password")
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/CN=localhost" -addext "subjectAltName=DNS:localhost" \
  -keyout "$MOD/certs/local.key" -out "$MOD/certs/local.crt" 2>/dev/null
chmod 644 "$MOD/secrets/postgres_superuser_password" "$MOD/secrets/database_url" "$MOD"/certs/*
# The Dolt passwords as the agent leaves them: 0400, the API's user's, in a
# 0700 root folder (quality-sdk decision 0009).
asroot "chown 65532:65532 '$MOD/ca' '$NODE' '$UPD' && cd '$MOD/secrets' && chown 65532:65532 $GEN &&
  chmod 400 $GEN && chmod 700 ."
sed -e "s|^QD_MODULE_DIR=.*|QD_MODULE_DIR=$MOD|" -e "s|^QD_MACHINE_DIR=.*|QD_MACHINE_DIR=$NODE|" \
  -e "s|^QD_CA_DIR=.*|QD_CA_DIR=$MOD/ca|" -e "s|^QD_UPDATER_DIR=.*|QD_UPDATER_DIR=$UPD|" \
  -e "s|^IMAGE_API=.*|IMAGE_API=$API|" -e "s|^IMAGE_WEB=.*|IMAGE_WEB=$WEB|" -e "s|^IMAGE_DOLT=.*|IMAGE_DOLT=$DOLT|" \
  -e "s|^APP_DOMAIN=.*|APP_DOMAIN=localhost|" -e "s|^INTERNAL_DOMAIN=.*|INTERNAL_DOMAIN=$NAME|" \
  -e "s|^POSTGRES_SERVER_NAME=.*|POSTGRES_SERVER_NAME=$NAME|" -e "s|^OBS_VM=.*|OBS_VM=$P|" \
  -e "s|^POSTGRES_PORT=.*|POSTGRES_PORT=55432|" \
  -e "s|^POSTGRES_SUPERUSER_PASSWORD=.*|POSTGRES_SUPERUSER_PASSWORD=$SUPER|" \
  -e "s|^APP_DB_PASSWORD=.*|APP_DB_PASSWORD=$APPPW|" \
  "$ROOT/infra/package/env.example" >"$MOD/.env"
cat >>"$MOD/.env" <<EOF
APP_STACK=$P
QD_METRICS_VOLUME=$P-metrics
POSTGRES_BIND=127.0.0.1
EOF
# Not the package's: this machine must not publish 80, 443 or 9443.
cat >"$D/ports.yml" <<'EOF'
services:
  traefik:
    ports: !override
      - "127.0.0.1:59443:9443"
EOF
if out=$(dc config --quiet 2>&1); then ok "the package's compose reads with the .env"; else ko "config: $out"; exit 1; fi
others=$(dc config --format json | grep -o '"name": *"[^"]*dolt[^"]*"' | grep -v "\"$P" || true)
[ -z "$others" ] || { ko "a Dolt volume that is not this script's: $others"; exit 1; }
oneshot() { # NAME ARGS: an overlay of the package's PostgreSQL, as the installer runs it
  local name=$1
  shift
  docker compose --project-name "$P" --env-file "$MOD/.env" \
    --file "$MOD/current/infra/postgres/docker-compose.yml" --file "$MOD/current/infra/postgres/tls.yml" \
    --file "$MOD/current/infra/postgres/$name.yml" --profile "$name" run --rm --no-deps "$name" "$@"
}
health() { docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$(dc ps -q "$1")" 2>/dev/null; }

# --- The first start, in the installer's order (decision 0082) ------------------
docker run --rm --name "$P-ca" --user 65532:65532 --network none -v "$MOD/ca:/run/qd-ca" \
  --entrypoint /usr/local/bin/server "$API" ca-init --name "qs verify" \
  --cert /run/qd-ca/ca.crt --key /run/qd-ca/ca.key >/dev/null 2>&1 || ko "ca-init"
dc up --detach --no-build postgres >/dev/null 2>&1 || ko "up -d postgres"
for _ in $(seq 30); do oneshot app-machine --name "$NAME" --dir /run/qd-node >/dev/null 2>&1 && break; sleep 2; done
if out=$(dc up --detach --no-build --remove-orphans --wait --wait-timeout 300 2>&1); then
  ok "the agent's up --wait"
else
  ko "up: $(printf '%s' "$out" | tail -n 3)"
  dc logs --no-log-prefix --tail 20 dolt api 2>&1
fi
oneshot updater-certificate --name qs-verify-updater --dir /run/qd-updater >/dev/null 2>&1 || ko "updater-certificate"
put() { asroot "cp '$1' '$MOD/internal/.$2.new' && chmod 644 '$MOD/internal/.$2.new' && mv -f '$MOD/internal/.$2.new' '$MOD/internal/$2'"; }
put "$MOD/ca/ca.crt" machines-ca.crt
put "$MOD/current/infra/traefik/internal.yml" internal.yml
# The application machine's certificate, as a client of the internal entry point.
asroot "cp '$NODE/machine.pem' '$NODE/ca.crt' '$D/client/' && chown $(id -u) '$D'/client/* && chmod 600 '$D'/client/*"

# --- 1. Dolt, its network, its first start --------------------------------------
DOLT_ID=$(dc ps -q dolt)
expect "1. Dolt" healthy "$(health dolt)"
expect "1. running" "api dolt node postgres traefik web" \
  "$(dc ps --services --status running 2>/dev/null | sort | tr '\n' ' ' | sed 's/ $//')"
expect "1. Dolt's networks" "data-quality-catalog" \
  "$(docker inspect --format '{{range $k, $v := .NetworkSettings.Networks}}{{$k}} {{end}}' "$DOLT_ID" | sed 's/ $//')"
expect "1. Dolt published nowhere" "" "$(docker port "$DOLT_ID")"
expect "1. the network is internal" true "$(docker network inspect --format '{{.Internal}}' data-quality-catalog)"
logs=$(dc logs --no-log-prefix dolt 2>&1)
expect "1. its first init, once" 1 "$(printf '%s\n' "$logs" | grep -c 'first init done')"
expect "1. the catalog's database made for qd_app" 1 "$(printf '%s\n' "$logs" | grep -c "database $DB ready for qd_app")"

# --- 2. qd_app over TLS, with the machine's certificate ---------------------------
mysql() { # PASSWORD [mysql args]: a client on Dolt's network, the CA checked
  local pw=$1
  shift
  docker run --rm --network data-quality-catalog -v "$D/client:/c:ro" -e MYSQL_PWD="$pw" "$MYSQL" \
    mysql --host=dolt --user=qd_app --ssl-mode=VERIFY_IDENTITY --ssl-ca=/c/ca.crt "$@" -N -B 2>&1
}
asroot "chmod 644 '$D'/client/*"
cert=(--ssl-cert=/c/machine.pem --ssl-key=/c/machine.pem)
expect "2. qd_app sees the catalog's database" "$DB" "$(mysql "$DOLT_APP" "${cert[@]}" -e "SHOW DATABASES LIKE 'tenant\\_%'")"
case $(mysql wrong "${cert[@]}" -e "SELECT 1") in
  *"Access denied"*) ok "2. a wrong password: refused" ;; *) ko "2. a wrong password was not refused" ;;
esac
case $(mysql "$DOLT_APP" -e "SELECT 1") in
  1) ko "2. without a client certificate: accepted" ;; *) ok "2. without a client certificate: refused" ;;
esac

# --- 3. The API reaches Dolt ----------------------------------------------------------
case $(openssl x509 -in "$D/client/machine.pem" -noout -ext subjectAltName 2>/dev/null) in
  *"DNS:$NAME"*"DNS:dolt"*) ok "3. the machine's certificate names $NAME and dolt" ;;
  *) ko "3. the machine's certificate does not name dolt" ;;
esac
machine() { # PATH: the body, as the application machine
  curl -s --max-time 40 --cacert "$D/client/ca.crt" --cert "$D/client/machine.pem" \
    --resolve "$NAME:59443:127.0.0.1" -X POST "https://$NAME:59443/api$1"
}
reaches() { # LABEL
  local out=""
  for _ in $(seq 20); do
    out=$(machine /internal/machines/dolt/hold)
    case $out in *'"free":true'*) break ;; esac
    sleep 2
  done
  case $out in
    *'"free":true'*'"running":'[0-9]*) ok "$1: the API asked Dolt: $out" ;;
    *) ko "$1: the API did not reach Dolt: $out"; dc logs --no-log-prefix --tail 10 api 2>&1 ;;
  esac
  case $(machine /internal/machines/dolt/release) in
    *'"released":true'*) ok "$1: released" ;; *) ko "$1: not released" ;;
  esac
}
reaches "3. dolt/hold"

# --- 4. The agent's up again ---------------------------------------------------------------
dc up --detach --no-build --remove-orphans --wait --wait-timeout 120 >/dev/null 2>&1 || ko "4. up again"
expect "4. Dolt not recreated" "$DOLT_ID" "$(dc ps -q dolt)"
expect "4. not initialised again" 1 "$(dc logs --no-log-prefix dolt 2>&1 | grep -c 'first init done')"

# --- 5. An installation from before Dolt gets it -----------------------------------------
dc rm --stop --force dolt >/dev/null 2>&1
docker volume rm "${P}_dolt-data" >/dev/null 2>&1 || ko "5. remove ${P}_dolt-data"
if out=$(dc up --detach --no-build --remove-orphans --wait --wait-timeout 300 2>&1); then
  ok "5. the agent's up adds Dolt to the running stack"
else
  ko "5. up: $(printf '%s' "$out" | tail -n 3)"
fi
expect "5. Dolt" healthy "$(health dolt)"
expect "5. initialised as new" 1 "$(dc logs --no-log-prefix dolt 2>&1 | grep -c 'first init done')"
reaches "5. dolt/hold"

[ "$FAILED" = 0 ] && echo "ALL OK" || { echo "SOME CHECKS FAILED"; exit 1; }
