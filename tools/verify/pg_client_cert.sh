#!/bin/bash
# Verification of PostgreSQL's client certificate authentication for the
# queue's role (decisions 0023 and 0030): `hostssl … cert`, who a certificate
# is (its CN, or its whole subject through a map), what an expired one and
# one from another CA get, what happens to a connection already open when
# its certificate expires, and whether the server takes a renewed
# certificate of its own without a restart.
#
# It runs its own throwaway PostgreSQL (postgres:16-alpine, the image of
# infra/postgres) with a throwaway CA; nothing of the dev stack is touched.
# No rows at all. Run from the repository root:
#
#   bash tools/verify/pg_client_cert.sh
#
# Needs docker and openssl 3.4 or later (`-not_after`).
set -u
export LC_ALL=C
IMAGE=${PG_IMAGE:-postgres:16-alpine}
NAME=qd-verify-pg-client-cert
D=$(mktemp -d)
chmod 700 "$D"
trap 'docker rm -f "$NAME" >/dev/null 2>&1; rm -rf "$D"' EXIT
FAILED=0

ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { # NAME EXPECTED ACTUAL
  if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi
}
like() { # NAME PATTERN ACTUAL
  case "$3" in
    *"$2"*) ok "$1: $(echo "$3" | head -1 | cut -c1-110)" ;;
    *) ko "$1: expected '$2', got '$3'" ;;
  esac
}

# --- Certificates ------------------------------------------------------------
# ca NAME: a CA of its own.
ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/CN=qd verify $1" -addext "basicConstraints=critical,CA:TRUE" \
    -keyout "$D/$1.key" -out "$D/$1.crt" 2>/dev/null
}
# cert FILE CA SUBJECT USAGE [OPENSSL X509 OPTION...]: a key and a certificate of that CA.
cert() {
  local f=$1 ca=$2 subj=$3 usage=$4
  shift 4
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$subj" \
    -keyout "$D/$f.key" -out "$D/$f.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=%s\nsubjectAltName=DNS:localhost\n' "$usage" >"$D/ext"
  openssl x509 -req -in "$D/$f.csr" -CA "$D/$ca.crt" -CAkey "$D/$ca.key" -CAcreateserial \
    -extfile "$D/ext" -out "$D/$f.crt" "$@" 2>/dev/null
}
after() { date -u -d "+$1 seconds" +%Y%m%d%H%M%SZ; }
ca ca
ca other
cert server ca "/CN=localhost" serverAuth -days 1
cert queue ca "/CN=qd_queue" clientAuth -days 1
cert machine ca "/O=qd/OU=worker/CN=worker-7" clientAuth -days 1
cert trino ca "/O=qd/OU=trino/CN=trino-1" clientAuth -days 1
cert stranger other "/CN=qd_queue" clientAuth -days 1
cert late ca "/CN=qd_queue" clientAuth \
  -not_before "$(date -u -d '-2 hours' +%Y%m%d%H%M%SZ)" -not_after "$(date -u -d '-1 hour' +%Y%m%d%H%M%SZ)"

# --- A throwaway PostgreSQL that asks for a certificate ------------------------
docker rm -f "$NAME" >/dev/null 2>&1
docker run -d --name "$NAME" -e POSTGRES_PASSWORD="$(openssl rand -hex 12)" "$IMAGE" >/dev/null ||
  { echo "could not start $IMAGE"; exit 1; }
ready() { for _ in $(seq 60); do docker exec "$NAME" pg_isready -q -h 127.0.0.1 && return 0; sleep 1; done; return 1; }
sleep 3
ready || { echo "PostgreSQL did not start"; exit 1; }
docker exec "$NAME" mkdir -p /tls /client
install() { # FILE...: into /tls, the server's
  for f in "$@"; do docker cp -q "$D/$f" "$NAME:/tls/$f"; done
  docker exec "$NAME" sh -c 'chown -R postgres /tls && chmod 600 /tls/*.key'
}
install ca.crt server.crt server.key
for f in ca.crt queue machine trino stranger late; do
  case $f in *.crt) docker cp -q "$D/$f" "$NAME:/client/$f" ;; *)
    docker cp -q "$D/$f.crt" "$NAME:/client/$f.crt"
    docker cp -q "$D/$f.key" "$NAME:/client/$f.key"
    ;;
  esac
done
docker exec "$NAME" sh -c 'chown -R postgres /client && chmod 600 /client/*.key'
# A certificate is its CN (qd_queue), or, with the map, a worker of the
# installation whatever its name (the subject, as PostgreSQL writes it:
# CN first).
cat >"$D/pg_hba.conf" <<'EOF'
local   all       postgres                 trust
hostssl all       qd_queue   all           cert
hostssl all       qd_mapped  all           cert clientname=DN map=workers
EOF
cat >"$D/pg_ident.conf" <<'EOF'
workers  "/^CN=[a-z0-9-]+,OU=worker,O=qd$"  qd_mapped
EOF
docker cp -q "$D/pg_hba.conf" "$NAME:/var/lib/postgresql/data/pg_hba.conf"
docker cp -q "$D/pg_ident.conf" "$NAME:/var/lib/postgresql/data/pg_ident.conf"
docker exec "$NAME" sh -c 'chown postgres /var/lib/postgresql/data/pg_*.conf'
admin() { docker exec -u postgres "$NAME" psql -XAtq -c "$1"; }
admin "CREATE ROLE qd_queue LOGIN" >/dev/null
admin "CREATE ROLE qd_mapped LOGIN" >/dev/null
admin "ALTER SYSTEM SET ssl = on" >/dev/null
admin "ALTER SYSTEM SET ssl_cert_file = '/tls/server.crt'" >/dev/null
admin "ALTER SYSTEM SET ssl_key_file = '/tls/server.key'" >/dev/null
admin "ALTER SYSTEM SET ssl_ca_file = '/tls/ca.crt'" >/dev/null
docker restart "$NAME" >/dev/null
sleep 2
ready || { echo "PostgreSQL did not restart with TLS"; docker logs "$NAME" 2>&1 | tail -5; exit 1; }

# as ROLE CERT [SQL]: over TCP with TLS, the server verified; CERT `-` is none.
as() {
  local role=$1 c=$2 sql=${3:-select current_user}
  local conn="host=localhost dbname=postgres user=$role sslmode=verify-full sslrootcert=/client/ca.crt"
  [ "$c" = - ] || conn="$conn sslcert=/client/$c.crt sslkey=/client/$c.key"
  docker exec -u postgres "$NAME" psql -XAtq "$conn" -c "$sql" 2>&1
}

echo "== Who a certificate is"
expect "a certificate whose CN is the role" qd_queue "$(as qd_queue queue)"
like "the connection is TLS with a client certificate" "t|" \
  "$(as qd_queue queue "select ssl, client_dn from pg_stat_ssl where pid = pg_backend_pid()" | tr -d ' ')"
like "no certificate" "requires a valid client certificate" "$(as qd_queue -)"
like "a certificate of another CA" "unknown ca" "$(as qd_queue stranger)"
like "an expired certificate" "certificate expired" "$(as qd_queue late)"
like "a certificate with another CN" "certificate authentication failed" "$(as qd_queue machine)"
expect "a worker's certificate (OU=worker), by the map" qd_mapped "$(as qd_mapped machine)"
like "another role's certificate (OU=trino), by the map" "certificate authentication failed" "$(as qd_mapped trino)"
like "without TLS" "no pg_hba.conf entry" \
  "$(docker exec -u postgres "$NAME" psql -XAtq "host=localhost dbname=postgres user=qd_queue sslmode=disable" -c "select 1" 2>&1)"

echo "== A connection open when its certificate expires"
cert brief ca "/CN=qd_queue" clientAuth -not_after "$(after 6)"
docker cp -q "$D/brief.crt" "$NAME:/client/brief.crt"
docker cp -q "$D/brief.key" "$NAME:/client/brief.key"
docker exec "$NAME" sh -c 'chown postgres /client/brief.* && chmod 600 /client/brief.key'
expect "it goes on after the expiry (checked at the handshake only)" "qd_queue" \
  "$(as qd_queue brief "select pg_sleep(9); select current_user" | tail -1)"
like "a new connection with it" "certificate expired" "$(as qd_queue brief)"
expect "the server can end the sessions of a role" t \
  "$( (as qd_queue queue "select pg_sleep(5)" >/dev/null &) ; sleep 1
      admin "select bool_and(pg_terminate_backend(pid)) from pg_stat_activity where usename = 'qd_queue'")"

echo "== The server's own certificate, renewed"
serial() { docker exec "$NAME" sh -c 'echo | openssl s_client -connect localhost:5432 -starttls postgres 2>/dev/null | openssl x509 -noout -serial' 2>/dev/null; }
docker exec "$NAME" sh -c 'command -v openssl >/dev/null' || docker exec "$NAME" apk add -q --no-cache openssl >/dev/null 2>&1
before=$(serial)
cert server ca "/CN=localhost" serverAuth -days 1
install server.crt server.key
same=$(serial)
expect "before a reload, the old one is still served" "$before" "$same"
expect "pg_reload_conf()" t "$(admin "select pg_reload_conf()")"
sleep 1
now=$(serial)
if [ -n "$now" ] && [ "$now" != "$before" ]; then ok "after a reload, the new one, with no restart: $now"; else ko "the certificate did not change: $now"; fi
expect "clients go on connecting" qd_queue "$(as qd_queue queue)"
# A CA added to ssl_ca_file (a rotation of the CA) is also read at a reload.
cat "$D/ca.crt" "$D/other.crt" >"$D/both.crt"
docker cp -q "$D/both.crt" "$NAME:/tls/ca.crt"
docker exec "$NAME" chown postgres /tls/ca.crt
admin "select pg_reload_conf()" >/dev/null
sleep 1
expect "a second CA added to ssl_ca_file, after a reload" qd_queue "$(as qd_queue stranger)"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
