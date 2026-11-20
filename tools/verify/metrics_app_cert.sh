#!/bin/bash
# Verification of decision 0071 step 2: Alloy collecting the application
# VM's services and pushing them to Mimir, with the repository's own files
# (a copy of infra/, every file readable): the observability stack with
# certificate.yml, alloy/app.alloy and app-postgres.alloy; PostgreSQL with
# tls.yml and its initdb (the metrics' role); Dolt with certificate.yml;
# Traefik with traefik.yml, every dynamic file, internal.yml and
# certificate.yml's entrypoint. A throwaway CA, and the application VM's
# machine certificate (qd-app) in an agent directory written as the agent
# writes it. Beside them, in a test overlay of this script's own: the lake
# (RustFS behind a gate requiring a client certificate, as
# mimir_stack_cert.sh), a TCP pass-through standing for the VM's published
# ports (LOKI_ADDRESS), and a fake target.
#
# It shows: each service's series in Mimir, `up` 1, with exactly the
# allowed metric and label names; a forbidden label stripped and a metric
# outside the lists dropped (the fake, as Grafana); a scrape over the
# sample limit or the label limit failing whole; Traefik's metrics entry
# point refusing a client without the CA's certificate and plain HTTP, and
# Grafana's /metrics refusing one without its basic auth; PostgreSQL's
# metrics' role refused without the certificate and outside `postgres`;
# development's plain router answering only its loopback; and the logs
# still reaching Loki.
#
# Containers, networks and volumes qd-verify-metrics-app…, restart "no";
# Mimir on 127.0.0.1:19009 and Loki on 127.0.0.1:13100 only; nothing of the
# dev stack is touched (Alloy reads the Docker socket, collecting only this
# project's logs). Nothing secret is printed. Run it alone: it waits for no
# compiler, no other qd-verify- container and a load under 4, and stops
# above 8.
#
#   bash tools/verify/metrics_app_cert.sh
#
# Needs docker with Compose v2, curl, openssl and python3.
set -u
export LC_ALL=C
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
P=qd-verify-metrics-app
NET=10.213.79
HELPER=alpine:3.20
CURL=curlimages/curl:8.11.1
PY=python:3.12-slim
TRAEFIK=traefik:v3.6
D=$(mktemp -d)
chmod 755 "$D"
T=$D/tree
OBS=(-f "$T/infra/observability/docker-compose.yml" -f "$T/infra/observability/certificate.yml")
obs() { docker compose --project-directory "$T" --env-file "$D/env" -p $P "${OBS[@]}" -f "$D/test.yml" "$@"; }
pg() { docker compose --env-file "$D/env" -p $P-pg -f "$T/infra/postgres/docker-compose.yml" \
  -f "$T/infra/postgres/tls.yml" -f "$D/pg.yml" "$@"; }
dolt() { docker compose --project-directory "$T" --env-file "$D/env" -p $P-dolt \
  -f "$T/infra/dolt/docker-compose.yml" -f "$T/infra/dolt/certificate.yml" -f "$D/dolt.yml" "$@"; }
cleanup() {
  docker ps -aq --filter "name=^/$P" | xargs -r docker rm -f >/dev/null 2>&1
  if [ -f "$D/test.yml" ]; then
    obs down -v --remove-orphans >/dev/null 2>&1
    pg down -v --remove-orphans >/dev/null 2>&1
    dolt down -v --remove-orphans >/dev/null 2>&1
  fi
  docker volume ls -q --filter "name=^$P" | xargs -r docker volume rm >/dev/null 2>&1
  docker network ls -q --filter "name=^$P" | xargs -r docker network rm >/dev/null 2>&1
  docker run --rm -v "$D:/d" "$HELPER" rm -rf /d/agent /d/dyn >/dev/null 2>&1
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

# --- The tree, certificates, secrets, the agent's directory --------------------------
mkdir -p "$T"
cp -r "$ROOT/infra" "$T/infra"
chmod -R a+rX "$T"
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
cert worker ca "/O=qd/OU=worker/CN=worker-1" worker-1
cert lake ca "/O=qd/OU=lake/CN=lake-1" lake-1
mkdir -p "$D/src" "$D/secrets" "$D/gate" "$D/c" "$D/vm" "$D/www"
cp "$D/ca.crt" "$D/app.pem" "$D/src/"
cp "$D/ca.crt" "$D/lake.crt" "$D/lake.key" "$D/gate/"
cp "$D/ca.crt" "$D/app.pem" "$D/app.crt" "$D/app.key" "$D/stranger.pem" "$D/worker.crt" "$D/worker.key" "$D/c/"
for s in grafana_admin_password grafana_oidc_client_secret grafana_pg_password grafana_metrics_password; do
  openssl rand -hex 16 >"$D/secrets/$s"
done
chmod 755 "$D"/src "$D"/secrets "$D"/gate "$D"/c "$D"/vm "$D"/www
chmod 644 "$D"/src/* "$D"/secrets/* "$D"/gate/* "$D"/c/*
docker run --rm -v "$D:/d" "$HELPER" sh -c "mkdir -p /d/agent && chmod 700 /d/agent &&
  cp /d/src/ca.crt /d/agent/ca.crt && chmod 644 /d/agent/ca.crt &&
  cp /d/src/app.pem /d/agent/machine.pem && chmod 600 /d/agent/machine.pem && chown -R 65532:65532 /d/agent"
ROOT_KEY=qd$(openssl rand -hex 8)
ROOT_SECRET=$(openssl rand -hex 16)

# --- The test overlays: the lake, the VM's ports, the fake target ----------------------
. "$ROOT/tools/verify/metrics_app_overlays.sh"
docker network create "$P-edge" >/dev/null

# --- (a) The files as Compose renders them ---------------------------------------------
echo "== (a) rendered"
render() { docker compose --project-directory "$T" --env-file "$D/env" -p $P "$@" config --format json 2>/dev/null; }
render -f "$T/infra/observability/docker-compose.yml" -f "$T/infra/observability/certificate.yml" >"$D/cert.json"
render -f "$T/infra/observability/docker-compose.yml" -f "$T/infra/observability/compose.dev.yml" >"$D/dev.json"
python3 -I - "$D/cert.json" "$D/dev.json" >"$D/render.txt" <<'PY'
import json, sys
for f in sys.argv[1:]:
    s = json.load(open(f))["services"]
    a, g = s["alloy"], s["grafana"]
    e = a["environment"]
    files = sorted(v["target"].rsplit("/", 1)[1] for v in a["volumes"] if v["target"].startswith("/etc/qd-alloy/"))
    print(a["command"][-1], ",".join(files), e.get("METRICS_SCHEME", "-"), e["METRICS_ROUTER_ADDRESS"],
          e["METRICS_PUSH_URL"], "dsn" if "sslmode=verify-full" in e.get("METRICS_POSTGRES_DSN", "") else "-",
          ",".join(a.get("secrets") and [x["source"] for x in a["secrets"]] or []),
          g["environment"]["GF_METRICS_BASIC_AUTH_USERNAME"], g["environment"]["GF_METRICS_BASIC_AUTH_PASSWORD__FILE"])
PY
expect "installation: Alloy loads app.alloy and app-postgres.alloy beside config.alloy, over HTTPS, the VM's name, the verify-full DSN, Grafana's metrics password" \
  "/etc/qd-alloy app-postgres.alloy,app.alloy,config.alloy https qd-app:9445 https://qd-app:9009/api/v1/push dsn grafana_metrics_password qd-metrics /run/secrets/grafana_metrics_password" \
  "$(sed -n 1p "$D/render.txt")"
expect "development: app.alloy and dev-host.alloy, no PostgreSQL, plain HTTP on Traefik's loopback" \
  "/etc/qd-alloy app.alloy,config.alloy,dev-host.alloy - 127.0.0.1:9445 http://127.0.0.1:9009/api/v1/push - grafana_metrics_password qd-metrics /run/secrets/grafana_metrics_password" \
  "$(sed -n 2p "$D/render.txt")"
expect "Traefik's certificate.yml publishes the metrics entry point" "0.0.0.0:9445->9445" "$(ACME_EMAIL=v@example.com QD_MACHINE_DIR=$D/agent \
  docker compose -f "$T/infra/traefik/docker-compose.yml" -f "$T/infra/traefik/certificate.yml" config --format json 2>/dev/null |
  python3 -I -c 'import json, sys
for p in json.load(sys.stdin)["services"]["traefik"]["ports"]:
    if p["target"] == 9445: print("%s:%s->%s" % (p.get("host_ip", ""), p["published"], p["target"]))')"
expect "Dolt's certificate.yml: on Traefik's network, its metrics port published nowhere" "$P-edge no" \
  "$(dolt config --format json 2>/dev/null | python3 -I -c 'import json, sys
c = json.load(sys.stdin)
d = c["services"]["dolt"]
print(c["networks"]["edge"]["name"], "yes" if any(p["target"] == 11228 for p in d.get("ports", [])) else "no")')"

# --- (b) Up: the lake and its buckets, PostgreSQL, Dolt, Traefik, the stack ------------
echo "== (b) starting"
pg up -d postgres >"$D/up.log" 2>&1 || { echo "could not start PostgreSQL"; tail -5 "$D/up.log"; exit 1; }
dolt up -d dolt >"$D/up.log" 2>&1 || { echo "could not start Dolt"; tail -5 "$D/up.log"; exit 1; }
obs up -d rustfs gate >"$D/up.log" 2>&1 || { echo "could not start the lake"; tail -5 "$D/up.log"; exit 1; }
for _ in $(seq 30); do
  docker run --rm --network "container:$P-rustfs-1" "$CURL" -s -o /dev/null -m 2 http://127.0.0.1:9000/ && break
  sleep 1
done
mkdir -p "$D/st/infra/observability"
cp "$T/infra/observability/setup-storage.sh" "$D/st/infra/observability/"
printf 'RUSTFS_ACCESS_KEY=%s\nRUSTFS_SECRET_KEY=%s\n' "$ROOT_KEY" "$ROOT_SECRET" >"$D/st/.env"
chmod 600 "$D/st/.env"
RUSTFS_CONTAINER=$P-rustfs-1 sh "$D/st/infra/observability/setup-storage.sh" >"$D/storage.log" 2>&1
expect "setup-storage.sh made the buckets and users" 0 "$(grep -cv ': 200$\|appended' "$D/storage.log")"
{ grep -v '_S3_ACCESS_KEY=\|_S3_SECRET_KEY=' "$D/env"; grep '_S3_ACCESS_KEY=\|_S3_SECRET_KEY=' "$D/st/.env"; } >"$D/env.new"
mv "$D/env.new" "$D/env" && chmod 600 "$D/env"
traefik_run "$P-traefik" "" -e GRAFANA_URL=http://grafana:3000
docker network connect "${P}_default" "$P-traefik"
traefik_run "$P-traefik-dev" plain
for _ in $(seq 60); do [ "$(docker inspect -f '{{.State.Health.Status}}' "$P-pg-postgres-1" 2>/dev/null)" = healthy ] && break; sleep 2; done
obs up -d grafana loki mimir vm fake alloy >"$D/up.log" 2>&1 || { echo "could not start the stack"; tail -5 "$D/up.log"; exit 1; }

# --- (c) The hops: who is refused -------------------------------------------------------
echo "== (c) the hops"
via() { # CERT PATH [CURL OPTION...]: through the VM's port 9445, the status (000: no handshake)
  local cert=$1 path=$2 c=()
  shift 2
  [ "$cert" != none ] && c=(--cert "/c/$cert.pem")
  docker run --rm --network "${P}_default" -v "$D/c:/c:ro" "$CURL" -s -o /dev/null -w '%{http_code}' -m 10 \
    --cacert /c/ca.crt --resolve "qd-app:9445:$NET.10" "${c[@]}" "$@" "https://qd-app:9445$path" 2>/dev/null
}
for _ in $(seq 60); do [ "$(via app /metrics)" = 200 ] && break; sleep 2; done
expect "Traefik's metrics with the machine's certificate" 200 "$(via app /metrics)"
expect "without a certificate, or with another CA's: no handshake" "000 000" "$(via none /metrics) $(via stranger /metrics)"
expect "plain HTTP on the metrics entry point: no router (development's is off here)" 404 \
  "$(docker run --rm --network "${P}_default" "$CURL" -s -o /dev/null -w '%{http_code}' -m 10 "http://$NET.10:9445/metrics")"
for _ in $(seq 60); do [ "$(via app /dolt/metrics)" = 200 ] && break; sleep 2; done
expect "Dolt's metrics through the router, Dolt's certificate verified against the CA" 200 "$(via app /dolt/metrics)"
for _ in $(seq 60); do [ "$(via app /grafana/metrics)" = 401 ] && break; sleep 2; done
expect "Grafana's metrics through the router: 401 without its basic auth, 200 with it" "401 200" \
  "$(via app /grafana/metrics) $(via app /grafana/metrics -u "qd-metrics:$(cat "$D/secrets/grafana_metrics_password")")"
expect "Grafana's /grafana/metrics on its own port (the public route's upstream) without it: 401" 401 \
  "$(docker run --rm --network "${P}_default" "$CURL" -s -o /dev/null -w '%{http_code}' -m 10 http://grafana:3000/grafana/metrics)"
dev() { docker run --rm --network "container:$P-traefik-dev" "$CURL" -s -o /dev/null -w '%{http_code}' -m 10 "http://127.0.0.1:9445$1"; }
expect "development's router: Traefik's metrics on its loopback, 404 from another container" "200 404" \
  "$(dev /metrics) $(docker run --rm --network "$P-edge" "$CURL" -s -o /dev/null -w '%{http_code}' -m 10 "http://$P-traefik-dev:9445/metrics")"
expect "no error from Traefik about the metrics routers or metrics-dev.yml" 0 \
  "$(docker logs "$P-traefik" 2>&1 | grep '"level":"error"' | grep -c 'metrics')"
note "Traefik's other errors: $(docker logs "$P-traefik" 2>&1 | grep '"level":"error"' | grep -v metrics | sed 's/.*"error":"\([^"]*\)".*/\1/' | sort | uniq -c | head -3 | tr -s ' \n' ' ')(no Docker provider here)"
psql_as() { # CERT DB: qd_metrics over TLS through the VM's port, the first line of the answer
  docker run --rm --network "${P}_default" -v "$D/c:/c:ro" --add-host "qd-app:$NET.10" --entrypoint sh postgres:16-alpine -c "
    cp /c/* /tmp/ && chmod 600 /tmp/*.key && psql 'host=qd-app port=5432 user=qd_metrics dbname=$2 sslmode=verify-full sslrootcert=/tmp/ca.crt $1' -wXAtc 'select 1' 2>&1" | head -1 | sed 's/.*failed: //' | cut -c1-120
}
expect "PostgreSQL: qd_metrics with the machine's certificate" 1 "$(psql_as 'sslcert=/tmp/app.crt sslkey=/tmp/app.key' postgres)"
expect "qd_metrics without a certificate: refused" yes "$(psql_as '' postgres | grep -q 'FATAL\|certificate' && echo yes || echo no)"
expect "qd_metrics with a worker's certificate: refused" yes "$(psql_as 'sslcert=/tmp/worker.crt sslkey=/tmp/worker.key' postgres | grep -q FATAL && echo yes || echo no)"
r=$(psql_as 'sslcert=/tmp/app.crt sslkey=/tmp/app.key' knime)
expect "qd_metrics to the application's database: refused" yes "$(echo "$r" | grep -q 'FATAL\|no password' && echo yes || echo "no: $r")"

. "$ROOT/tools/verify/metrics_app_series.sh"

echo "   Alloy, Mimir, Loki, Grafana, Dolt, PostgreSQL and RustFS as the compose files name them; $TRAEFIK; Compose $(docker compose version --short)"
echo "$PASSED passed, $FAILED failed"
[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $((FAILED > 0))
