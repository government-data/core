#!/bin/bash
# Verification of decision 0071 step 1: Mimir in the observability stack as
# the repository's files run it. docker-compose.yml with certificate.yml
# (and the development files without it) rendered by docker compose, then
# run as its own project with mimir/mimir.yml, Grafana's provisioning and
# grafana/tls/entrypoint.sh, and a throwaway CA. Beside it, in a test overlay
# of this script's own: RustFS behind a Traefik requiring a client
# certificate of the CA (the lake's gate, its TLS; its forwardAuth is in
# loki_thanos_lake.sh, which does not look at the bucket).
#
# It shows: setup-storage.sh, run in a copy of the tree against that RustFS,
# making Mimir's bucket and user (and Loki's), again changing nothing, the
# user kept to its bucket; Mimir as the agent's user, ready over HTTPS with
# the machine's certificate and refusing a client without one or with
# another CA's, its gRPC and gossip ports not reached from another
# container, its rings on its loopback; a remote write refused without a
# certificate and taken from Alloy presenting the machine's; a block's
# meta.json in the lake bucket, through the gate, after /ingester/flush;
# the retention as the setting gives it; and Grafana's provisioned
# datasource, with the machine's certificate, answering a PromQL query
# through Grafana's API.
#
# Its own project (qd-verify-metrics-stack: containers, network, volumes),
# Mimir's port published on 127.0.0.1:19009 only; nothing of the dev stack
# is touched. Throwaway CA, keys and secrets, removed at the end; nothing
# secret is printed. Run it alone: it waits for no compiler, no other
# qd-verify- container and a load under 4, and stops above 8.
#
#   bash tools/verify/mimir_stack_cert.sh
#
# Needs docker with Compose v2, curl, openssl and python3.
set -u
export LC_ALL=C
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
P=qd-verify-metrics-stack
PORT=19009
HELPER=alpine:3.20
CURL=curlimages/curl:8.11.1
D=$(mktemp -d)
chmod 755 "$D"
OBS=(-f "$ROOT/infra/observability/docker-compose.yml")
compose() { docker compose --project-directory "$ROOT" --env-file "$D/env" -p $P "${OBS[@]}" \
  -f "$ROOT/infra/observability/certificate.yml" -f "$D/test.yml" "$@"; }
cleanup() {
  docker ps -aq --filter "name=^/$P-" | xargs -r docker rm -f >/dev/null 2>&1
  [ -f "$D/test.yml" ] && compose down -v --remove-orphans >/dev/null 2>&1
  docker volume ls -q --filter "name=^${P}_" | xargs -r docker volume rm >/dev/null 2>&1
  docker network rm "$P-edge" >/dev/null 2>&1
  docker run --rm -v "$D:/d" "$HELPER" rm -rf /d/agent >/dev/null 2>&1
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

busy() {
  pgrep -x rustc >/dev/null && { echo "rustc running"; return 0; }
  pgrep -x cc1plus >/dev/null && { echo "cc1plus running"; return 0; }
  local others
  others=$(docker ps --format '{{.Names}}' | grep '^qd-verify-' | grep -cv "^$P-")
  [ "$others" -gt 0 ] && { echo "$others other qd-verify- containers"; return 0; }
  awk -v l="$(cut -d' ' -f1 /proc/loadavg)" 'BEGIN { exit !(l >= 4) }' && { echo "load $(cut -d' ' -f1 /proc/loadavg)"; return 0; }
  return 1
}
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

# --- Certificates, secrets, the agent's directory --------------------------------
ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/O=qd/CN=qd verify $1" -addext "basicConstraints=critical,CA:TRUE" \
    -keyout "$D/$1.key" -out "$D/$1.crt" 2>/dev/null
}
cert() { # FILE CA SUBJECT DNS: FILE.crt, FILE.key and FILE.pem, both
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$3" \
    -keyout "$D/$1.k" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:%s\n' "$4" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/$2.crt" -CAkey "$D/$2.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  openssl pkey -in "$D/$1.k" -out "$D/$1.key"
  cat "$D/$1.crt" "$D/$1.key" >"$D/$1.pem"
  rm -f "$D/$1.k" "$D/$1.csr"
}
ca ca
ca other
cert app ca "/O=qd/OU=app/CN=qd-app" qd-app
cert stranger other "/O=qd/OU=app/CN=qd-app" qd-app
cert lake ca "/O=qd/OU=lake/CN=lake-1" lake-1
mkdir -p "$D/src" "$D/secrets" "$D/gate"
cp "$D/ca.crt" "$D/app.pem" "$D/src/"
cp "$D/ca.crt" "$D/lake.crt" "$D/lake.key" "$D/gate/"
for s in grafana_admin_password grafana_oidc_client_secret grafana_pg_password grafana_metrics_password; do
  openssl rand -hex 16 >"$D/secrets/$s"
done
chmod 755 "$D"/src "$D"/secrets "$D"/gate && chmod 644 "$D"/src/* "$D"/secrets/* "$D"/gate/*
docker run --rm -v "$D:/d" "$HELPER" sh -c "mkdir -p /d/agent && chmod 700 /d/agent &&
  cp /d/src/ca.crt /d/agent/ca.crt && chmod 644 /d/agent/ca.crt &&
  cp /d/src/app.pem /d/agent/machine.pem && chmod 600 /d/agent/machine.pem && chown -R 65532:65532 /d/agent"
ROOT_KEY=qd$(openssl rand -hex 8)
ROOT_SECRET=$(openssl rand -hex 16)

# --- (a) setup-storage.sh, in a copy of the tree, against a throwaway RustFS -------
cat >"$D/gate/gate.yml" <<'YML'
http:
  routers:
    lake:
      entryPoints: [lake]
      rule: "PathPrefix(`/`)"
      tls: {}
      service: rustfs
  services:
    rustfs:
      loadBalancer:
        servers:
          - url: "http://rustfs:9000"
tls:
  stores:
    default:
      defaultCertificate: {certFile: /g/lake.crt, keyFile: /g/lake.key}
  options:
    default:
      clientAuth:
        caFiles: [/g/ca.crt]
        clientAuthType: RequireAndVerifyClientCert
YML
chmod 644 "$D/gate/gate.yml"
# No restart policy for any of them: a failing one stays down.
cat >"$D/test.yml" <<YML
services:
  mimir:
    restart: "no"
    ports: !override
      - "127.0.0.1:$PORT:9009"
  mimir-volume:
    restart: "no"
  grafana:
    restart: "no"
  rustfs:
    restart: "no"
    image: $(sed -n 's/^ *image: \${RUSTFS_IMAGE:-\(.*\)}$/\1/p' "$ROOT/infra/rustfs/docker-compose.yml")
    command: ["server", "--address", ":9000", "/data"]
    environment:
      RUSTFS_ACCESS_KEY: $ROOT_KEY
      RUSTFS_SECRET_KEY: $ROOT_SECRET
      RUSTFS_VOLUMES: /data
    tmpfs:
      - /data:uid=10001,gid=10001
  gate:
    restart: "no"
    image: traefik:v3.6
    command: ["--entrypoints.lake.address=:9000", "--providers.file.directory=/g", "--log.level=ERROR"]
    volumes:
      - $D/gate:/g:ro
    networks:
      default:
        aliases: [lake-1]
YML
# Compose needs Mimir's keys to start anything; the first start is the lake's.
printf 'MIMIR_S3_ACCESS_KEY=x\nMIMIR_S3_SECRET_KEY=x\nLOKI_S3_ACCESS_KEY=x\nLOKI_S3_SECRET_KEY=x\n' >"$D/env"
cat >>"$D/env" <<ENV
QD_SECRETS_DIR=$D/secrets
QD_MACHINE_DIR=$D/agent
LOKI_SERVER_NAME=qd-app
LOKI_S3_ENDPOINT=lake-1:9000
GRAFANA_PG_HOST=qd-pg
OBS_VM=verify
QD_EDGE_NETWORK=$P-edge
ENV
chmod 600 "$D/env"
# Traefik's network, which certificate.yml has Mimir join: a throwaway one.
docker network create "$P-edge" >/dev/null
compose up -d rustfs gate >"$D/up.log" 2>&1 || { echo "could not start the lake"; tail -5 "$D/up.log"; exit 1; }
mkdir -p "$D/tree/infra/observability"
cp "$ROOT/infra/observability/setup-storage.sh" "$D/tree/infra/observability/"
printf 'RUSTFS_ACCESS_KEY=%s\nRUSTFS_SECRET_KEY=%s\n' "$ROOT_KEY" "$ROOT_SECRET" >"$D/tree/.env"
chmod 600 "$D/tree/.env"
storage() { RUSTFS_CONTAINER=$P-rustfs-1 sh "$D/tree/infra/observability/setup-storage.sh" 2>&1; }
for _ in $(seq 30); do
  docker run --rm --network "container:$P-rustfs-1" "$CURL" -s -o /dev/null -m 2 http://127.0.0.1:9000/ && break
  sleep 1
done
echo "== (a) setup-storage.sh"
out=$(storage)
expect "it makes both buckets, policies and users" \
  "bucket qd-observability: 200|policy qd-observability-loki: 200|user qd-observability-loki: 200|user's policy: 200|bucket qd-metrics: 200|policy qd-metrics-mimir: 200|user qd-metrics-mimir: 200|user's policy: 200" \
  "$(echo "$out" | grep -v 'appended' | tr '\n' '|' | sed 's/|$//')"
expect "Mimir's keys appended to .env, the secret never printed" "1 1 no" \
  "$(grep -c '^MIMIR_S3_ACCESS_KEY=qd-metrics-mimir$' "$D/tree/.env") $(grep -c '^MIMIR_S3_SECRET_KEY=[0-9a-f]\{60\}$' "$D/tree/.env") \
$(echo "$out" | grep -qF "$(sed -n 's/^MIMIR_S3_SECRET_KEY=//p' "$D/tree/.env")" && echo yes || echo no)"
lines=$(wc -l <"$D/tree/.env")
out=$(storage)
expect "run again: every step 200, .env unchanged" "0 $lines" \
  "$(echo "$out" | grep -cv ': 200$') $(wc -l <"$D/tree/.env")"
MKEY=$(sed -n 's/^MIMIR_S3_ACCESS_KEY=//p' "$D/tree/.env")
MSECRET=$(sed -n 's/^MIMIR_S3_SECRET_KEY=//p' "$D/tree/.env")
s3() { # METHOD PATH [KEY SECRET]: through RustFS's own port; the status, the body in $D/s3.out
  docker run --rm --network "${P}_default" "$CURL" -s -o - -w '\n%{http_code}' -X "$1" \
    --aws-sigv4 "aws:amz:us-east-1:s3" --user "${3:-$MKEY}:${4:-$MSECRET}" "http://rustfs:9000$2" >"$D/s3.out" 2>/dev/null
  tail -n1 "$D/s3.out"
}
expect "Mimir's user lists its bucket and not Loki's" "200 403" \
  "$(s3 GET '/qd-metrics?list-type=2') $(s3 GET '/qd-observability?list-type=2')"
{ grep -v '_S3_ACCESS_KEY=\|_S3_SECRET_KEY=' "$D/env"; grep '_S3_ACCESS_KEY=\|_S3_SECRET_KEY=' "$D/tree/.env"; } >"$D/env.new"
mv "$D/env.new" "$D/env" && chmod 600 "$D/env"

# --- (b) The files as Compose renders them -----------------------------------------
render() { docker compose --project-directory "$ROOT" --env-file "$1" -p $P "${OBS[@]}" "${@:2}" config --format json 2>/dev/null; }
echo "== (b) rendered"
render "$D/env" -f "$ROOT/infra/observability/certificate.yml" >"$D/cert.json"
python3 -I - "$D/cert.json" >"$D/cert.txt" <<'PY'
import json, sys
s = json.load(open(sys.argv[1]))["services"]
m, g = s["mimir"], s["grafana"]
c = " ".join(m["command"])
print("mimir", m["user"], "tls" if "-server.http-tls-client-auth=RequireAndVerifyClientCert" in c else "-",
      "s3-cert" if "-common.storage.s3.http.tls-cert-path=/run/qd-node/machine.pem" in c else "-",
      m["environment"]["MIMIR_S3_ENDPOINT"], m["environment"]["MIMIR_S3_INSECURE"], m["environment"]["MIMIR_RETENTION"],
      m["depends_on"]["mimir-volume"]["condition"], any(v["target"] == "/run/qd-node" and v.get("read_only") for v in m["volumes"]))
print("grafana", g["environment"]["MIMIR_URL"], g["environment"]["MIMIR_SERVER_NAME"])
PY
expect "Mimir: the agent's user, client certificates required, the S3 client's certificate, Loki's gate, its retention, after the volume, the agent's files" \
  "mimir 65532:65532 tls s3-cert lake-1:9000 false 395d service_completed_successfully True" "$(sed -n 1p "$D/cert.txt")"
expect "Grafana: Mimir by its service name over HTTPS, checked for the machine's name" \
  "grafana https://mimir:9009 qd-app" "$(sed -n 2p "$D/cert.txt")"
render "$D/env" -f "$ROOT/infra/observability/compose.dev.yml" >"$D/dev.json"
expect "development: Mimir in Traefik's namespace over plain HTTP, Grafana at 127.0.0.1:9009" \
  "container:data-quality-traefik-1 127.0.0.1:9000 true None http://127.0.0.1:9009" "$(python3 -I -c '
import json, sys
s = json.load(open(sys.argv[1]))["services"]
m = s["mimir"]
print(m["network_mode"], m["environment"]["MIMIR_S3_ENDPOINT"], m["environment"]["MIMIR_S3_INSECURE"], m.get("user"),
      s["grafana"]["environment"]["MIMIR_URL"])' "$D/dev.json")"
expect "the retention is one setting (MIMIR_RETENTION)" 180d "$( (cat "$D/env"; echo MIMIR_RETENTION=180d) >"$D/env.ret"
  render "$D/env.ret" | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["services"]["mimir"]["environment"]["MIMIR_RETENTION"])')"

# --- (c) Mimir --------------------------------------------------------------------
compose up -d mimir grafana >"$D/up.log" 2>&1 || { echo "could not start Mimir and Grafana"; tail -5 "$D/up.log"; exit 1; }
MIMIR_C=$P-mimir-1 GRAFANA_C=$P-grafana-1
at() { # PEM PATH [CURL OPTION...]: Mimir's status over HTTPS presenting PEM (none: no certificate)
  local pem=$1 path=$2 c=()
  shift 2
  [ "$pem" != none ] && c=(--cert "$D/$pem.pem")
  curl -sS -o "$D/at.out" -w '%{http_code}' --max-time 10 --cacert "$D/ca.crt" \
    --resolve "qd-app:$PORT:127.0.0.1" "${c[@]}" "$@" "https://qd-app:$PORT$path" 2>"$D/at.err"
}
for _ in $(seq 120); do [ "$(at app /ready)" = 200 ] && break; sleep 1; done
echo "== (c) Mimir"
expect "ready over HTTPS, the machine's certificate presented" 200 "$(at app /ready)"
[ "$(at app /ready)" = 200 ] || note "Mimir's last lines: $(docker logs "$MIMIR_C" 2>&1 | tail -3 | cut -c1-300)"
expect "Mimir runs as the agent's user" 65532 "$(docker top "$MIMIR_C" | awk 'NR == 2 { print $1 }')"
expect "no client certificate: no handshake" "000 yes" "$(at none /ready) $(grep -q 'alert certificate required' "$D/at.err" && echo yes || echo no)"
expect "another CA's certificate: no handshake" "000 yes" "$(at stranger /ready) $(grep -q 'alert unknown ca' "$D/at.err" && echo yes || echo no)"
MIP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$MIMIR_C")
reach() { # PORT: whether a TCP connection from another container is taken (curl's 7: refused)
  docker run --rm --network "${P}_default" "$CURL" -s -o /dev/null -m 3 "http://$MIP:$1/" >/dev/null 2>&1
  case $? in 7 | 28) echo closed ;; *) echo open ;; esac
}
expect "from another container: 9009 open, gRPC (9096) and gossip (7946) closed" "open closed closed" \
  "$(reach 9009) $(reach 9096) $(reach 7946)"
at app /store-gateway/ring >/dev/null
expect "its rings on its loopback, not the container's address" "yes no" \
  "$(grep -q '127.0.0.1:9096' "$D/at.out" && echo yes || echo no) $(grep -q "$MIP" "$D/at.out" && echo yes || echo no)"
at app /config >/dev/null
expect "the retention as the setting gives it, in days" 395 "$(python3 -I -c '
import re, sys
v = re.search(r"compactor_blocks_retention_period: *(\S+)", open(sys.argv[1]).read()).group(1)
units = {"y": 365 * 24, "w": 7 * 24, "d": 24, "h": 1}
hours = sum(int(n) * units[u] for n, u in re.findall(r"(\d+)([ywdh])", v))
print(hours // 24 if hours else v)' "$D/at.out" 2>/dev/null)"
expect "a push without a client certificate: no handshake" 000 "$(at none /api/v1/push -X POST)"
code=$(at app /api/v1/push -X POST)
expect "a push with the machine's certificate reaches the handler (an empty body refused there, 4xx)" 4xx "${code%??}xx"

# --- (d) Alloy's remote write, a flush, the block in the lake ------------------------
cat >"$D/push.alloy" <<'ALLOY'
logging { level = "warn" }
prometheus.exporter.self "alloy" { }
prometheus.scrape "alloy" {
  targets         = prometheus.exporter.self.alloy.targets
  scrape_interval = "5s"
  scrape_timeout  = "4s"
  forward_to      = [prometheus.remote_write.mimir.receiver]
}
prometheus.remote_write "mimir" {
  external_labels = { vm = "qd-verify" }
  endpoint {
    url = "https://qd-app:9009/api/v1/push"
    tls_config {
      ca_file   = "/run/qd-node/ca.crt"
      cert_file = "/run/qd-node/machine.pem"
      key_file  = "/run/qd-node/machine.pem"
    }
  }
}
ALLOY
chmod 644 "$D/push.alloy"
ALLOY=$(python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1]))["services"]["alloy"]["image"])' "$D/cert.json")
docker run -d --name "$P-alloy" --network "${P}_default" --add-host "qd-app:$MIP" \
  -v "$D/agent:/run/qd-node:ro" -v "$D/push.alloy:/etc/alloy/push.alloy:ro" \
  "$ALLOY" run --storage.path=/tmp/alloy --disable-reporting /etc/alloy/push.alloy >/dev/null
echo "== (d) remote write and the lake"
series() { # PROMQL: how many series an instant query returns, asked of Mimir directly
  at app /prometheus/api/v1/query -G --data-urlencode "query=$1" >/dev/null
  grep -o '"metric":' "$D/at.out" | wc -l
}
n=0
for _ in $(seq 40); do n=$(series 'alloy_build_info{vm="qd-verify"}'); [ "$n" -gt 0 ] && break; sleep 3; done
expect "Alloy's remote write, presenting the machine's certificate, taken" 1 "$n"
note "flush the ingester: $(at app /ingester/flush -X POST)"
blocks=0
for _ in $(seq 40); do
  s3 GET "/qd-metrics?list-type=2&prefix=blocks/" >/dev/null
  blocks=$(grep -o '<Key>[^<]*meta.json' "$D/s3.out" | wc -l)
  [ "$blocks" -gt 0 ] && break
  sleep 3
done
expect "a block's meta.json in qd-metrics, written through the gate with the machine's certificate" yes \
  "$([ "$blocks" -gt 0 ] && echo yes || echo no)"
s3 GET "/qd-metrics?list-type=2" >/dev/null
note "the bucket: $(grep -o '<Key>' "$D/s3.out" | wc -l) objects, $(grep -o '<Size>[0-9]*' "$D/s3.out" | sed 's/<Size>//' | awk '{ s += $1 } END { print s + 0 }') bytes; Mimir's memory: $(docker stats --no-stream --format '{{.MemUsage}}' "$MIMIR_C")"

# --- (e) Grafana's datasource ------------------------------------------------------
echo "== (e) Grafana"
GPW=$(cat "$D/secrets/grafana_admin_password")
gf() { # PATH [WGET OPTION...]
  local path=$1
  shift
  docker exec "$GRAFANA_C" wget -qO- --header "Authorization: Basic $(printf 'qd-emergency-admin:%s' "$GPW" | base64 -w0)" \
    "$@" "http://127.0.0.1:3000$path" 2>/dev/null
}
for _ in $(seq 90); do [ "$(docker inspect -f '{{.State.Health.Status}}' "$GRAFANA_C")" = healthy ] && break; sleep 2; done
expect "healthy, by the compose file's own check" healthy "$(docker inspect -f '{{.State.Health.Status}}' "$GRAFANA_C")"
expect "the Mimir datasource: Prometheus type, Mimir's API, TLS with the machine's certificate, checked for qd-app" \
  "prometheus https://mimir:9009/prometheus true true qd-app" "$(gf /api/datasources/uid/mimir | python3 -I -c '
import json, sys
d = json.load(sys.stdin); j = d["jsonData"]
print(d["type"], d["url"], str(j.get("tlsAuth")).lower(), str(d["secureJsonFields"].get("tlsClientKey")).lower(), j.get("serverName"))')"
health=none
for _ in $(seq 30); do
  health=$(gf /api/datasources/uid/mimir/health | python3 -I -c 'import json, sys; print(json.load(sys.stdin).get("status"))' 2>/dev/null)
  [ "$health" = OK ] && break
  sleep 2
done
expect "its health check through Grafana" OK "$health"
q='{"queries":[{"refId":"A","datasource":{"uid":"mimir"},"expr":"alloy_build_info{vm=\"qd-verify\"}","instant":true}],"from":"now-10m","to":"now"}'
expect "a PromQL query through Grafana's API (/api/ds/query) answers Alloy's series" "200 1" \
  "$(gf /api/ds/query --header 'Content-Type: application/json' --post-data "$q" | python3 -I -c '
import json, sys
r = json.load(sys.stdin)["results"]["A"]
print(r.get("status"), sum(1 for f in r.get("frames", []) if f.get("data", {}).get("values")))' 2>/dev/null)"

echo "   Mimir $(docker run --rm --name "$P-version" "$(python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1]))["services"]["mimir"]["image"])' "$D/cert.json")" \
  -version 2>/dev/null | sed -n 's/^Mimir, version \([^ ]*\).*/\1/p'), Alloy, Grafana and RustFS as the compose files name them; Compose $(docker compose version --short)"
echo "$PASSED passed, $FAILED failed"
[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $((FAILED > 0))
