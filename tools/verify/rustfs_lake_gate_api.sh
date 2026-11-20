#!/bin/bash
# Verification of the lake's gate as an installation runs it (decision 0066,
# point 10), with the repository's own files and server:
#   - RustFS behind its gate: infra/rustfs/docker-compose.yml with
#     certificate.yml and gate.yml (a Traefik 3.6 that requires a client
#     certificate of the CA and asks the API before each request);
#   - the application's Traefik: infra/traefik/traefik.yml, its dynamic
#     files and internal.yml, with the gates' entry point (`lakegate`);
#   - the API: `server` on a throwaway PostgreSQL made by
#     infra/postgres/initdb, the CA of `server ca-init`, and machines that
#     joined through the real routes (a join token by the admin, POST
#     /install/join with a signing request, approved by the admin), the
#     lake node itself among them: its certificate is RustFS's and the
#     gate's;
#   - the worker's side: the proxy of infra/worker/lake.yml
#     (lake/entrypoint.sh), on its loopback, which presents its machine.pem
#     and takes the renewed one by itself, and DuckDB 2.0, which reaches it
#     over TLS under the worker's own name, as QD_LAKE_URL has it.
# It shows: an approved worker and a lake replica pass; a pending machine
# does not, and gets the API's 401; a forged header changes nothing; a
# machine that asks the API by hand, naming another, is refused, not being
# a gate; DuckDB writes and reads, a large multipart object included, and
# how often the API read `machines` for it; a certificate renewed through
# the machines' entry point takes over; a machine retired by the admin
# loses the lake within the pass's 5 seconds; each refusal leaves its audit
# row; the API down, nothing passes; the gate takes its own renewed
# certificate without a restart; and the application itself reaches the
# lake through the gate: the API as RustFS's platform admin, presenting
# the machine.pem LAKE_CLIENT_CERT_FILE names and the renewed one once the
# file changes, with no restart, and the application's Traefik, for the
# signed requests of the public entry point and RustFS's console
# (LAKE_TRANSPORT), which no machine without a pass gets. A joined
# machine's certificate stands there for the application VM's, which the
# installer signs (step 11, not built).
#
# Its own Compose project (qd-verify-lake-gate-api: RustFS, gate),
# containers qd-verify-lake-gate-api-pg, -app, -proxy and -duck, a network
# qd-verify-lake-gate-api-front, ports 127.0.0.1:59040 (PostgreSQL), 59042
# (the gate), 59043 (the proxy), 59044 (the gates' entry point), 59045 (the
# machines'), 59046 (the public one); the API listens on that network's gateway address, port
# 59041. Nothing of the dev stack is touched. RustFS's root credentials and
# the admin's password are random, never printed, and used only in what is
# thrown away at the end. Synthetic numbers only. The lake node is named
# `localhost`, as its clients run on this machine. Run from the repository
# root, with the server built (cargo build -p server):
#
#   bash tools/verify/rustfs_lake_gate_api.sh    (SERVER=target/debug/server,
#                                                 BIG_ROWS=10000000)
#
# Needs docker with Compose v2 (!reset), curl 7.75 or later, openssl 3 and
# python3; DuckDB runs in a container (python:3.12-slim, duckdb from PyPI),
# where the worker's name is its loopback.
set -u
export LC_ALL=C
P=qd-verify-lake-gate-api
PGC=$P-pg
PX=$P-proxy
DUCK=$P-duck
DUCK_IMAGE=python:3.12-slim
APPT=$P-app
FRONT=$P-front
PGPORT=59040
APIPORT=59041
GATE=59042
PROXY=59043
LAKEGATE=59044
MACHINES=59045
PUBLIC=59046
PROXY_IMAGE=traefik:v3.6
PG_IMAGE=postgres:16-alpine
BIG_ROWS=${BIG_ROWS:-10000000}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SERVER=${SERVER:-$ROOT/target/debug/server}
[ -x "$SERVER" ] || { echo "no server at $SERVER: cargo build -p server"; exit 1; }
"$SERVER" openapi 2>/dev/null | grep -q '/internal/machines/lake/check' ||
  { echo "$SERVER has no lake check route: build it again"; exit 1; }
D=$(mktemp -d)
chmod 755 "$D"
FAILED=0
SERVER_PID=
mkdir -p "$D/infra/postgres" "$D/dynamic" "$D/certs" "$D/le"
cp -r "$ROOT/infra/rustfs" "$D/infra/rustfs"
cp -r "$ROOT/infra/postgres/initdb" "$D/infra/postgres/initdb"
mkdir -p "$D/infra/worker" && cp -r "$ROOT/infra/worker/lake" "$D/infra/worker/lake"
chmod -R a+rX "$D/infra"
mkdir -m 700 "$D/machine" "$D/worker" "$D/secret" "$D/appnode"
KEY=qd$(openssl rand -hex 8)
SECRET=$(openssl rand -hex 16)
IMAGE=$(sed -n 's/^ *image: \${RUSTFS_IMAGE:-\(.*\)}$/\1/p' "$ROOT/infra/rustfs/docker-compose.yml")
cat >"$D/.env" <<EOF
APP_STACK=$P
RUSTFS_ACCESS_KEY=$KEY
RUSTFS_SECRET_KEY=$SECRET
QD_MACHINE_DIR=$D/machine
QD_MACHINE_NAME=localhost
QD_LAKE_CHECK_URL=https://internal.localhost:9444/api/internal/machines/lake/check
QD_GATE_CHECK_EVERY=2
QD_RUSTFS_TLS_CHECK_EVERY=2
RUSTFS_TLS_RELOAD_INTERVAL=5
RUSTFS_BIND=127.0.0.1
RUSTFS_PORT=$GATE
EOF
FILES="-f infra/rustfs/docker-compose.yml -f infra/rustfs/certificate.yml -f infra/rustfs/gate.yml"
# shellcheck disable=SC2086
compose() { (cd "$D" && docker compose --project-directory . --env-file .env -p $P $FILES "$@"); }
asroot() { docker run --rm --user 0 --entrypoint sh -v "$D:/d" "$IMAGE" -c "$1"; }
stop_api() { [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null && wait "$SERVER_PID" 2>/dev/null; SERVER_PID=; }
cleanup() {
  stop_api
  docker rm -f "$PX" "$DUCK" "$PGC" "$APPT" >/dev/null 2>&1
  compose down -v --remove-orphans >/dev/null 2>&1
  docker network rm "$FRONT" >/dev/null 2>&1
  asroot 'rm -rf /d/machine/*' >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT

ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }
refused() { case "$2" in rows* | big\ whole*) ko "$1: got through: $2" ;; *) ok "$1: $(echo "$2" | cut -c1-100)" ;; esac; }

echo "== The installation: PostgreSQL, the CA, the API, RustFS behind the gate"
# PostgreSQL as an installation makes it: the application's role and the queue's.
SUPER=$(openssl rand -hex 12)
APP=$(openssl rand -hex 12)
docker rm -f "$PGC" >/dev/null 2>&1
docker run -d --name "$PGC" -p "127.0.0.1:$PGPORT:5432" -v "$D/infra/postgres/initdb:/docker-entrypoint-initdb.d:ro" \
  -e POSTGRES_PASSWORD="$SUPER" -e POSTGRES_DB=qd -e APP_DB_USER=qd_app -e APP_DB_PASSWORD="$APP" \
  -e QD_QUEUE_PASSWORD="$(openssl rand -hex 12)" "$PG_IMAGE" >/dev/null || { echo "could not start $PG_IMAGE"; exit 1; }
sql() { docker exec -u postgres "$PGC" psql -XAtq -d qd -c "$1" 2>/dev/null; }
for _ in $(seq 60); do [ "$(sql 'select 1')" = 1 ] && [ "$(sql "select count(*) from pg_roles where rolname = 'qd_queue'")" = 1 ] && break; sleep 1; done

# The CA, by the installer's command. Only the application's Traefik's own
# server certificates are signed by hand here; every machine's is the API's.
"$SERVER" ca-init --name verify --cert "$D/ca.crt" --key "$D/secret/ca.key" >/dev/null 2>&1 || { echo "ca-init failed"; exit 1; }
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 -subj "/O=qd/CN=another CA" \
  -addext "basicConstraints=critical,CA:TRUE" -keyout "$D/other.key" -out "$D/other.crt" 2>/dev/null
# leaf NAME CA CAKEY SUBJECT SAN: NAME.crt, NAME.key and NAME.pem (the certificate then the PKCS#8 key).
leaf() {
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$4" -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nkeyUsage=digitalSignature\nextendedKeyUsage=clientAuth,serverAuth\nsubjectAltName=%s\n' "$5" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$2" -CAkey "$3" -CAcreateserial -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  openssl pkey -in "$D/$1.key" -out "$D/$1.p8"
  cat "$D/$1.crt" "$D/$1.p8" >"$D/$1.pem"
}
leaf internal "$D/ca.crt" "$D/secret/ca.key" "/O=qd/CN=internal.localhost" DNS:internal.localhost
leaf local "$D/ca.crt" "$D/secret/ca.key" "/CN=localhost" DNS:localhost
leaf stranger "$D/other.crt" "$D/other.key" "/O=qd/OU=worker/CN=worker-7" DNS:worker-7

# Where the API listens: the gateway of a network the application's Traefik is on.
docker network rm "$FRONT" >/dev/null 2>&1
docker network create "$FRONT" >/dev/null || { echo "could not make the network"; exit 1; }
API_HOST=$(docker network inspect "$FRONT" -f '{{(index .IPAM.Config 0).Gateway}}')
[ -n "$API_HOST" ] || { echo "no gateway address for $FRONT"; exit 1; }
API=http://$API_HOST:$APIPORT

# The API, with what an installation gives it and nothing of the lake or Trino.
ADMIN_PASSWORD=Qd-$(openssl rand -hex 14)
cat >"$D/secret/api.env" <<EOF
DATABASE_URL=postgres://qd_app:$APP@127.0.0.1:$PGPORT/qd
APP_HOST=$API_HOST
APP_PORT=$APIPORT
APP_PUBLIC_URL=https://qd.example
SETTINGS_ENCRYPTION_KEY=$(openssl rand -base64 32)
OIDC_SIGNING_KEY=$("$SERVER" oidc-keygen 2>/dev/null | tail -1)
ADMIN_BOOTSTRAP_EMAIL=admin@example.com
ADMIN_BOOTSTRAP_PASSWORD=$ADMIN_PASSWORD
CA_CERT_FILE=$D/ca.crt
CA_KEY_FILE=$D/secret/ca.key
INTERNAL_CLIENT_CERT_HEADER=trusted
PAYMENT_PROVIDER=none
RUST_LOG=error
EOF
# shellcheck disable=SC2046
server() { env -i HOME="$HOME" PATH="$PATH" $(cat "$D/secret/api.env") "$SERVER" "$@"; }
start_api() {
  # shellcheck disable=SC2046
  (exec env -i HOME="$HOME" PATH="$PATH" $(cat "$D/secret/api.env") "$SERVER" serve) >>"$D/api.log" 2>&1 &
  SERVER_PID=$! # the server itself, so that stopping it leaves nothing behind
  for _ in $(seq 90); do [ "$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "$API/health")" = 200 ] && return 0; sleep 1; done
  echo "the API did not start"; tail -3 "$D/api.log" | cut -c1-200; return 1
}
start_api || exit 1
server bootstrap-admin >/dev/null 2>&1
J=$D/secret/headers
login=$(curl -s -o /dev/null -w '%{http_code}' -D "$J" -H 'content-type: application/json' \
  -d "{\"email\":\"admin@example.com\",\"password\":\"$ADMIN_PASSWORD\"}" "$API/auth/login")
expect "the admin signs in to the API" 200 "$login"
[ "$login" = 200 ] || { tail -3 "$D/api.log" | cut -c1-300; exit 1; }
# The session's cookies are Secure and the API is asked over plain HTTP on
# an address that is not the loopback, where curl neither keeps nor sends
# them: they are read from the answer and sent as a header.
COOKIES=$(tr -d '\r' <"$J" | sed -n 's/^[Ss]et-[Cc]ookie: \([^;]*\).*/\1/p' | paste -sd';' | sed 's/;/; /g')
CSRF=$(printf '%s' "$COOKIES" | tr ';' '\n' | sed -n 's/^ *csrf=//p' | python3 -c 'import sys,urllib.parse;print(urllib.parse.unquote(sys.stdin.read().strip()))')
admin() { curl -s -H "Cookie: $COOKIES" -H "x-csrf-token: $CSRF" -H 'content-type: application/json' "$@"; }
field() { python3 -c 'import sys,json;print(json.load(sys.stdin).get(sys.argv[1],""))' "$1"; }
id_of() { admin "$API/machines" | python3 -c 'import sys,json;print(next(m["id"] for m in json.load(sys.stdin)["machines"] if m["name"]==sys.argv[1]))' "$1"; }
state_of() { admin "$API/machines" | python3 -c 'import sys,json;print(next(m["state"] for m in json.load(sys.stdin)["machines"] if m["name"]==sys.argv[1]))' "$1"; }
# pem NAME RESPONSE-FILE: NAME.crt of the answer and NAME.pem with its key.
pem() {
  python3 -c 'import sys,json;sys.stdout.write(json.load(open(sys.argv[1]))["certificate"])' "$2" >"$D/$1.crt"
  openssl pkey -in "$D/$1.key" -out "$D/$1.p8" 2>/dev/null
  cat "$D/$1.crt" "$D/$1.p8" >"$D/$1.pem"
}
# join ROLE NAME: by the real routes: a join token, the signing request, its certificate (pending).
join() {
  local token
  token=$(admin -d "{\"role\":\"$1\"}" "$API/machines/join-tokens" | field token)
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/CN=$2" -keyout "$D/$2.key" -out "$D/$2.csr" 2>/dev/null
  python3 -c 'import sys,json;print(json.dumps({"token":sys.argv[1],"name":sys.argv[2],"csr":open(sys.argv[3]).read()}))' \
    "$token" "$2" "$D/$2.csr" >"$D/$2.join"
  curl -s -H 'content-type: application/json' -d @"$D/$2.join" "$API/install/join" >"$D/$2.joined"
  rm -f "$D/$2.join"
  pem "$2" "$D/$2.joined"
}
approve() { admin -o /dev/null -w '%{http_code}' -X POST "$API/machines/$(id_of "$1")/approve"; }
# header NAME: its certificate as Traefik writes X-Forwarded-Tls-Client-Cert.
header() { openssl x509 -in "$D/$1.crt" -outform DER | base64 -w0 | python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.stdin.read().strip(),safe=""))'; }
join lake localhost
join worker worker-7
join lake-replica replica-1
join worker worker-8
join trino-worker app-1 # in the place of the application VM's own
expect "a worker joined by the real routes is pending" pending "$(state_of worker-7)"
expect "approved by the admin: the lake node, a worker, a replica" "200 200 200" "$(approve localhost) $(approve worker-7) $(approve replica-1)"
approve app-1 >/dev/null
like_subject=$(openssl x509 -in "$D/worker-7.crt" -noout -subject | sed 's/^subject=//')
expect "its certificate, signed by the API's CA" "O=qd, OU=worker, CN=worker-7" "$like_subject"

# The lake node's agent directory, as its agent leaves it (uid 10001).
# node PEM: written aside and renamed.
node() {
  asroot "cp /d/ca.crt /d/machine/.ca.crt.new && cp /d/$1.pem /d/machine/.machine.pem.new &&
    chown 10001:10001 /d/machine /d/machine/.ca.crt.new /d/machine/.machine.pem.new &&
    chmod 644 /d/machine/.ca.crt.new && chmod 600 /d/machine/.machine.pem.new &&
    mv /d/machine/.ca.crt.new /d/machine/ca.crt && mv /d/machine/.machine.pem.new /d/machine/machine.pem"
}
node localhost

# The application's Traefik, with the repository's files: the machines'
# entry point and the gates', the API behind as a file service.
cp "$ROOT"/infra/traefik/dynamic/*.yml "$ROOT/infra/traefik/internal.yml" "$D/dynamic/"
cp "$D/internal.crt" "$D/dynamic/internal.crt" && cp "$D/internal.key" "$D/dynamic/internal.key"
cp "$D/ca.crt" "$D/dynamic/machines-ca.crt"
cp "$D/local.crt" "$D/local.key" "$D/certs/"
cp "$D/app-1.pem" "$D/dynamic/app-machine.pem"
printf 'http:\n  services:\n    verify-api:\n      loadBalancer:\n        servers:\n          - url: "%s"\n' "$API" >"$D/dynamic/verify-api.yml"
chmod 755 "$D/dynamic" "$D/certs" "$D/le" && chmod 644 "$D/dynamic"/* "$D/certs"/*
docker rm -f "$APPT" >/dev/null 2>&1
docker run -d --name "$APPT" --network "$FRONT" -p "127.0.0.1:$LAKEGATE:9444" -p "127.0.0.1:$MACHINES:9443" -p "127.0.0.1:$PUBLIC:443" \
  -e TRAEFIK_CERTIFICATESRESOLVERS_LETSENCRYPT_ACME_EMAIL=verify@example.com \
  -e APP_DOMAIN=localhost -e INTERNAL_DOMAIN=internal.localhost \
  -e LAKE_TRANSPORT=lake@file -e LAKE_SERVER_NAME=localhost -e RUSTFS_URL=https://gate:9000 \
  -e INTERNAL_API_SERVICE=verify-api@file -e TRINO_RULES_SERVICE=verify-api@file \
  -v "$ROOT/infra/traefik/traefik.yml:/etc/traefik/traefik.yml:ro" \
  -v "$D/dynamic:/etc/traefik/dynamic:ro" -v "$D/certs:/etc/traefik/certs:ro" -v "$D/le:/letsencrypt" \
  "$PROXY_IMAGE" --configFile=/etc/traefik/traefik.yml >/dev/null || { echo "could not start the application's Traefik"; exit 1; }

# RustFS and its gate, as infra/rustfs has them; the gate finds the
# application's Traefik by the internal domain's name.
up=$(compose up -d --wait --wait-timeout 180 rustfs gate 2>&1) || { echo "RustFS and its gate did not start: $up" | tail -5; compose logs 2>&1 | tail -10; exit 1; }
docker network connect --alias internal.localhost "${P}_default" "$APPT" || { echo "could not join the gate's network"; exit 1; }
# entry PORT CERT PATH [CURL OPTION...]: asked by hand at an entry point of the application's Traefik.
entry() {
  local port=$1 cert=$2 path=$3
  shift 3
  curl -s -o "$D/body" -w '%{http_code}' --max-time 10 --cacert "$D/ca.crt" --cert "$D/$cert.pem" \
    --connect-to "internal.localhost:$port:127.0.0.1:$port" "$@" "https://internal.localhost:$port$path"
}
# info NAME ROLE: what a gate's Traefik writes of NAME's certificate.
info() {
  python3 -c 'import sys,urllib.parse;print(urllib.parse.quote("Subject=\"O=qd,OU=%s,CN=%s\";SerialNumber=\"%d\"" % (sys.argv[2], sys.argv[1], int(sys.argv[3], 16)), safe=""))' \
    "$1" "$2" "$(openssl x509 -in "$D/$1.crt" -noout -serial | cut -d= -f2)"
}

# at NAME METHOD PATH [CURL OPTION...]: a signed request to the gate with NAME's certificate.
at() {
  local cert=$1 method=$2 path=$3
  shift 3
  curl -s -o "$D/body" -w '%{http_code}' --max-time 15 --cacert "$D/ca.crt" --cert "$D/$cert.pem" \
    -X "$method" --aws-sigv4 "aws:amz:us-east-1:s3" --user "$KEY:$SECRET" "$@" "https://localhost:$GATE$path"
}
for _ in $(seq 40); do [ "$(at worker-7 GET /)" = 200 ] && break; sleep 1; done

echo "== Who passes"
expect "an approved worker makes a bucket through the gate" 200 "$(at worker-7 PUT /qd-verify)"
expect "an approved lake replica lists it" 200 "$(at replica-1 GET /qd-verify)"
expect "a pending machine: the API's 401, passed on by the gate" 401 "$(at worker-8 PUT /qd-pending)"
expect "the pending machine with the worker forged in both headers: written over, 401" 401 \
  "$(at worker-8 PUT /qd-forged -H "X-Forwarded-Tls-Client-Cert: $(header worker-7)" -H "X-Forwarded-Tls-Client-Cert-Info: $(info worker-7 worker)")"
expect "another CA's certificate: refused at the handshake" 000 "$(at stranger GET /qd-verify)"
expect "the refused never reached RustFS" "404 404" "$(at worker-7 GET /qd-pending) $(at worker-7 GET /qd-forged)"
expect "a worker asks the API by hand at the gates' entry point, naming the approved worker: not a gate, 401" 401 \
  "$(entry $LAKEGATE worker-8 /api/internal/machines/lake/check -H "X-Forwarded-Tls-Client-Cert-Info: $(info worker-7 worker)")"
expect "the approved worker itself, naming itself: not a gate either" 401 \
  "$(entry $LAKEGATE worker-7 /api/internal/machines/lake/check -H "X-Forwarded-Tls-Client-Cert-Info: $(info worker-7 worker)")"
expect "on the machines' entry point the name does not arrive: the lake node itself gets 401" 401 \
  "$(entry $MACHINES localhost /api/internal/machines/lake/check -H "X-Forwarded-Tls-Client-Cert-Info: $(info worker-7 worker)")"
expect "the lake node asking by hand at the gates' entry point, as its gate does" 200 \
  "$(entry $LAKEGATE localhost /api/internal/machines/lake/check -H "X-Forwarded-Tls-Client-Cert-Info: $(info worker-7 worker)")"

echo "== DuckDB through the worker's proxy, the gate and the API"
# given NAME: the worker's agent directory holds NAME's machine.pem, written aside and renamed
# as the agent does; the proxy looks every 2 seconds.
given() {
  cp "$D/ca.crt" "$D/worker/ca.crt"
  install -m 600 "$D/$1.pem" "$D/worker/.machine.pem.new" && mv "$D/worker/.machine.pem.new" "$D/worker/machine.pem"
  sleep 5
}
given worker-7
# The worker's proxy, by the repository's entrypoint, on the loopback.
docker rm -f "$PX" "$DUCK" >/dev/null 2>&1
docker run -d --name "$PX" --init --network host --user "$(id -u):$(id -g)" --tmpfs "/run/qd-lake:uid=$(id -u),mode=0700" \
  -v "$D/infra/worker/lake/entrypoint.sh:/etc/qd/lake/entrypoint.sh:ro" -v "$D/worker:/run/qd-node:ro" \
  -e QD_LAKE_GATE="localhost:$GATE" -e QD_LAKE_LISTEN="127.0.0.1:$PROXY" -e QD_LAKE_CHECK_EVERY=2 \
  --entrypoint /etc/qd/lake/entrypoint.sh "$PROXY_IMAGE" >/dev/null
for _ in $(seq 30); do (exec 3<>"/dev/tcp/127.0.0.1/$PROXY") 2>/dev/null && { sleep 2; break; }; sleep 1; done
proxy_started=$(docker inspect -f '{{.State.StartedAt}}' "$PX")
# DuckDB where the worker's name is its loopback, as in the worker's container (extra_hosts).
docker run -d --name "$DUCK" --network host --add-host worker-7:127.0.0.1 -v "$D:/d:ro" "$DUCK_IMAGE" sleep 3600 >/dev/null
docker exec "$DUCK" sh -c "pip install -q --disable-pip-version-check --root-user-action=ignore duckdb==2.0.0 >/dev/null 2>&1 &&
  python3 -c 'import duckdb; duckdb.connect().execute(\"INSTALL httpfs\")'" || echo "DuckDB could not be installed in $DUCK"
cat >"$D/duck.py" <<'EOF'
import os, sys, time, duckdb
mode = sys.argv[1]
c = duckdb.connect()
c.execute("LOAD httpfs")
c.execute("SET ca_cert_file = '/d/ca.crt'")
c.execute(f"""CREATE SECRET s (TYPE s3, KEY_ID '{os.environ['S3_KEY']}', SECRET '{os.environ['S3_SECRET']}',
  ENDPOINT '{os.environ['ENDPOINT']}', URL_STYLE 'path', USE_SSL true, REGION 'us-east-1')""")
try:
    if mode == "small":
        c.execute("COPY (SELECT 7 AS n) TO 's3://qd-verify/x.csv'")
        print("rows", c.execute("SELECT sum(n) FROM read_csv('s3://qd-verify/x.csv')").fetchone()[0])
    else:
        rows, url = int(os.environ["ROWS"]), "s3://qd-verify/big.parquet"
        t0 = time.monotonic()
        c.execute(f"""COPY (SELECT range AS n, hash(range) AS h1, hash(range + {rows}) AS h2 FROM range({rows}))
          TO '{url}' (FORMAT parquet, COMPRESSION uncompressed)""")
        t1 = time.monotonic()
        n, total = c.execute(f"SELECT count(*), sum(n) FROM read_parquet('{url}') WHERE h1 IS NOT NULL AND h2 IS NOT NULL").fetchone()
        t2 = time.monotonic()
        size = c.execute(f"SELECT size FROM read_blob('{url}')").fetchone()[0]
        good = n == rows and total == rows * (rows - 1) // 2
        print(f"big {'whole' if good else 'WRONG'} {size // 1000000}MB write {t1 - t0:.1f}s read {t2 - t1:.1f}s")
except Exception as e:
    print(str(e).splitlines()[0][:160])
EOF
# duck MODE [HOST]: at the proxy under the worker's name, or under HOST.
duck() {
  docker exec -e ENDPOINT="${2:-worker-7}:$PROXY" -e S3_KEY="$KEY" -e S3_SECRET="$SECRET" -e ROWS="$BIG_ROWS" \
    "$DUCK" python3 -I /d/duck.py "$1" 2>&1 | tail -1
}
# lookups: how often the API read `machines` (its scans, as PostgreSQL counts them).
lookups() { sleep 2; sql "select pg_stat_clear_snapshot(); select coalesce(seq_scan,0)+coalesce(idx_scan,0) from pg_stat_user_tables where relname = 'machines'" | tail -1; }
expect "DuckDB writes and reads over TLS to its proxy, the API deciding" "rows 7" "$(duck small)"
refused "the proxy's certificate is the machine's: under another name DuckDB does not trust it" "$(duck small localhost)"
l0=$(lookups)
big=$(duck big)
l1=$(lookups)
case "$big" in big\ whole*) ok "a large object (multipart) written and read back: $big" ;; *) ko "the large object: $big" ;; esac
echo "[INFO] the API read \`machines\` $((l1 - l0)) times for it (over a hundred requests through the gate; passes kept 5 s)"
at worker-7 DELETE /qd-verify/big.parquet >/dev/null

echo "== A renewed certificate"
# As the agent does: a new key, signed for the certificate it holds.
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/CN=worker-7" -keyout "$D/renewed.key" -out "$D/renewed.csr" 2>/dev/null
python3 -c 'import sys,json;print(json.dumps({"csr":open(sys.argv[1]).read()}))' "$D/renewed.csr" >"$D/renew.json"
renew=$(entry $MACHINES worker-7 /api/internal/machines/renew -H 'content-type: application/json' -d @"$D/renew.json")
cp "$D/body" "$D/renewed.json"
expect "POST /internal/machines/renew on the machines' entry point, with the certificate in use" 200 "$renew"
pem renewed "$D/renewed.json"
expect "until the new one is presented, the one in use still passes" 200 "$(at worker-7 GET /qd-verify)"
expect "the renewed certificate passes (and becomes the one in use)" 200 "$(at renewed GET /qd-verify)"
sleep 6
expect "the one it replaced is refused once its pass ended" 401 "$(at worker-7 GET /qd-verify)"
given renewed
expect "DuckDB, the proxy having taken the renewed machine.pem by itself" "rows 7" "$(duck small)"
expect "the proxy was not restarted" "$proxy_started" "$(docker inspect -f '{{.State.StartedAt}}' "$PX")"

echo "== The application reaches the lake through the gate"
expect "the application's Traefik, a signed request of the public entry point: its certificate presented" 200 \
  "$(curl -s -o /dev/null -w '%{http_code}' --max-time 15 --cacert "$D/ca.crt" --aws-sigv4 "aws:amz:us-east-1:s3" --user "$KEY:$SECRET" "https://localhost:$PUBLIC/qd-verify")"
# console URL [CERT]: RustFS's console page, asked for at URL; its status and what it is.
console() { curl -s -o /dev/null -w '%{http_code} %{content_type}' --max-time 15 --cacert "$D/ca.crt" ${2:+--cert "$D/$2.pem"} "$1/rustfs/console/"; }
case "$(console "https://localhost:$PUBLIC")" in 200\ text/html*) ok "RustFS's console, a person's browser on the public entry point: served by RustFS's own port behind the gate" ;;
*) ko "the console on the public entry point: $(console "https://localhost:$PUBLIC")" ;; esac
expect "the console at the gate, a pending machine: the API's 401" "401" "$(console "https://localhost:$GATE" worker-8 | cut -d' ' -f1)"
expect "the console at the gate, no certificate: refused at the handshake" "000" "$(console "https://localhost:$GATE" | cut -d' ' -f1)"
# The API as RustFS's platform admin (the throwaway's root here), with the machine's certificate.
install -m 600 "$D/app-1.pem" "$D/appnode/machine.pem"
cat >>"$D/secret/api.env" <<EOF
LAKE_ENDPOINT=https://localhost:$GATE
LAKE_CA_FILE=$D/ca.crt
LAKE_CLIENT_CERT_FILE=$D/appnode/machine.pem
RUSTFS_PLATFORM_ADMIN_ACCESS_KEY=$KEY
RUSTFS_PLATFORM_ADMIN_SECRET_KEY=$SECRET
EOF
stop_api
start_api || exit 1
area=$(admin -o "$D/body" -w '%{http_code}' -d '{"name":"finance"}' "$API/areas")
case "$area" in 200 | 201) ok "the API makes an area in RustFS through the gate, presenting the machine's certificate: $area" ;; *) ko "the area: $area $(cut -c1-200 "$D/body")" ;; esac
serial_in_use() { admin "$API/machines" | python3 -c 'import sys,json;print(next(m["cert_serial"] for m in json.load(sys.stdin)["machines"] if m["name"]==sys.argv[1]))' "$1"; }
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/CN=app-1" -keyout "$D/app2.key" -out "$D/app2.csr" 2>/dev/null
python3 -c 'import sys,json;print(json.dumps({"csr":open(sys.argv[1]).read()}))' "$D/app2.csr" >"$D/renew.json"
expect "its machine renews" 200 "$(entry $MACHINES app-1 /api/internal/machines/renew -H 'content-type: application/json' -d @"$D/renew.json")"
cp "$D/body" "$D/app2.json" && pem app2 "$D/app2.json"
install -m 600 "$D/app2.pem" "$D/appnode/.machine.pem.new" && mv "$D/appnode/.machine.pem.new" "$D/appnode/machine.pem"
sleep 6
second=$(admin -o "$D/body" -w '%{http_code}' -d '{"name":"treasury"}' "$API/areas")
case "$second" in 200 | 201) ok "with machine.pem renewed, the API goes on, not restarted: $second" ;; *) ko "the second area: $second $(cut -c1-200 "$D/body")" ;; esac
expect "and what it presented is the renewed certificate, now the one in use" \
  "$(openssl x509 -in "$D/app2.crt" -noout -serial | cut -d= -f2 | tr 'A-F' 'a-f')" "$(serial_in_use app-1)"

echo "== Retired, refusals audited, the API down"
at renewed GET /qd-verify >/dev/null
retiring=$(id_of worker-7)
expect "the admin retires the worker" 200 "$(admin -o /dev/null -w '%{http_code}' -X POST "$API/machines/$retiring/retire")"
t0=$(date +%s.%N)
kept=
for _ in $(seq 40); do
  [ "$(at renewed GET /qd-verify)" = 401 ] && { kept=$(python3 -c 'import sys,time;print(f"{time.time()-float(sys.argv[1]):.1f}")' "$t0"); break; }
  sleep 0.2
done
if [ -n "$kept" ] && python3 -c 'import sys;sys.exit(0 if float(sys.argv[1]) <= 7 else 1)' "$kept"; then
  ok "retired, it kept the lake for ${kept}s (a pass lasts 5)"
else ko "retired, it still reached the lake after '${kept:-20}'s"; fi
refused "and DuckDB through its proxy" "$(duck small)"
expect "the lake replica goes on" 200 "$(at replica-1 GET /qd-verify)"
audited=$(sql "select string_agg(action || ' ' || n, ', ' order by action) from (select action, count(*) n from admin_audit_events where event_type = 'certificate_refused' and action like 'lake:%' group by action) a")
case "$audited" in *lake:certificate*lake:gate*lake:unnamed*) ok "each refusal left its audit row: $audited" ;; *) ko "audit rows of refusals: ${audited:-none}" ;; esac
before=$(docker inspect -f '{{.State.StartedAt}}' "$P-gate-1" "$P-rustfs-1" | tr '\n' ' ')
stop_api
down=$(at replica-1 PUT /qd-down)
case "$down" in 200) ko "the API down, the gate let it through" ;; *) ok "the API down, the gate refuses: $down" ;; esac
start_api || exit 1
expect "back up, the replica passes again and what was refused never reached RustFS" "200 404" \
  "$(at replica-1 GET /qd-verify) $(at replica-1 GET /qd-down)"
expect "neither the gate nor RustFS was restarted" "$before" "$(docker inspect -f '{{.State.StartedAt}}' "$P-gate-1" "$P-rustfs-1" | tr '\n' ' ')"

echo "== The gate's own certificate, renewed"
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/CN=localhost" -keyout "$D/node2.key" -out "$D/node2.csr" 2>/dev/null
python3 -c 'import sys,json;print(json.dumps({"csr":open(sys.argv[1]).read()}))' "$D/node2.csr" >"$D/renew.json"
expect "the lake node renews on the machines' entry point" 200 \
  "$(entry $MACHINES localhost /api/internal/machines/renew -H 'content-type: application/json' -d @"$D/renew.json")"
cp "$D/body" "$D/node2.json" && pem node2 "$D/node2.json"
node node2
served() { openssl s_client -connect "127.0.0.1:$GATE" -servername localhost -cert "$D/replica-1.pem" </dev/null 2>/dev/null | openssl x509 -noout -serial 2>/dev/null; }
want=$(openssl x509 -in "$D/node2.crt" -noout -serial)
for _ in $(seq 30); do [ "$(served)" = "$want" ] && break; sleep 1; done
expect "the gate serves the renewed certificate" "$want" "$(served)"
for _ in $(seq 30); do [ "$(at replica-1 GET /qd-verify)" = 200 ] && break; sleep 1; done
expect "and asks with it, RustFS behind serving it too: the replica passes" 200 "$(at replica-1 GET /qd-verify)"
expect "still neither the gate nor RustFS restarted" "$before" "$(docker inspect -f '{{.State.StartedAt}}' "$P-gate-1" "$P-rustfs-1" | tr '\n' ' ')"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
