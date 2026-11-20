#!/bin/bash
# Shared by the verification scripts of decision 0072: the counters, the wait
# for a quiet machine and the watchdog, a throwaway Mimir 3.2.4 (monolithic,
# its blocks on a tmpfs) and the cleanup of everything named "$P-…".
# Sourced after P is set; nothing here publishes a port or prints a key.
set -u
export LC_ALL=C
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
MIMIR_IMAGE=grafana/mimir:3.2.4
ALLOY_IMAGE=grafana/alloy:v1.21.3
GRAFANA_IMAGE=grafana/grafana:13.2.5
CURL=curlimages/curl:8.11.1
NET=$P-net
D=$(mktemp -d)
chmod 755 "$D"
FAILED=0
PASSED=0
ok() { printf '[OK  ] %s\n' "$*"; PASSED=$((PASSED + 1)); }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=$((FAILED + 1)); }
note() { printf '[NOTE] %s\n' "$*"; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }
yn() { "$@" >/dev/null 2>&1 && echo yes || echo no; }

cleanup() {
  docker ps -aq --filter "name=^/$P-" | xargs -r docker rm -f >/dev/null 2>&1
  docker volume ls -q --filter "name=^$P-" | xargs -r docker volume rm >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
  rm -rf "$D"
}
busy() {
  pgrep -x rustc >/dev/null && { echo "rustc running"; return 0; }
  pgrep -x cc1plus >/dev/null && { echo "cc1plus running"; return 0; }
  local others
  others=$(docker ps --format '{{.Names}}' | grep '^qd-verify-' | grep -cv "^$P-")
  [ "$others" -gt 0 ] && { echo "$others other qd-verify- containers"; return 0; }
  awk -v l="$(cut -d' ' -f1 /proc/loadavg)" 'BEGIN { exit !(l >= 4) }' && { echo "load $(cut -d' ' -f1 /proc/loadavg)"; return 0; }
  return 1
}
# Waits for a quiet machine, starts the watchdog (load above 8: everything
# of this script removed and the script ended) and the network.
start_run() {
  while why=$(busy); do echo "[wait] $why"; sleep 60; done
  ( while sleep 10; do
      awk -v l="$(cut -d' ' -f1 /proc/loadavg)" 'BEGIN { exit !(l > 8) }' || continue
      echo "[STOP] load above 8: removing $P- containers"
      docker ps -aq --filter "name=^/$P-" | xargs -r docker rm -f >/dev/null 2>&1
      kill -TERM $$
      exit
    done ) &
  WATCHDOG=$!
  trap 'kill $WATCHDOG 2>/dev/null; cleanup' EXIT
  trap 'exit 130' INT TERM
  docker network create ${NET_OPTS:-} "$NET" >/dev/null
}

# --- A throwaway CA and certificates (server and client usage, as the node
# agent's), readable by the containers' users: throwaway keys.
ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/O=qd/CN=qd verify $1" -addext "basicConstraints=critical,CA:TRUE" \
    -keyout "$D/$1.key" -out "$D/$1.crt" 2>/dev/null
  chmod 644 "$D/$1".*
}
cert() { # NAME CA SUBJECT DNS: NAME.crt, NAME.key and NAME.pem (both)
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$3" \
    -keyout "$D/$1.k" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:%s\n' "$4" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/$2.crt" -CAkey "$D/$2.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  openssl pkey -in "$D/$1.k" -out "$D/$1.key"
  cat "$D/$1.crt" "$D/$1.key" >"$D/$1.pem"
  rm -f "$D/$1.k" "$D/$1.csr"
  chmod 644 "$D/$1".*
}

# --- Mimir: mimir_up [extra limits as YAML lines]. Named $P-mimir, alias mimir
# on $NET, no port published; MIMIR_FLAGS adds command-line flags.
mimir_up() {
  cat >"$D/mimir.yml" <<YML
multitenancy_enabled: false
target: all
server: { http_listen_port: 9009, grpc_listen_address: 127.0.0.1, grpc_listen_port: 9096, log_level: warn }
memberlist: { bind_addr: [127.0.0.1] }
blocks_storage: { backend: filesystem, filesystem: { dir: /data/blocks }, tsdb: { dir: /data/tsdb }, bucket_store: { sync_dir: /data/sync } }
ruler_storage: { backend: filesystem, filesystem: { dir: /data/rules } }
alertmanager_storage: { backend: filesystem, filesystem: { dir: /data/am } }
activity_tracker: { filepath: /data/activity.log }
compactor: { data_dir: /data/compactor, sharding_ring: { instance_addr: 127.0.0.1, kvstore: { store: memberlist } } }
distributor: { ring: { instance_addr: 127.0.0.1, kvstore: { store: memberlist } } }
ingester: { ring: { instance_addr: 127.0.0.1, kvstore: { store: memberlist }, replication_factor: 1 } }
store_gateway: { sharding_ring: { instance_addr: 127.0.0.1, replication_factor: 1 } }
querier: { ring: { instance_addr: 127.0.0.1 } }
query_scheduler: { ring: { instance_addr: 127.0.0.1 } }
ruler: { rule_path: /data/ruler, ring: { instance_addr: 127.0.0.1 } }
frontend: { address: 127.0.0.1 }
usage_stats: { enabled: false }
limits:
  max_global_series_per_user: 20000
  max_label_names_per_series: 20
${1:-}
YML
  chmod 644 "$D/mimir.yml"
  docker run -d --name "$P-mimir" --network "$NET" --network-alias mimir --tmpfs /data:mode=1777 \
    -v "$D:/c:ro" "$MIMIR_IMAGE" -config.file=/c/mimir.yml ${MIMIR_FLAGS:-} >/dev/null
  local i
  for i in $(seq 90); do [ "$(mimir_get /ready -o /dev/null -w '%{http_code}')" = 200 ] && return 0; sleep 2; done
  note "Mimir not ready: $(docker logs "$P-mimir" 2>&1 | tail -2 | cut -c1-300)"
  return 1
}
# mimir_get PATH [CURL OPTION...]: Mimir over plain HTTP, or with $MIMIR_CERT
# (a name in $D) over HTTPS when MIMIR_TLS=1; the body on stdout.
mimir_get() {
  local path=$1 c=()
  shift
  if [ "${MIMIR_TLS:-0}" = 1 ]; then
    c=(--cacert /c/ca.crt)
    [ -n "${MIMIR_CERT:-}" ] && c+=(--cert "/c/$MIMIR_CERT.pem")
    docker run --rm --network "$NET" -v "$D:/c:ro" "$CURL" -s -m 30 "${c[@]}" "$@" "https://mimir:9009$path"
  else
    docker run --rm --network "$NET" "$CURL" -s -m 30 "$@" "http://mimir:9009$path"
  fi
}
# promql EXPR: an instant query's JSON.
promql() { mimir_get /prometheus/api/v1/query -G --data-urlencode "query=$1"; }
# scalar EXPR: the first value of an instant query, or "none".
scalar() {
  promql "$1" | python3 -I -c '
import json, sys
r = json.load(sys.stdin)["data"]["result"]
print(r[0]["value"][1] if r else "none")' 2>/dev/null || echo none
}
