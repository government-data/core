#!/bin/bash
# Verification for decision 0071, step 4, RustFS's own metrics: RustFS 1.0.5
# has no Prometheus endpoint and pushes OTLP, with no client certificate, so
# its push to the node's Alloy carries the node's token (the user's ruling of
# 2026-11-14). It shows, with the repository's files (infra/rustfs/tls's
# entrypoint, which reads the token; the lake roles' Alloy configurations):
#
#   names    which metric and label names RustFS pushes (Alloy keeping all),
#            to choose point 8's names and check point 10's labels
#   replica  the replica's layout: Alloy on the compose network `otlp`, which
#            RustFS alone shares with it, listening on the machine's name
#            (an alias on that network): RustFS's series reach Mimir with the
#            token, within the lists; the receiver refuses a push without the
#            token, with another, or without TLS; it is not reached from the
#            other network; the token in no log and in nothing `docker
#            inspect` shows
#   pool     a pool node's layout: Alloy in RustFS's network namespace, on
#            its loopback, as RustFS resolves its own name there; and whether
#            Alloy, under the compose file's loop, goes on pushing after
#            RustFS restarts, by `docker restart` and by its process killed
#            (a crash, its restart policy starting it again)
#   renewal  the machine's certificate renewed while Alloy runs: the
#            receiver serves the new one, and once the old has expired Alloy
#            pushes to Mimir with the new one, without a restart
#   gap      (not run by default, about 25 minutes) a pool node as the
#            compose file runs it, RustFS restarted six times: what Alloy
#            does while it stops, and which samples Mimir lacks after
#
# Throwaway containers, networks and volumes qd-verify-metrics-rustfs-…, a
# throwaway CA; the token is made as join.sh makes it and never printed. Each
# group waits for a quiet machine (no rustc or cc1plus, no other qd-verify-
# container, load under 4); a load above 8 stops the run.
#
#   bash tools/verify/metrics_rustfs_push.sh [names|replica|pool|renewal|gap]...
#
# Needs docker, openssl 3 and python3; the images rustfs/rustfs:1.0.5,
# grafana/alloy:v1.21.3, grafana/mimir:3.2.4, curlimages/curl:8.11.1 and
# alpine:3.20.
set -u
export LC_ALL=C
P=qd-verify-metrics-rustfs
NET=$P-net OTLP=$P-otlp
RUSTFS=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
ALLOY=${ALLOY_IMAGE:-grafana/alloy:v1.21.3}
MIMIR=${MIMIR_IMAGE:-grafana/mimir:3.2.4}
CURL=curlimages/curl:8.11.1
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
NODE=node-1
D=$(mktemp -d)
chmod 755 "$D"
mkdir -m 700 "$D/secret"
FAILED=0 PASSED=0
ALLOY_PRE=()

cleanup() {
  docker ps -aq --filter "name=^/$P-" | xargs -r docker rm -f -v >/dev/null 2>&1
  docker network rm "$NET" "$OTLP" >/dev/null 2>&1
  docker volume ls -q --filter "name=^$P-" | xargs -r docker volume rm >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

ok() { printf '[OK  ] %s\n' "$*"; PASSED=$((PASSED + 1)); }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=$((FAILED + 1)); }
note() { printf '[NOTE] %s\n' "$*"; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

busy() {
  pgrep -x rustc >/dev/null && { echo "rustc running"; return 0; }
  pgrep -x cc1plus >/dev/null && { echo "cc1plus running"; return 0; }
  local others load
  others=$(docker ps --format '{{.Names}}' | grep '^qd-verify-' | grep -cv "^$P-")
  [ "$others" -gt 0 ] && { echo "$others other qd-verify- containers"; return 0; }
  load=$(cut -d' ' -f1 /proc/loadavg)
  awk -v l="$load" 'BEGIN { exit !(l >= 4) }' && { echo "load $load"; return 0; }
  return 1
}
wait_quiet() { local why; while why=$(busy); do echo "[wait] $why"; sleep 60; done; }
watchdog() {
  while sleep 10; do
    awk -v l="$(cut -d' ' -f1 /proc/loadavg)" 'BEGIN { exit !(l > 8) }' || continue
    echo "[STOP] load above 8: removing $P- containers"
    docker ps -aq --filter "name=^/$P-" | xargs -r docker rm -f >/dev/null 2>&1
    kill -TERM $$
    return
  done
}
end_group() {
  docker ps -aq --filter "name=^/$P-" | xargs -r docker rm -f -v >/dev/null 2>&1
  docker volume ls -q --filter "name=^$P-" | xargs -r docker volume rm >/dev/null 2>&1
}

# --- Certificates: a throwaway CA, Mimir's as qd-app, the machine's ---------------
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 -subj "/CN=qd verify rustfs" \
  -addext "basicConstraints=critical,CA:TRUE" -keyout "$D/secret/ca.key" -out "$D/ca.crt" 2>/dev/null
# cert NAME CN OU [NOT_AFTER]: a certificate of the CA for CN, both usages, as
# the CA signs a machine's (0066): $D/NAME.crt and .key.
cert() {
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/O=qd/OU=$3/CN=$2" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:%s\n' "$2" >"$D/$1.ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/ca.crt" -CAkey "$D/secret/ca.key" -CAcreateserial \
    -extfile "$D/$1.ext" ${4:+-not_after "$4"} -days 1 -out "$D/$1.crt" 2>/dev/null
  chmod 644 "$D/$1.key" # throwaway: given to the containers' users
}
mkdir -p "$D/mimir"
cert app qd-app app
cp "$D/ca.crt" "$D/app.crt" "$D/app.key" "$D/mimir/"
printf '%s\n' 'multitenancy_enabled: false' 'target: all' 'server: { http_listen_port: 9009, grpc_listen_address: 127.0.0.1, log_level: warn }' \
  'common: { storage: { backend: filesystem, filesystem: { dir: /data/store } } }' \
  'blocks_storage: { storage_prefix: blocks, tsdb: { dir: /data/tsdb }, bucket_store: { sync_dir: /data/tsdb-sync } }' \
  'ruler_storage: { storage_prefix: ruler }' 'alertmanager_storage: { storage_prefix: alertmanager }' 'ruler: { rule_path: /data/ruler }' \
  'activity_tracker: { filepath: /data/activity.log }' \
  'compactor: { data_dir: /data/compactor, sharding_ring: { kvstore: { store: memberlist } } }' \
  'distributor: { ring: { instance_addr: 127.0.0.1, kvstore: { store: memberlist } } }' \
  'ingester: { ring: { instance_addr: 127.0.0.1, kvstore: { store: memberlist }, replication_factor: 1 } }' \
  'store_gateway: { sharding_ring: { replication_factor: 1 } }' 'frontend: { address: 127.0.0.1 }' 'usage_stats: { enabled: false }' >"$D/mimir/mimir.yml"
chmod 755 "$D/mimir" && chmod 644 "$D/mimir"/*
# The repository's files, readable by the containers' users.
mkdir -p "$D/files"
cp "$ROOT/infra/rustfs/tls/entrypoint.sh" "$D/files/rustfs-tls.sh"
cp "$ROOT/infra/observability/alloy/lake-replica.alloy" "$ROOT/infra/observability/alloy/lake.alloy" "$D/files/"
chmod 755 "$D/files" "$D/files/rustfs-tls.sh" && chmod 644 "$D/files"/*.alloy

networks() {
  docker network inspect "$NET" >/dev/null 2>&1 || docker network create "$NET" >/dev/null
  docker network inspect "$OTLP" >/dev/null 2>&1 || docker network create --internal "$OTLP" >/dev/null
}
# node NOT_AFTER: the node's files as the agent and join.sh leave them, in
# volumes: machine/ (ca.crt 0644, machine.pem 0600) and the token (0600),
# RustFS's user's (10001); the token from /dev/urandom as join.sh makes it,
# copied to secret/token (0600) for the checks alone.
node() {
  cert machine "$NODE" lake "${1:-}"
  docker run --rm --name "$P-init" -v "$D:/h:ro" -v "$P-machine:/m" -v "$P-token:/t" alpine:3.20 sh -c '
    cp /h/ca.crt /m/ca.crt && cat /h/machine.crt /h/machine.key >/m/machine.pem &&
    od -An -tx1 -N32 /dev/urandom | tr -d " \n" >/t/token &&
    chown -R 10001:10001 /m /t && chmod 700 /m /t && chmod 644 /m/ca.crt && chmod 600 /m/machine.pem /t/token' ||
    ko "could not write the node's files"
  (umask 077 && docker run --rm --name "$P-init" -v "$P-token:/t:ro" alpine:3.20 cat /t/token >"$D/secret/token")
}
# renew: a new machine.pem (a day), written aside and renamed, as the agent does.
renew() {
  cert machine "$NODE" lake
  docker run --rm --name "$P-init" -v "$D:/h:ro" -v "$P-machine:/m" alpine:3.20 sh -c '
    cat /h/machine.crt /h/machine.key >/m/.machine.pem.new && chown 10001:10001 /m/.machine.pem.new &&
    chmod 600 /m/.machine.pem.new && mv /m/.machine.pem.new /m/machine.pem'
}
mimir() {
  docker run -d --name "$P-mimir" --network "$NET" --network-alias qd-app --restart no --tmpfs /data:mode=1777 \
    -v "$D/mimir:/etc/mimir:ro" "$MIMIR" -config.file=/etc/mimir/mimir.yml \
    -server.http-tls-cert-path=/etc/mimir/app.crt -server.http-tls-key-path=/etc/mimir/app.key \
    -server.http-tls-ca-path=/etc/mimir/ca.crt -server.http-tls-client-auth=RequireAndVerifyClientCert >/dev/null ||
    ko "could not start Mimir"
}
# mq API PARAM: Mimir's Prometheus API as the application VM.
mq() {
  docker run --rm --network "$NET" -v "$D/mimir:/c:ro" "$CURL" -sS -G --max-time 15 --cacert /c/ca.crt \
    --cert /c/app.crt --key /c/app.key "https://qd-app:9009/prometheus/api/v1/$1" --data-urlencode "$2" 2>/dev/null
}
# value PROMQL: the first result's value, or 0.
value() { mq query "query=$1" | python3 -c 'import json,sys;r=json.load(sys.stdin)["data"]["result"];print(r[0]["value"][1] if r else 0)' 2>/dev/null || echo 0; }
# rustfs NETWORK [DOCKER OPTION...]: RustFS as the lake roles' compose files
# run it, through the repository's entrypoint, pushing to https://node-1:4318.
rustfs() {
  local net=$1
  shift
  docker run -d --name "$P-rustfs" --network "$net" --restart "${RESTART:-no}" --init "$@" \
    --entrypoint /etc/qd/rustfs/entrypoint.sh \
    -e RUSTFS_ACCESS_KEY="qd$(openssl rand -hex 8)" -e RUSTFS_SECRET_KEY="$(openssl rand -hex 16)" \
    -e RUSTFS_VOLUMES=/data -e RUSTFS_OBS_LOGGER_LEVEL=info \
    -e RUSTFS_TLS_PATH=/run/rustfs-tls -e RUSTFS_TLS_RELOAD_ENABLE=1 -e RUSTFS_SERVER_MTLS_ENABLE=0 \
    -e QD_RUSTFS_TLS_SOURCE=/run/qd-node -e QD_RUSTFS_TLS_TRUST_CA=1 \
    -e RUSTFS_OBS_ENDPOINT="https://$NODE:4318" -e RUSTFS_OBS_METRICS_EXPORT_ENABLED="${EXPORT:-true}" \
    -e RUSTFS_OBS_TRACES_EXPORT_ENABLED=false -e RUSTFS_OBS_LOGS_EXPORT_ENABLED=false \
    -e RUSTFS_OBS_METER_INTERVAL="${METER:-10}" -e RUSTFS_OBS_TLS_CA_FILE=/run/qd-node/ca.crt \
    -e QD_RUSTFS_OBS_TOKEN_FILE=/run/qd-otlp/token \
    --tmpfs /run/rustfs-tls:uid=10001,gid=10001,mode=0700 --tmpfs /data:uid=10001,gid=10001 \
    -v "$D/files/rustfs-tls.sh:/etc/qd/rustfs/entrypoint.sh:ro" -v "$P-machine:/run/qd-node:ro" \
    -v "$P-token:/run/qd-otlp:ro" "$RUSTFS" server --address :9000 >/dev/null || ko "could not start RustFS"
}
# alloy ROLE-FILE NETWORK-OPTION...: Alloy as the lake roles' compose files run it.
alloy() {
  local file=$1
  shift
  local data=(--tmpfs /var/lib/qd-alloy:uid=10001,gid=10001)
  # ALLOY_DATA: a volume for its storage, as the compose file's alloy-data.
  [ -n "${ALLOY_DATA:-}" ] && data=(-v "$ALLOY_DATA:/var/lib/qd-alloy")
  docker create --name "$P-alloy" --restart "${RESTART:-no}" --user 10001:10001 --read-only --tmpfs /tmp \
    "${data[@]}" --tmpfs /run/qd-metrics:mode=0755 "$@" \
    -e METRICS_PUSH_URL=https://qd-app:9009/api/v1/push -e METRICS_CA_FILE=/run/qd-node/ca.crt \
    -e METRICS_CERT_FILE=/run/qd-node/machine.pem -e OBS_VM="$NODE" -e OTLP_TOKEN_FILE=/run/qd-otlp/token \
    -v "$P-machine:/run/qd-node:ro" -v "$P-token:/run/qd-otlp:ro" -v "$D/files/$file:/etc/qd-alloy/$file:ro" \
    "$ALLOY" "${ALLOY_PRE[@]}" run --server.http.listen-addr=127.0.0.1:12345 --storage.path=/var/lib/qd-alloy \
    --disable-reporting /etc/qd-alloy >/dev/null || ko "could not create Alloy"
}
ip_on() { docker inspect -f "{{(index .NetworkSettings.Networks \"$2\").IPAddress}}" "$P-$1"; }
# age SELECTOR: seconds since the selector's latest sample, 999 with none (value's
# 0 for no answer, or for timestamp() over several names, which Mimir refuses
# as one label set, would read as fresh).
age() {
  mq query "query=time() - max(timestamp($1))" |
    python3 -c 'import json,sys;r=json.load(sys.stdin)["data"]["result"];print(int(float(r[0]["value"][1])) if r else 999)' 2>/dev/null || echo 999
}
# wait_series PROMQL N: until PROMQL counts at least N (3 minutes at most).
wait_series() {
  for _ in $(seq 36); do
    [ "$(value "$1" | cut -d. -f1)" -ge "$2" ] 2>/dev/null && return 0
    sleep 5
  done
  return 1
}
# s3 PATH [CURL OPTION...]: a request to RustFS as root, on its network.
s3() {
  local p=$1
  shift
  docker run --rm --network "$OTLP" --user 0 -v "$D:/h:ro" "$CURL" -s -o /dev/null -w '%{http_code}' --max-time 10 \
    --cacert /h/ca.crt --resolve "$NODE:9000:$(ip_on rustfs "$OTLP")" --aws-sigv4 aws:amz:us-east-1:s3 \
    --user "$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$P-rustfs" | sed -n 's/^RUSTFS_ACCESS_KEY=//p'):$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$P-rustfs" | sed -n 's/^RUSTFS_SECRET_KEY=//p')" \
    "$@" "https://$NODE:9000$p"
}
traffic() {
  echo synthetic >"$D/obj" && chmod 644 "$D/obj"
  note "S3 traffic for RustFS's counters: $(s3 /qd-verify -X PUT) $(s3 /qd-verify/o.txt -T /h/obj) $(s3 /qd-verify/o.txt) $(s3 /qd-verify/missing.txt)"
}
# token_in WHAT...: how many of the containers' logs and `docker inspect`s hold the token.
token_in() {
  local c n=0
  for c in "$@"; do
    docker logs "$P-$c" 2>&1 | grep -qFf "$D/secret/token" && n=$((n + 1))
    docker inspect "$P-$c" | grep -qFf "$D/secret/token" && n=$((n + 1))
  done
  echo "$n"
}
# push FROM-NETWORK URL [CURL OPTION...]: an empty OTLP/HTTP push; the status.
push() {
  local net=$1 url=$2
  shift 2
  docker run --rm --network "$net" --user 0 -v "$D:/h:ro" "$CURL" -s -o /dev/null -w '%{http_code}' --max-time 10 \
    --cacert /h/ca.crt -H 'content-type: application/json' -d '{}' "$@" "$url" 2>/dev/null
}
auth_header() { (umask 077 && printf 'authorization: Bearer %s\n' "$(cat "$D/secret/token")" >"$D/secret/auth"); }

# --- names: what RustFS pushes, Alloy keeping all ------------------------------------
group_names() {
  echo "== names: RustFS 1.0.5's OTLP metrics, every name and label kept"
  networks
  node
  # The replica's file with RustFS's keep lists opened.
  sed '/^prometheus.relabel "rustfs"/,/^}/ s/regex *= ".*"/regex = ".+"/' "$D/files/lake-replica.alloy" >"$D/files/all.alloy"
  chmod 644 "$D/files/all.alloy"
  mimir
  rustfs "$OTLP"
  alloy all.alloy --network "$NET"
  docker network connect --alias "$NODE" "$OTLP" "$P-alloy" && docker start "$P-alloy" >/dev/null
  sleep 15
  traffic
  if wait_series 'count({__name__=~"rustfs_.+"})' 1; then sleep 25; else ko "no RustFS series in Mimir"; fi
  mq series 'match[]={__name__=~"rustfs_.+"}' | python3 -c '
import json, sys, collections
s = json.load(sys.stdin)["data"]
names = collections.defaultdict(set)
for x in s:
    names[x["__name__"]].update(k for k in x if k != "__name__")
print("[NOTE] RustFS series:", len(s), "names:", len(names))
for n in sorted(names):
    print("[NAME]", n, " ".join(sorted(names[n] - {"job", "instance", "vm", "role"})))
labels = collections.defaultdict(set)
for x in s:
    for k, v in x.items():
        labels[k].add(v)
scopes = collections.defaultdict(set)
for x in s:
    if "collection_scope" in x:
        scopes[x["collection_scope"]].add(x["__name__"].split("_")[1])
for k in sorted(scopes):
    print("[SCOPE]", k, " ".join(sorted(scopes[k])))
for k in sorted(labels):
    if k not in ("__name__",):
        print("[LABEL]", k, len(labels[k]), " ".join(sorted(labels[k]))[:200])'
  end_group
}

# --- replica: Alloy on the network otlp, by the machine's name -------------------------
group_replica() {
  echo "== replica: Alloy on the network otlp (RustFS's alone), listening on the machine's name"
  networks
  node
  auth_header
  mimir
  rustfs "$OTLP"
  alloy lake-replica.alloy --network "$NET"
  docker network connect --alias "$NODE" "$OTLP" "$P-alloy" && docker start "$P-alloy" >/dev/null
  sleep 15
  traffic
  wait_series 'count({__name__=~"rustfs_.+",vm="node-1",role="lake-replica"})' 1 ||
    note "Alloy's last lines: $(docker logs "$P-alloy" 2>&1 | tail -3 | cut -c1-300)"
  sleep 25
  mq series 'match[]={job="rustfs",vm="node-1"}' | python3 -c '
import json, re, sys
s = json.load(sys.stdin)["data"]
keep = re.compile(sys.argv[1])
ok = {"__name__", "job", "instance", "vm", "role", "api", "method", "status_class", "pool", "drive", "le"}
print(len(s), len({x["__name__"] for x in s}), len([x for x in s if not keep.fullmatch(x["__name__"])]),
      " ".join(sorted({k for x in s for k in x} - ok)) or "-")' \
    "$(sed -n '/^prometheus.relabel "rustfs"/,/^}/ s/^ *regex *= "\(rustfs_.*\)"$/\1/p' "$D/files/lake-replica.alloy" | head -1)" >"$D/rs"
  read -r series names outside labels <"$D/rs"
  expect "RustFS's series in Mimir, pushed with the token: names outside the keep list, labels outside the list" "0 -" "$outside $labels"
  note "RustFS: $series series, $names names after the keep list"
  expect "none carries bucket, disk_id, process_pid or a path" 0 \
    "$(mq series 'match[]={job="rustfs",vm="node-1"}' | grep -c '"bucket"\|"disk_id"\|"process_pid"\|"path"')"
  expect "no sample Mimir refused (a series made twice by the labels dropped would be)" 0 \
    "$(value 'sum(prometheus_remote_storage_samples_failed_total{vm="node-1"})' | cut -d. -f1)"
  expect "requests by S3 API: s3:PutObject counted" 1 "$(value 'count(rustfs_api_requests_total{api="s3:PutObject",vm="node-1"})')"
  local otlp_url="https://$NODE:4318/v1/metrics"
  expect "the receiver, from RustFS's network, with the token" 200 "$(push "$OTLP" "$otlp_url" -H @/h/secret/auth)"
  expect "… without a token" 401 "$(push "$OTLP" "$otlp_url")"
  expect "… with another token" 401 "$(push "$OTLP" "$otlp_url" -H "authorization: Bearer $(openssl rand -hex 32)")"
  expect "… without TLS: the TLS listener's 400, nothing taken" 400 "$(push "$OTLP" "http://$NODE:4318/v1/metrics" -H @/h/secret/auth)"
  expect "… from the other network, at Alloy's address there: refused" 000 \
    "$(push "$NET" "https://$(ip_on alloy "$NET"):4318/v1/metrics" -k -H @/h/secret/auth)"
  note "Alloy listens on: $(docker run --rm --network "container:$P-alloy" alpine:3.20 netstat -ltn 2>/dev/null | awk 'NR > 2 { print $4 }' | tr '\n' ' ')"
  expect "the token in no log and in no docker inspect of RustFS and Alloy" 0 "$(token_in rustfs alloy)"
  expect "nor in what docker exec's env shows of RustFS" 0 "$(docker exec "$P-rustfs" env | grep -cFf "$D/secret/token")"
  expect "RustFS's process has the header (as root on the host could read)" 1 \
    "$(docker exec "$P-rustfs" sh -c 'for p in /proc/[0-9]*; do tr "\0" "\n" <$p/environ 2>/dev/null; done' | grep -c '^RUSTFS_OBS_ENDPOINT_METRICS_HEADERS=authorization=Bearer ' | grep -c '^[1-9]')"
  note "RustFS's log lines (info): $(docker logs "$P-rustfs" 2>&1 | wc -l), on OTLP or export: $(docker logs "$P-rustfs" 2>&1 | grep -ci 'otlp\|export')"
  note "Alloy's lines on refusals: $(docker logs "$P-alloy" 2>&1 | grep -ci 'unauth\|401\|bearer')"
  # Metrics off, as the compose file without --metrics-url: the endpoint set, export off.
  docker rm -f "$P-rustfs" >/dev/null
  EXPORT=false rustfs "$OTLP"
  sleep 40
  note "metrics off: RustFS's lines on OTLP, export or errors: $(docker logs "$P-rustfs" 2>&1 | grep -ci 'otlp\|export\|error'); running: $(docker inspect -f '{{.State.Status}}' "$P-rustfs")"
  end_group
}

# pool_restart LABEL COMMAND...: RustFS restarted by COMMAND (Docker gives it
# a new network namespace); then, within 3 minutes, Alloy must push from the
# new one again, its own and RustFS's series fresh (under 60 s old, at least
# 60 s after the restart began, so newer than the restart).
pool_restart() {
  local label=$1 r0 a0 said0 t0 fresh=999 rfresh=999
  shift
  r0=$(docker inspect -f '{{.State.StartedAt}}' "$P-rustfs")
  a0=$(docker inspect -f '{{.RestartCount}}' "$P-alloy")
  said0=$(docker logs "$P-alloy" 2>&1 | grep -c "^qd: RustFS restarted")
  t0=$(date +%s)
  "$@" >/dev/null 2>&1
  for _ in $(seq 36); do
    sleep 5
    [ $(($(date +%s) - t0)) -ge 60 ] || continue
    fresh=$(age 'up{job="alloy",vm="node-1",role="lake"}')
    rfresh=$(age 'rustfs_process_uptime_seconds{vm="node-1",role="lake"}')
    [ "$fresh" -lt 60 ] && [ "$rfresh" -lt 60 ] && break
  done
  note "$label, $(($(date +%s) - t0)) s after: RustFS $([ "$(docker inspect -f '{{.State.StartedAt}}' "$P-rustfs")" != "$r0" ] && echo restarted || echo "not restarted"); Alloy's last sample $fresh s old, RustFS's $rfresh s; Alloy restarted by its policy $(($(docker inspect -f '{{.RestartCount}}' "$P-alloy") - a0)) times, its loop's line $(($(docker logs "$P-alloy" 2>&1 | grep -c "^qd: RustFS restarted") - said0)) times"
  note "$label: Alloy $(docker inspect -f '{{.State.Status}}, started {{.State.StartedAt}}' "$P-alloy" | cut -c1-40) (RustFS $(docker inspect -f '{{.State.StartedAt}}' "$P-rustfs" | cut -c12-19)); its interfaces: $(docker exec "$P-alloy" cat /proc/net/dev 2>/dev/null | awk -F: 'NR > 2 { print $1 }' | tr -d ' ' | tr '\n' ' ')"
  expect "$label: Alloy pushes again from RustFS's new namespace (its last sample under 60 s old)" 1 "$([ "$fresh" -lt 60 ] && echo 1 || echo 0)"
  expect "… and RustFS's own series arrive again" 1 "$([ "$rfresh" -lt 60 ] && echo 1 || echo 0)"
  # Once: an Alloy that could not start again (its write-ahead log after the
  # loop's KILL, say) would be restarted by its policy over and over.
  expect "… Alloy started again once by its policy" 1 "$(($(docker inspect -f '{{.RestartCount}}' "$P-alloy") - a0))"
}

# pool_node: RustFS with the compose file's restart policy (for a crash) and
# Alloy in its namespace under the pool's compose file's loop, restarted by
# Docker, started.
pool_node() {
  RESTART=unless-stopped rustfs "$NET" --add-host "$NODE:127.0.0.1"
  sed -n '/^  alloy:$/,/^    command:$/p' "$ROOT/infra/rustfs/pool/docker-compose.yml" |
    sed -n '/^      - |$/,/^      - alloy$/p' | sed '1d;$d; s/^        //; s/\$\$/$/g' >"$D/files/loop.sh"
  chmod 644 "$D/files/loop.sh"
  ALLOY_PRE=(/etc/qd/loop.sh)
  RESTART=unless-stopped alloy lake.alloy --network "container:$P-rustfs" \
    -v "$D/files/loop.sh:/etc/qd/loop.sh:ro" --entrypoint /bin/sh
  ALLOY_PRE=()
  docker start "$P-alloy" >/dev/null
}

# --- pool: Alloy in RustFS's network namespace ---------------------------------------------
group_pool() {
  echo "== pool: Alloy in RustFS's network namespace, on its loopback"
  networks
  node
  mimir
  pool_node
  if wait_series 'count({__name__=~"rustfs_.+",vm="node-1",role="lake"})' 1; then
    ok "RustFS's series of role lake in Mimir, pushed to Alloy on RustFS's loopback"
  else
    ko "no RustFS series of role lake: $(docker logs "$P-alloy" 2>&1 | tail -2 | cut -c1-300)"
  fi
  expect "the receiver from another container, at RustFS's address" 000 \
    "$(push "$NET" "https://$(ip_on rustfs "$NET"):4318/v1/metrics" -k)"
  pool_restart "docker restart" docker restart "$P-rustfs"
  # RustFS's process killed, as a crash: its restart policy starts it again.
  pool_restart "RustFS killed" docker exec -u 0 "$P-rustfs" sh -c 'kill -KILL -1'
  end_group
}

# --- gap: what Alloy does across RustFS's restarts, and what Mimir lacks after -------------
# A pool node as the compose file runs it: RustFS pushing every 30 s, Alloy's
# storage on a volume (alloy-data). RustFS restarted six times, both ways. For
# each: when the loop said its line, Alloy stopped and started again, and its
# lines in between; from Mimir's raw samples afterwards, each series' gap
# around the restart, how many samples of the time Alloy was away arrived
# later (sent from its write-ahead log, or pushed again by RustFS), and how
# long after Alloy's start each series' first sample came; and, as the loop
# ends Alloy with KILL (the user's ruling of 2026-11-16), what Alloy logs of
# its write-ahead log in its first 90 s after each start (replay, repair,
# corruption, errors).
group_gap() {
  echo "== gap: Alloy across RustFS's restarts, its storage on a volume, RustFS pushing every 30 s"
  networks
  node
  mimir
  docker volume create "$P-alloy-data" >/dev/null
  docker run --rm --name "$P-init" -v "$P-alloy-data:/d" alpine:3.20 chown 10001:10001 /d
  METER=30 ALLOY_DATA=$P-alloy-data pool_node
  wait_series 'count(rustfs_process_uptime_seconds{vm="node-1",role="lake"})' 1 || ko "no RustFS series before the restarts"
  sleep 120
  local way t0 first n=0 r s q
  first=$(date +%s)
  : >"$D/restarts"
  for way in restart kill restart kill restart kill; do
    n=$((n + 1))
    t0=$(date +%s.%N)
    if [ "$way" = restart ]; then
      pool_restart "gap $n, docker restart" docker restart "$P-rustfs"
    else
      pool_restart "gap $n, RustFS killed" docker exec -u 0 "$P-rustfs" sh -c 'kill -KILL -1'
    fi
    echo "$n $way $t0 $(docker inspect -f '{{.State.FinishedAt}} {{.State.StartedAt}}' "$P-alloy")" >>"$D/restarts"
    sleep 90
  done
  r=$(($(date +%s) - first + 300))
  for s in rustfs_process_uptime_seconds up_alloy up_files; do
    case $s in
    up_alloy) q='up{job="alloy",vm="node-1"}' ;;
    up_files) q='up{job="files",vm="node-1"}' ;;
    *) q="$s{vm=\"node-1\"}" ;;
    esac
    mq query "query=$q[${r}s]" >"$D/raw-$s.json"
  done
  docker logs -t "$P-alloy" >"$D/alloy.log" 2>&1
  docker logs -t "$P-rustfs" >"$D/rustfs.log" 2>&1
  python3 - "$D" <<'PY'
import datetime, json, re, sys
d = sys.argv[1]
def ts(x):
    x = x.rstrip("Z")
    head, _, frac = x.partition(".")
    return datetime.datetime.fromisoformat(head).replace(tzinfo=datetime.timezone.utc).timestamp() + float("0." + (frac or "0"))
rows = [l.split() for l in open(d + "/restarts")]
raw = {}
for s in ("rustfs_process_uptime_seconds", "up_alloy", "up_files"):
    r = json.load(open(f"{d}/raw-{s}.json"))["data"]["result"]
    raw[s] = sorted(float(t) for t, _ in r[0]["values"]) if r else []
alloy = []
for l in open(d + "/alloy.log"):
    t, _, rest = l.partition(" ")
    try:
        alloy.append((ts(t), rest.strip()))
    except ValueError:
        pass
rlog = []
for l in open(d + "/rustfs.log"):
    t, _, rest = l.partition(" ")
    try:
        rlog.append((ts(t), rest.strip()))
    except ValueError:
        pass
for n, way, t0, fin, start in rows:
    t0, fin, start = float(t0), ts(fin), ts(start)
    said = [t for t, m in alloy if t >= t0 and m.startswith("qd: RustFS restarted")]
    print(f"[NOTE] gap {n} ({way}): loop's line +{said[0] - t0:.0f} s, Alloy stopped +{fin - t0:.0f} s, started +{start - t0:.0f} s" if said else f"[NOTE] gap {n}: no loop line")
    seen = {}
    for t, m in alloy:
        if not (t0 <= t <= start + 1):
            continue
        msg = re.search(r'msg="([^"]*)"|msg=(\S+)', m)
        key = (msg.group(1) or msg.group(2)) if msg else m[:90]
        if key not in seen:
            extra = " ".join(re.findall(r'\b(?:level|count|err)=("[^"]{0,90}|\S+)', m))
            seen[key] = f"+{t - t0:.0f} s {key} [{extra[:160]}]"
    for v in list(seen.values())[:14]:
        print("[LINE]   " + v)
    for s, xs in raw.items():
        before = [x for x in xs if x < t0]
        away = [x for x in xs if t0 <= x < start]
        after = [x for x in xs if x >= start]
        if before and after:
            print(f"[GAP ]   {s}: last before +{before[-1] - t0:.0f} s, first after Alloy's start +{after[0] - start:.0f} s from its start; "
                  f"gap {after[0] - before[-1]:.0f} s; samples timed while Alloy was away (arrived later): {len(away)}")
        else:
            print(f"[GAP ]   {s}: samples before {len(before)}, after {len(after)}")
    wal = {}
    for t, m in alloy:
        if start - 1 <= t <= start + 90 and re.search(r"(?i)\bwal\b|segment|corrupt|repair|replay|checkpoint|level=error", m):
            msg = re.search(r'msg="([^"]*)"|msg=(\S+)', m)
            key = (msg.group(1) or msg.group(2)) if msg else m[:90]
            extra = " ".join(re.findall(r'\b(?:level|err|component_id|segment|duration)=("[^"]{0,90}|\S+)', m))
            wal.setdefault(key, f"+{t - start:.0f} s {key} [{extra[:200]}]")
    print(f"[WAL ]   Alloy's lines on its write-ahead log or at error in its first 90 s: {len(wal)}")
    for v in list(wal.values())[:10]:
        print("[WAL ]     " + v)
    rl = [m for t, m in rlog if t0 <= t <= start + 60 and re.search(r"(?i)export|otlp|metric", m)]
    print(f"[NOTE]   RustFS's lines on export from the restart to a minute after Alloy's start: {len(rl)}" + (f"; first: {rl[0][:220]}" if rl else ""))
PY
  end_group
}

# --- renewal: the machine's certificate renewed, Alloy not restarted -------------------------
group_renewal() {
  echo "== renewal: machine.pem renewed while Alloy runs"
  networks
  local until
  until=$(date -u -d '+150 seconds' +%Y%m%d%H%M%SZ)
  node "$until"
  mimir
  rustfs "$OTLP"
  alloy lake-replica.alloy --network "$NET"
  docker network connect --alias "$NODE" "$OTLP" "$P-alloy" && docker start "$P-alloy" >/dev/null
  wait_series 'count(up{job="alloy",vm="node-1"})' 1 || ko "Alloy's first push did not arrive"
  served() { docker run --rm --network "$OTLP" --user 0 -v "$D:/h:ro" "$CURL" -s -o /dev/null --max-time 10 -k -w '%{certs}' "https://$NODE:4318/" 2>/dev/null | openssl x509 -noout -serial 2>/dev/null | cut -d= -f2; }
  local old
  old=$(served)
  expect "the receiver serves the first certificate" "$(openssl x509 -noout -serial -in "$D/machine.crt" | cut -d= -f2)" "$old"
  renew
  sleep 75
  expect "the receiver serves the renewed one, Alloy not restarted" "$(openssl x509 -noout -serial -in "$D/machine.crt" | cut -d= -f2)" "$(served)"
  # Past the first certificate's end, Mimir restarted: every connection new.
  while [ "$(date -u +%Y%m%d%H%M%SZ)" \< "$until" ]; do sleep 5; done
  docker restart "$P-mimir" >/dev/null
  sleep 90
  local fresh rfresh
  fresh=$(age 'up{job="alloy",vm="node-1"}')
  rfresh=$(age 'rustfs_process_uptime_seconds{vm="node-1"}')
  note "renewal: Alloy's last sample $fresh s old, RustFS's $rfresh s"
  expect "after the first certificate expired and Mimir restarted, Alloy pushes with the new one (sample under 60 s old)" 1 \
    "$([ "$fresh" -lt 60 ] && echo 1 || echo 0)"
  expect "… and RustFS's series still arrive" 1 "$([ "$rfresh" -lt 60 ] && echo 1 || echo 0)"
  note "Alloy's restarts: $(docker inspect -f '{{.RestartCount}}' "$P-alloy"), started at $(docker inspect -f '{{.State.StartedAt}}' "$P-alloy" | cut -c12-19)"
  end_group
}

GROUPS_TO_RUN=("$@")
[ ${#GROUPS_TO_RUN[@]} -gt 0 ] || GROUPS_TO_RUN=(names replica pool renewal)
watchdog &
WATCHDOG=$!
trap 'kill $WATCHDOG 2>/dev/null; cleanup' EXIT
for g in "${GROUPS_TO_RUN[@]}"; do
  wait_quiet
  echo "[$(date -u +%H:%M:%S)] load $(cut -d' ' -f1 /proc/loadavg)"
  "group_$g"
done
echo
echo "$PASSED passed, $FAILED failed ($(date -u +%Y-%m-%d))"
[ "$FAILED" = 0 ]
