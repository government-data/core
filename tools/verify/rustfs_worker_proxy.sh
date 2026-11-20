#!/bin/bash
# Verification of a TLS proxy beside the worker for a RustFS that requires
# client certificates (decision 0066, point 10): DuckDB 2.0's S3 client
# cannot present one (tools/verify/rustfs_stack_cert.sh), so a proxy on the
# worker's loopback would present the worker's machine certificate for it.
# The proxy is Traefik 3.6, the one the installation already runs, as a TCP
# proxy: it does not read the requests, it only wraps them in TLS.
# It shows, against the repository's RustFS with RUSTFS_MTLS=1:
#   - DuckDB writes and reads the lake through the proxy, its requests
#     signed for the proxy's address (SigV4 signs the Host header), both
#     with plain HTTP on the loopback and with TLS on the loopback;
#   - without the proxy DuckDB cannot reach it;
#   - the agent's machine.pem (certificate then key) serves as both files;
#   - the proxy takes a renewed machine.pem without a restart, but only
#     when its configuration file is touched: it does not watch the
#     certificate's file.
#
# Its own Compose project (qd-verify-rustfs-proxy), volume and ports
# 127.0.0.1:59010 (RustFS), 59011 and 59012 (the proxy); nothing of the dev
# stack is touched. RustFS's root credentials are random and are used here
# for the signed requests, as nothing else exists in a RustFS thrown away
# at the end (an installation never uses root, AGENTS.md). One synthetic
# row. Run from the repository root:
#
#   bash tools/verify/rustfs_worker_proxy.sh
#
# Needs docker with Compose v2, curl 7.75 or later, openssl 3 and uv (duckdb).
set -u
export LC_ALL=C
P=qd-verify-rustfs-proxy
PORT=59010
PLAIN=59011
TLS=59012
IMAGE=rustfs/rustfs:1.0.4
PROXY_IMAGE=traefik:v3.6
D=$(mktemp -d)
chmod 755 "$D"
FAILED=0
mkdir -p "$D/infra" "$D/dyn"
cp -r infra/rustfs "$D/infra/rustfs"
chmod -R a+rX "$D/infra"
mkdir -m 700 "$D/machine" "$D/worker"
KEY=qd$(openssl rand -hex 8)
SECRET=$(openssl rand -hex 16)
cat >"$D/.env" <<EOF
APP_STACK=$P
RUSTFS_ACCESS_KEY=$KEY
RUSTFS_SECRET_KEY=$SECRET
QD_MACHINE_DIR=$D/machine
RUSTFS_BIND=127.0.0.1
RUSTFS_PORT=$PORT
RUSTFS_MTLS=1
EOF
FILES="-f infra/rustfs/docker-compose.yml -f infra/rustfs/certificate.yml"
# shellcheck disable=SC2086
compose() { (cd "$D" && docker compose --project-directory . --env-file .env -p "$P" $FILES "$@"); }
asroot() { docker run --rm --user 0 --entrypoint sh -v "$D:/d" "$IMAGE" -c "$1"; }
cleanup() {
  docker rm -f "$P-proxy" >/dev/null 2>&1
  compose down -v --remove-orphans >/dev/null 2>&1
  asroot 'rm -rf /d/machine/*' >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT

ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

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
ca ca
ca other
leaf lake ca "/O=qd/OU=lake/CN=localhost"
leaf worker ca "/O=qd/OU=worker/CN=worker-7"
leaf worker2 ca "/O=qd/OU=worker/CN=worker-7"
leaf stranger other "/O=qd/OU=worker/CN=worker-7"
asroot "cp /d/ca.crt /d/machine/ca.crt && cp /d/lake.pem /d/machine/machine.pem &&
  chown -R 10001:10001 /d/machine && chmod 644 /d/machine/ca.crt && chmod 600 /d/machine/machine.pem"
# given PEM: the worker's agent directory, written aside and renamed.
given() {
  cp "$D/ca.crt" "$D/worker/ca.crt"
  install -m 600 "$D/$1.pem" "$D/worker/.machine.pem.new" && mv "$D/worker/.machine.pem.new" "$D/worker/machine.pem"
}

# The proxy: TCP only, one upstream, the agent's files as they are.
cat >"$D/dyn/lake.yml" <<EOF
tcp:
  routers:
    plain:
      entryPoints: [plain]
      rule: "HostSNI(\`*\`)"
      service: lake
    tls:
      entryPoints: [tls]
      rule: "HostSNI(\`*\`)"
      service: lake
      tls: {}
  services:
    lake:
      loadBalancer:
        serversTransport: lake
        servers:
          - address: "127.0.0.1:$PORT"
            tls: true
  serversTransports:
    lake:
      tls:
        serverName: localhost
        rootCAs: [/worker/ca.crt]
        certificates:
          - certFile: /worker/machine.pem
            keyFile: /worker/machine.pem
tls:
  stores:
    default:
      defaultCertificate:
        certFile: /worker/machine.pem
        keyFile: /worker/machine.pem
EOF
proxy() {
  docker rm -f "$P-proxy" >/dev/null 2>&1
  docker run -d --name "$P-proxy" --network host --user "$(id -u):$(id -g)" \
    -v "$D/dyn:/dyn:ro" -v "$D/worker:/worker:ro" "$PROXY_IMAGE" \
    --providers.file.directory=/dyn --providers.file.watch=true \
    --entrypoints.plain.address=127.0.0.1:$PLAIN --entrypoints.tls.address=127.0.0.1:$TLS \
    --log.level=ERROR >/dev/null
  for _ in $(seq 30); do (exec 3<>"/dev/tcp/127.0.0.1/$PLAIN") 2>/dev/null && { sleep 2; return 0; }; sleep 1; done
  echo "the proxy does not listen"; docker logs "$P-proxy" 2>&1 | tail -5
}
proxy_started() { docker inspect -f '{{.State.StartedAt}}' "$P-proxy"; }

# duck ENDPOINT SSL: DuckDB writes one synthetic row and reads it back.
cat >"$D/duck.py" <<'EOF'
import os, duckdb
c = duckdb.connect()
c.execute("LOAD httpfs")
c.execute(f"SET ca_cert_file = '{os.environ['CA']}'")
c.execute(f"""CREATE SECRET s (TYPE s3, KEY_ID '{os.environ['S3_KEY']}', SECRET '{os.environ['S3_SECRET']}',
  ENDPOINT '{os.environ['ENDPOINT']}', URL_STYLE 'path', USE_SSL {os.environ['SSL']}, REGION 'us-east-1')""")
try:
    c.execute("COPY (SELECT 7 AS n) TO 's3://qd-verify/x.csv'")
    print("rows", c.execute("SELECT sum(n) FROM read_csv('s3://qd-verify/x.csv')").fetchone()[0])
except Exception as e:
    print(str(e).splitlines()[0][:160])
EOF
duck() {
  (cd "$D" && CA="$D/ca.crt" ENDPOINT=$1 SSL=$2 S3_KEY=$KEY S3_SECRET=$SECRET \
    uv run -q --no-project --with duckdb==2.0.0 python3 -I duck.py 2>&1 | tail -1)
}

echo "== RustFS requiring a client certificate"
up=$(compose up -d --wait --wait-timeout 120 rustfs 2>&1) ||
  { echo "RustFS did not become healthy: $up" | tail -5; compose logs rustfs 2>&1 | tail -20; exit 1; }
s3() { curl -s -o /dev/null -w '%{http_code}' --max-time 5 --cacert "$D/ca.crt" "$@"; }
for _ in $(seq 30); do [ "$(s3 --cert "$D/worker.pem" "https://localhost:$PORT/qd-verify")" != 000 ] && break; sleep 1; done
expect "a bucket made with the worker's certificate" 200 \
  "$(s3 --cert "$D/worker.pem" -X PUT --aws-sigv4 "aws:amz:us-east-1:s3" --user "$KEY:$SECRET" "https://localhost:$PORT/qd-verify")"
expect "no certificate: refused at the handshake" 000 "$(s3 "https://localhost:$PORT/qd-verify")"
direct=$(duck "localhost:$PORT" true)
case "$direct" in rows*) ko "DuckDB got through without the proxy: $direct" ;; *) ok "DuckDB without the proxy cannot reach it: $(echo "$direct" | cut -c1-100)" ;; esac

echo "== Through the proxy on the worker's loopback"
given worker
proxy
expect "DuckDB over plain HTTP on the loopback, signed for the proxy's address" "rows 7" "$(duck "127.0.0.1:$PLAIN" false)"
expect "DuckDB over TLS on the loopback (the machine's certificate, the CA verified)" "rows 7" "$(duck "localhost:$TLS" true)"
expect "the proxy answers no one without the lake behind (it is TCP only)" 403 \
  "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://127.0.0.1:$PLAIN/qd-verify")"

echo "== A certificate RustFS refuses, then the renewed one"
given stranger
proxy
refused=$(duck "127.0.0.1:$PLAIN" false)
case "$refused" in rows*) ko "another CA's certificate got through: $refused" ;; *) ok "with another CA's certificate the proxy is refused: $(echo "$refused" | cut -c1-100)" ;; esac
before=$(proxy_started)
given worker2
sleep 5
# Traefik reads the certificate files when it reads the configuration that
# names them, and watches that file only.
stale=$(duck "127.0.0.1:$PLAIN" false)
case "$stale" in rows*) ko "the renewed machine.pem was taken by itself: $stale" ;; *) ok "machine.pem renamed in place is not taken by itself: $(echo "$stale" | cut -c1-90)" ;; esac
touch "$D/dyn/lake.yml"
sleep 5
expect "once its configuration file is touched, the renewed certificate is used" "rows 7" "$(duck "127.0.0.1:$PLAIN" false)"
expect "the proxy was not restarted" "$before" "$(proxy_started)"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
