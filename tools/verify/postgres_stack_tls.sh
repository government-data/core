#!/bin/bash
# Verification of the repository's PostgreSQL with TLS (decision 0066, step
# 8): infra/postgres/docker-compose.yml with infra/postgres/tls.yml, as an
# installation runs it, with a throwaway CA and the node agent's directory
# made as the agent leaves it (ca.crt 0644, machine.pem 0600, both this
# user's, not PostgreSQL's). It shows: TLS is required for every remote
# connection; the queue's role opens only with a worker's certificate, not
# with another role's, another CA's or a password; the API's role with its
# password over TLS, verifying the server's name; the certificate the agent
# renews is served after the watcher's next look, without a restart; the
# backups go over TLS too.
#
# Its own Compose project (qd-verify-pg-tls), network and volumes, and port
# 127.0.0.1:55433; nothing of the dev stack is touched. The repository's
# infra/postgres is copied first and made readable, as a checkout with a
# restrictive umask leaves its files unreadable to the container's user.
# Removes its containers and volumes at the end. Run from the repository root:
#
#   bash tools/verify/postgres_stack_tls.sh
#
# Needs docker with Compose v2 and openssl.
set -u
export LC_ALL=C
P=qd-verify-pg-tls
PORT=55433
NAME=qd-db
D=$(mktemp -d)
chmod 755 "$D"
FAILED=0
cp -r infra/postgres "$D/postgres"
chmod -R a+rX "$D/postgres"
mkdir -m 700 "$D/machine"
mkdir -m 755 "$D/client"
cat >"$D/verify.yml" <<EOF
networks:
  backend:
    name: $P-backend
EOF
export POSTGRES_SUPERUSER_PASSWORD APP_DB_PASSWORD
POSTGRES_SUPERUSER_PASSWORD=$(openssl rand -hex 12)
APP_DB_PASSWORD=$(openssl rand -hex 12)
export QD_MACHINE_DIR="$D/machine" POSTGRES_SERVER_NAME=$NAME POSTGRES_BIND=127.0.0.1
export POSTGRES_PORT=$PORT QD_PG_TLS_CHECK_EVERY=2
unset QD_QUEUE_PASSWORD
compose() {
  docker compose -p "$P" -f "$D/postgres/docker-compose.yml" -f "$D/postgres/tls.yml" \
    -f "$D/verify.yml" --profile backup "$@"
}
cleanup() { compose down -v --remove-orphans >/dev/null 2>&1; rm -rf "$D"; }
trap cleanup EXIT

ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }
like() {
  case "$3" in
    *"$2"*) ok "$1: $(echo "$3" | grep -m1 . | cut -c1-110)" ;;
    *) ko "$1: expected '$2', got '$3'" ;;
  esac
}

# --- Certificates, as the installation's CA and the agent make them -----------
ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/O=qd/CN=qd verify $1" -addext "basicConstraints=critical,CA:TRUE" \
    -keyout "$D/$1.key" -out "$D/$1.crt" 2>/dev/null
}
# leaf NAME CA SUBJECT DNS: NAME.pem, the certificate then the PKCS#8 key.
leaf() {
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$3" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nkeyUsage=digitalSignature\nextendedKeyUsage=clientAuth,serverAuth\nsubjectAltName=DNS:%s\n' "$4" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/$2.crt" -CAkey "$D/$2.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  openssl pkey -in "$D/$1.key" -out "$D/$1.p8"
  cat "$D/$1.crt" "$D/$1.p8" >"$D/$1.pem"
}
# agent PEM: the agent's files, each written aside and renamed.
agent() {
  install -m 644 "$D/ca.crt" "$D/machine/.ca.crt.new" && mv "$D/machine/.ca.crt.new" "$D/machine/ca.crt"
  install -m 600 "$D/$1.pem" "$D/machine/.machine.pem.new" && mv "$D/machine/.machine.pem.new" "$D/machine/machine.pem"
}
ca ca
ca other
leaf server ca "/O=qd/OU=application/CN=$NAME" "$NAME"
leaf worker ca "/O=qd/OU=worker/CN=worker-7" worker-7
leaf trino ca "/O=qd/OU=trino-worker/CN=trino-1" trino-1
leaf stranger other "/O=qd/OU=worker/CN=worker-7" worker-7
agent server
for f in worker trino stranger; do install -m 600 "$D/$f.pem" "$D/client/$f.pem"; done
install -m 644 "$D/ca.crt" "$D/client/ca.crt"

# --- The stack ---------------------------------------------------------------
up=$(compose up -d --wait --wait-timeout 120 postgres 2>&1) ||
  { echo "PostgreSQL did not become healthy: $up" | tail -5; compose logs postgres 2>&1 | tail -20; exit 1; }
ok "the repository's PostgreSQL with tls.yml is healthy"

# psql_as CONNINFO [SQL]: from a client on the stack's network, as this user.
psql_as() {
  docker run --rm --network "$P-backend" -u "$(id -u):$(id -g)" -v "$D/client:/client:ro" \
    postgres:16-alpine psql -XAtq "$1" -c "${2:-select current_user}" 2>&1
}
APP="dbname=quality_data user=quality_data_app password=$APP_DB_PASSWORD sslrootcert=/client/ca.crt"
QUEUE="host=$NAME dbname=quality_data user=qd_queue sslmode=verify-full sslrootcert=/client/ca.crt"
admin() { compose exec -T -u postgres postgres psql -XAtq -d quality_data -c "$1" 2>&1; }

echo "== The files PostgreSQL reads, and who watches them"
expect "the installed key's owner and mode" "postgres 600" \
  "$(compose exec -T postgres stat -c '%U %a' /var/lib/postgresql/tls/machine.pem)"
expect "the watcher is a child of the container's init, not of PostgreSQL" 1 \
  "$(compose exec -T postgres sh -c 'for p in /proc/[0-9]*; do
      [ "$p" = /proc/1 ] && continue
      tr "\0" " " <"$p/cmdline" 2>/dev/null | grep -q "qd/postgres/[e]ntrypoint" &&
        awk "/^PPid:/ {print \$2}" "$p/status"; done' | sort -u)"

echo "== TLS for every remote connection"
expect "the API's role over TLS, the server's name verified" "quality_data_app|t" \
  "$(psql_as "host=$NAME sslmode=verify-full $APP" "select current_user, ssl from pg_stat_ssl where pid = pg_backend_pid()")"
like "the API's role without TLS" "no encryption" "$(psql_as "host=$NAME sslmode=disable $APP")"
like "the API's role by another name than the certificate's" "does not match host name" \
  "$(psql_as "host=postgres sslmode=verify-full $APP")"

echo "== The queue: a worker's certificate, and nothing else"
expect "a worker's certificate (OU=worker)" qd_queue "$(psql_as "$QUEUE sslcert=/client/worker.pem sslkey=/client/worker.pem")"
like "another role's certificate (OU=trino-worker)" "certificate authentication failed" \
  "$(psql_as "$QUEUE sslcert=/client/trino.pem sslkey=/client/trino.pem")"
like "a worker's certificate of another CA" "unknown ca" \
  "$(psql_as "$QUEUE sslcert=/client/stranger.pem sslkey=/client/stranger.pem")"
like "a password, no certificate" "requires a valid client certificate" \
  "$(psql_as "$QUEUE password=anything")"
like "without TLS" "no encryption" "$(psql_as "host=$NAME dbname=quality_data user=qd_queue sslmode=disable")"
expect "the queue's role has no password" t \
  "$(admin "select rolpassword is null from pg_authid where rolname = 'qd_queue'")"
expect "the API's role may see and end the queue's sessions" t \
  "$(admin "select pg_has_role('quality_data_app', 'pg_read_all_stats', 'USAGE') and pg_has_role('quality_data_app', 'pg_signal_backend', 'USAGE')")"

echo "== The certificate the agent renews"
serial() {
  echo | openssl s_client -connect "127.0.0.1:$PORT" -starttls postgres -servername "$NAME" 2>/dev/null |
    openssl x509 -noout -serial 2>/dev/null
}
before=$(serial)
[ -n "$before" ] && ok "served: $before" || ko "no certificate served on 127.0.0.1:$PORT"
leaf server ca "/O=qd/OU=application/CN=$NAME" "$NAME"
agent server
sleep 6
now=$(serial)
if [ -n "$now" ] && [ "$now" != "$before" ]; then ok "after the watcher's look, the renewed one, no restart: $now"; else ko "still $now"; fi
like "the watcher reloaded PostgreSQL" "PostgreSQL reloaded" "$(compose logs postgres 2>&1 | grep postgres-tls | tail -1)"
expect "PostgreSQL was not restarted" 0 "$(docker inspect -f '{{.RestartCount}}' "$(compose ps -q postgres)")"
expect "a worker still connects" qd_queue "$(psql_as "$QUEUE sslcert=/client/worker.pem sslkey=/client/worker.pem")"

echo "== The backups over TLS"
dump=$(compose run --rm --no-deps -T --entrypoint sh postgres-backup -c \
  'pg_dump -Fc -f /tmp/dump && psql -XAtq -c "select ssl from pg_stat_ssl where pid = pg_backend_pid()"' 2>&1)
expect "pg_dump and psql as the backup sidecar runs them (verify-full)" t "$(echo "$dump" | tail -1)"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
