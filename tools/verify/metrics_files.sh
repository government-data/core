#!/bin/bash
# Verification of decision 0071 step 3: the Rust services' metrics file, read
# by Alloy and pushed to Mimir. The real API (the `server` binary this
# checkout built, run on this machine, never built here) serves against a
# throwaway PostgreSQL and writes its file into a folder of the test; the
# repository's alloy/app.alloy, in Alloy, reads that folder as the volume
# data-quality-metrics and pushes to a throwaway Mimir (monolithic, its
# blocks on its own disk: the lake and TLS are step 1's and 2's).
#
# It shows: the file written aside and renamed, 0644, every 15 s, valid
# text Alloy reads (`up` 1, no scrape error); in Mimir only the names of
# app.alloy's keep list for the job `files`, and only its label names;
# every `route` a template of the router or `unmatched`, though requests
# carried synthetic ids and unknown paths; every `kind` one of the queue's,
# with the jobs the test queued; qd_machine_approved (decision 0072, step
# 3) for the approved and draining machines alone, its `vm` and `role` the
# machine's in Mimir too (the remote write's external labels do not replace
# them); and the API, with no folder to write in, still answering, with one
# warning in its log for many ticks.
#
# Containers and the network qd-verify-metrics-files…, restart "no", removed
# on exit; Mimir on 127.0.0.1:19019, PostgreSQL on 127.0.0.1:15439, the API
# on 127.0.0.1:18089. Nothing of the dev stack is touched; nothing secret is
# printed. Run it alone: it waits for no compiler, no other qd-verify-
# container and a load under 4, and stops above 8.
#
#   bash tools/verify/metrics_files.sh   [SERVER_BIN=target/debug/server]
#
# Needs docker, curl and python3.
set -u
export LC_ALL=C
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
P=qd-verify-metrics-files
SERVER_BIN=${SERVER_BIN:-${CARGO_TARGET_DIR:-$ROOT/target}/debug/server}
MIMIR=grafana/mimir:3.2.4
ALLOY=grafana/alloy:v1.21.3
PG=postgres:16-alpine
HELPER=alpine:3.20
D=$(mktemp -d)
chmod 755 "$D"
API_PID=
cleanup() {
  [ -n "$API_PID" ] && kill "$API_PID" 2>/dev/null
  docker ps -aq --filter "name=^/$P" | xargs -r docker rm -f >/dev/null 2>&1
  docker volume ls -q --filter "name=^$P" | xargs -r docker volume rm >/dev/null 2>&1
  docker network ls -q --filter "name=^$P" | xargs -r docker network rm >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
FAILED=0
PASSED=0
ok() { printf '[OK  ] %s\n' "$*"; PASSED=$((PASSED + 1)); }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=$((FAILED + 1)); }
note() { printf '[NOTE] %s\n' "$*"; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

[ -x "$SERVER_BIN" ] || { echo "no server binary at $SERVER_BIN: build it first (cargo build -p server)"; exit 2; }
busy() {
  pgrep -x rustc >/dev/null && { echo "rustc running"; return 0; }
  pgrep -x cc1plus >/dev/null && { echo "cc1plus running"; return 0; }
  local others
  others=$(docker ps --format '{{.Names}}' | grep '^qd-verify-' | grep -cv "^$P")
  [ "$others" -gt 0 ] && { echo "$others other qd-verify- containers"; return 0; }
  awk -v l="$(cut -d' ' -f1 /proc/loadavg)" 'BEGIN { exit !(l >= 4) }' && { echo "load $(cut -d' ' -f1 /proc/loadavg)"; return 0; }
  return 1
}
while why=$(busy); do echo "[wait] $why"; sleep 60; done
( while sleep 10; do
    awk -v l="$(cut -d' ' -f1 /proc/loadavg)" 'BEGIN { exit !(l > 8) }' || continue
    echo "[STOP] load above 8: removing $P containers"
    docker ps -aq --filter "name=^/$P" | xargs -r docker rm -f >/dev/null 2>&1
    kill -TERM $$
    exit
  done ) &
WATCHDOG=$!
trap 'kill $WATCHDOG 2>/dev/null; cleanup' EXIT

# --- PostgreSQL, Mimir and Alloy ----------------------------------------------------
docker network create "$P" >/dev/null
mkdir -p "$D/metrics" "$D/settings" "$D/alloy"
chmod 755 "$D/metrics" "$D/alloy"
openssl rand -hex 16 >"$D/pg_password"
printf 'postgres://postgres:%s@127.0.0.1:15439/qd' "$(cat "$D/pg_password")" >"$D/database_url"
chmod 600 "$D/database_url"
docker run -d --name "$P-pg" --network "$P" --restart no -p 127.0.0.1:15439:5432 \
  -e POSTGRES_DB=qd -e POSTGRES_PASSWORD_FILE=/run/pw -v "$D/pg_password:/run/pw:ro" "$PG" >/dev/null
cp "$ROOT/infra/observability/alloy/app.alloy" "$D/alloy/app.alloy"
chmod 644 "$D/alloy/app.alloy"
cat >"$D/mimir.yml" <<'YML'
multitenancy_enabled: false
target: all
server: { http_listen_port: 9009, grpc_listen_address: 127.0.0.1, log_level: warn }
common: { storage: { backend: filesystem, filesystem: { dir: /data/store } } }
blocks_storage:
  storage_prefix: blocks
  tsdb: { dir: /data/tsdb }
  bucket_store: { sync_dir: /data/tsdb-sync }
ruler_storage: { storage_prefix: ruler }
alertmanager_storage: { storage_prefix: alertmanager }
ruler: { rule_path: /data/ruler }
activity_tracker: { filepath: /data/activity.log }
compactor: { data_dir: /data/compactor, sharding_ring: { kvstore: { store: memberlist } } }
distributor: { ring: { instance_addr: 127.0.0.1, kvstore: { store: memberlist } } }
ingester: { ring: { instance_addr: 127.0.0.1, kvstore: { store: memberlist }, replication_factor: 1 } }
store_gateway: { sharding_ring: { replication_factor: 1 } }
frontend: { address: 127.0.0.1 }
limits: { max_global_series_per_user: 20000, max_label_names_per_series: 20 }
usage_stats: { enabled: false }
YML
chmod 644 "$D/mimir.yml"
docker volume create "$P-mimir" >/dev/null
docker run --rm -v "$P-mimir:/d" "$HELPER" chmod 1777 /d
docker run -d --name "$P-mimir" --network "$P" --network-alias mimir --restart no -p 127.0.0.1:19019:9009 \
  -v "$D/mimir.yml:/etc/mimir.yml:ro" -v "$P-mimir:/data" "$MIMIR" -config.file=/etc/mimir.yml >/dev/null
for _ in $(seq 60); do [ "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:19019/ready)" = 200 ] && break; sleep 2; done
expect "Mimir ready" 200 "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:19019/ready)"

# --- The real API, writing its file -----------------------------------------------
for _ in $(seq 30); do docker exec "$P-pg" pg_isready -U postgres -d qd >/dev/null 2>&1 && break; sleep 1; done
# The queue's role, which the module's migrations grant to (infra/postgres/initdb).
docker exec "$P-pg" psql -q -U postgres -d qd -c 'CREATE ROLE qd_queue LOGIN' >/dev/null
api() { # METRICS_DIR LOG: the server in the background
  env -i PATH="$PATH" HOME="$D" DATABASE_URL_FILE="$D/database_url" APP_HOST=127.0.0.1 APP_PORT=18089 \
    SETTINGS_KEY_FILE="$D/settings/settings.key" OBS_VM=verify-files RUST_LOG=info \
    QD_METRICS_DIR="$1" "$SERVER_BIN" serve >"$2" 2>&1 &
  API_PID=$!
  for _ in $(seq 60); do [ "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:18089/health)" = 200 ] && return; sleep 2; done
}
api "$D/metrics" "$D/api.log"
expect "the API answers /health" 200 "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:18089/health)"
[ "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:18089/health)" = 200 ] ||
  note "the API's last lines: $(tail -3 "$D/api.log" | cut -c1-300 | tr '\n' ' ')"

# Synthetic jobs of the main tenant: two evaluations ready, one in backoff.
docker exec -i "$P-pg" psql -q -U postgres -d qd -v ON_ERROR_STOP=1 >/dev/null <<'SQL'
INSERT INTO quality.jobs (tenant_id, kind, dataset, not_before) VALUES
  ('00000000-0000-0000-0000-000000000001', 'evaluate', 'dataset-x', now() - interval '2 minutes'),
  ('00000000-0000-0000-0000-000000000001', 'evaluate', 'dataset-y', now()),
  ('00000000-0000-0000-0000-000000000001', 'map', 'dataset-x', now() + interval '1 hour');
INSERT INTO machines (tenant_id, name, role, state) VALUES
  ('00000000-0000-0000-0000-000000000001', 'worker-1', 'worker', 'approved'),
  ('00000000-0000-0000-0000-000000000001', 'lake-a', 'lake', 'draining'),
  ('00000000-0000-0000-0000-000000000001', 'worker-2', 'worker', 'pending'),
  ('00000000-0000-0000-0000-000000000001', 'replica-1', 'lake-replica', 'retired');
SQL
# Requests a client could send: synthetic ids, a nested id, unknown paths.
for path in /health /health /quality/datasets/verify-ds-123/cuts /quality/datasets/0b5a7c1e-77/charts \
  /quality/datasets/verify-ds-123/charts/zone-b /no/such/path/verify-42 /verify-unknown; do
  curl -s -o /dev/null "http://127.0.0.1:18089$path"
done
sleep 32 # two writes, and the queue read once
F="$D/metrics/api.prom"
expect "api.prom written, mode 644, nothing left aside" "644 api.prom" \
  "$(stat -c %a "$F" 2>/dev/null) $(ls -A "$D/metrics" | tr '\n' ' ' | sed 's/ $//')"
age=$(( $(date +%s) - $(stat -c %Y "$F" 2>/dev/null || echo 0) ))
expect "written again within 15 s" yes "$([ "$age" -le 16 ] && echo yes || echo "no: ${age}s old")"
python3 -I - "$F" >"$D/file.txt" <<'PY'
import re, sys
text = open(sys.argv[1]).read()
names, routes, kinds, labels, machines = set(), set(), set(), set(), set()
for line in text.splitlines():
    if line.startswith("#"):
        continue
    m = re.match(r'^([a-zA-Z_:][a-zA-Z0-9_:]*)(\{(.*)\})? (\S+)$', line)
    if not m:
        print("BAD", line); continue
    names.add(m.group(1))
    found = dict(re.findall(r'([a-zA-Z_]+)="([^"]*)"', m.group(3) or ""))
    for k, v in found.items():
        labels.add(k)
        if k == "route": routes.add(v)
        if k == "kind": kinds.add(v)
    if m.group(1) == "qd_machine_approved":
        machines.add(f"{found.get('vm')}/{found.get('role')}/{len(found)}/{m.group(4)}")
print("names", " ".join(sorted(names)))
print("machines", " ".join(sorted(machines)))
print("labels", " ".join(sorted(labels)))
print("routes", " ".join(sorted(routes)))
print("kinds", " ".join(sorted(kinds)))
PY
note "the file: $(grep -c . "$F") lines; $(grep '^names' "$D/file.txt")"
expect "every line of the file is the text format" 0 "$(grep -c '^BAD' "$D/file.txt")"
expect "the file's routes: templates and unmatched only" \
  "routes /health /quality/datasets/{dataset}/charts /quality/datasets/{dataset}/charts/{chart} /quality/datasets/{dataset}/cuts unmatched" \
  "$(grep '^routes' "$D/file.txt")"
expect "the file's label names" "labels kind le role route service status_class vm" "$(grep '^labels' "$D/file.txt")"
expect "the file's machines: approved and draining, by name and role alone, 1" \
  "machines lake-a/lake/2/1 worker-1/worker/2/1" "$(grep '^machines' "$D/file.txt")"
expect "the evaluations ready, the one in backoff not" "2 0" \
  "$(grep '^qd_queue_ready_jobs{kind="evaluate"}' "$F" | cut -d' ' -f2) $(grep '^qd_queue_ready_jobs{kind="map"}' "$F" | cut -d' ' -f2)"

# --- Alloy reads it with app.alloy -----------------------------------------------
docker run -d --name "$P-alloy" --network "$P" --restart no \
  -e OBS_VM=verify-files -e METRICS_PUSH_URL=http://mimir:9009/api/v1/push \
  -v "$D/alloy:/etc/qd-alloy:ro" -v "$D/metrics:/run/qd-metrics:ro" \
  "$ALLOY" run --storage.path=/tmp/alloy --disable-reporting /etc/qd-alloy >/dev/null
mimir() { curl -sS -G --max-time 15 "http://127.0.0.1:19019/prometheus/api/v1/$1" "${@:2}" 2>/dev/null; }
value() { # QUERY: the first result's value, or none
  mimir query --data-urlencode "query=$1" | python3 -I -c '
import json, sys
r = json.load(sys.stdin)["data"]["result"]
print(r[0]["value"][1] if r else "none")' 2>/dev/null
}
for _ in $(seq 30); do [ "$(value 'up{job="files",vm="verify-files"}')" = 1 ] && break; sleep 5; done
expect "Alloy's scrape of the folder: up" 1 "$(value 'up{job="files",vm="verify-files"}')"
[ "$(value 'up{job="files",vm="verify-files"}')" = 1 ] ||
  note "Alloy's last lines: $(docker logs "$P-alloy" 2>&1 | tail -3 | cut -c1-300 | tr '\n' ' ')"
sleep 35 # a second scrape
expect "no scrape error on the file" 0 "$(value 'node_textfile_scrape_error{job="files",vm="verify-files"}')"
mimir series --data-urlencode 'match[]={job="files",vm="verify-files"}' \
  --data-urlencode "start=$(($(date +%s) - 900))" | python3 -I -c '
import json, sys
d = json.load(sys.stdin)["data"]
print("count", len(d))
print("names", " ".join(sorted({s["__name__"] for s in d})))
print("labels", " ".join(sorted({k for s in d for k in s})))
print("routes", " ".join(sorted({s["route"] for s in d if "route" in s})))
print("kinds", " ".join(sorted({s["kind"] for s in d if "kind" in s})))
print("files", " ".join(sorted({s["file"] for s in d if "file" in s})))
print("values", " ".join(sorted({v for s in d for k, v in s.items() if k != "__name__"})))
' >"$D/series.txt" 2>/dev/null
note "Mimir: $(grep '^count' "$D/series.txt") series of the job files; $(grep '^names' "$D/series.txt")"
H() { echo "$1_bucket $1_count $1_sum"; }
allowed="up node_textfile_mtime_seconds node_textfile_scrape_error process_cpu_seconds_total process_resident_memory_bytes qd_http_requests_total $(H qd_http_request_duration_seconds) qd_queue_ready_jobs qd_queue_oldest_ready_seconds qd_machine_alerts_unknown_total"
outside() { python3 -I -c 'import sys; a = set(sys.argv[1].split()); r = [w for w in sys.argv[2].split()[1:] if w not in a]; print(" ".join(r) or "none")' "$1" "$2"; }
expect "every metric name in Mimir is in the keep list" none "$(outside "$allowed" "$(grep '^names' "$D/series.txt")")"
expect "the API's own series arrived" yes "$(grep '^names' "$D/series.txt" | grep -q 'qd_http_requests_total.*qd_queue_ready_jobs' && echo yes || echo no)"
expect "every label name in Mimir is in the list" none \
  "$(outside "__name__ job instance vm role file service route status_class kind le" "$(grep '^labels' "$D/series.txt")")"
expect "the routes in Mimir: templates and unmatched only" \
  "routes /health /quality/datasets/{dataset}/charts /quality/datasets/{dataset}/charts/{chart} /quality/datasets/{dataset}/cuts unmatched" \
  "$(grep '^routes' "$D/series.txt")"
expect "the kinds in Mimir: the queue's list" \
  "kinds evaluate extract lake_tables maintain map query records tables test_rule" "$(grep '^kinds' "$D/series.txt")"
expect "no label value carries an id, a dataset or a path sent" none \
  "$(grep '^values' "$D/series.txt" | grep -oE 'verify-ds|0b5a7c1e|zone-b|verify-42|verify-unknown|dataset-x|dataset-y|/no/such' | tr '\n' ' ' | sed 's/ $//' | grep . || echo none)"
machines=$(mimir series --data-urlencode 'match[]=qd_machine_approved' \
  --data-urlencode "start=$(($(date +%s) - 900))" | python3 -I -c '
import json, sys
d = json.load(sys.stdin)["data"]
print(" ".join(sorted("/".join([s.get(k, "-") for k in ("job", "vm", "role")] + [str(len(s))]) for s in d)))' 2>/dev/null)
expect "the machines in Mimir: their own vm and role, not the remote write's" \
  "files/lake-a/lake/5 files/worker-1/worker/5" "$machines"
expect "the ready evaluations in Mimir" 2 "$(value 'qd_queue_ready_jobs{job="files",kind="evaluate",vm="verify-files"}')"
expect "the process's memory, by service" yes \
  "$(v=$(value 'process_resident_memory_bytes{job="files",service="api",vm="verify-files"}'); [ "$v" != none ] && echo yes || echo no)"
note "the file's age in Mimir: $(grep '^files' "$D/series.txt")"

# --- No folder: the API goes on ------------------------------------------------------
kill "$API_PID" 2>/dev/null
wait "$API_PID" 2>/dev/null
api "$D/no-such-folder" "$D/api-nofolder.log"
expect "with no folder for its file, the API still answers" 200 "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:18089/health)"
sleep 47 # three writes or more
expect "one warning about the file for all those writes" 1 "$(grep -c 'metrics file not written' "$D/api-nofolder.log")"
expect "and still answering after them" 200 "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:18089/health)"

echo "   $MIMIR, $ALLOY, $PG; the server binary $(stat -c '%y' "$SERVER_BIN" | cut -d. -f1)"
echo "$PASSED passed, $FAILED failed"
[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $((FAILED > 0))
