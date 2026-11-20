#!/bin/bash
# The main lake as the replication's source (decision 0066 step 11, the
# user's rulings 1, 2 and 7 of 2026-11-13): its platform admin user made at
# stack-up by infra/rustfs/admin-user.sh, and its replication proxy, the
# overlay infra/rustfs/replication.yml (the worker's proxy entrypoint with
# QD_LAKE_HTTP1_ONLY=1, in RustFS's network namespace).
#
# The source is the repository's own compose files, as a lake node runs
# them: docker-compose.yml, certificate.yml, gate.yml and replication.yml,
# a throwaway project with a throwaway CA. The replica is RustFS 1.0.5
# behind the repository's gate (its API check stood in by whoami), reached
# from the source's `replication` network by its name.
#
#   (a) the overlay merges with the others; without QD_LAKE_REPLICA_GATE it
#       is refused; RustFS sits on `lake` and `replication`;
#   (b) admin-user.sh makes qd-platform-admin once and shows its keys once
#       (64 hex digits, never printed here); run again it shows none and
#       the first secret still works; the secret is in no container's log
#       and no file of the installation;
#   (c) the proxy's listener offers HTTP/1.1 alone; the worker's default
#       (the same entrypoint without the option) still offers HTTP/2; a
#       value other than 0 or 1 stops it;
#   (d) with the admin keys alone, the remote target through the proxy
#       (`lake-1:9100`, secure) answers an ARN, and an object written after
#       a rule reaches the replica, locked;
#   (e) RustFS with ca.crt in its TLS directory (QD_RUSTFS_TLS_TRUST_CA)
#       still trusts the system's roots for its OIDC discovery; without
#       them it does not reach it (the control);
#   (f) RustFS restarted (Docker gives it a new network namespace and
#       leaves the proxy in the old one, its loopback alone): the proxy
#       back in the new one by itself, and an object written afterwards
#       reaching the replica; where one written at once ends up is noted;
#       then the proxy stopped for four minutes, an object written then
#       (FAILED), and whether RustFS sends it again once the proxy is back
#       (noted at the end, after (g));
#   (g) the worker's lake proxy, infra/worker/lake.yml as it is, with
#       docker-compose.yml's worker stood in by a shell holding the network
#       namespace (worker-standin.yml; the worker's image is not built) and
#       the replica's gate as its lake: restarted three ways, the worker
#       must reach the lake through its proxy again by itself, within 240 s.
#
# F_ROUNDS=N repeats (f)'s docker restart and kill N times; SKIP_G=1 skips
# (g); RUSTFS_LOG_LEVEL (warn) is the source's, whose lines on replication
# from each object written once the proxy was back are shown after (f).
#
# Throwaway containers only (qd-verify-srcproxy…), the source's gate on
# this host's loopback alone, random root keys and secrets never printed,
# synthetic objects. Run from the repository root:
#
#   bash tools/verify/rustfs_source_proxy.sh
#
# Needs docker (compose v2), openssl and uv (boto3, requests).
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
TRAEFIK=${TRAEFIK_IMAGE:-traefik:v3.6}
WHOAMI=${WHOAMI_IMAGE:-traefik/whoami:v1.11}
PY=${PYTHON_IMAGE:-python:3.12-slim}
CURL=${CURL_IMAGE:-curlimages/curl:8.11.1}
P=qd-verify-srcproxy
PORT=${QD_VERIFY_PORT:-59031}
D=$(mktemp -d)
chmod 755 "$D"
FAILED=0
mkdir -p "$D/infra/worker"
cp -r infra/rustfs "$D/infra/rustfs"
cp -r infra/worker/lake "$D/infra/worker/lake"
mkdir -m 700 "$D/machine"
FILES="-f infra/rustfs/docker-compose.yml -f infra/rustfs/certificate.yml -f infra/rustfs/gate.yml -f infra/rustfs/replication.yml -f oidc.yml"
# shellcheck disable=SC2086
compose() { (cd "$D" && docker compose --project-directory . --env-file .env -p "$P" $FILES "$@"); }
asroot() { docker run --rm --user 0 --entrypoint sh -v "$D:/d" "$IMAGE" -c "$1"; }
W=$P-w
WFILES="-f infra/worker/docker-compose.yml -f infra/worker/lake.yml -f worker-standin.yml"
# shellcheck disable=SC2086
wcompose() { (cd "$D" && docker compose --project-directory . --env-file w.env -p "$W" $WFILES "$@"); }
cleanup() {
  [ -f "$D/w.env" ] && wcompose down -v --remove-orphans >/dev/null 2>&1
  docker volume rm "$P-wmachine" >/dev/null 2>&1
  compose down -v --remove-orphans >/dev/null 2>&1
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f -v >/dev/null 2>&1
  docker network rm "$P" "${P}_replication" >/dev/null 2>&1
  asroot 'rm -rf /d/machine/*' >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
ok() { printf '[OK  ] %s\n' "$*"; }
ko() {
  printf '[FAIL] %s\n' "$*"
  FAILED=1
}
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

# --- A throwaway CA, the machines' certificates, a "public" CA -------------
ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/CN=qd verify $1" -addext "basicConstraints=critical,CA:TRUE" \
    -keyout "$D/$1.key" -out "$D/$1.crt" 2>/dev/null
}
cert() { # NAME CA
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/O=qd/CN=$1" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:%s\n' "$1" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/$2.crt" -CAkey "$D/$2.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  openssl pkey -in "$D/$1.key" -out "$D/$1.p8"
  cat "$D/$1.crt" "$D/$1.p8" >"$D/$1.pem"
  mkdir -p "$D/agent-$1" "$D/tls-$1"
  cp "$D/$1.pem" "$D/agent-$1/machine.pem"
  cp "$D/$2.crt" "$D/agent-$1/ca.crt"
  cp "$D/$1.crt" "$D/tls-$1/rustfs_cert.pem"
  cp "$D/$1.p8" "$D/tls-$1/rustfs_key.pem"
  cp "$D/$2.crt" "$D/tls-$1/client_ca.crt"
  cp "$D/$2.crt" "$D/tls-$1/ca.crt"
}
ca ca
ca public
cert lake-1 ca
cert replica-1 ca
cert app-1 ca
cert check ca
cert oidc public
# The source node's agent files, as uid 10001's.
asroot "cp /d/ca.crt /d/machine/ca.crt && cp /d/lake-1.pem /d/machine/machine.pem &&
  chown -R 10001:10001 /d/machine && chmod 600 /d/machine/machine.pem"
# A stand-in of the installation's public address for RustFS's OIDC
# discovery, its certificate of another CA, trusted as a system root.
mkdir -p "$D/www/api/.well-known"
printf '{"issuer":"https://oidc/api","authorization_endpoint":"https://oidc/a","token_endpoint":"https://oidc/t","jwks_uri":"https://oidc/api/jwks","response_types_supported":["code"],"subject_types_supported":["public"],"id_token_signing_alg_values_supported":["RS256"]}' \
  >"$D/www/api/.well-known/openid-configuration"
printf '{"keys":[]}' >"$D/www/api/jwks"
cat >"$D/srv.py" <<'EOF'
import http.server, ssl, sys
class H(http.server.SimpleHTTPRequestHandler):
    def log_message(self, f, *a):
        sys.stderr.write("HIT %s\n" % self.path)
s = http.server.ThreadingHTTPServer(("", 443), lambda *a: H(*a, directory="/w"))
c = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
c.load_cert_chain("/c/oidc.crt", "/c/oidc.key")
s.socket = c.wrap_socket(s.socket, server_side=True)
s.serve_forever()
EOF
cat >"$D/oidc.yml" <<'EOF'
services:
  rustfs:
    environment:
      SSL_CERT_FILE: /etc/ssl/public.crt
    volumes:
      - ./public.crt:/etc/ssl/public.crt:ro
EOF
cat >"$D/.env" <<EOF
APP_STACK=$P
RUSTFS_ACCESS_KEY=qd$(openssl rand -hex 8)
RUSTFS_SECRET_KEY=$(openssl rand -hex 20)
QD_MACHINE_DIR=$D/machine
QD_MACHINE_NAME=lake-1
QD_LAKE_CHECK_URL=https://check/api/internal/machines/lake/check
QD_LAKE_REPLICA_GATE=replica-1:9000
RUSTFS_BIND=127.0.0.1
RUSTFS_PORT=$PORT
APP_DOMAIN=oidc
RUSTFS_OIDC_ENABLE=on
RUSTFS_OIDC_CLIENT_SECRET=synthetic
RUSTFS_LOG_LEVEL=${RUSTFS_LOG_LEVEL:-warn}
EOF
chmod -R a+rX "$D/infra" "$D"/agent-* "$D"/tls-* "$D/www" "$D/srv.py" "$D/oidc.yml" "$D/public.crt" "$D"/*.crt "$D"/*.key

echo "== (a) the overlay"
expect "the four files merge" 0 "$(compose config -q >/dev/null 2>&1; echo $?)"
nogate=$(cd "$D" && sed '/^QD_LAKE_REPLICA_GATE=/d' .env >.env.nogate &&
  docker compose --project-directory . --env-file .env.nogate -p "$P" $FILES config -q 2>&1)
case "$nogate" in
*"set QD_LAKE_REPLICA_GATE"*) ok "without QD_LAKE_REPLICA_GATE it is refused" ;;
*) ko "without QD_LAKE_REPLICA_GATE: $nogate" ;;
esac
expect "RustFS's networks" "lake replication" "$(compose config --format json | python3 -c 'import json,sys; print(" ".join(sorted(json.load(sys.stdin)["services"]["rustfs"]["networks"])))')"

# --- The replica, and the stand-ins, on a network of their own --------------
docker network create "$P" >/dev/null || exit 1
run() { docker run -d --network "$P" "$@" >/dev/null || exit 1; }
printf 'RUSTFS_ACCESS_KEY=qd%s\nRUSTFS_SECRET_KEY=%s\n' "$(openssl rand -hex 8)" "$(openssl rand -hex 20)" >"$D/root-replica.env"
chmod 644 "$D/root-replica.env"
run --name "$P-check" --network-alias check -v "$D:/c:ro" "$WHOAMI" --port 443 --cert /c/check.crt --key /c/check.key
run --name "$P-replica-fs" --env-file "$D/root-replica.env" -e RUSTFS_VOLUMES=/data \
  -e RUSTFS_TLS_PATH=/tls -v "$D/tls-replica-1:/tls:ro" "$IMAGE" server --address :9000
run --name "$P-replica-gate" --user 10001:10001 --init \
  --entrypoint /etc/qd/gate/entrypoint.sh --tmpfs /run/qd-gate:uid=10001,gid=10001,mode=0700 \
  -e QD_MACHINE_NAME=replica-1 -e QD_GATE_UPSTREAM="https://$P-replica-fs:9000" \
  -e QD_LAKE_CHECK_URL=https://check/api/internal/machines/lake/check \
  -v "$D/infra/rustfs/gate/entrypoint.sh:/etc/qd/gate/entrypoint.sh:ro" \
  -v "$D/agent-replica-1:/run/qd-node:ro" "$TRAEFIK"
run --name "$P-oidc" -v "$D:/c:ro" -v "$D/www:/w:ro" "$PY" python3 -u /c/srv.py

echo "== the source's stack"
up=$(compose up -d --wait --wait-timeout 180 2>&1) ||
  { echo "$up" | tail -5; compose logs 2>&1 | tail -30; exit 1; }
# The replica's gate and the public address, reachable by name from the
# source's way out, as another server and the installation would be.
docker network connect --alias replica-1 "${P}_replication" "$P-replica-gate" || exit 1
docker network connect --alias oidc "${P}_replication" "$P-oidc" || exit 1
expect "the proxy runs as the agent's user" "10001:10001" "$(docker inspect -f '{{.Config.User}}' "$P-replication-proxy-1")"
expect "RustFS's TLS directory holds ca.crt" yes "$(docker exec "$P-rustfs-1" sh -c 'test -s /run/rustfs-tls/ca.crt && echo yes')"

echo "== (b) the platform's admin user, at stack-up"
asr() { RUSTFS_CONTAINER="$P-rustfs-1" QD_MACHINE_NAME= QD_LAKE_WAIT=120 sh "$D/infra/rustfs/admin-user.sh"; }
asr >"$D/first.out" 2>&1
expect "the first run" 0 "$?"
expect "the block shown once" 1 "$(grep -c 'SHOWN ONLY THIS ONCE' "$D/first.out")"
SECRET=$(sed -n 's/^  secret key: //p' "$D/first.out")
expect "its secret is 64 hex digits" yes "$(printf '%s' "$SECRET" | grep -Eq '^[0-9a-f]{64}$' && echo yes)"
printf 'ADMIN_ACCESS_KEY=qd-platform-admin\nADMIN_SECRET_KEY=%s\n' "$SECRET" >"$D/admin-source.env"
asr >"$D/second.out" 2>&1
expect "the second run" 0 "$?"
expect "it says the user is there" 1 "$(grep -c 'is there already' "$D/second.out")"
expect "and shows no secret" 0 "$(grep -c 'secret key:' "$D/second.out")"
leaked=0
for c in rustfs-1 gate-1 replication-proxy-1; do
  docker logs "$P-$c" 2>&1 | grep -qF "$SECRET" && leaked=1
done
expect "the secret in no container's log" 0 "$leaked"
expect "the secret in no file of the installation" 0 \
  "$(grep -rlF "$SECRET" "$D/infra" "$D/.env" 2>/dev/null | wc -l | tr -d ' ')"
# The replica's, made with its root keys as join.sh does.
POLICY=$(sed -n "s/^ADMIN_POLICY='\(.*\)'$/\1/p" infra/join/join.sh)
rsecret=$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')
printf 'ADMIN_ACCESS_KEY=qd-platform-admin\nADMIN_SECRET_KEY=%s\n' "$rsecret" >"$D/admin-replica.env"
rask() { # METHOD PATH BODY
  printf 'url = "https://replica-1:9000%s"\nrequest = "%s"\ndata-binary = "%s"\n' "$2" "$1" "$(printf '%s' "$3" | sed 's/"/\\"/g')" |
    docker exec -i "$P-replica-fs" curl -sS -o /dev/null -w '%{http_code}' --resolve replica-1:9000:127.0.0.1 \
      --cacert /tls/ca.crt --variable %RUSTFS_ACCESS_KEY --variable %RUSTFS_SECRET_KEY \
      --expand-user '{{RUSTFS_ACCESS_KEY}}:{{RUSTFS_SECRET_KEY}}' --aws-sigv4 aws:amz:us-east-1:s3 -K -
}
expect "the replica's admin user (policy, user, attached)" "200 200 200" "$(rask PUT '/rustfs/admin/v3/add-canned-policy?name=qd-platform-admin' "$POLICY") $(rask PUT '/rustfs/admin/v3/add-user?accessKey=qd-platform-admin' "{\"secretKey\":\"$rsecret\",\"status\":\"enabled\"}") $(rask PUT '/rustfs/admin/v3/set-user-or-group-policy?policyName=qd-platform-admin&userOrGroup=qd-platform-admin&isGroup=false' '')"

echo "== (c) the proxy's protocols"
alpn() { # PORT: what a client offering h2 and http/1.1 is given on the source's loopback
  docker run --rm --network "container:$P-rustfs-1" -v "$D:/c:ro" "$PY" python3 -I -c "
import socket, ssl
c = ssl.create_default_context(cafile='/c/ca.crt'); c.set_alpn_protocols(['h2', 'http/1.1'])
with c.wrap_socket(socket.create_connection(('127.0.0.1', $1)), server_hostname='lake-1') as s:
    print(s.selected_alpn_protocol())" 2>&1 | tail -1
}
expect "the replication proxy offers HTTP/1.1 alone" "http/1.1" "$(alpn 9100)"
wproxy() { # NAME PORT [OPTION]: the same entrypoint as the worker runs it
  docker run -d --name "$P-$1" --network "container:$P-rustfs-1" --user 10001:10001 --init \
    --entrypoint /etc/qd/lake/entrypoint.sh --tmpfs /run/qd-lake:uid=10001,gid=10001,mode=0700 \
    -e QD_LAKE_GATE=replica-1:9000 -e QD_LAKE_LISTEN="127.0.0.1:$2" ${3:+-e "QD_LAKE_HTTP1_ONLY=$3"} \
    -v "$D/infra/worker/lake/entrypoint.sh:/etc/qd/lake/entrypoint.sh:ro" \
    -v "$D/machine:/run/qd-node:ro" "$TRAEFIK" >/dev/null
}
wproxy worker 9101
wproxy wrong 9102 yes
sleep 4
expect "the worker's default still offers HTTP/2" "h2" "$(alpn 9101)"
expect "QD_LAKE_HTTP1_ONLY=yes stops it" "exited" "$(docker inspect -f '{{.State.Status}}' "$P-wrong")"
case "$(docker logs "$P-wrong" 2>&1)" in
*"QD_LAKE_HTTP1_ONLY is 0 or 1"*) ok "saying why" ;;
*) ko "it did not say why: $(docker logs "$P-wrong" 2>&1 | tail -1)" ;;
esac

echo "== (d) the replication through the overlay's proxy, by the admin keys"
ip() { docker inspect -f "{{(index .NetworkSettings.Networks \"$2\").IPAddress}}" "$1"; }
if D=$D SOURCE_IP=$(ip "$P-rustfs-1" "${P}_replication") REPLICA_IP=$(ip "$P-replica-gate" "$P") \
  uv run -q --no-project --with boto3==1.44.2 --with requests==2.34.2 \
  python3 -I tools/verify/rustfs_source_proxy.py; then :; else FAILED=1; fi

echo "== (e) RustFS's other calls out, with ca.crt"
hits() { docker logs "$P-oidc" 2>&1 | grep -c 'HIT /api/.well-known/openid-configuration'; }
before=$(hits)
compose restart rustfs >/dev/null 2>&1
sleep 15
after=$(hits)
expect "the source fetched its OIDC discovery with ca.crt there (system roots kept)" yes \
  "$([ "$after" -gt "$before" ] && echo yes || echo "no ($before, $after)")"
before=$after
docker run -d --name "$P-control" --network "${P}_replication" --env-file "$D/root-replica.env" \
  -e RUSTFS_VOLUMES=/data -e RUSTFS_TLS_PATH=/tls -v "$D/tls-lake-1:/tls:ro" \
  -e RUSTFS_IDENTITY_OPENID_ENABLE=on -e RUSTFS_IDENTITY_OPENID_CONFIG_URL=https://oidc/api/.well-known/openid-configuration \
  -e RUSTFS_IDENTITY_OPENID_CLIENT_ID=rustfs -e RUSTFS_IDENTITY_OPENID_CLIENT_SECRET=synthetic \
  -e RUSTFS_IDENTITY_OPENID_REDIRECT_URI=https://oidc/cb -e RUSTFS_OUTBOUND_ALLOW_ORIGINS=https://oidc \
  "$IMAGE" server --address :9000 >/dev/null
sleep 15
expect "the control, without the public CA, did not reach it" "$before" "$(hits)"

echo "== (f) RustFS restarted: the proxy in its namespace"
PROXY=$P-replication-proxy-1
pifaces() { docker exec "$PROXY" grep -c : /proc/net/dev 2>/dev/null || echo none; }
plistens() { docker exec "$P-rustfs-1" curl -sk -o /dev/null -w '%{http_code}' --max-time 5 https://127.0.0.1:9100/ 2>/dev/null; }
fpy() { # left|after WITHIN LABEL
  D=$D SOURCE_IP=$(ip "$P-rustfs-1" "${P}_replication") REPLICA_IP=$(ip "$P-replica-gate" "$P") \
    uv run -q --no-project --with boto3==1.44.2 --with requests==2.34.2 \
    python3 -I tools/verify/rustfs_source_proxy.py restarted "$1" "$2" "$3" || FAILED=1
}
started() { docker inspect -f '{{.State.StartedAt}} {{.RestartCount}}' "$1"; }
# restartway LABEL COMMAND...: RustFS restarted by COMMAND, the proxy
# answering before; then the proxy must answer in RustFS's namespace again
# by itself, and an object written then reach the replica.
restartway() {
  local label=$1 p0 r0 back='' said
  shift
  docker restart "$PROXY" >/dev/null
  for _ in $(seq 60); do case $(plistens) in 4??) break ;; esac; sleep 1; done
  case $(plistens) in 4??) ok "$label: the proxy answers on RustFS's loopback" ;; *) ko "$label: the proxy answered '$(plistens)'" ;; esac
  p0=$(started "$PROXY") r0=$(started "$P-rustfs-1")
  T0=$(date +%s)
  "$@" >/dev/null 2>&1
  for _ in $(seq 120); do [ "$(started "$P-rustfs-1")" != "$r0" ] && break; sleep 1; done
  fpy left 120 "$label"
  echo "   ($label: RustFS $([ "$(started "$P-rustfs-1")" != "$r0" ] && echo restarted || echo "not restarted");" \
    "the proxy $([ "${p0% *}" != "$(started "$PROXY" | cut -d' ' -f1)" ] && echo restarted || echo "not restarted") by then," \
    "its interfaces $(pifaces))"
  for _ in $(seq 180); do
    if [ "$(pifaces)" -gt 1 ] 2>/dev/null; then
      case $(plistens) in 4??) back=$(($(date +%s) - T0)) && break ;; esac
    fi
    sleep 1
  done
  expect "$label: the proxy answers in RustFS's namespace again, by itself" yes "${back:+yes}"
  said=$(docker logs "$PROXY" 2>&1 | grep -c '^qd: RustFS restarted')
  echo "   ($label: back ${back:-never} s after RustFS's restart began; the proxy restarted by its policy" \
    "$(($(started "$PROXY" | cut -d' ' -f2) - ${p0#* })) times, the loop's line $said times in its log)"
  echo "$label $(date +%s.%N)" >>"$D/marks"
  fpy after 300 "$label"
}
# The overlay's proxy runs under the pool's loop, the same text.
expect "the overlay's proxy entrypoint is the pool node's" same "$(uv run -q --no-project --with pyyaml==6.0.2 python3 -I -c '
import sys, yaml
e = [yaml.safe_load(open(f))["services"]["replication-proxy"]["entrypoint"] for f in sys.argv[1:]]
print("same" if e[0] == e[1] and e[0][0] == "/bin/sh" else "different")' infra/rustfs/replication.yml infra/rustfs/pool/docker-compose.yml)"
restartway "compose restart" compose restart rustfs
for round in $(seq "${F_ROUNDS:-1}"); do
  [ "$round" = 1 ] && n='' || n=" $round"
  restartway "docker restart$n" docker restart "$P-rustfs-1"
  # RustFS's process killed, as a crash: its restart policy starts it again.
  restartway "RustFS killed$n" docker exec -u 0 "$P-rustfs-1" sh -c 'kill -KILL -1'
done
# The proxy away for minutes (stopped: its policy leaves it so), an object
# written meanwhile; whether RustFS sends it again later is noted at the end.
docker stop "$PROXY" >/dev/null
T0=$(date +%s)
fpy left 60 "proxy stopped"
sleep 240
fpy status 1 "proxy stopped"
docker start "$PROXY" >/dev/null
for _ in $(seq 60); do case $(plistens) in 4??) break ;; esac; sleep 1; done
echo "proxy-stopped $(date +%s.%N)" >>"$D/marks"
fpy after 300 "proxy stopped"
echo "   (proxy stopped: back $(($(date +%s) - T0)) s after it was stopped, counting the next object's wait)"

# RustFS's lines on replication and its target, from each object written
# once the proxy was back to 90 s after (RUSTFS_LOG_LEVEL, warn by default).
docker logs -t "$P-rustfs-1" >"$D/rustfs.log" 2>&1
# RustFS writes its own lines to files under /logs, not to its output.
docker cp "$P-rustfs-1:/logs" "$D/rustfs-logs" >/dev/null 2>&1
# QD_VERIFY_RUSTFS_LOG=DIR keeps them (synthetic objects; read before sharing).
[ -n "${QD_VERIFY_RUSTFS_LOG:-}" ] && cp -r "$D/rustfs-logs" "$QD_VERIFY_RUSTFS_LOG"
python3 -I - "$D" <<'PY'
import datetime, re, sys
d = sys.argv[1]
def ts(x):
    head, _, frac = x.rstrip("Z").partition(".")
    return datetime.datetime.fromisoformat(head).replace(tzinfo=datetime.timezone.utc).timestamp() + float("0." + (frac or "0"))
lines = []
for l in open(d + "/rustfs.log", errors="replace"):
    t, _, m = l.partition(" ")
    try:
        lines.append((ts(t), m.strip()))
    except ValueError:
        pass
pat = re.compile(r"(?i)replicat|target|offline|online|heartbeat|timed? ?out|mrf")
for mark in open(d + "/marks"):
    label, t0 = mark.rsplit(" ", 1)
    t0 = float(t0)
    sel = [(t - t0, m) for t, m in lines if t0 - 5 <= t <= t0 + 90 and pat.search(m)]
    print(f"[LOG ] {label}: {len(sel)} lines on replication from 5 s before its object to 90 s after")
    seen = set()
    for dt, m in sel:
        key = re.sub(r"[0-9a-f-]{8,}|\d+", "#", m)[-160:]
        if key not in seen:
            seen.add(key)
            print(f"[LOG ]   +{dt:.0f} s {m[:300]}")
PY

if [ -n "${SKIP_G:-}" ]; then
  fpy summary 1 all
  echo
  [ "$FAILED" = 0 ] && echo "ALL OK (g skipped)" || echo "SOME CHECKS FAILED (g skipped)"
  exit "$FAILED"
fi
echo "== (g) the worker's lake proxy when the worker restarts"
# The worker stood in by a shell that holds its network namespace and ends
# when its child is killed; its lake is the replica's gate (any gate that
# takes a machine's certificate of the CA), reached on the replication
# network as a worker reaches its lake's.
cert worker-1 ca
cp infra/worker/docker-compose.yml infra/worker/lake.yml "$D/infra/worker/"
printf 'QD_MACHINE_NAME=worker-1\nQD_LAKE_GATE=replica-1:9000\nQD_LAKE_CHECK_EVERY=2\n' >"$D/w.env"
cat >"$D/worker-standin.yml" <<YML
services:
  worker:
    image: $CURL
    build: !reset null
    user: "65532:65532"
    entrypoint: ["/bin/sh", "-c", "sleep 2147483647 & wait"]
    networks: [replication]
networks:
  replication:
    name: ${P}_replication
    external: true
volumes:
  machine:
    name: $P-wmachine
    external: true
YML
chmod -R a+rX "$D/infra/worker" "$D/w.env" "$D/worker-standin.yml"
docker run --rm --user 0 --entrypoint sh -v "$D:/d:ro" -v "$P-wmachine:/m" "$IMAGE" -c 'cp /d/ca.crt /m/ca.crt &&
  cp /d/worker-1.pem /m/machine.pem && chown -R 65532:65532 /m && chmod 644 /m/ca.crt && chmod 600 /m/machine.pem'
up=$(wcompose up -d --no-build worker lake 2>&1) || ko "the worker's stack did not start: $(echo "$up" | tail -2)"
WK=$W-worker-1 LK=$W-lake-1
# wanswers: the worker's request to its lake, as QD_LAKE_URL has it (an
# unsigned one: RustFS's 403 behind the gate means the way is whole), in
# HTTP/1.1 as the worker's DuckDB speaks (the gate behind a TCP proxy hears
# no HTTP/2, see (c)).
wanswers() { docker exec "$WK" curl -s --http1.1 -o /dev/null -w '%{http_code}' --max-time 5 --cacert /var/lib/qd-node/ca.crt https://worker-1:9000/ 2>/dev/null; }
wifaces() { docker exec "$LK" grep -c : /proc/net/dev 2>/dev/null || echo none; }
wrestart() { # LABEL COMMAND...
  local label=$1 l0 w0 said0 back=''
  shift
  # Each way from a proxy in the worker's namespace, whatever the last left.
  docker restart "$LK" >/dev/null
  for _ in $(seq 60); do case $(wanswers) in 4??) break ;; esac; sleep 1; done
  case $(wanswers) in 4??) ok "$label: the worker reaches the lake through its proxy" ;; *) ko "$label: before, the worker got '$(wanswers)'" ;; esac
  l0=$(started "$LK") w0=$(started "$WK") said0=$(docker logs "$LK" 2>&1 | grep -c '^qd: ')
  T0=$(date +%s)
  "$@" >/dev/null 2>&1
  for _ in $(seq 60); do [ "$(started "$WK")" != "$w0" ] && break; sleep 1; done
  for _ in $(seq 240); do
    case $(wanswers) in 4??) back=$(($(date +%s) - T0)) && break ;; esac
    sleep 1
  done
  expect "$label: the worker reaches the lake through its proxy again, by itself" yes "${back:+yes}"
  echo "   ($label: the worker $([ "$(started "$WK")" != "$w0" ] && echo restarted || echo "not restarted"); back ${back:-never} s after;" \
    "the proxy $([ "${l0% *}" != "$(started "$LK" | cut -d' ' -f1)" ] && echo restarted || echo "not restarted")," \
    "by its policy $(($(started "$LK" | cut -d' ' -f2) - ${l0#* })) times, the loop's line $(($(docker logs "$LK" 2>&1 | grep -c '^qd: ') - said0)) times;" \
    "its interfaces $(wifaces); the worker's last answer '$(wanswers)')"
}
expect "the worker reaches the lake through its proxy, at first" yes \
  "$(for _ in $(seq 60); do case $(wanswers) in 4??) echo yes && break ;; esac; sleep 1; done)"
# The worker's proxy runs under the pool proxy's loop, the same text but the line it logs.
expect "the worker's proxy entrypoint is the pool node's but its line" same "$(uv run -q --no-project --with pyyaml==6.0.2 python3 -I -c '
import re, sys, yaml
s = [yaml.safe_load(open(sys.argv[1]))["services"]["lake"], yaml.safe_load(open(sys.argv[2]))["services"]["replication-proxy"]]
e = [[re.sub(r"echo \"qd: [^\"]*\"", "echo LINE", x) for x in v.get("entrypoint", [])] for v in s]
print("same" if e[0] == e[1] and e[0][0] == "/bin/sh" else "different")' infra/worker/lake.yml infra/rustfs/pool/docker-compose.yml)"
wrestart "worker: compose restart" wcompose restart worker
wrestart "worker: docker restart" docker restart "$WK"
wrestart "worker killed" docker exec -u 0 "$WK" sh -c 'kill -KILL -1'
fpy summary 1 all

echo
[ "$FAILED" = 0 ] && echo "ALL OK" || echo "SOME CHECKS FAILED"
exit "$FAILED"
