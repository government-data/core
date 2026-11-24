#!/bin/bash
# The application's own code configures the replication of the main lake
# into the replica, end to end (decision 0066 step 11, the user's rulings of
# 2026-11-13), with RustFS 1.0.5 and Traefik 3.6:
#
#   - the main lake is the repository's compose files as a lake node runs
#     them (docker-compose.yml, certificate.yml, gate.yml and
#     replication.yml), `lake-1`, its platform admin user made by
#     infra/rustfs/admin-user.sh, its gate published on this host's
#     loopback;
#   - the replica is RustFS behind the repository's gate, `replica-1`, its
#     admin user made as join.sh makes it, its gate on the loopback too and
#     reached from the main lake's `replication` network by its name;
#   - both gates' API check is stood in by whoami (every machine passes).
#
# lake-rustfs-admin's tests/rustfs_live.rs, the lake admin adapter's
# replication (decision 0078, step 11), presenting an
# `app` machine's certificate at both gates with each lake's admin keys,
# configures a tenant's warehouse (made on the main lake beforehand, as
# nothing in the application makes it yet) and is told a missing one is
# not there; rustfs_replication_app.py checks what it configured and that
# an object written to the main lake arrives locked on the replica. Then
# the adapter runs again, and the configuration is the same.
#
# Throwaway containers only (qd-verify-replapp…), a throwaway CA, random
# keys never printed, synthetic objects. Run from the repository root:
#
#   bash tools/verify/rustfs_replication_app.sh
#
# Needs docker (compose v2), openssl, uv (boto3, requests) and cargo.
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
TRAEFIK=${TRAEFIK_IMAGE:-traefik:v3.6}
WHOAMI=${WHOAMI_IMAGE:-traefik/whoami:v1.11}
P=qd-verify-replapp
SRC_PORT=${QD_VERIFY_SOURCE_PORT:-59051}
REP_PORT=${QD_VERIFY_REPLICA_PORT:-59052}
WAREHOUSE=qd-tenant-0000000000000000000000000000000a
D=$(mktemp -d)
chmod 755 "$D"
mkdir -p "$D/infra/worker"
cp -r infra/rustfs "$D/infra/rustfs"
cp -r infra/worker/lake "$D/infra/worker/lake"
mkdir -m 700 "$D/machine"
FILES="-f infra/rustfs/docker-compose.yml -f infra/rustfs/certificate.yml -f infra/rustfs/gate.yml -f infra/rustfs/replication.yml"
# shellcheck disable=SC2086
compose() { (cd "$D" && docker compose --project-directory . --env-file .env -p "$P" $FILES "$@"); }
asroot() { docker run --rm --user 0 --entrypoint sh -v "$D:/d" "$IMAGE" -c "$1"; }
cleanup() {
  compose down -v --remove-orphans >/dev/null 2>&1
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f -v >/dev/null 2>&1
  docker network rm "$P" >/dev/null 2>&1
  asroot 'rm -rf /d/machine/*' >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/CN=qd verify replication app" -addext "basicConstraints=critical,CA:TRUE" \
  -keyout "$D/ca.key" -out "$D/ca.crt" 2>/dev/null
cert() { # NAME OU [SAN]
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/O=qd/OU=$2/CN=$1" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:%s%s\n' \
    "$1" "${3:+,DNS:$3}" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/ca.crt" -CAkey "$D/ca.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  openssl pkey -in "$D/$1.key" -out "$D/$1.p8"
  cat "$D/$1.crt" "$D/$1.p8" >"$D/$1.pem"
  mkdir -p "$D/agent-$1" "$D/tls-$1"
  cp "$D/$1.pem" "$D/agent-$1/machine.pem"
  cp "$D/ca.crt" "$D/agent-$1/ca.crt"
  cp "$D/$1.crt" "$D/tls-$1/rustfs_cert.pem"
  cp "$D/$1.p8" "$D/tls-$1/rustfs_key.pem"
  cp "$D/ca.crt" "$D/tls-$1/client_ca.crt"
  cp "$D/ca.crt" "$D/tls-$1/ca.crt"
}
cert lake-1 lake localhost
cert replica-1 lake-replica localhost
cert app-1 app
cert check app
asroot "cp /d/ca.crt /d/machine/ca.crt && cp /d/lake-1.pem /d/machine/machine.pem &&
  chown -R 10001:10001 /d/machine && chmod 600 /d/machine/machine.pem"
cat >"$D/.env" <<ENV
APP_STACK=$P
RUSTFS_ACCESS_KEY=qd$(openssl rand -hex 8)
RUSTFS_SECRET_KEY=$(openssl rand -hex 20)
QD_MACHINE_DIR=$D/machine
QD_MACHINE_NAME=lake-1
QD_LAKE_CHECK_URL=https://check/api/internal/machines/lake/check
QD_LAKE_REPLICA_GATE=replica-1:9000
RUSTFS_BIND=127.0.0.1
RUSTFS_PORT=$SRC_PORT
ENV
chmod -R a+rX "$D/infra" "$D"/agent-* "$D"/tls-* "$D"/*.crt "$D"/*.key

docker network create "$P" >/dev/null || exit 1
run() { docker run -d --network "$P" "$@" >/dev/null || exit 1; }
printf 'RUSTFS_ACCESS_KEY=qd%s\nRUSTFS_SECRET_KEY=%s\n' "$(openssl rand -hex 8)" "$(openssl rand -hex 20)" >"$D/root-replica.env"
chmod 644 "$D/root-replica.env"
run --name "$P-check" --network-alias check -v "$D:/c:ro" "$WHOAMI" --port 443 --cert /c/check.crt --key /c/check.key
run --name "$P-replica-fs" --env-file "$D/root-replica.env" -e RUSTFS_VOLUMES=/data \
  -e RUSTFS_TLS_PATH=/tls -v "$D/tls-replica-1:/tls:ro" "$IMAGE" server --address :9000
run --name "$P-replica-gate" --user 10001:10001 --init -p "127.0.0.1:$REP_PORT:9000" \
  --entrypoint /etc/qd/gate/entrypoint.sh --tmpfs /run/qd-gate:uid=10001,gid=10001,mode=0700 \
  -e QD_MACHINE_NAME=replica-1 -e QD_GATE_UPSTREAM="https://$P-replica-fs:9000" \
  -e QD_LAKE_CHECK_URL=https://check/api/internal/machines/lake/check \
  -v "$D/infra/rustfs/gate/entrypoint.sh:/etc/qd/gate/entrypoint.sh:ro" \
  -v "$D/agent-replica-1:/run/qd-node:ro" "$TRAEFIK"
echo "== the main lake's stack"
up=$(compose up -d --wait --wait-timeout 180 2>&1) || { echo "$up" | tail -5; compose logs 2>&1 | tail -30; exit 1; }
docker network connect --alias replica-1 "${P}_replication" "$P-replica-gate" || exit 1
docker network connect --alias check "${P}_default" "$P-check" || exit 1

echo "== the platform's admin users"
RUSTFS_CONTAINER="$P-rustfs-1" QD_LAKE_WAIT=120 sh "$D/infra/rustfs/admin-user.sh" >"$D/admin.out" 2>&1 ||
  { echo "[FAIL] admin-user.sh"; tail -3 "$D/admin.out" | grep -v 'secret key'; exit 1; }
printf 'ADMIN_ACCESS_KEY=qd-platform-admin\nADMIN_SECRET_KEY=%s\n' "$(sed -n 's/^  secret key: //p' "$D/admin.out")" >"$D/admin-source.env"
rm -f "$D/admin.out"
POLICY=$(sed -n "s/^ADMIN_POLICY='\(.*\)'$/\1/p" infra/join/join.sh)
rsecret=$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')
printf 'ADMIN_ACCESS_KEY=qd-platform-admin\nADMIN_SECRET_KEY=%s\n' "$rsecret" >"$D/admin-replica.env"
rask() { # METHOD PATH BODY, signed with the replica's root keys inside its container
  printf 'url = "https://replica-1:9000%s"\nrequest = "%s"\ndata-binary = "%s"\n' "$2" "$1" "$(printf '%s' "$3" | sed 's/"/\\"/g')" |
    docker exec -i "$P-replica-fs" curl -sS -o /dev/null -w '%{http_code}' --resolve replica-1:9000:127.0.0.1 \
      --cacert /tls/ca.crt --variable %RUSTFS_ACCESS_KEY --variable %RUSTFS_SECRET_KEY \
      --expand-user '{{RUSTFS_ACCESS_KEY}}:{{RUSTFS_SECRET_KEY}}' --aws-sigv4 aws:amz:us-east-1:s3 -K -
}
made="$(rask PUT '/rustfs/admin/v3/add-canned-policy?name=qd-platform-admin' "$POLICY") $(rask PUT '/rustfs/admin/v3/add-user?accessKey=qd-platform-admin' "{\"secretKey\":\"$rsecret\",\"status\":\"enabled\"}") $(rask PUT '/rustfs/admin/v3/set-user-or-group-policy?policyName=qd-platform-admin&userOrGroup=qd-platform-admin&isGroup=false' '')"
[ "$made" = "200 200 200" ] || { echo "[FAIL] the replica's admin user: $made"; exit 1; }
unset rsecret
echo "[OK  ] both admin users made, the main lake's by admin-user.sh"
# The tenant's warehouse on the main lake, by its admin user, as nothing in
# the application makes it yet: the bucket and its table bucket.
cat >"$D/warehouse.py" <<'PY'
import hashlib, os, requests
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.credentials import Credentials
D = os.environ["D"]
d = dict(l.strip().split("=", 1) for l in open(f"{D}/admin-source.env") if "=" in l)
for path in (f"/{os.environ['WAREHOUSE']}", f"/iceberg/v1/buckets/{os.environ['WAREHOUSE']}"):
    url = os.environ["SOURCE"] + path
    a = AWSRequest(method="PUT", url=url, data=b"", headers={"x-amz-content-sha256": hashlib.sha256(b"").hexdigest()})
    S3SigV4Auth(Credentials(d["ADMIN_ACCESS_KEY"], d["ADMIN_SECRET_KEY"]), "s3", "us-east-1").add_auth(a)
    r = requests.put(url, headers=dict(a.headers), verify=f"{D}/ca.crt", cert=(f"{D}/app-1.crt", f"{D}/app-1.key"), timeout=30)
    print(f"[{'OK  ' if r.status_code == 200 else 'FAIL'}] main lake: PUT {path.split('/')[1] if path.count('/') == 1 else 'iceberg table bucket'}: {r.status_code}")
    if r.status_code != 200:
        raise SystemExit(1)
PY
py() { D=$D SOURCE="https://localhost:$SRC_PORT" REPLICA="https://localhost:$REP_PORT" WAREHOUSE=$WAREHOUSE \
  uv run -q --no-project --with boto3==1.44.2 --with requests==2.34.2 python3 -I "$@"; }
py "$D/warehouse.py" || exit 1

cat "$D/app-1.crt" "$D/app-1.p8" >"$D/app-1.pem"
adapter() { # RUN
  echo "== run $1 of the application's adapter"
  if LAKE_CA_FILE=$D/ca.crt LAKE_CLIENT_CERT_FILE=$D/app-1.pem QD_VERIFY_SOURCE="https://localhost:$SRC_PORT" \
    QD_VERIFY_REPLICA_PORT=$REP_PORT QD_VERIFY_NODE=localhost QD_VERIFY_PROXY=lake-1:9100 \
    QD_VERIFY_WAREHOUSE=$WAREHOUSE QD_VERIFY_MAIN_KEYS=$D/admin-source.env QD_VERIFY_REPLICA_KEYS=$D/admin-replica.env \
    cargo test -j 2 -q -p lake-rustfs-admin --test rustfs_live the_adapter_configures -- --ignored --nocapture >"$D/rust.log" 2>&1; then
    grep -E '^(configured|a warehouse)' "$D/rust.log" | sed 's/^/[OK  ] /'
  else
    echo "[FAIL] the adapter:"
    grep -v -i secret "$D/rust.log" | tail -20
    exit 1
  fi
}
adapter 1
MODE=first py tools/verify/rustfs_replication_app.py || FAILED=1
adapter 2
MODE=second py tools/verify/rustfs_replication_app.py || FAILED=1
echo
[ "${FAILED:-0}" = 0 ] && echo "ALL OK" || echo "SOME CHECKS FAILED"
exit "${FAILED:-0}"
