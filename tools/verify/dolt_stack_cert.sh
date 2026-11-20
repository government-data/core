#!/bin/bash
# Verification of the repository's Dolt with the machines' certificates
# (decision 0066, point 11, step 10): infra/dolt/docker-compose.yml with
# infra/dolt/certificate.yml, as an installation runs it, with a throwaway
# CA and the application VM's node agent directory made as the agent leaves
# it (ca.crt 0644, machine.pem 0600, this user's). It shows: Dolt serves the
# agent's certificate and only TLS; it requires a client certificate of the
# CA (none, another CA's, refused; root's own `dolt sql` too); the API's
# user connects presenting the machine's certificate and verifying Dolt's
# name; tenant.sh and backup.sh work through the overlay's `sql`; a renewed
# machine.pem is not served until Dolt restarts, and is after it, with the
# committed data kept; Dolt's own list shows another connection's running
# statement to the API's user.
#
# Its own Compose project (qd-verify-dolt-cert), volumes (APP_STACK) and port
# 127.0.0.1:55306; nothing of the dev stack is touched. The repository's
# infra/dolt is copied first and made readable. Removes its containers and
# volumes at the end. Run from the repository root:
#
#   bash tools/verify/dolt_stack_cert.sh
#
# Needs docker with Compose v2 and openssl 3; pulls mysql:8.4 (the `sql` client).
set -u
export LC_ALL=C
P=qd-verify-dolt-cert
PORT=55306
NAME=qd-app
D=$(mktemp -d)
chmod 755 "$D"
FAILED=0
mkdir -p "$D/infra"
cp -r infra/dolt "$D/infra/dolt"
chmod -R a+rX "$D/infra"
mkdir -m 700 "$D/machine"
mkdir -m 755 "$D/client"
cat >"$D/.env" <<EOF
APP_STACK=$P
DOLT_ROOT_PASSWORD=$(openssl rand -hex 12)
DOLT_APP_PASSWORD=$(openssl rand -hex 12)
QD_MACHINE_DIR=$D/machine
DOLT_BIND=127.0.0.1
DOLT_PORT=$PORT
QD_EDGE_NETWORK=$P-edge
EOF
set -a; . "$D/.env"; set +a
# The application's Traefik network, which Dolt joins for its metrics
# (decision 0071): a throwaway one here.
docker network create "$P-edge" >/dev/null
FILES="-f infra/dolt/docker-compose.yml -f infra/dolt/certificate.yml"
# shellcheck disable=SC2086
compose() { (cd "$D" && docker compose --project-directory . --env-file .env -p "$P" $FILES "$@"); }
cleanup() {
  compose --profile sql down -v --remove-orphans >/dev/null 2>&1
  docker network rm "$P-edge" >/dev/null 2>&1
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
serial() { openssl x509 -noout -serial -in "$D/$1.crt"; }
ca ca
ca other
leaf app ca "/O=qd/OU=app/CN=$NAME" "$NAME"
leaf worker ca "/O=qd/OU=worker/CN=worker-7" worker-7
leaf stranger other "/O=qd/OU=app/CN=$NAME" "$NAME"
agent app
for f in app worker stranger; do install -m 644 "$D/$f.pem" "$D/client/$f.pem"; done
install -m 644 "$D/ca.crt" "$D/client/ca.crt"

# --- The stack ---------------------------------------------------------------
up=$(compose up -d --wait --wait-timeout 120 dolt 2>&1) ||
  { echo "Dolt did not become healthy: $up" | tail -5; compose logs dolt 2>&1 | tail -20; exit 1; }
ok "the repository's Dolt with certificate.yml is healthy"
# Its address on the project's network (it is on Traefik's too).
IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"${P}_default\").IPAddress}}" "$P-dolt-1")

# my USER CERT MODE [HOST] -- SQL: the MySQL client from another container,
# reaching Dolt as $NAME (its address), as the API does.
my() {
  local user=$1 cert=$2 mode=$3 host=${4:-$NAME} pw
  pw=$([ "$user" = qd_app ] && echo "$DOLT_APP_PASSWORD" || echo x)
  shift 4
  local tls=(--ssl-mode="$mode" --ssl-ca=/client/ca.crt)
  [ "$cert" != - ] && tls+=(--ssl-cert="/client/$cert.pem" --ssl-key="/client/$cert.pem")
  [ "$mode" = DISABLED ] && tls=(--ssl-mode=DISABLED)
  docker run --rm --network "${P}_default" --add-host "$NAME:$IP" -e MYSQL_PWD="$pw" \
    -v "$D/client:/client:ro" mysql:8.4 mysql --host="$host" --port=3306 --user="$user" \
    "${tls[@]}" -N -B "$@" 2>&1 | grep -v 'Using a password' | grep . | tail -1
}
# served: the serial of the certificate Dolt serves now.
served() {
  openssl s_client -starttls mysql -connect "127.0.0.1:$PORT" </dev/null 2>/dev/null |
    openssl x509 -noout -serial 2>/dev/null
}

echo "== Its certificate, TLS only, a client certificate of the CA"
for _ in $(seq 30); do [ -n "$(served)" ] && break; sleep 1; done
expect "it serves the agent's machine.pem" "$(serial app)" "$(served)"
expect "the API's user, presenting the machine's certificate, Dolt's name verified" \
  "qd_app@%" "$(my qd_app app VERIFY_IDENTITY "$NAME" -e 'select current_user()')"
like "the same by another name (its address): the name is verified" "verify failed" \
  "$(my qd_app app VERIFY_IDENTITY "$IP" -e 'select current_user()')"
like "without TLS: refused" "insecure" "$(my qd_app app DISABLED "$NAME" -e 'select 1')"
like "TLS without a certificate: refused at the handshake" "ERROR" \
  "$(my qd_app - VERIFY_IDENTITY "$NAME" -e 'select 1')"
like "a certificate of another CA: refused" "ERROR" \
  "$(my qd_app stranger VERIFY_IDENTITY "$NAME" -e 'select 1')"
expect "another role's certificate of the CA passes the handshake: the password is the user's" \
  "qd_app@%" "$(my qd_app worker VERIFY_IDENTITY "$NAME" -e 'select current_user()')"
like "root's own dolt sql inside the container: locked out (no certificate)" "certificate required" \
  "$(docker exec "$P-dolt-1" dolt sql -q 'select 1' 2>&1)"

echo "== tenant.sh and backup.sh, root presenting the machine's certificate"
cp infra/dolt/tenant.sh infra/dolt/backup.sh "$D/infra/dolt/"
export DOLT_COMPOSE_FILES=$FILES DOLT_PROJECT=$P DOLT_ENV_FILE=.env
like "tenant.sh add" "add done" "$(sh "$D/infra/dolt/tenant.sh" add verify 2>&1)"
my qd_app app VERIFY_IDENTITY "$NAME" -D tenant_verify -e \
  "CREATE TABLE t (id INT PRIMARY KEY, v TEXT); INSERT INTO t VALUES (1, 'synthetic'); CALL DOLT_COMMIT('-Am', 'verify', '--author', 'Verify <verify@example.org>')" >/dev/null
expect "the API's user commits in the tenant's database" "1" \
  "$(my qd_app app VERIFY_IDENTITY "$NAME" -D tenant_verify -e 'select count(*) from dolt_log where message = "verify"')"
like "backup.sh" "tenant_verify backed up" "$(sh "$D/infra/dolt/backup.sh" 2>&1)"
like "the backup is in the volume" "tenant_verify" "$(docker exec "$P-dolt-1" ls /backups)"

echo "== Dolt's own list sees another connection's statement"
my qd_app app VERIFY_IDENTITY "$NAME" -e 'select sleep(6)' >/dev/null &
sleep 3
expect "one connection in Query besides the asker" "1" \
  "$(my qd_app app VERIFY_IDENTITY "$NAME" -e "select count(*) from information_schema.processlist where command = 'Query' and id <> connection_id()")"
wait

echo "== A renewed certificate: served only after a restart"
leaf app2 ca "/O=qd/OU=app/CN=$NAME" "$NAME"
agent app2
install -m 644 "$D/app2.pem" "$D/client/app2.pem"
sleep 5
expect "after the agent renewed, Dolt still serves its own copy" "$(serial app)" "$(served)"
began=$(date +%s)
docker restart "$P-dolt-1" >/dev/null
for _ in $(seq 90); do [ "$(served)" = "$(serial app2)" ] && break; sleep 1; done
expect "after its restart, the renewed one" "$(serial app2)" "$(served)"
echo "       (served again $(($(date +%s) - began)) s after docker restart began)"
for _ in $(seq 30); do
  [ "$(my qd_app app2 VERIFY_IDENTITY "$NAME" -e 'select 1')" = 1 ] && break
  sleep 1
done
expect "the committed row is kept" "synthetic" \
  "$(my qd_app app2 VERIFY_IDENTITY "$NAME" -D tenant_verify -e 'select v from t where id = 1')"
expect "the old certificate of the CA is still accepted as a client's (X.509 alone)" \
  "qd_app@%" "$(my qd_app app VERIFY_IDENTITY "$NAME" -e 'select current_user()')"
like "tenant.sh remove" "remove done" "$(sh "$D/infra/dolt/tenant.sh" remove verify 2>&1)"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
