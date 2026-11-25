#!/bin/bash
# Verification of the lake's settings in the core's package (decision 0091,
# point 4; 0078 step 16, the package's part), on this machine: the package
# tools/package.sh makes, laid out and started as package-dolt.sh does (the
# installer's order, the agent's command line), with a .env that names a
# lake and a fake admin adapter, then:
#
#   1. the STS issuer's keys reach the API as files (Docker secrets, 0400,
#      the API's user's), never as variables;
#   2. the API, with LAKE_ADMIN_URL set, asks the adapter's GET /about at
#      start and logs that it serves lake-admin/1, presenting the
#      application machine's certificate and trusting the installation's
#      CA by default; it does not log "no lake admin adapter";
#   3. with no lake in the .env (the package's default), the API starts
#      and says the lake is not reached, as before;
#   4. the OIDC signing keys (quality-sdk decision 0012, step 3) reach the
#      API as files: its provider is on, and its JWK Set has an Ed25519 and
#      a P-256 key, asked of the API on its PostgreSQL network.
#
# The fake adapter is a few lines of Python on the API's PostgreSQL network
# (data-quality-backend), by the name lake-admin.<machine>, with a
# certificate the installation's CA signs for role `lake`. It answers
# /lake-admin/v1/about and nothing else. No lake runs: the jobs' credentials
# are not asked.
#
# The images are CI's, never built here:
#
#   gh run download <run> -n image-api -n image-web -D <dir>
#   docker load -i <dir>/qd-api.tar && docker load -i <dir>/qd-web.tar
#   QD_API_IMAGE=qd-api:<sha> QD_WEB_IMAGE=qd-web:<sha> bash tools/verify/package-lake.sh
#
# Publishes only 127.0.0.1:59443 (the internal entry point) and
# 127.0.0.1:55432 (PostgreSQL); not 80 nor 443. Uses the fixed networks
# data-quality-edge, -backend, -api, -db and -catalog, so the dev stack must
# be down. Its containers and volumes are qd-verify-lake…; everything is
# removed on exit, also on failure. Run alone (no build running), from the
# repository root, on a commit (tools/package.sh packs HEAD). Needs docker,
# openssl.
set -u
export LC_ALL=C
API=${QD_API_IMAGE:?set QD_API_IMAGE to the API image of a CI artifact}
WEB=${QD_WEB_IMAGE:?set QD_WEB_IMAGE to the web image of a CI artifact}
DOLT=${QD_DOLT_IMAGE:-dolthub/dolt-sql-server@sha256:24bdcff7a90bef6d15e8825be502f1c057842d079411390448dc13398c8bf619}
ALPINE=${ALPINE_IMAGE:-alpine:3}
PYTHON=${PYTHON_IMAGE:-python:3.12-alpine}
P=qd-verify-lake
NAME=qd-app
ADAPTER=lake-admin.$NAME
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
D=$(mktemp -d)
chmod 755 "$D"
MOD=$D/opt/quality-data/quality-data
NODE=$D/node
UPD=$D/updater
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
dc() {
  docker compose --project-name "$P" --project-directory "$MOD/current" \
    --file "$MOD/current/compose.yml" --file "$D/ports.yml" --env-file "$MOD/.env" "$@"
}
asroot() { docker run --rm --name "$P-root" -v "$D:$D" "$ALPINE" sh -c "$1"; }
cleanup() {
  docker rm -f "$P-adapter" "$P-root" "$P-ca" >/dev/null 2>&1
  dc down --volumes --remove-orphans --timeout 5 >/dev/null 2>&1
  docker run --rm -v "$D:/d" "$ALPINE" sh -c 'rm -rf /d/* /d/.[!.]*' >/dev/null 2>&1
  rm -rf -- "${D:?}"
}
trap cleanup EXIT
umask 022

for n in data-quality-edge data-quality-backend data-quality-api data-quality-db data-quality-catalog; do
  if docker network inspect "$n" >/dev/null 2>&1; then ko "the network $n exists: stop the dev stack"; exit 1; fi
done

# --- The package and the files beside the version, as the installer writes them
VERSION=$(tr -d '[:space:]' <"$ROOT/version.txt")
OUT_DIR=$D "$ROOT/tools/package.sh" "$VERSION" >/dev/null || { ko "tools/package.sh"; exit 1; }
mkdir -p "$MOD/$VERSION" "$MOD/secrets" "$MOD/certs" "$MOD/internal" "$MOD/ca" "$NODE" "$UPD" "$D/adapter"
tar -xzf "$D/quality-data-$VERSION.tar.gz" -C "$MOD/$VERSION"
ln -s "$VERSION" "$MOD/current"
SUPER=$(openssl rand -hex 16)
APPPW=$(openssl rand -hex 16)
printf '%s' "$SUPER" >"$MOD/secrets/postgres_superuser_password"
printf 'postgres://quality_data_app:%s@%s:5432/quality_data?sslmode=verify-full&sslrootcert=/run/qd-ca/ca.crt' \
  "$APPPW" "$NAME" >"$MOD/secrets/database_url"
# As the update agent makes module.yaml's secrets: random bytes, base64url,
# no newline (16 for the issuer's access key, 32 for the rest).
gen() { openssl rand -base64 "$2" | tr '+/' '-_' | tr -d '=\n' >"$MOD/secrets/$1"; }
gen dolt-root-password 32
gen dolt-app-password 32
gen rustfs-sts-issuer-access-key 16
gen rustfs-sts-issuer-secret-key 32
# The OIDC keys as the agent's (ring's PKCS#8), by the API's own command.
keygen() { docker run --rm --network none --entrypoint /usr/local/bin/server "$API" oidc-keygen "$@" | tr -d '\n'; }
keygen >"$MOD/secrets/oidc-signing-key"
keygen --es256 >"$MOD/secrets/oidc-signing-key-es256"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/CN=localhost" -addext "subjectAltName=DNS:localhost" \
  -keyout "$MOD/certs/local.key" -out "$MOD/certs/local.crt" 2>/dev/null
chmod 644 "$MOD/secrets/postgres_superuser_password" "$MOD/secrets/database_url" "$MOD"/certs/*
GEN="dolt-root-password dolt-app-password rustfs-sts-issuer-access-key rustfs-sts-issuer-secret-key oidc-signing-key oidc-signing-key-es256"
asroot "cd '$MOD/secrets' && chown 65532:65532 '$MOD/ca' '$NODE' '$UPD' $GEN && chmod 400 $GEN && chmod 700 ."
# env FILE [LAKE]: the .env, naming the lake and its adapter when LAKE is 1.
env_file() {
  sed -e "s|^QD_MODULE_DIR=.*|QD_MODULE_DIR=$MOD|" -e "s|^QD_MACHINE_DIR=.*|QD_MACHINE_DIR=$NODE|" \
    -e "s|^QD_CA_DIR=.*|QD_CA_DIR=$MOD/ca|" -e "s|^QD_UPDATER_DIR=.*|QD_UPDATER_DIR=$UPD|" \
    -e "s|^IMAGE_API=.*|IMAGE_API=$API|" -e "s|^IMAGE_WEB=.*|IMAGE_WEB=$WEB|" -e "s|^IMAGE_DOLT=.*|IMAGE_DOLT=$DOLT|" \
    -e "s|^APP_DOMAIN=.*|APP_DOMAIN=localhost|" -e "s|^INTERNAL_DOMAIN=.*|INTERNAL_DOMAIN=$NAME|" \
    -e "s|^POSTGRES_SERVER_NAME=.*|POSTGRES_SERVER_NAME=$NAME|" -e "s|^OBS_VM=.*|OBS_VM=$P|" \
    -e "s|^POSTGRES_PORT=.*|POSTGRES_PORT=55432|" \
    -e "s|^POSTGRES_SUPERUSER_PASSWORD=.*|POSTGRES_SUPERUSER_PASSWORD=$SUPER|" \
    -e "s|^APP_DB_PASSWORD=.*|APP_DB_PASSWORD=$APPPW|" \
    "$ROOT/infra/package/env.example" >"$MOD/.env"
  printf 'APP_STACK=%s\nQD_METRICS_VOLUME=%s-metrics\nPOSTGRES_BIND=127.0.0.1\n' "$P" "$P" >>"$MOD/.env"
  [ "${1:-}" != 1 ] || sed -i -e "s|^LAKE_ENDPOINT=.*|LAKE_ENDPOINT=https://lake-1:9000|" \
    -e "s|^LAKE_ADMIN_URL=.*|LAKE_ADMIN_URL=https://$ADAPTER:9020|" "$MOD/.env"
}
env_file 1
cat >"$D/ports.yml" <<'EOF'
services:
  traefik:
    ports: !override
      - "127.0.0.1:59443:9443"
EOF
oneshot() {
  local name=$1
  shift
  docker compose --project-name "$P" --env-file "$MOD/.env" \
    --file "$MOD/current/infra/postgres/docker-compose.yml" --file "$MOD/current/infra/postgres/tls.yml" \
    --file "$MOD/current/infra/postgres/$name.yml" --profile "$name" run --rm --no-deps "$name" "$@"
}

# --- The first start, in the installer's order, then the fake adapter ----------
docker run --rm --name "$P-ca" --user 65532:65532 --network none -v "$MOD/ca:/run/qd-ca" \
  --entrypoint /usr/local/bin/server "$API" ca-init --name "qd verify" \
  --cert /run/qd-ca/ca.crt --key /run/qd-ca/ca.key >/dev/null 2>&1 || ko "ca-init"
dc up --detach --no-build postgres >/dev/null 2>&1 || ko "up -d postgres"
for _ in $(seq 30); do oneshot app-machine --name "$NAME" --dir /run/qd-node >/dev/null 2>&1 && break; sleep 2; done
# The adapter's certificate, role `lake`, signed by the installation's CA
# in a throwaway container: the CA's key never leaves its folder.
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/OU=lake/CN=$ADAPTER" \
  -keyout "$D/adapter/key.pem" -out "$D/adapter/req.csr" 2>/dev/null
printf 'subjectAltName=DNS:%s\nextendedKeyUsage=serverAuth\n' "$ADAPTER" >"$D/adapter/ext"
docker run --rm -v "$MOD/ca:/ca:ro" -v "$D/adapter:/a" "$ALPINE" sh -c \
  "apk add -q openssl >/dev/null && openssl x509 -req -in /a/req.csr -CA /ca/ca.crt -CAkey /ca/ca.key \
   -CAcreateserial -CAserial /a/ca.srl -days 1 -extfile /a/ext -out /a/cert.pem && chmod 644 /a/*" >/dev/null 2>&1 ||
  ko "the adapter's certificate"
cat >"$D/adapter/serve.py" <<'EOF'
import http.server, json, ssl
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/lake-admin/v1/about":
            self.send_error(404); return
        body = json.dumps({"protocols": ["lake-admin/1"], "implementation": "qd-verify-fake", "version": "0.0.1"}).encode()
        self.send_response(200); self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body))); self.end_headers(); self.wfile.write(body)
        print("about asked", flush=True)
s = http.server.HTTPServer(("0.0.0.0", 9020), H)
c = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); c.load_cert_chain("/a/cert.pem", "/a/key.pem")
s.socket = c.wrap_socket(s.socket, server_side=True)
s.serve_forever()
EOF
chmod 644 "$D/adapter/key.pem"
docker run -d --name "$P-adapter" --network data-quality-backend --network-alias "$ADAPTER" \
  -v "$D/adapter:/a:ro" "$PYTHON" python -u /a/serve.py >/dev/null || ko "the fake adapter"
if out=$(dc up --detach --no-build --remove-orphans --wait --wait-timeout 300 2>&1); then
  ok "the agent's up --wait"
else
  ko "up: $(printf '%s' "$out" | tail -n 3)"
fi

# --- 1. The issuer's keys, as files ---------------------------------------------------
API_ID=$(dc ps -q api)
mounts=$(docker inspect -f '{{range .Mounts}}{{.Destination}} {{end}}' "$API_ID")
for f in rustfs_sts_issuer_access_key rustfs_sts_issuer_secret_key; do
  case " $mounts " in
    *" /run/secrets/$f "*) ok "1. $f is a secret of the API" ;;
    *) ko "1. $f is not among the API's mounts: $mounts" ;;
  esac
done
vars=$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$API_ID")
for v in RUSTFS_STS_ISSUER_ACCESS_KEY= RUSTFS_STS_ISSUER_SECRET_KEY=; do
  line=$(printf '%s\n' "$vars" | grep "^$v" || true)
  [ "$line" = "$v" ] && ok "1. $v is empty: the key comes from its file" || ko "1. $v is set"
done

# --- 2. The API asks the adapter --------------------------------------------------------
seen=''
for _ in $(seq 30); do
  seen=$(dc logs --no-log-prefix api 2>&1 | grep -E "main lake's adapter|no lake admin adapter|not reachable from here" || true)
  [ -z "$seen" ] || break
  sleep 2
done
case $seen in
  *"serves lake-admin/1"*) ok "2. the API logs the adapter's GET /about: lake-admin/1" ;;
  *) ko "2. the API on the adapter: ${seen:-nothing}"; dc logs --no-log-prefix --tail 15 api 2>&1 ;;
esac
docker logs "$P-adapter" 2>&1 | grep -q "about asked" && ok "2. the fake adapter was asked" || ko "2. the fake adapter was not asked"
dc logs --no-log-prefix api 2>&1 | grep -q "no lake admin adapter" && ko "2. it logs no lake admin adapter" ||
  ok "2. no \"no lake admin adapter\""

# --- 3. No lake in the .env -------------------------------------------------------------
env_file
if dc up --detach --no-build --remove-orphans --wait --wait-timeout 300 >/dev/null 2>&1; then
  ok "3. the API starts with no lake"
else
  ko "3. up with no lake"
fi
for _ in $(seq 15); do
  ! dc logs --no-log-prefix --since 2m api 2>&1 | grep -q "main lake's adapter did not answer\|no LAKE_ADMIN_URL" || break
  [ "$(dc ps -q api)" != "$API_ID" ] && break
  sleep 2
done
[ "$(dc ps -q api)" != "$API_ID" ] && ok "3. the API was recreated with the new .env" || ko "3. the API was not recreated"

# --- 4. The OIDC signing keys, as files ---------------------------------------------------
dc logs --no-log-prefix api 2>&1 | grep -q "oidc provider enabled" && ok "4. the OIDC provider is on" ||
  ko "4. the OIDC provider is not on"
jwks=''
for _ in $(seq 15); do
  jwks=$(docker run --rm --network data-quality-backend "$ALPINE" wget -qO- http://api:8080/oauth/jwks 2>/dev/null || true)
  [ -z "$jwks" ] || break
  sleep 2
done
case $jwks in
  *'"OKP"'*'"EC"'* | *'"EC"'*'"OKP"'*) ok "4. the JWK Set has an Ed25519 and a P-256 key" ;;
  *) ko "4. the JWK Set: ${jwks:-nothing}" ;;
esac

[ "$FAILED" = 0 ] && echo "ALL OK" || { echo "SOME CHECKS FAILED"; exit 1; }
