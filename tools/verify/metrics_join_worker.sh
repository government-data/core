#!/bin/bash
# Verification of decision 0071 step 4 for the role worker: a machine joined
# with the real command ends with its Alloy pushing the worker's and the
# node agent's series to Mimir over mutual TLS. With the installation of
# tools/verify/join_installation.sh (this checkout's `server`, the CA of
# `server ca-init`, Traefik in front as https://qd.example) told where the
# machines' metrics go (MACHINES_METRICS_PUSH_URL, https://qd-app:9009),
# and a throwaway Mimir 3.2.4 named qd-app on the installation's network,
# serving HTTPS with a certificate of that CA for qd-app and requiring a
# client certificate of it, the administrator asks for a worker's token and
# the command the API answers is run, as given, on a "machine": a Docker
# 27.5 (dind) container as root, its host name worker-7, where join.sh
# starts the role's compose. It shows: the command names --metrics-url; run
# twice before starting, the same files; a script naming another hash for
# worker.alloy stops, writing nothing; started, the node agent joins with
# the token, Alloy runs as the agent's user, with the machine's
# certificate, and Mimir gets the series of vm worker-7 (role worker): the
# node agent's file and the worker's, `up` 1, every metric and label name
# within worker.alloy's lists, a label and a metric outside them dropped;
# no container of the machine publishes a port, Alloy listens on its
# loopback alone and the node and the worker on nothing; Mimir refuses a
# push without a certificate; the command once more is refused (401) and
# changes nothing.
#
# The node agent is the real one (target/debug/qd-node, built by `cargo
# build -p node-agent`), in an image made here with the libraries it links
# and a wrapper that trusts the front's certificate (QD_CA_FILE), as a
# public one would be. The worker is a stand-in writing worker.prom in the
# format quality::worker_metrics writes (the names and labels of its
# tests, plus a label and a metric outside the lists): the worker's binary
# is not built here, as building it alone rebuilds DuckDB.
#
# Containers qd-verify-metrics-join-pg, -front, -mimir and -machine (the
# worker's own containers live inside the machine), network
# qd-verify-metrics-join-net, image qd-verify-metrics-join:worker, ports
# 127.0.0.1:59090 (PostgreSQL) and 59099 (Mimir); the API listens on the
# network's gateway address, port 59091. All removed on exit. Nothing of
# the dev stack is touched; tokens and keys are never printed. Run it alone:
# it waits for no compiler, no other qd-verify- container and a load under
# 4, and stops above 8.
#
#   bash tools/verify/metrics_join_worker.sh   (SERVER=target/debug/server)
#
# Needs docker (privileged containers), curl, openssl 3, python3 and ldd,
# and the images grafana/mimir:3.2.4, grafana/alloy:v1.21.3, traefik:v3.6,
# docker:27.5.1-dind and alpine:3.20.
set -u
export LC_ALL=C
P=qd-verify-metrics-join
PGPORT=59090
APIPORT=59091
MIMIR_PORT=59099
M=$P-machine
MIMIR=grafana/mimir:3.2.4
ALLOY=grafana/alloy:v1.21.3
DIND=docker:27.5.1-dind
IMAGE=$P:worker
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SERVER=${SERVER:-$ROOT/target/debug/server}
AGENT_BIN=${AGENT_BIN:-$ROOT/target/debug/qd-node}
[ -x "$SERVER" ] && [ -x "$AGENT_BIN" ] || { echo "no $SERVER or $AGENT_BIN: cargo build -p server, -p node-agent"; exit 1; }
"$SERVER" openapi 2>/dev/null | grep -q 'worker.alloy' || { echo "$SERVER does not serve worker.alloy: build it again"; exit 1; }

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

D=$(mktemp -d)
chmod 755 "$D"
FAILED=0
mkdir -m 700 "$D/secret"
API_ENV="MACHINES_METRICS_PUSH_URL=https://qd-app:9009/api/v1/push
INSTALL_RATE_LIMIT_MAX_REQUESTS=200"
# The front's certificate an end entity's, as rustls (the agent) requires.
FRONT_EXT=basicConstraints=critical,CA:FALSE
cleanup() {
  [ -n "${WATCHDOG:-}" ] && kill "$WATCHDOG" 2>/dev/null
  install_cleanup
  docker rm -f -v "$M" "$P-mimir" >/dev/null 2>&1
  docker network rm "$P-net" >/dev/null 2>&1
  docker rmi "$IMAGE" >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
( while sleep 10; do
    awk -v l="$(cut -d' ' -f1 /proc/loadavg)" 'BEGIN { exit !(l > 8) }' || continue
    echo "[STOP] load above 8: removing $P containers"
    docker rm -f -v "$M" "$P-mimir" "$P-pg" "$P-front" >/dev/null 2>&1
    kill -TERM $$
    exit
  done ) &
WATCHDOG=$!
. "$ROOT/tools/verify/join_installation.sh"

echo "== Mimir as qd-app, HTTPS with a certificate of the installation's CA, a client's required"
# cert NAME SUBJECT DNS: NAME.crt and NAME.key under secret/, signed by the CA.
cert() {
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$2" \
    -keyout "$D/secret/$1.key" -out "$D/secret/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:%s\n' "$3" >"$D/secret/$1.ext"
  openssl x509 -req -in "$D/secret/$1.csr" -CA "$D/ca.crt" -CAkey "$D/secret/ca.key" -CAcreateserial \
    -extfile "$D/secret/$1.ext" -days 1 -out "$D/secret/$1.crt" 2>/dev/null
}
cert app "/O=qd/OU=app/CN=qd-app" qd-app
mkdir -p "$D/mimir"
cp "$D/ca.crt" "$D/secret/app.crt" "$D/secret/app.key" "$D/mimir/"
cat >"$D/mimir/mimir.yml" <<'YML'
multitenancy_enabled: false
target: all
server:
  http_listen_port: 9009
  grpc_listen_address: 127.0.0.1
  log_level: warn
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
# Read by Mimir's user: the test's own throwaway key, given its mode here.
chmod 755 "$D/mimir" && chmod 644 "$D/mimir"/*
docker run -d --name "$P-mimir" --network "$NET" --network-alias qd-app --restart no --tmpfs /data:mode=1777 \
  -p "127.0.0.1:$MIMIR_PORT:9009" -v "$D/mimir:/etc/mimir:ro" "$MIMIR" -config.file=/etc/mimir/mimir.yml \
  -server.http-tls-cert-path=/etc/mimir/app.crt -server.http-tls-key-path=/etc/mimir/app.key \
  -server.http-tls-ca-path=/etc/mimir/ca.crt -server.http-tls-client-auth=RequireAndVerifyClientCert >/dev/null ||
  { echo "could not start Mimir"; exit 1; }
mimir() { # PATH [CURL OPTION…]: Mimir's answer, as the application VM
  curl -sS --max-time 15 --cacert "$D/ca.crt" --cert "$D/secret/app.crt" --key "$D/secret/app.key" \
    --resolve "qd-app:$MIMIR_PORT:127.0.0.1" "${@:2}" "https://qd-app:$MIMIR_PORT$1" 2>/dev/null
}
for _ in $(seq 60); do [ "$(mimir /ready -o /dev/null -w '%{http_code}')" = 200 ] && break; sleep 2; done
expect "Mimir ready over mutual TLS" 200 "$(mimir /ready -o /dev/null -w '%{http_code}')"
nocert=$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 --cacert "$D/ca.crt" --resolve "qd-app:$MIMIR_PORT:127.0.0.1" \
  -X POST --data-binary x "https://qd-app:$MIMIR_PORT/api/v1/push")
expect "a push without a client certificate: no handshake" 000 "$nocert"

echo "== The image: the real node agent, the worker's stand-in"
mkdir -p "$D/img/root/usr/local/lib/qd" "$D/img/root/usr/local/bin" "$D/img/root/usr/local/share"
cp "$AGENT_BIN" "$D/img/root/usr/local/lib/qd/qd-node"
strip "$D/img/root/usr/local/lib/qd/qd-node"
# The libraries it links, at the paths it names (its toolchain's).
for f in $(ldd "$AGENT_BIN" | awk '/=>/ { print $3 } /ld-linux/ { print $1 }' | sort -u); do
  mkdir -p "$D/img/root$(dirname "$f")"
  cp -L "$f" "$D/img/root$f"
done
cp "$D/dynamic/front.crt" "$D/img/root/usr/local/share/qd-verify-front.crt"
cat >"$D/img/root/usr/local/bin/qd-node" <<'EOF'
#!/bin/sh
# The front's certificate, trusted as a public one would be.
export QD_CA_FILE=/usr/local/share/qd-verify-front.crt
exec /usr/local/lib/qd/qd-node "$@"
EOF
# The worker's stand-in: worker.prom every 15 s, written aside and renamed,
# in quality::worker_metrics' names and labels, plus a label (dataset) and
# a metric outside worker.alloy's lists, which must not reach Mimir.
cat >"$D/img/root/usr/local/bin/qd-worker" <<'EOF'
#!/bin/sh
umask 022
while :; do
  { echo '# TYPE qd_worker_slots gauge'
    echo 'qd_worker_slots 1'
    echo '# TYPE qd_worker_jobs_running gauge'
    echo 'qd_worker_jobs_running 0'
    echo '# TYPE qd_worker_jobs counter'
    echo 'qd_worker_jobs_total{kind="evaluate",result="ok"} 3'
    echo 'qd_worker_jobs_total{kind="extract",result="failed",dataset="synthetic-1"} 1'
    echo '# TYPE qd_worker_job_duration_seconds histogram'
    echo 'qd_worker_job_duration_seconds_bucket{kind="evaluate",le="0.5"} 1'
    echo 'qd_worker_job_duration_seconds_bucket{kind="evaluate",le="+Inf"} 3'
    echo 'qd_worker_job_duration_seconds_sum{kind="evaluate"} 4.5'
    echo 'qd_worker_job_duration_seconds_count{kind="evaluate"} 3'
    echo '# TYPE qd_verify_outside gauge'
    echo 'qd_verify_outside 1'
    echo '# EOF'
  } >/run/qd-metrics/.worker.prom.tmp && mv /run/qd-metrics/.worker.prom.tmp /run/qd-metrics/worker.prom
  sleep 15
done
EOF
chmod 755 "$D/img/root/usr/local/bin/"* "$D/img/root/usr/local/lib/qd/qd-node"
cat >"$D/img/Dockerfile" <<'EOF'
FROM alpine:3.20
COPY root/ /
RUN mkdir -m 0700 /var/lib/qd-node && chown 65532:65532 /var/lib/qd-node && mkdir -p /run/qd-metrics && chmod 1777 /run/qd-metrics
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/qd-worker"]
EOF
chmod -R a+rX "$D/img"
docker build -q -t "$IMAGE" "$D/img" >/dev/null || { echo "could not build the test image"; exit 1; }

echo "== The machine: Docker in Docker, worker-7"
for image in "$ALLOY" "$DIND" alpine:3.20; do
  docker image inspect "$image" >/dev/null 2>&1 || docker pull -q "$image" >/dev/null || { echo "no $image"; exit 1; }
done
cp "$D/dynamic/front.crt" "$D/secret/front.crt"
docker run -d --name "$M" --privileged --network "$NET" --hostname worker-7 -e DOCKER_TLS_CERTDIR= \
  -v "$D/secret:/s:ro" "$DIND" >/dev/null || { echo "could not start the machine"; exit 1; }
for _ in $(seq 60); do docker exec "$M" docker info >/dev/null 2>&1 && break; sleep 1; done
on() { docker exec "$M" sh -c "$1"; }
on 'apk add -q --no-cache curl' >/dev/null 2>&1 || { echo "could not add curl to the machine"; exit 1; }
for image in "$ALLOY" "$IMAGE" alpine:3.20; do docker save "$image" | docker exec -i "$M" docker load -q >/dev/null; done

issue worker w1
field "$D/secret/w1.json" install command >"$D/secret/w1.sh"
chmod 600 "$D/secret"/*
expect "the command names where the metrics go" 1 "$(grep -c -- "--metrics-url 'https://qd-app:9009/api/v1/push'" "$D/secret/w1.sh")"
W=/opt/qd/worker
# run DIR SCRIPT [ENV...]: SCRIPT (in /s) run as root in the machine, from
# /m/DIR; its exit status, its output in m-DIR.out.
run() {
  local dir=$1 script=$2
  shift 2
  on "mkdir -p /m/$dir"
  docker exec -w "/m/$dir" -e CURL_CA_BUNDLE=/s/front.crt "$@" "$M" sh "/s/$script" >"$D/m-$dir.out" 2>&1
}
said() { grep -c "$2" "$D/m-$1.out"; }
state() {
  on "cd $W && find . -print | sort | xargs stat -c '%n %a %u %s' && find . -type f -print | sort | xargs sha256sum" | sha256sum
}
# The worker's image, a line join.sh does not own (as the installation's
# registry would give it).
on "mkdir -p $W && echo QD_WORKER_IMAGE=$IMAGE >$W/.env"

echo "== Not started, twice: the same"
run run1 w1.sh -e QD_JOIN_NO_START=1
expect "it runs to the end, the worker's files checked" "0 1" "$? $(said run1 'the worker compose file and its files checked')"
expect "worker.alloy is this version's, 0644" "644 $(sha256sum "$ROOT/infra/observability/alloy/worker.alloy" | cut -d' ' -f1)" \
  "$(on "stat -c %a $W/worker.alloy") $(on "sha256sum $W/worker.alloy" | cut -d' ' -f1)"
mimir_ip=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$P-mimir")
expect "metrics.yml: qd-app at the address the machine's DNS gives" "- \"qd-app:$mimir_ip\"" "$(on "grep -- '- ' $W/metrics.yml" | sed 's/^ *//')"
expect "the environment for Alloy" "OBS_VM=worker-7|METRICS_PUSH_URL=https://qd-app:9009/api/v1/push|COMPOSE_PROFILES=metrics" \
  "$(on "grep -E '^(OBS_VM|METRICS_PUSH_URL|COMPOSE_PROFILES)=' $W/.env" | paste -sd'|')"
stamp=$(state)
run run2 w1.sh -e QD_JOIN_NO_START=1
expect "run again: the same files" "0 $stamp" "$? $(state)"

echo "== A changed Alloy configuration: refused by its hash"
on "sed 's/^WORKER_ALLOY_SHA256=.*/WORKER_ALLOY_SHA256=$(printf '0%.0s' $(seq 64))/' /m/run1/qd-join.sh" >"$D/secret/other-hash.sh"
sed 's/^.*&& sh qd-join.sh /sh \/s\/other-hash.sh /' "$D/secret/w1.sh" | sed '1,2{/^curl\|^ *&& echo/d}' >"$D/secret/hash.sh"
chmod 600 "$D/secret/hash.sh" "$D/secret/other-hash.sh"
run hash hash.sh -e QD_JOIN_NO_START=1 -e QD_JOIN_DIR=/opt/qd-refused
expect "worker.alloy not of the hash the script names: stopped, nothing written" "1 1 no files" \
  "$? $(said hash 'worker.alloy is not this version') $(on '[ -e /opt/qd-refused ] && echo files || echo no files')"

echo "== Started: the agent joins, Alloy pushes"
run run3 w1.sh
expect "it starts" "0 1" "$? $(said run3 'started: worker-7 joins')"
for _ in $(seq 60); do [ "$(sql "select count(*) from machines where name = 'worker-7'")" = 1 ] && break; sleep 2; done
expect "the machine joined with the token the script left: pending" "worker-7|worker|pending" \
  "$(sql "select name || '|' || role || '|' || state from machines")"
c() { echo "data-quality-worker-$1-1"; }
expect "Alloy runs as the agent's user, read-only, its volume prepared" "65532:65532 true running exited:0" \
  "$(on "docker inspect -f '{{.Config.User}} {{.HostConfig.ReadonlyRootfs}} {{.State.Status}}' $(c alloy)") $(on "docker inspect -f '{{.State.Status}}:{{.State.ExitCode}}' $(c alloy-volume)")"
q() { mimir /prometheus/api/v1/query -G --data-urlencode "query=$1"; }
for _ in $(seq 60); do
  [ "$(q 'count({vm="worker-7",job="files",__name__=~"qd_node_.*|qd_worker_.*"})' | python3 -I -c 'import json,sys;r=json.load(sys.stdin)["data"]["result"];print(r[0]["value"][1] if r else 0)')" -ge 6 ] 2>/dev/null && break
  sleep 5
done
mimir /prometheus/api/v1/series -G --data-urlencode 'match[]={vm="worker-7"}' >"$D/series.json"
python3 -I - "$D/series.json" >"$D/series.txt" <<'PY'
import json, re, sys
s = json.load(open(sys.argv[1]))["data"]
names = sorted({x["__name__"] for x in s})
labels = sorted({k for x in s for k in x})
allowed_names = re.compile(r"up|node_textfile_mtime_seconds|node_textfile_scrape_error|process_cpu_seconds_total|process_resident_memory_bytes|qd_worker_slots|qd_worker_jobs_running|qd_worker_jobs_total|qd_worker_job_duration_seconds_(bucket|sum|count)|qd_worker_claim_paused|qd_node_renewals_total|qd_node_certificate_expiry_timestamp_seconds|prometheus_remote_storage_samples_failed_total")
allowed_labels = {"__name__", "job", "instance", "file", "service", "kind", "result", "le", "vm", "role", "component_id", "host", "reason", "remote_name", "url"}
print("count", len(s))
print("jobs", " ".join(sorted({x["job"] for x in s})))
print("roles", " ".join(sorted({x["role"] for x in s})))
print("files", " ".join(sorted({x["file"] for x in s if "file" in x})))
print("services", " ".join(sorted({x["service"] for x in s if "service" in x})))
print("outside_names", " ".join(n for n in names if not allowed_names.fullmatch(n)) or "none")
print("outside_labels", " ".join(l for l in labels if l not in allowed_labels) or "none")
print("node", " ".join(n for n in names if n.startswith("qd_node_")))
print("worker", " ".join(n for n in names if n.startswith("qd_worker_")))
print("names", " ".join(names))
PY
line() { sed -n "s/^$1 //p" "$D/series.txt"; }
echo "   series of worker-7: $(line count); names: $(line names)"
expect "the jobs, role worker" "alloy files|worker" "$(line jobs)|$(line roles)"
expect "both files read, each with its age" "/run/qd-metrics/node-agent.prom /run/qd-metrics/worker.prom" "$(line files)"
expect "the node agent's series (the real agent's file)" "qd_node_certificate_expiry_timestamp_seconds qd_node_renewals_total" "$(line node)"
expect "the worker's series" "qd_worker_job_duration_seconds_bucket qd_worker_job_duration_seconds_count qd_worker_job_duration_seconds_sum qd_worker_jobs_running qd_worker_jobs_total qd_worker_slots" "$(line worker)"
expect "every metric name within worker.alloy's lists (qd_verify_outside dropped)" none "$(line outside_names)"
expect "every label name within them (dataset dropped)" none "$(line outside_labels)"
expect "up is 1 for files and alloy" "1 1" "$(q 'up{vm="worker-7"}' | python3 -I -c 'import json,sys;print(" ".join(x["value"][1] for x in sorted(json.load(sys.stdin)["data"]["result"], key=lambda x: x["metric"]["job"])))')"
expiry=$(q 'qd_node_certificate_expiry_timestamp_seconds{vm="worker-7"}' | python3 -I -c 'import json,sys;r=json.load(sys.stdin)["data"]["result"];print(int(float(r[0]["value"][1])) if r else 0)')
expect "the certificate's expiry is in the future (the agent's certificate in use)" yes "$([ "$expiry" -gt "$(date +%s)" ] && echo yes || echo "no: $expiry")"

echo "== Nothing listens for another machine"
expect "no container of the machine publishes a port" "" "$(on "docker ps -a --format '{{.Ports}}'" | tr -d '\n')"
# listening CONTAINER: the addresses its network namespace listens on (TCP
# LISTEN), but Docker's own resolver on 127.0.0.11 (0B00007F), in every one.
listening() {
  on "docker run --rm --network container:$1 alpine:3.20 cat /proc/net/tcp /proc/net/tcp6" |
    awk '$4 == "0A" && $2 !~ /^0B00007F:/ { print $2 }' | sort -u | paste -sd' '
}
expect "Alloy listens on its loopback alone (127.0.0.1:12345)" "0100007F:3039" "$(listening "$(c alloy)")"
expect "the node agent and the worker listen on nothing" "|" "$(listening "$(c node)")|$(listening "$(c worker)")"
expect "Alloy refused from another container of the machine" refused \
  "$(on "docker run --rm --network data-quality-worker_default alpine:3.20 nc -z -w 3 $(c alloy) 12345" >/dev/null 2>&1 && echo answered || echo refused)"

echo "== The command again: its token used"
stamp=$(state)
run run4 w1.sh
expect "401, the files unchanged" "1 1 $stamp" "$? $(said run4 'unknown, used or expired') $(state)"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
