#!/bin/bash
# Bucket replication into the lake replica behind its gate (decisions 0027,
# 0028 and 0029, point 7; 0066 step 11, the user's ruling 3 of 2026-11-13:
# the replication reaches the replica's gate through a proxy beside the
# source that presents its machine certificate, as the worker's does).
#
# Two RustFS 1.0.5 over TLS with certificates of one throwaway CA:
#   - the replica, `replica-1`, behind the repository's gate
#     (infra/rustfs/gate/entrypoint.sh: a client certificate of the CA
#     required, forwardAuth to a stand-in of the API's check that lets every
#     machine pass, as the API lets an approved one);
#   - the source, `lake-1`, with the repository's worker proxy
#     (infra/worker/lake/entrypoint.sh) in its network namespace, on its
#     loopback, presenting lake-1's certificate to the replica's gate.
# Each gets the platform's admin user (qd-platform-admin, the policy join.sh
# sets) made with its root keys, as join.sh does; everything after is
# signed with those admin keys alone, never root. The application is played
# by rustfs_replication_gate.py, presenting an `app` machine's certificate
# at the replica's gate. It checks, and prints what RustFS answers:
#   (a) the gate refuses a client without a certificate; the admin keys
#       read `info` through it;
#   (b) the replica's bucket with object lock (COMPLIANCE, a default
#       retention) and the source's versioned table bucket, by the admin;
#   (c) the remote target set straight at the gate (refused: RustFS
#       presents no certificate), through the worker's proxy as it is
#       (refused: RustFS offers HTTP/2, which the gate behind a TCP proxy
#       never hears) and through the same proxy offering HTTP/1.1 alone
#       (an ARN);
#   (d) the replication rule without delete replication; an object written
#       before and after it reaches the replica, locked; the statuses and
#       metrics RustFS gives;
#   (e) a second configuration run;
#   (f) a delete on the source does not reach the replica;
#   (g) the replica's gate down: what the source says, then the gate back;
#   (h) the lake admin adapter's health (decision 0078, step 11; run by
#       cargo from crates/lake-rustfs-admin/tests/rustfs_live.rs) against the
#       replica's gate, published on this host's loopback alone
#       (QD_VERIFY_GATE_PORT, 59020): its drives, a wrong secret, and no
#       certificate. QD_VERIFY_SKIP_RUST=1 leaves it out.
#
# Throwaway containers only (qd-verify-replgate-…, on a network of their
# own, the replica's gate alone published on the loopback), random root keys and secrets never printed, one
# synthetic object per check. Run from the repository root:
#
#   bash tools/verify/rustfs_replication_gate.sh
#
# Needs docker, openssl, uv (boto3, requests) and cargo.
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
TRAEFIK=${TRAEFIK_IMAGE:-traefik:v3.6}
WHOAMI=${WHOAMI_IMAGE:-traefik/whoami:v1.11}
P=qd-verify-replgate
# The replica's gate on this host's loopback alone, for the adapter's run.
GATE_PORT=${QD_VERIFY_GATE_PORT:-59020}
D=$(mktemp -d)
chmod 755 "$D"
cleanup() {
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f -v >/dev/null 2>&1
  docker network rm "$P" >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT

# --- A throwaway CA and the machines' certificates ------------------------------
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/CN=qd verify replication" -addext "basicConstraints=critical,CA:TRUE" \
  -keyout "$D/ca.key" -out "$D/ca.crt" 2>/dev/null
cert() { # NAME ROLE [SAN]: client and server use in one, the name its DNS name
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/O=qd/OU=$2/CN=$1" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:%s%s\n' \
    "$1" "${3:+,DNS:$3}" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/ca.crt" -CAkey "$D/ca.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  # The agent's files, as the gate and the proxy read them.
  mkdir -p "$D/agent-$1"
  cat "$D/$1.crt" "$D/$1.key" >"$D/agent-$1/machine.pem"
  cp "$D/ca.crt" "$D/agent-$1/ca.crt"
  # RustFS's TLS directory, as rustfs-tls.sh writes it with QD_RUSTFS_TLS_TRUST_CA.
  mkdir -p "$D/tls-$1"
  cp "$D/$1.crt" "$D/tls-$1/rustfs_cert.pem"
  cp "$D/$1.key" "$D/tls-$1/rustfs_key.pem"
  cp "$D/ca.crt" "$D/tls-$1/ca.crt"
  cp "$D/ca.crt" "$D/tls-$1/client_ca.crt"
}
cert replica-1 lake-replica localhost
cert lake-1 lake
cert app-1 app
cert check app
# The entrypoints as the repository has them, readable by their users.
cp infra/rustfs/gate/entrypoint.sh "$D/gate.sh"
cp infra/worker/lake/entrypoint.sh "$D/proxy.sh"
# The same proxy is run twice: as the worker's (its default) and with its
# listener offering HTTP/1.1 alone (QD_LAKE_HTTP1_ONLY=1, ALPN): the gate
# speaks to the proxy over a connection of its own, so the client and the
# gate must agree on HTTP/1.1, the one protocol Traefik's TCP proxy leaves
# them.
chmod -R a+rX "$D"
keys() { # FILE: random root keys, as an env file only docker reads
  printf 'RUSTFS_ACCESS_KEY=qd%s\nRUSTFS_SECRET_KEY=%s\n' "$(openssl rand -hex 8)" "$(openssl rand -hex 20)" >"$1"
  chmod 644 "$1" # read by docker as this user; a throwaway directory
}
keys "$D/root-replica.env"
keys "$D/root-source.env"

docker network create "$P" >/dev/null || exit 1
run() { docker run -d --network "$P" "$@" >/dev/null || exit 1; }
# The API's check, stood in by whoami over TLS: 200 to every machine.
run --name "$P-check" --network-alias check -v "$D:/c:ro" "$WHOAMI" \
  --port 443 --cert /c/check.crt --key /c/check.key
# The replica: RustFS on the gate's network alone, the gate as replica-1.
run --name "$P-replica-fs" --env-file "$D/root-replica.env" -e RUSTFS_VOLUMES=/data \
  -e RUSTFS_TLS_PATH=/tls -v "$D/tls-replica-1:/tls:ro" "$IMAGE" server --address :9000
run --name "$P-replica-gate" --network-alias replica-1 --user 10001:10001 --init \
  -p "127.0.0.1:$GATE_PORT:9000" \
  --entrypoint /etc/qd/gate/entrypoint.sh --tmpfs /run/qd-gate:uid=10001,gid=10001,mode=0700 \
  -e QD_MACHINE_NAME=replica-1 -e QD_GATE_UPSTREAM="https://$P-replica-fs:9000" \
  -e QD_LAKE_CHECK_URL=https://check/api/internal/machines/lake/check \
  -v "$D/gate.sh:/etc/qd/gate/entrypoint.sh:ro" \
  -v "$D/agent-replica-1:/run/qd-node:ro" "$TRAEFIK"
# The source: RustFS named lake-1 on its own loopback too, so it reaches its
# proxy there by the name of the proxy's certificate.
run --name "$P-source" --network-alias lake-1 --add-host lake-1:127.0.0.1 \
  --env-file "$D/root-source.env" -e RUSTFS_VOLUMES=/data -e RUSTFS_TLS_PATH=/tls \
  -v "$D/tls-lake-1:/tls:ro" "$IMAGE" server --address :9000
proxy() { # NAME PORT HTTP1_ONLY: a proxy on the source's loopback
  docker run -d --name "$P-$1" --network "container:$P-source" --user 65532:65532 --init \
    --entrypoint /etc/qd/lake/entrypoint.sh --tmpfs /run/qd-lake:uid=65532,gid=65532,mode=0700 \
    -e QD_LAKE_GATE=replica-1:9000 -e QD_LAKE_LISTEN="127.0.0.1:$2" -e QD_LAKE_HTTP1_ONLY="$3" \
    -v "$D/proxy.sh:/etc/qd/lake/entrypoint.sh:ro" \
    -v "$D/agent-lake-1:/run/qd-node:ro" "$TRAEFIK" >/dev/null || exit 1
}
proxy proxy 9100 1
proxy proxy-h2 9101 0
sleep 8

# --- The platform's admin user on each, with the root keys, as join.sh ----------
POLICY='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:*"],"Resource":["arn:aws:s3:::qd-tenant-*","arn:aws:s3:::qd-tenant-*/*"]},{"Effect":"Allow","Action":["admin:*"],"Resource":["arn:aws:s3:::*"]}]}'
admin_user() { # CONTAINER NAME FILE: makes qd-platform-admin, its keys in FILE
  local secret
  secret=$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')
  printf 'ADMIN_ACCESS_KEY=qd-platform-admin\nADMIN_SECRET_KEY=%s\n' "$secret" >"$3"
  # shellcheck disable=SC2016
  ask() { # METHOD PATH BODY
    printf 'url = "https://%s:9000%s"\nrequest = "%s"\ndata = "%s"\n' "$2" "$4" "$3" "$5" |
      docker exec -i "$1" curl -sS -o /dev/null -w '%{http_code}' --resolve "$2:9000:127.0.0.1" \
        --cacert /tls/ca.crt --variable %RUSTFS_ACCESS_KEY --variable %RUSTFS_SECRET_KEY \
        --expand-user '{{RUSTFS_ACCESS_KEY}}:{{RUSTFS_SECRET_KEY}}' --aws-sigv4 aws:amz:us-east-1:s3 -K -
  }
  local body a b c
  body=$(printf '%s' "$POLICY" | sed 's/"/\\"/g')
  a=$(ask "$1" "$2" PUT "/rustfs/admin/v3/add-canned-policy?name=qd-platform-admin" "$body")
  b=$(ask "$1" "$2" PUT "/rustfs/admin/v3/add-user?accessKey=qd-platform-admin" \
    "{\\\"secretKey\\\":\\\"$secret\\\",\\\"status\\\":\\\"enabled\\\"}")
  c=$(ask "$1" "$2" PUT "/rustfs/admin/v3/set-user-or-group-policy?policyName=qd-platform-admin&userOrGroup=qd-platform-admin&isGroup=false" "")
  echo "$a $b $c"
}
echo "== the platform's admin user, made with the root keys (as join.sh)"
echo "replica: $(admin_user "$P-replica-fs" replica-1 "$D/admin-replica.env")"
echo "source:  $(admin_user "$P-source" lake-1 "$D/admin-source.env")"

ip() { docker inspect -f "{{(index .NetworkSettings.Networks \"$P\").IPAddress}}" "$1"; }
for c in check replica-fs replica-gate source proxy proxy-h2; do
  if [ "$(docker inspect -f '{{.State.Running}}' "$P-$c")" != true ]; then
    echo "[FAIL] $P-$c is not running:"
    docker logs "$P-$c" 2>&1 | tail -5
    exit 1
  fi
done
# (h) The application's own adapter (infrastructure's RustfsLakeAdmin)
# against the replica's gate, before the driver stops it: the pasted keys
# give its drives, a wrong secret is refused, no certificate is refused.
echo "== (h) the application's adapter against the replica's gate"
cat "$D/app-1.crt" "$D/app-1.key" >"$D/app-1.pem"
if [ "${QD_VERIFY_SKIP_RUST:-0}" = 1 ]; then
  echo "[SKIP] QD_VERIFY_SKIP_RUST=1"
elif LAKE_CA_FILE=$D/ca.crt LAKE_CLIENT_CERT_FILE=$D/app-1.pem QD_VERIFY_GATE_PORT=$GATE_PORT \
  QD_VERIFY_NODE=localhost QD_VERIFY_ADMIN_KEYS=$D/admin-replica.env \
  cargo test -j 2 -q -p lake-rustfs-admin --test rustfs_live a_lake_behind_its_gate -- --ignored --nocapture >"$D/rust.log" 2>&1; then
  echo "[OK  ] RustfsLakeAdmin: drives answered, a wrong secret refused, no certificate refused"
  grep '^without a certificate' "$D/rust.log"
else
  echo "[FAIL] RustfsLakeAdmin:"
  tail -20 "$D/rust.log"
  RUST_FAILED=1
fi

D=$D P=$P REPLICA_IP=$(ip "$P-replica-gate") SOURCE_IP=$(ip "$P-source") \
  uv run -q --no-project --with boto3==1.44.2 --with requests==2.34.2 \
  python3 -I tools/verify/rustfs_replication_gate.py || exit 1
[ "${RUST_FAILED:-0}" = 0 ] || exit 1
