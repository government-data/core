#!/bin/bash
# Verification of a gate on the lake's side for RustFS (decision 0066, point
# 10), a prototype: a Traefik 3.6 in front of RustFS, in HTTP mode, on a TLS
# entry point that requires a client certificate of the installation's CA
# (RequireAndVerifyClientCert), passes it on (passTLSClientCert, pem) and asks
# a validator before each request (forwardAuth). The validator is a stub of
# what the API would do: it believes only the first certificate of
# X-Forwarded-Tls-Client-Cert (as crates/auth-gateway/src/http/client_cert.rs),
# and answers 200 only if its serial is in an allow-list it re-reads at every
# call (the serials in use, point 14) and its OU is worker, trino, lake or app;
# otherwise 403. It counts its calls. RustFS sits behind, with the
# repository's infra/rustfs files and its TLS, not requiring client
# certificates (RUSTFS_MTLS=0), its port not published, on a network of its
# own shared only with the gate. The worker reaches the gate through the TCP
# proxy on its loopback of tools/verify/rustfs_worker_proxy.sh, which
# presents its machine.pem. It shows:
#   - DuckDB 2.0 writes and reads through worker proxy, gate and RustFS, its
#     SigV4 signatures accepted: the gate passes the Host header on unchanged
#     (Traefik's default, passHostHeader), so a signature made for the
#     proxy's loopback address holds;
#   - a large object (a multipart upload of synthetic numbers, over 200 MB)
#     written and read back whole through the chain;
#   - the Iceberg REST catalog of RustFS (/iceberg) through the chain, as the
#     worker attaches it: a namespace, a table, one synthetic row;
#   - the refusals: no certificate, another CA's, a serial not in use, a role
#     not allowed (RustFS never reached), a forged certificate header, the
#     validator down; a serial taken out of the list refuses that machine's
#     next request with nothing restarted;
#   - the cost: the validator's calls, and the time of the large upload and
#     read (a) straight to a RustFS over TLS without client certificates,
#     (b) through the worker's proxy to a RustFS that requires them, (c)
#     through the worker's proxy and the gate;
#   - RustFS cannot be reached but through the gate in this layout.
#
# Two Compose projects, qd-verify-lake-gate (RustFS, gate, validator) and
# qd-verify-lake-native (the RustFS of (a) and (b)), and the proxy container
# qd-verify-lake-gate-proxy; ports 127.0.0.1:59020 (the native RustFS), 59021
# (the gate), 59023 and 59024 (the proxy, to each). Nothing of the dev stack
# is touched. The validator is Python's standard library, run with uv's own
# Python (python-build-standalone) mounted into an image already here
# (mysql:8.4, only for its glibc). RustFS's root credentials are random and
# used here for the signed requests, as nothing else exists in a RustFS thrown
# away at the end (an installation never uses root, AGENTS.md); in an
# installation the job's STS credentials sign. Synthetic numbers only. The
# large object is removed after each run. Run from the repository root:
#
#   bash tools/verify/rustfs_lake_gate.sh        (BIG_ROWS=10000000 by default)
#
# Needs docker with Compose v2 (!reset), curl 7.75 or later, openssl 3 and uv
# (duckdb, a managed Python 3.12).
set -u
export LC_ALL=C
PG=qd-verify-lake-gate
PN=qd-verify-lake-native
PX=$PG-proxy
NATIVE=59020
GATE=59021
PX_NATIVE=59023
PX_GATE=59024
PROXY_IMAGE=traefik:v3.6
GLIBC_IMAGE=mysql:8.4
BIG_ROWS=${BIG_ROWS:-10000000}
PYHOME=$(dirname "$(dirname "$(readlink -f "$(uv python find --managed-python 3.12)")")")
D=$(mktemp -d)
chmod 755 "$D"
FAILED=0
mkdir -p "$D/infra" "$D/dyn"
cp -r infra/rustfs "$D/infra/rustfs"
chmod -R a+rX "$D/infra"
mkdir -m 700 "$D/machine" "$D/worker" "$D/gate" "$D/state"
KEY=qd$(openssl rand -hex 8)
SECRET=$(openssl rand -hex 16)
IMAGE=$(sed -n 's/^ *image: \${RUSTFS_IMAGE:-\(.*\)}$/\1/p' infra/rustfs/docker-compose.yml)
for p in $PG $PN; do
  cat >"$D/$p.env" <<EOF
APP_STACK=$p
RUSTFS_ACCESS_KEY=$KEY
RUSTFS_SECRET_KEY=$SECRET
QD_MACHINE_DIR=$D/machine
RUSTFS_BIND=127.0.0.1
RUSTFS_PORT=$NATIVE
RUSTFS_MTLS=0
QD_UID=$(id -u):$(id -g)
QD_PYTHON=$PYHOME
EOF
done
FILES="-f infra/rustfs/docker-compose.yml -f infra/rustfs/certificate.yml"
# shellcheck disable=SC2086
gate_compose() { (cd "$D" && docker compose --project-directory . --env-file $PG.env -p $PG $FILES -f gate.yml "$@"); }
# shellcheck disable=SC2086
native_compose() { (cd "$D" && docker compose --project-directory . --env-file $PN.env -p $PN $FILES "$@"); }
asroot() { docker run --rm --user 0 --entrypoint sh -v "$D:/d" "$IMAGE" -c "$1"; }
cleanup() {
  docker rm -f "$PX" >/dev/null 2>&1
  gate_compose down -v --remove-orphans >/dev/null 2>&1
  native_compose down -v --remove-orphans >/dev/null 2>&1
  asroot 'rm -rf /d/machine/*' >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT

ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }
refused() { case "$2" in rows* | iceberg*) ko "$1: got through: $2" ;; *) ok "$1: $(echo "$2" | cut -c1-100)" ;; esac; }

ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/O=qd/CN=qd verify $1" -addext "basicConstraints=critical,CA:TRUE" \
    -keyout "$D/$1.key" -out "$D/$1.crt" 2>/dev/null
}
# leaf NAME CA SUBJECT: NAME.pem, the certificate then the PKCS#8 key, for localhost.
leaf() {
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$3" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nkeyUsage=digitalSignature\nextendedKeyUsage=clientAuth,serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n' >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/$2.crt" -CAkey "$D/$2.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  openssl pkey -in "$D/$1.key" -out "$D/$1.p8"
  cat "$D/$1.crt" "$D/$1.p8" >"$D/$1.pem"
}
serial() { openssl x509 -noout -serial -in "$D/$1.crt" | cut -d= -f2; }
ca ca
ca other
leaf lake ca "/O=qd/OU=lake/CN=localhost"
leaf worker ca "/O=qd/OU=worker/CN=worker-7"
leaf trino ca "/O=qd/OU=trino/CN=trino-1"
leaf admin ca "/O=qd/OU=admin/CN=worker-7"
leaf unlisted ca "/O=qd/OU=worker/CN=worker-8"
leaf stranger other "/O=qd/OU=worker/CN=worker-7"
asroot "cp /d/ca.crt /d/machine/ca.crt && cp /d/lake.pem /d/machine/machine.pem &&
  chown -R 10001:10001 /d/machine && chmod 644 /d/machine/ca.crt && chmod 600 /d/machine/machine.pem"
cp "$D/ca.crt" "$D/lake.pem" "$D/gate/"
cp "$D/ca.crt" "$D/worker/" && install -m 600 "$D/worker.pem" "$D/worker/machine.pem"
# allow SERIAL...: the serials in use, written aside and renamed.
allow() { printf '%s\n' "$@" >"$D/state/.allow" && mv "$D/state/.allow" "$D/state/allow"; }
allow "$(serial worker)" "$(serial trino)" "$(serial admin)"
calls() { cat "$D/state/calls" 2>/dev/null || echo 0; }

# The validator: what the API would answer to forwardAuth.
cat >"$D/state/validator.py" <<'EOF'
import base64, http.server, os, ssl, sys, tempfile, threading, urllib.parse
STATE, ROLES, LOCK, CALLS = sys.argv[1], {"worker", "trino", "lake", "app"}, threading.Lock(), [0]

def count():
    with LOCK:
        CALLS[0] += 1
        with open(f"{STATE}/.calls", "w") as f:
            f.write(str(CALLS[0]))
        os.replace(f"{STATE}/.calls", f"{STATE}/calls")

def certificate(values):
    """Serial and OUs of the first certificate of the one header, or None."""
    if len(values) != 1:
        return None
    first = urllib.parse.unquote(values[0].strip()).split(",")[0].strip()
    try:
        pem = ssl.DER_cert_to_PEM_cert(base64.b64decode(first, validate=True))
        with tempfile.NamedTemporaryFile("w", suffix=".pem") as f:
            f.write(pem)
            f.flush()
            d = ssl._ssl._test_decode_cert(f.name)
    except Exception:
        return None
    return d["serialNumber"], [v for rdn in d["subject"] for k, v in rdn if k == "organizationalUnitName"]

class Gate(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def check(self):
        count()
        cert = certificate(self.headers.get_all("X-Forwarded-Tls-Client-Cert") or [])
        try:
            allowed = set(open(f"{STATE}/allow").read().split())
        except OSError:
            allowed = set()
        why = ("no certificate" if cert is None else "serial not in use" if cert[0] not in allowed
               else "role not allowed" if len(cert[1]) != 1 or cert[1][0] not in ROLES else None)
        body = b"" if why is None else f"qd-gate: {why}\n".encode()
        self.send_response(200 if why is None else 403)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    do_GET = do_PUT = do_POST = do_HEAD = do_DELETE = check

    def log_message(self, *args):
        pass

http.server.ThreadingHTTPServer(("0.0.0.0", 8080), Gate).serve_forever()
EOF

# The lake's side: RustFS unpublished on `lake`, the gate on `lake`, `front`
# and the published edge, the validator on `front` only.
cat >"$D/gate.yml" <<EOF
services:
  rustfs:
    ports: !reset []
    networks: [lake]
  gate:
    image: $PROXY_IMAGE
    user: \${QD_UID}
    networks: [lake, front, edge]
    ports: ["127.0.0.1:$GATE:8443"]
    volumes: ["./dyn:/dyn:ro", "./gate:/gate:ro"]
    command: [--providers.file.directory=/dyn, --providers.file.watch=true,
              --entrypoints.lake.address=:8443, --log.level=ERROR]
  validator:
    image: $GLIBC_IMAGE
    user: \${QD_UID}
    networks: [front]
    entrypoint: [/py/bin/python3.12, -I, /state/validator.py, /state]
    volumes: ["\${QD_PYTHON}:/py:ro", "./state:/state"]
networks:
  lake: {internal: true}
  front: {internal: true}
  edge: {}
EOF
cat >"$D/dyn/gate.yml" <<'EOF'
http:
  routers:
    lake:
      entryPoints: [lake]
      rule: "PathPrefix(`/`)"
      tls: {}
      middlewares: [machine-certificate, gate]
      service: rustfs
  middlewares:
    machine-certificate:
      passTLSClientCert:
        pem: true
    gate:
      forwardAuth:
        address: http://validator:8080/check
  services:
    rustfs:
      loadBalancer:
        serversTransport: rustfs
        servers:
          - url: https://rustfs:9000
  serversTransports:
    rustfs:
      serverName: localhost
      rootCAs: [/gate/ca.crt]
tls:
  options:
    default:
      minVersion: VersionTLS12
      clientAuth:
        caFiles: [/gate/ca.crt]
        clientAuthType: RequireAndVerifyClientCert
  stores:
    default:
      defaultCertificate:
        certFile: /gate/lake.pem
        keyFile: /gate/lake.pem
EOF

# The worker's proxy: TCP only, to the native RustFS and to the gate.
mkdir -p "$D/pxdyn"
cat >"$D/pxdyn/lake.yml" <<EOF
tcp:
  routers:
    native: {entryPoints: [native], rule: "HostSNI(\`*\`)", service: native}
    gate: {entryPoints: [gate], rule: "HostSNI(\`*\`)", service: gate}
  services:
    native:
      loadBalancer:
        serversTransport: worker
        servers: [{address: "127.0.0.1:$NATIVE", tls: true}]
    gate:
      loadBalancer:
        serversTransport: worker
        servers: [{address: "127.0.0.1:$GATE", tls: true}]
  serversTransports:
    worker:
      tls:
        serverName: localhost
        rootCAs: [/worker/ca.crt]
        certificates:
          - {certFile: /worker/machine.pem, keyFile: /worker/machine.pem}
EOF

# duck MODE ENDPOINT SSL [BUCKET]: DuckDB through the S3 API (and the catalog).
cat >"$D/duck.py" <<'EOF'
import os, sys, time, duckdb
mode, ep, ssl = sys.argv[1], os.environ["ENDPOINT"], os.environ["SSL"]
calls = lambda: int(open(os.environ["CALLS"]).read()) if os.path.exists(os.environ["CALLS"]) else 0
c = duckdb.connect()
c.execute("LOAD httpfs")
if ssl == "true":
    c.execute(f"SET ca_cert_file = '{os.environ['CA']}'")
c.execute(f"""CREATE SECRET s (TYPE s3, KEY_ID '{os.environ['S3_KEY']}', SECRET '{os.environ['S3_SECRET']}',
  ENDPOINT '{ep}', URL_STYLE 'path', USE_SSL {ssl}, REGION 'us-east-1')""")
try:
    if mode == "small":
        c.execute("COPY (SELECT 7 AS n) TO 's3://qd-verify/x.csv'")
        print("rows", c.execute("SELECT sum(n) FROM read_csv('s3://qd-verify/x.csv')").fetchone()[0])
    elif mode == "big":
        rows, url = int(os.environ["ROWS"]), "s3://qd-verify/big.parquet"
        k0, t0 = calls(), time.monotonic()
        c.execute(f"""COPY (SELECT range AS n, hash(range) AS h1, hash(range + {rows}) AS h2 FROM range({rows}))
          TO '{url}' (FORMAT parquet, COMPRESSION uncompressed)""")
        k1, t1 = calls(), time.monotonic()
        n, total = c.execute(f"SELECT count(*), sum(n) FROM read_parquet('{url}') WHERE h1 IS NOT NULL AND h2 IS NOT NULL").fetchone()
        k2, t2 = calls(), time.monotonic()
        size = c.execute(f"SELECT size FROM read_blob('{url}')").fetchone()[0]
        good = n == rows and total == rows * (rows - 1) // 2
        print(f"big {'whole' if good else 'WRONG'} {size // 1000000}MB write {t1 - t0:.1f}s read {t2 - t1:.1f}s "
              f"calls write {k1 - k0} read {k2 - k1}")
    elif mode == "iceberg":
        c.execute("LOAD avro; LOAD iceberg")
        scheme = "https" if ssl == "true" else "http"
        c.execute(f"""ATTACH '{sys.argv[2]}' AS lake (TYPE iceberg, URI '{scheme}://{ep}/iceberg',
          AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s)""")
        c.execute("CREATE SCHEMA IF NOT EXISTS lake.verify")
        c.execute("CREATE TABLE IF NOT EXISTS lake.verify.numbers (n INTEGER)")
        c.execute("INSERT INTO lake.verify.numbers VALUES (7)")
        print("iceberg", c.execute("SELECT sum(n) FROM lake.verify.numbers").fetchone()[0])
except Exception as e:
    print(str(e).splitlines()[0][:160])
EOF
duck() {
  (cd "$D" && CA="$D/ca.crt" ENDPOINT=$2 SSL=$3 S3_KEY=$KEY S3_SECRET=$SECRET ROWS=$BIG_ROWS CALLS="$D/state/calls" \
    uv run -q --no-project --with duckdb==2.0.0 python3 -I duck.py "$1" "${4:-}" 2>&1 | tail -1)
}
# at CERT METHOD PATH [CURL OPTION...]: a signed request to the gate; the body in $D/body.
at() {
  local cert=$1 method=$2 path=$3
  shift 3
  # shellcheck disable=SC2046
  curl -s -o "$D/body" -w '%{http_code}' --max-time 10 --cacert "$D/ca.crt" $([ "$cert" = - ] || echo --cert "$D/$cert.pem") \
    -X "$method" --aws-sigv4 "aws:amz:us-east-1:s3" --user "$KEY:$SECRET" "$@" "https://localhost:$GATE$path"
}
said() { tr -d '\r\n' <"$D/body" | cut -c1-60; }
started() { docker inspect -f '{{.State.StartedAt}}' "$1"; }

echo "== The lake's side: RustFS (${IMAGE##*:}, RUSTFS_MTLS=0, unpublished) behind the gate"
up=$(gate_compose up -d --wait --wait-timeout 120 2>&1) ||
  { echo "the gate's stack did not start: $up" | tail -5; gate_compose logs 2>&1 | tail -20; exit 1; }
for _ in $(seq 30); do [ "$(at worker GET /)" != 000 ] && [ "$(at worker GET /)" != 502 ] && break; sleep 1; done
expect "a bucket made through the gate with the worker's certificate" 200 "$(at worker PUT /qd-verify)"
expect "a role allowed (trino) lists it" 200 "$(at trino GET /qd-verify)"
expect "no certificate: refused at the gate's handshake" 000 "$(at - GET /qd-verify)"
expect "another CA's certificate: refused at the handshake" 000 "$(at stranger GET /qd-verify)"
expect "a serial not in use: 403" "403 qd-gate: serial not in use" "$(at unlisted PUT /qd-verify/denied-1 -d 1) $(said)"
expect "a role not allowed (OU=admin, its serial listed): 403" "403 qd-gate: role not allowed" "$(at admin PUT /qd-verify/denied-2 -d 1) $(said)"
forged=$(openssl x509 -in "$D/worker.crt" -outform DER | base64 -w0)
expect "admin's certificate with the worker's forged in the header: replaced, 403" "403 qd-gate: role not allowed" \
  "$(at admin PUT /qd-verify/denied-3 -d 1 -H "X-Forwarded-Tls-Client-Cert: $forged") $(said)"
expect "an unlisted serial with a listed one forged in the header: 403" "403 qd-gate: serial not in use" \
  "$(at unlisted PUT /qd-verify/denied-4 -d 1 -H "X-Forwarded-Tls-Client-Cert: $forged") $(said)"
expect "the refused writes never reached RustFS" "404 404 404 404" \
  "$(for i in 1 2 3 4; do printf '%s ' "$(at worker HEAD /qd-verify/denied-$i -I)"; done | sed 's/ $//')"
expect "the gate is not S3's authentication: unsigned, RustFS itself refuses" 403 \
  "$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 --cacert "$D/ca.crt" --cert "$D/worker.pem" "https://localhost:$GATE/qd-verify")"

echo "== DuckDB through the worker's proxy and the gate"
docker run -d --name "$PX" --network host --user "$(id -u):$(id -g)" \
  -v "$D/pxdyn:/dyn:ro" -v "$D/worker:/worker:ro" "$PROXY_IMAGE" \
  --providers.file.directory=/dyn --providers.file.watch=true \
  --entrypoints.native.address=127.0.0.1:$PX_NATIVE --entrypoints.gate.address=127.0.0.1:$PX_GATE \
  --log.level=ERROR >/dev/null
for _ in $(seq 30); do (exec 3<>"/dev/tcp/127.0.0.1/$PX_GATE") 2>/dev/null && { sleep 2; break; }; sleep 1; done
refused "DuckDB straight to the gate (no certificate to present)" "$(duck small "localhost:$GATE" true)"
k0=$(calls)
expect "DuckDB writes and reads, signed for the proxy's loopback address, Host passed on as it came" "rows 7" \
  "$(duck small "127.0.0.1:$PX_GATE" false)"
small_calls=$(($(calls) - k0))
expect "the Iceberg catalog through the chain: a warehouse's bucket, enabled for tables" "200 200" \
  "$(at worker PUT /qd-verify-lake) $(at worker PUT /iceberg/v1/buckets/qd-verify-lake --data '')"
k0=$(calls)
expect "DuckDB attaches it, makes a namespace and a table, inserts and reads one row" "iceberg 7" \
  "$(duck iceberg "127.0.0.1:$PX_GATE" false qd-verify-lake)"
iceberg_calls=$(($(calls) - k0))

echo "== A serial taken out of the list, the validator down"
before="$(started "$PG-gate-1") $(started "$PG-validator-1")"
allow "$(serial trino)" "$(serial admin)"
refused "the worker's serial taken out: its next request is refused" "$(duck small "127.0.0.1:$PX_GATE" false)"
expect "and curl with it: 403" "403 qd-gate: serial not in use" "$(at worker GET /qd-verify) $(said)"
allow "$(serial worker)" "$(serial trino)" "$(serial admin)"
expect "put back, it gets through again" "rows 7" "$(duck small "127.0.0.1:$PX_GATE" false)"
expect "nothing was restarted" "$before" "$(started "$PG-gate-1") $(started "$PG-validator-1")"
gate_compose stop validator >/dev/null 2>&1
down=$(at worker PUT /qd-verify/denied-5 -d 1)
case "$down" in 2*) ko "the validator down, a request got through: $down" ;; *) ok "the validator down, the gate refuses: $down $(said)" ;; esac
refused "and DuckDB through the proxy" "$(duck small "127.0.0.1:$PX_GATE" false)"
gate_compose start validator >/dev/null 2>&1
for _ in $(seq 20); do [ "$(at worker GET /qd-verify)" = 200 ] && break; sleep 1; done
expect "back up, the request refused while it was down never reached RustFS" 404 "$(at worker HEAD /qd-verify/denied-5 -I)"

echo "== RustFS reachable only through the gate"
expect "RustFS publishes no port" "" "$(docker port "$PG-rustfs-1")"
expect "its only network" "${PG}_lake" "$(docker inspect -f '{{range $k, $v := .NetworkSettings.Networks}}{{$k}} {{end}}' "$PG-rustfs-1" | sed 's/ $//')"
expect "who is on that network" "$PG-gate-1 $PG-rustfs-1" \
  "$(docker network inspect -f '{{range .Containers}}{{.Name}} {{end}}' "${PG}_lake" | tr ' ' '\n' | grep . | sort | tr '\n' ' ' | sed 's/ $//')"
expect "the network is internal (no route out, no port mapping)" true "$(docker network inspect -f '{{.Internal}}' "${PG}_lake")"
expect "from the validator's network RustFS is not even a name" "unreachable" \
  "$(docker exec "$PG-validator-1" /py/bin/python3.12 -I -c "import socket
try: socket.create_connection(('rustfs', 9000), 3); print('reached')
except OSError: print('unreachable')")"

echo "== Cost: a large object (multipart), $BIG_ROWS rows of synthetic numbers"
big() { # LABEL ENDPOINT SSL CERT-FOR-DELETE
  local out
  out=$(duck big "$2" "$3")
  case "$out" in big\ whole*) ok "$1: ${out#big }" ;; *) ko "$1: $out" ;; esac
  echo "$1|${out#big whole }" >>"$D/costs"
}
k0=$(calls)
big "(c) worker proxy + gate (forwardAuth) + RustFS" "127.0.0.1:$PX_GATE" false
big_calls=$(($(calls) - k0))
# Whether the gate held the parts in memory: its cgroup's peak, in MB.
gate_peak=$(($(docker exec "$PG-gate-1" cat /sys/fs/cgroup/memory.peak 2>/dev/null || echo 0) / 1000000))
at worker DELETE /qd-verify/big.parquet >/dev/null
up=$(native_compose up -d --wait --wait-timeout 120 rustfs 2>&1) || { echo "the native RustFS did not start: $up" | tail -5; exit 1; }
s3n() { curl -s -o /dev/null -w '%{http_code}' --max-time 10 --cacert "$D/ca.crt" -X "$1" --aws-sigv4 "aws:amz:us-east-1:s3" --user "$KEY:$SECRET" "${@:3}" "https://localhost:$NATIVE$2"; }
for _ in $(seq 30); do [ "$(s3n GET /)" = 200 ] && break; sleep 1; done
expect "the native RustFS, no client certificate required: a bucket" 200 "$(s3n PUT /qd-verify)"
big "(a) straight to RustFS over TLS, no proxy, no gate" "localhost:$NATIVE" true
s3n DELETE /qd-verify/big.parquet >/dev/null
sed -i 's/^RUSTFS_MTLS=0$/RUSTFS_MTLS=1/' "$D/$PN.env"
native_compose up -d --wait --wait-timeout 120 rustfs >/dev/null 2>&1
for _ in $(seq 30); do [ "$(s3n GET / --cert "$D/worker.pem")" = 200 ] && break; sleep 1; done
expect "the native RustFS now requires a client certificate" 000 "$(s3n GET /qd-verify)"
big "(b) worker proxy + RustFS requiring client certificates" "127.0.0.1:$PX_NATIVE" false
s3n DELETE /qd-verify/big.parquet --cert "$D/worker.pem" >/dev/null

echo "== Cost, measured"
echo "validator calls: small write+read $small_calls, Iceberg attach+create+insert+read $iceberg_calls, large write+read $big_calls"
echo "the gate's peak memory over the whole run, the large object's included: ${gate_peak}MB"
column -t -s'|' "$D/costs"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
