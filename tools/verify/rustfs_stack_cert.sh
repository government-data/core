#!/bin/bash
# Verification of the repository's RustFS with the machines' certificates
# (decision 0066, point 10, step 10): infra/rustfs/docker-compose.yml with
# infra/rustfs/certificate.yml, as an installation runs it, with a throwaway
# CA and the lake node's agent directory as the agent leaves it (ca.crt,
# machine.pem 0600, owned by RustFS's uid 10001, as the agent runs as it).
# It shows: RustFS serves the agent's certificate over HTTPS only; a renewed
# machine.pem is served without a restart; with RUSTFS_MTLS=1 a client
# without a certificate of the CA is refused at the handshake and one with
# it is answered; and whether the worker's DuckDB (2.0, the repository's
# line, through Python) can reach it either way: it has no setting to
# present a client certificate.
#
# Its own Compose project (qd-verify-rustfs-cert), volume (APP_STACK) and
# port 127.0.0.1:59000; nothing of the dev stack is touched. RustFS's root
# credentials are random, only start the container and are never used for
# a request: the S3 requests are anonymous or signed with keys RustFS does
# not know, so their answer (403) only says TLS let them through. The name
# in the certificate is `localhost`, as the clients run on this machine.
# Removes its containers and volume at the end. Run from the repository root:
#
#   bash tools/verify/rustfs_stack_cert.sh
#
# Needs docker with Compose v2, curl, openssl 3 and uv (duckdb).
set -u
export LC_ALL=C
P=qd-verify-rustfs-cert
PORT=59000
IMAGE=rustfs/rustfs:1.0.4
D=$(mktemp -d)
chmod 755 "$D"
FAILED=0
mkdir -p "$D/infra"
cp -r infra/rustfs "$D/infra/rustfs"
chmod -R a+rX "$D/infra"
mkdir -m 700 "$D/machine"
cat >"$D/.env" <<EOF
APP_STACK=$P
RUSTFS_ACCESS_KEY=qd$(openssl rand -hex 8)
RUSTFS_SECRET_KEY=$(openssl rand -hex 16)
QD_MACHINE_DIR=$D/machine
RUSTFS_BIND=127.0.0.1
RUSTFS_PORT=$PORT
RUSTFS_TLS_RELOAD_INTERVAL=5
QD_RUSTFS_TLS_CHECK_EVERY=2
EOF
FILES="-f infra/rustfs/docker-compose.yml -f infra/rustfs/certificate.yml"
# shellcheck disable=SC2086
compose() { (cd "$D" && docker compose --project-directory . --env-file .env -p "$P" $FILES "$@"); }
# As root in a throwaway container: the agent's files belong to uid 10001.
asroot() { docker run --rm --user 0 --entrypoint sh -v "$D:/d" "$IMAGE" -c "$1"; }
cleanup() { compose down -v --remove-orphans >/dev/null 2>&1; asroot 'rm -rf /d/machine/*' >/dev/null 2>&1; rm -rf "$D"; }
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

ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/O=qd/CN=qd verify $1" -addext "basicConstraints=critical,CA:TRUE" \
    -keyout "$D/$1.key" -out "$D/$1.crt" 2>/dev/null
}
# leaf NAME CA SUBJECT: NAME.pem, the certificate then the PKCS#8 key, for localhost.
leaf() {
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$3" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nkeyUsage=digitalSignature\nextendedKeyUsage=clientAuth,serverAuth\nsubjectAltName=DNS:localhost\n' >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/$2.crt" -CAkey "$D/$2.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  openssl pkey -in "$D/$1.key" -out "$D/$1.p8"
  cat "$D/$1.crt" "$D/$1.p8" >"$D/$1.pem"
}
# agent PEM: the agent's files, written aside and renamed, as uid 10001.
agent() {
  asroot "cp /d/ca.crt /d/machine/.ca.crt.new && cp /d/$1.pem /d/machine/.machine.pem.new &&
    chown 10001:10001 /d/machine /d/machine/.ca.crt.new /d/machine/.machine.pem.new &&
    chmod 644 /d/machine/.ca.crt.new && chmod 600 /d/machine/.machine.pem.new &&
    mv /d/machine/.ca.crt.new /d/machine/ca.crt && mv /d/machine/.machine.pem.new /d/machine/machine.pem"
}
serial() { openssl x509 -noout -serial -in "$D/$1.crt"; }
served() { openssl s_client -connect "127.0.0.1:$PORT" -servername localhost </dev/null 2>/dev/null | openssl x509 -noout -serial 2>/dev/null; }
# s3 [CURL OPTION...]: an anonymous request's status, over HTTPS verified against the CA.
s3() { curl -s -o /dev/null -w '%{http_code}' --max-time 5 --cacert "$D/ca.crt" "$@" "https://localhost:$PORT/qd-verify/x"; }
started() { docker inspect -f '{{.State.StartedAt}}' "$P-rustfs-1"; }

ca ca
ca other
leaf lake ca "/O=qd/OU=lake/CN=localhost"
leaf client ca "/O=qd/OU=worker/CN=worker-7"
leaf stranger other "/O=qd/OU=worker/CN=worker-7"
agent lake

# duck: DuckDB reading through the S3 API with keys RustFS does not know.
cat >"$D/duck.py" <<'EOF'
import os, duckdb
c = duckdb.connect()
c.execute("LOAD httpfs")
c.execute(f"SET ca_cert_file = '{os.environ['CA']}'")
c.execute(f"""CREATE SECRET s (TYPE s3, KEY_ID 'qdverifyunknown', SECRET 'qdverifyunknown0000',
  ENDPOINT 'localhost:{os.environ['PORT']}', URL_STYLE 'path', USE_SSL true, REGION 'us-east-1')""")
try:
    c.execute("SELECT * FROM read_csv('s3://qd-verify/x.csv')").fetchall()
    print("read")
except Exception as e:
    print(str(e).splitlines()[0][:160])
EOF
duck() { (cd "$D" && CA="$D/ca.crt" PORT=$PORT uv run -q --no-project --with duckdb==2.0.0 python3 -I duck.py 2>&1 | tail -1); }

echo "== HTTPS with the agent's certificate, reloaded"
up=$(compose up -d --wait --wait-timeout 120 rustfs 2>&1) ||
  { echo "RustFS did not become healthy: $up" | tail -5; compose logs rustfs 2>&1 | tail -20; exit 1; }
for _ in $(seq 30); do [ -n "$(served)" ] && break; sleep 1; done
expect "it serves the agent's machine.pem" "$(serial lake)" "$(served)"
expect "an anonymous request over HTTPS, verified against the CA, is answered" 403 "$(s3)"
expect "plain HTTP gets no S3 answer" 000 "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://127.0.0.1:$PORT/qd-verify/x")"
expect "the key is not readable by others" "600" "$(docker exec "$P-rustfs-1" stat -c %a /run/rustfs-tls/rustfs_key.pem)"
before=$(started)
leaf lake2 ca "/O=qd/OU=lake/CN=localhost"
agent lake2
for _ in $(seq 40); do [ "$(served)" = "$(serial lake2)" ] && break; sleep 1; done
expect "the renewed machine.pem is served" "$(serial lake2)" "$(served)"
expect "without a restart" "$before" "$(started)"
like "DuckDB reaches it (TLS verified; the unknown keys refused by S3)" "403" "$(duck)"

echo "== RUSTFS_MTLS=1: a client certificate of the CA required"
sed -i 's/^RUSTFS_TLS_RELOAD_INTERVAL=5$/RUSTFS_TLS_RELOAD_INTERVAL=5\nRUSTFS_MTLS=1/' "$D/.env"
compose up -d --wait --wait-timeout 120 rustfs >/dev/null 2>&1
for _ in $(seq 30); do [ -n "$(served)" ] || [ "$(s3 --cert "$D/client.pem")" = 403 ] && break; sleep 1; done
expect "a worker's certificate of the CA is answered" 403 "$(s3 --cert "$D/client.pem")"
expect "no certificate: refused at the handshake" 000 "$(s3)"
expect "another CA's certificate: refused" 000 "$(s3 --cert "$D/stranger.pem")"
duck_mtls=$(duck)
case "$duck_mtls" in
  *403*) ko "DuckDB got through without a certificate: $duck_mtls" ;;
  *) ok "DuckDB, with no setting to present a client certificate, cannot reach it: $(echo "$duck_mtls" | cut -c1-110)" ;;
esac

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
