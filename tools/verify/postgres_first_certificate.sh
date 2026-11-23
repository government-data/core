#!/bin/bash
# Verification of PostgreSQL's first start (decision 0066, point 16, step
# 11; the user's ruling of 2026-11-13): infra/postgres/docker-compose.yml
# with tls.yml and app-machine.yml, as an installation runs them, on an
# empty volume and an empty node agent directory (uid 65532, 0700). It
# shows: without machine.pem PostgreSQL listens on its loopback alone,
# without TLS, is not healthy, and refuses another container; the real
# `server app-machine`, in PostgreSQL's network namespace as the agent's
# user, makes the machine and writes the agent's files; PostgreSQL then
# goes on with TLS by itself, serves that certificate and takes a worker's
# certificate as step 8 does; run again, the command says unchanged; a
# restart has no window; without TLS another container is refused in both
# phases.
#
# Its own Compose project (qd-verify-pg-first), network and volumes, and
# port 127.0.0.1:55434; nothing of the dev stack is touched. The command is
# this checkout's debug binary (`cargo build -p server`), run in a container
# with the Nix store mounted, instead of the API's image. Removes its
# containers, volumes and files at the end. Run from the repository root:
#
#   bash tools/verify/postgres_first_certificate.sh
#
# Needs docker with Compose v2, openssl and target/debug/server.
set -u
export LC_ALL=C
P=qd-verify-pg-first
PORT=55434
NAME=qd-app
SERVER=${QD_VERIFY_SERVER:-$PWD/target/debug/server}
[ -x "$SERVER" ] || { echo "no $SERVER: cargo build -p server"; exit 1; }
D=$(mktemp -d)
chmod 755 "$D"
FAILED=0
cp -r infra/postgres "$D/postgres"
chmod -R a+rX "$D/postgres"
mkdir -m 755 "$D/client" "$D/ca"
# The agent's directory, its user's (the API's image's, uid 65532).
docker run --rm -v "$D:/d" postgres:16-alpine install -d -o 65532 -g 65532 -m 700 /d/machine
# The binary is this user's (group `users`, 0750): the agent's user runs it
# with that group added.
cat >"$D/verify.yml" <<EOF
networks:
  backend:
    name: $P-backend
services:
  app-machine:
    image: postgres:16-alpine
    entrypoint: ["/server", "app-machine"]
    user: "65532:65532"
    group_add: ["$(stat -c %g "$SERVER")"]
    volumes:
      - /nix/store:/nix/store:ro
      - $SERVER:/server:ro
EOF
export POSTGRES_SUPERUSER_PASSWORD APP_DB_PASSWORD
POSTGRES_SUPERUSER_PASSWORD=$(openssl rand -hex 12)
APP_DB_PASSWORD=$(openssl rand -hex 12)
export QD_MACHINE_DIR="$D/machine" QD_CA_DIR="$D/ca" POSTGRES_SERVER_NAME=$NAME
export POSTGRES_BIND=127.0.0.1 POSTGRES_PORT=$PORT QD_PG_TLS_CHECK_EVERY=2
unset QD_QUEUE_PASSWORD API_IMAGE
compose() {
  docker compose -p "$P" -f "$D/postgres/docker-compose.yml" -f "$D/postgres/tls.yml" \
    -f "$D/postgres/app-machine.yml" -f "$D/verify.yml" "$@"
}
cleanup() {
  compose down -v --remove-orphans >/dev/null 2>&1
  docker run --rm -v "$D:/d" postgres:16-alpine rm -rf /d/machine
  rm -rf "$D"
}
trap cleanup EXIT

ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }
like() {
  case "$3" in
    *"$2"*) ok "$1: $(echo "$3" | grep -m1 -F -- "$2" | cut -c1-110)" ;;
    *) ko "$1: expected '$2', got '$3'" ;;
  esac
}

# --- The CA, as the installer makes it, and a worker's certificate -------------
"$SERVER" ca-init --name qd-verify --cert "$D/ca/ca.crt" --key "$D/ca/ca.key" >/dev/null ||
  { echo "server ca-init failed"; exit 1; }
chmod 644 "$D/ca/ca.key" # throwaway: readable by the agent's user in the container
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/O=qd/OU=worker/CN=worker-7" \
  -keyout "$D/worker.key" -out "$D/worker.csr" 2>/dev/null
printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=clientAuth\nsubjectAltName=DNS:worker-7\n' >"$D/ext"
openssl x509 -req -in "$D/worker.csr" -CA "$D/ca/ca.crt" -CAkey "$D/ca/ca.key" -CAcreateserial \
  -extfile "$D/ext" -days 1 -out "$D/worker.crt" 2>/dev/null
openssl pkey -in "$D/worker.key" -out "$D/worker.p8"
cat "$D/worker.crt" "$D/worker.p8" >"$D/client/worker.pem"
chmod 600 "$D/client/worker.pem"
install -m 644 "$D/ca/ca.crt" "$D/client/ca.crt"

# psql_as CONNINFO [SQL]: from another container on the stack's network.
psql_as() {
  docker run --rm --network "$P-backend" -u "$(id -u):$(id -g)" -v "$D/client:/client:ro" \
    postgres:16-alpine psql -XAtq "$1" -c "${2:-select current_user}" 2>&1
}
APP="dbname=quality_data user=quality_data_app password=$APP_DB_PASSWORD"
admin() { compose exec -T -u postgres postgres psql -XAtq -d quality_data -c "$1" 2>&1; }
health() { docker inspect -f '{{.State.Health.Status}}' "$(compose ps -q postgres)"; }
# The addresses listening on 5432, as /proc/net/tcp writes them.
listening() {
  compose exec -T postgres sh -c 'cat /proc/net/tcp /proc/net/tcp6' |
    awk '$4 == "0A" && $2 ~ /:1538$/ {print $2}' | sort | tr '\n' ' ' | sed 's/ $//'
}
app_machine() { compose run --rm -T app-machine --name "$NAME" --dir /run/qd-node 2>&1; }

# --- The first start ----------------------------------------------------------
compose up -d postgres >/dev/null 2>&1 || { echo "PostgreSQL did not start"; exit 1; }
for _ in $(seq 60); do
  compose exec -T postgres pg_isready -q -h 127.0.0.1 2>/dev/null && break
  sleep 2
done
echo "== Without machine.pem: the loopback alone, without TLS"
like "the entrypoint says why" "on its loopback alone" "$(compose logs postgres 2>&1 | grep postgres-tls)"
expect "listening on 5432 (127.0.0.1 only)" 0100007F:1538 "$(listening)"
expect "TLS" off "$(admin "show ssl")"
expect "the application's role on the loopback, in PostgreSQL's namespace" "quality_data_app" \
  "$(compose exec -T postgres psql -XAtq "host=127.0.0.1 $APP" -c "select current_user" 2>&1)"
like "the loopback without a password" "no password supplied" \
  "$(compose exec -T postgres psql -XAtqw "host=127.0.0.1 dbname=quality_data user=postgres" -c "select 1" 2>&1)"
like "another container, without TLS" "Connection refused" "$(psql_as "host=$NAME sslmode=disable $APP")"
like "another container, asking for TLS" "Connection refused" "$(psql_as "host=$NAME sslmode=require $APP")"
h=$(health)
if [ "$h" != healthy ]; then ok "not healthy, so nothing that waits for it starts: $h"; else ko "healthy without TLS"; fi

echo "== server app-machine in PostgreSQL's network namespace"
out=$(app_machine)
like "the first run" "$NAME (app, approved): certificate" "$out"
expect "the agent's files (owner, mode)" "65532 644 ca.crt|65532 600 machine.pem" \
  "$(docker run --rm -v "$D/machine:/m:ro" postgres:16-alpine sh -c \
    'cd /m && stat -c "%u %a %n" ca.crt machine.pem' | tr '\n' '|' | sed 's/|$//')"

echo "== PostgreSQL goes on with TLS by itself"
for _ in $(seq 60); do
  [ "$(health)" = healthy ] && break
  sleep 2
done
expect "healthy" healthy "$(health)"
like "the entrypoint says so" "goes on with TLS" "$(compose logs postgres 2>&1 | grep postgres-tls)"
expect "TLS" on "$(admin "show ssl")"
like "listening on 5432 (every address)" "00000000:1538" "$(listening)"
like "the certificate served" "O=qd, OU=app, CN=$NAME" \
  "$(echo | openssl s_client -connect "127.0.0.1:$PORT" -starttls postgres -servername "$NAME" 2>/dev/null |
    openssl x509 -noout -subject 2>/dev/null)"
expect "the API's role over TLS, the name verified" "quality_data_app|t" \
  "$(psql_as "host=$NAME sslmode=verify-full sslrootcert=/client/ca.crt $APP" \
    "select current_user, ssl from pg_stat_ssl where pid = pg_backend_pid()")"
expect "a worker's certificate (the queue, step 8)" qd_queue \
  "$(psql_as "host=$NAME dbname=quality_data user=qd_queue sslmode=verify-full sslrootcert=/client/ca.crt sslcert=/client/worker.pem sslkey=/client/worker.pem")"
like "another container, without TLS" "no encryption" "$(psql_as "host=$NAME sslmode=disable $APP")"
like "the loopback, without TLS" "no encryption" \
  "$(compose exec -T postgres psql -XAtq "host=127.0.0.1 sslmode=disable $APP" -c "select 1" 2>&1)"

echo "== Run again, and restarted"
like "the second run" "$NAME: unchanged" "$(app_machine)"
expect "machines named $NAME (role, state)" "1|app|approved" \
  "$(admin "select count(*), min(role), min(state) from machines where name = '$NAME'")"
compose restart postgres >/dev/null 2>&1
for _ in $(seq 60); do
  [ "$(health)" = healthy ] && break
  sleep 2
done
expect "restarted with machine.pem there: healthy" healthy "$(health)"
expect "first-start windows in the log" 1 "$(compose logs postgres 2>&1 | grep -c 'on its loopback alone')"
like "the third run, over TLS on the loopback" "$NAME: unchanged" "$(app_machine)"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
