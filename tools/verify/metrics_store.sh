#!/bin/bash
# Verification for the proposed decision on application metrics over time
# (0071): which of the installation's services expose Prometheus metrics in
# the versions this repository pins, on what port and path, with which
# labels; whether each can be served or scraped over TLS with a client
# certificate of an installation-like CA; whether Alloy v1.21.3 scrapes
# them (and a text file, and PostgreSQL through its embedded exporter) and
# forwards them; and whether the two stores weighed, Grafana Mimir in
# monolithic mode with its blocks in the lake and Prometheus as a
# remote-write receiver, take what Alloy sends over mutual TLS, and what
# each keeps on disk or in the bucket.
#
# Everything is throwaway, named qd-verify-metrics-…, with a throwaway CA and
# random root keys that only start the containers. Nothing of the dev stack
# is touched. The groups run one at a time, a few containers each, and each
# waits for a quiet machine (no rustc or cc1plus, no other qd-verify-
# container, load under 4); a load above 8 stops the run.
#
#   bash tools/verify/metrics_store.sh [probe|endpoints|tls|postgres|trino|mimir|prometheus]...
#
# With no group, all of them in that order. Needs docker, curl (SigV4) and
# openssl.
set -u
export LC_ALL=C
P=qd-verify-metrics
NET=$P-net
ALLOY=${ALLOY_IMAGE:-grafana/alloy:v1.21.3}
LOKI=${LOKI_IMAGE:-grafana/loki:3.7.9}
GRAFANA=${GRAFANA_IMAGE:-grafana/grafana:13.2.5}
TRAEFIK=${TRAEFIK_IMAGE:-traefik:v3.6}
DOLT=${DOLT_IMAGE:-dolthub/dolt-sql-server:2.6.1}
RUSTFS=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
TRINO=${TRINO_IMAGE:-trinodb/trino:484}
POSTGRES=${POSTGRES_IMAGE:-postgres:16-alpine}
MIMIR=${MIMIR_IMAGE:-grafana/mimir:3.2.4}
PROMETHEUS=${PROMETHEUS_IMAGE:-prom/prometheus:v3.15.1}
D=$(mktemp -d)
chmod 755 "$D"
FAILED=0
PASSED=0

cleanup() {
  docker ps -aq --filter "name=^/$P-" | xargs -r docker rm -f >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
  docker volume ls -q --filter "name=^$P-" | xargs -r docker volume rm >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

ok() { printf '[OK  ] %s\n' "$*"; PASSED=$((PASSED + 1)); }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=$((FAILED + 1)); }
note() { printf '[NOTE] %s\n' "$*"; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

# --- A quiet machine -------------------------------------------------------------
busy() {
  pgrep -x rustc >/dev/null && { echo "rustc running"; return 0; }
  pgrep -x cc1plus >/dev/null && { echo "cc1plus running"; return 0; }
  local others
  others=$(docker ps --format '{{.Names}}' | grep '^qd-verify-' | grep -cv "^$P-")
  [ "$others" -gt 0 ] && { echo "$others other qd-verify- containers"; return 0; }
  local load
  load=$(cut -d' ' -f1 /proc/loadavg)
  awk -v l="$load" 'BEGIN { exit !(l >= 4) }' && { echo "load $load"; return 0; }
  return 1
}
wait_quiet() {
  local why
  while why=$(busy); do echo "[wait] $why"; sleep 60; done
}
# Stops the run when the load passes 8 while our containers run.
watchdog() {
  while sleep 10; do
    awk -v l="$(cut -d' ' -f1 /proc/loadavg)" 'BEGIN { exit !(l > 8) }' || continue
    echo "[STOP] load above 8: removing qd-verify-metrics- containers"
    docker ps -aq --filter "name=^/$P-" | xargs -r docker rm -f >/dev/null 2>&1
    kill -TERM $$
    return
  done
}
end_group() { docker ps -aq --filter "name=^/$P-" | xargs -r docker rm -f >/dev/null 2>&1; }

ip() { docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$P-$1"; }
run() { # NAME ARGS...: a container on our network, its alias NAME, no restart policy
  local n=$1
  shift
  docker run -d --name "$P-$n" --network "$NET" --network-alias "$n" "$@" >/dev/null ||
    { ko "could not start $n"; return 1; }
}
# get URL [CURL OPTION...]: the status, the body in $D/body; 000 and the TLS
# alert when the handshake fails.
get() {
  local url=$1 code
  shift
  code=$(curl -sS -o "$D/body" -w '%{http_code}' --max-time 10 "$@" "$url" 2>"$D/err")
  if [ "$code" = 000 ]; then
    local why
    why=$(grep -o 'alert [a-z ]*\|Connection refused\|timed out\|reset by peer' "$D/err" | head -1)
    echo "000 ${why:-$(head -c 90 "$D/err" | tr '\n' ' ')}"
  else echo "$code"; fi
}
# tls NAME PORT PATH CERT: GET https://NAME:PORT/PATH at the container's IP,
# the server verified against our CA; CERT machine, stranger or -.
tls() {
  local args=()
  [ "$4" = - ] || args=(--cert "$D/$4.crt" --key "$D/$4.key")
  get "https://$1:$2$3" --cacert "$D/ca.crt" --resolve "$1:$2:$(ip "$1")" "${args[@]}" "${@:5}"
}
wait_for() { # SECONDS COMMAND...: until COMMAND prints 200
  local i
  for i in $(seq "$1"); do
    [ "$("${@:2}" | cut -c1-3)" = 200 ] && return 0
    sleep 1
  done
  return 1
}
# describe WHAT: series, metric names and label names of the body.
describe() {
  local series names labels
  series=$(grep -cv '^#' "$D/body")
  names=$(grep -v '^#' "$D/body" | sed 's/[{ ].*//' | sort -u | wc -l)
  labels=$(grep -v '^#' "$D/body" | grep -o '[{,][a-zA-Z_][a-zA-Z0-9_]*="' | tr -d '{,="' | sort -u | tr '\n' ' ')
  note "$1: $series series, $names metric names; labels: $labels"
  note "$1: process_cpu_seconds_total $(grep -c '^process_cpu_seconds_total' "$D/body"), process_resident_memory_bytes $(grep -c '^process_resident_memory_bytes' "$D/body")"
}

# --- Certificates ----------------------------------------------------------------
ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/CN=qd verify metrics $1" -addext "basicConstraints=critical,CA:TRUE" \
    -keyout "$D/$1.key" -out "$D/$1.crt" 2>/dev/null
}
# One machine's certificate, both usages, as the node agent's machine.pem
# (0066): every service of the throwaway "machine" presents it, and Alloy
# presents it as a client. Its CN is a PostgreSQL role for the exporter.
cert() { # FILE CA CN
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/O=qd/OU=app/CN=$3" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=%s\n' \
    "DNS:traefik,DNS:dolt,DNS:rustfs,DNS:grafana,DNS:loki,DNS:alloy,DNS:postgres,DNS:trino,DNS:mimir,DNS:prometheus" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/$2.crt" -CAkey "$D/$2.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  cat "$D/$1.crt" "$D/$1.key" >"$D/$1.pem"
  chmod 644 "$D/$1.key" "$D/$1.pem" # throwaway: readable by every image's user
}
ca ca
ca other
cert machine ca qd_metrics
cert stranger other qd_metrics
C=$D/certs
mkdir -p "$C"
cp "$D/ca.crt" "$D/machine.crt" "$D/machine.key" "$D/machine.pem" "$C/"
chmod 755 "$C"
chmod 644 "$C"/* # throwaway: readable by every image's user (Grafana 472, Prometheus 65534)

network() { docker network inspect "$NET" >/dev/null 2>&1 || docker network create "$NET" >/dev/null; }

# --- probe: what the binaries offer, nothing started -------------------------------
group_probe() {
  echo "== probe: flags and settings in the pinned binaries"
  docker create --name "$P-probe-dolt" "$DOLT" >/dev/null &&
    docker cp "$P-probe-dolt:/usr/local/bin/dolt" "$D/dolt" >/dev/null 2>&1
  if [ -f "$D/dolt" ]; then
    note "dolt 2.6.1 metrics settings (yaml tags near metrics): $(grep -ao 'yaml:"[a-z_]*' "$D/dolt" |
      sed 's/yaml:"//' | sort -u | grep -E '^(metrics|labels|host|port|tls_cert|tls_key|tls_ca|ca_cert|jwks|jwt_required_for_localhost|require_client_cert)$' | tr '\n' ' ')"
  else ko "dolt binary not found in the image"; fi
  docker create --name "$P-probe-rustfs" "$RUSTFS" >/dev/null &&
    docker cp "$P-probe-rustfs:/usr/bin/rustfs" "$D/rustfs" >/dev/null 2>&1
  if [ -f "$D/rustfs" ]; then
    note "rustfs 1.0.5 metrics variables: $(grep -ao 'RUSTFS_OBS_[A-Z_]*' "$D/rustfs" | grep -oE 'RUSTFS_OBS_(METRIC_ENDPOINT|ENDPOINT_METRICS_HEADERS|METRICS_EXPORT_ENABLED|METER_INTERVAL|TLS_CA_FILE|TLS_CERT_FILE|TLS_KEY_FILE|ENDPOINT)' | sort -u | tr '\n' ' ')"
  note "rustfs 1.0.5 OTLP client certificate variables: $(grep -aoE 'RUSTFS_OBS_TLS_(CERT|KEY|CLIENT)[A-Z_]*' "$D/rustfs" | sort -u | tr '\n' ' ')none other"
    note "rustfs 1.0.5 metrics paths in the binary: $(grep -ao '/rustfs/v[0-9]/metrics[a-z/]*\|/minio/v[0-9]/metrics[a-z/]*\|/metrics/v3[a-z/]*' "$D/rustfs" | sort -u | tr '\n' ' ')"
  else ko "rustfs binary not found at /usr/bin/rustfs"; fi
  rm -f "$D/dolt" "$D/rustfs"
  end_group
  note "mimir 3.2.4 TLS and storage flags: $(docker run --rm --name "$P-probe-mimir" "$MIMIR" -help-all 2>&1 |
    grep -oE '^ +-(server\.http-tls[a-z-]*|common\.storage\.s3\.http\.tls[a-z-]*|blocks-storage\.s3\.http\.tls[a-z-]*|ingest-storage\.enabled|blocks-storage\.tsdb\.retention-period|compactor\.blocks-retention-period)' |
    tr -d ' ' | sort -u | tr '\n' ' ')"
  note "prometheus v3.15.1 flags: $(docker run --rm --name "$P-probe-prom" "$PROMETHEUS" --help-long 2>&1 |
    grep -oE -- '--(web\.config\.file|web\.enable-remote-write-receiver|web\.enable-admin-api|storage\.tsdb\.retention\.[a-z]*|agent)' | sort -u | tr '\n' ' ')"
  note "mimir 3.2.4 ingest storage default: $(docker run --rm --name "$P-probe-mimir2" "$MIMIR" -help-all 2>&1 |
    grep -A1 -- '-ingest-storage.enabled' | tr -s ' \n' ' ' | cut -c1-200)"
}

# --- endpoints: the native metrics endpoints, plain HTTP inside the test ------------
traefik_plain() {
  run traefik "$TRAEFIK" --entrypoints.web.address=:80 --entrypoints.metrics.address=:8082 \
    --metrics.prometheus=true --metrics.prometheus.entryPoint=metrics \
    --metrics.prometheus.addRoutersLabels=true --log.level=ERROR
}
group_endpoints() {
  echo "== endpoints (a): Traefik, Dolt, RustFS"
  network
  traefik_plain
  mkdir -p "$D/dolt"
  cat >"$D/dolt/config.yaml" <<'YML'
log_level: warning
listener:
  host: 0.0.0.0
  port: 3306
metrics:
  host: 0.0.0.0
  port: 11228
YML
  run dolt -v "$D/dolt/config.yaml:/etc/dolt/servercfg.d/config.yaml:ro" "$DOLT"
  run rustfs -e RUSTFS_ACCESS_KEY="qd$(openssl rand -hex 8)" -e RUSTFS_SECRET_KEY="$(openssl rand -hex 16)" \
    -e RUSTFS_VOLUMES=/data -e RUSTFS_OBS_LOGGER_LEVEL=warn --tmpfs /data:uid=10001,gid=10001 \
    "$RUSTFS" server --address :9000 /data
  if wait_for 30 get "http://$(ip traefik):8082/metrics"; then
    ok "Traefik 3.6: /metrics on its own entry point (:8082)"
    describe "Traefik"
  else ko "Traefik /metrics: $(get "http://$(ip traefik):8082/metrics")"; fi
  if wait_for 60 get "http://$(ip dolt):11228/metrics"; then
    ok "Dolt 2.6.1: /metrics on metrics.port (:11228)"
    describe "Dolt"
  else ko "Dolt /metrics: $(get "http://$(ip dolt):11228/metrics")"; fi
  wait_for 30 get "http://$(ip rustfs):9000/health" >/dev/null
  local path found=""
  for path in /metrics /rustfs/v2/metrics/cluster /rustfs/v2/metrics/node /minio/v2/metrics/cluster \
    /minio/v2/metrics/node /minio/metrics/v3 /rustfs/metrics/v3; do
    found="$found $path=$(get "http://$(ip rustfs):9000$path" | cut -c1-3)"
  done
  note "RustFS 1.0.5, unsigned GETs on :9000:$found"
  end_group

  echo "== endpoints (b): Grafana, Loki, Alloy"
  run grafana "$GRAFANA"
  run loki "$LOKI" -config.file=/etc/loki/local-config.yaml
  run alloy -v "$D/alloy-self.alloy:/etc/alloy/config.alloy:ro" "$ALLOY" \
    run --server.http.listen-addr=0.0.0.0:12345 --disable-reporting /etc/alloy/config.alloy
  if wait_for 90 get "http://$(ip grafana):3000/metrics"; then
    ok "Grafana 13.2.5: /metrics on its HTTP port, on by default, no sign-in"
    describe "Grafana"
  else ko "Grafana /metrics: $(get "http://$(ip grafana):3000/metrics")"; fi
  if wait_for 90 get "http://$(ip loki):3100/metrics"; then
    ok "Loki 3.7.9: /metrics on its HTTP port (:3100)"
    describe "Loki"
  else ko "Loki /metrics: $(get "http://$(ip loki):3100/metrics")"; fi
  if wait_for 30 get "http://$(ip alloy):12345/metrics"; then
    ok "Alloy v1.21.3: /metrics on its own listener (:12345)"
    describe "Alloy"
  else ko "Alloy /metrics: $(get "http://$(ip alloy):12345/metrics")"; fi
  end_group
}
printf 'logging { level = "warn" }\n' >"$D/alloy-self.alloy"

# --- tls: each endpoint served over TLS, with or without a client certificate -----
group_tls() {
  echo "== tls (a): Traefik's metrics through a router that requires a client certificate"
  network
  traefik_mtls
  if wait_for 30 tls traefik 8443 /metrics machine; then
    ok "Traefik: /metrics over HTTPS with a certificate of the CA: 200"
  else ko "Traefik mTLS /metrics: $(tls traefik 8443 /metrics machine)"; fi
  expect "Traefik: no client certificate" "000 alert certificate required" "$(tls traefik 8443 /metrics -)"
  expect "Traefik: another CA's certificate" "000 alert unknown ca" "$(tls traefik 8443 /metrics stranger)"

  echo "== tls (b): Loki and Alloy serving their own /metrics with mutual TLS"
  run loki -v "$C:/certs:ro" "$LOKI" -config.file=/etc/loki/local-config.yaml \
    -server.http-tls-cert-path=/certs/machine.crt -server.http-tls-key-path=/certs/machine.key \
    -server.http-tls-ca-path=/certs/ca.crt -server.http-tls-client-auth=RequireAndVerifyClientCert
  if wait_for 90 tls loki 3100 /metrics machine; then ok "Loki: /metrics over mutual TLS: 200"
  else ko "Loki mTLS /metrics: $(tls loki 3100 /metrics machine)"; fi
  expect "Loki: no client certificate" "000 alert certificate required" "$(tls loki 3100 /metrics -)"
  cat >"$D/alloy-tls.alloy" <<'ALLOY'
logging { level = "warn" }
http {
  tls {
    cert_file        = "/certs/machine.crt"
    key_file         = "/certs/machine.key"
    client_ca_file   = "/certs/ca.crt"
    client_auth_type = "RequireAndVerifyClientCert"
  }
}
ALLOY
  run alloy -v "$C:/certs:ro" -v "$D/alloy-tls.alloy:/etc/alloy/config.alloy:ro" "$ALLOY" \
    run --server.http.listen-addr=0.0.0.0:12345 --disable-reporting /etc/alloy/config.alloy
  if wait_for 30 tls alloy 12345 /metrics machine; then ok "Alloy: its own /metrics over mutual TLS (http.tls block): 200"
  else ko "Alloy mTLS /metrics: $(tls alloy 12345 /metrics machine)"; fi
  expect "Alloy: no client certificate" "000 alert certificate required" "$(tls alloy 12345 /metrics -)"
  end_group

  echo "== tls (c): Grafana over HTTPS; Dolt's metrics with TLS settings"
  run grafana -v "$C:/certs:ro" -e GF_SERVER_PROTOCOL=https -e GF_SERVER_CERT_FILE=/certs/machine.crt \
    -e GF_SERVER_CERT_KEY=/certs/machine.key -e GF_METRICS_BASIC_AUTH_USERNAME=qd-metrics \
    -e GF_METRICS_BASIC_AUTH_PASSWORD="$(openssl rand -hex 12)" "$GRAFANA"
  wait_for 90 tls grafana 3000 /api/health - >/dev/null
  expect "Grafana: /api/health over HTTPS, no client certificate asked" 200 "$(tls grafana 3000 /api/health -)"
  expect "Grafana: /metrics with basic auth set, none sent" 401 "$(tls grafana 3000 /metrics - | cut -c1-3)"
  [ "$(tls grafana 3000 /api/health - | cut -c1-3)" = 200 ] ||
    note "Grafana: curl said $(head -c 200 "$D/err"); its lines on TLS: $(docker logs "$P-grafana" 2>&1 | grep -iE 'cert|tls|key|https' | tail -3 | tr '\n' ' ' | cut -c1-400)"
  note "Grafana: a client certificate is asked at the handshake: $(echo | openssl s_client -connect "$(ip grafana):3000" 2>/dev/null | grep -c 'Acceptable client certificate CA names')"
  mkdir -p "$D/dolt-tls"
  cat >"$D/dolt-tls/config.yaml" <<'YML'
log_level: warning
listener:
  host: 0.0.0.0
  port: 3306
metrics:
  host: 0.0.0.0
  port: 11228
  tls_cert: /certs/machine.crt
  tls_key: /certs/machine.key
  tls_ca: /certs/ca.crt
YML
  run dolt -v "$C:/certs:ro" -v "$D/dolt-tls/config.yaml:/etc/dolt/servercfg.d/config.yaml:ro" "$DOLT"
  sleep 20
  local running
  running=$(docker inspect -f '{{.State.Running}}' "$P-dolt")
  note "Dolt with tls_cert/tls_key/tls_ca under metrics: running=$running, https with a certificate: $(tls dolt 11228 /metrics machine), without: $(tls dolt 11228 /metrics -), plain: $(get "http://$(ip dolt):11228/metrics" | cut -c1-3)"
  [ "$running" = true ] || note "Dolt's last lines: $(docker logs "$P-dolt" 2>&1 | tail -2 | tr '\n' ' ' | cut -c1-240)"
  # Dolt behind the metrics router: Traefik asks Alloy for a certificate and
  # verifies Dolt's against the CA.
  mkdir -p "$D/traefik"
  cat >"$D/traefik/dolt.yml" <<'YML'
http:
  routers:
    dolt-metrics:
      entryPoints: [metrics]
      rule: "Path(`/dolt/metrics`)"
      middlewares: [dolt-strip]
      service: dolt-metrics
      tls: {}
  middlewares:
    dolt-strip:
      stripPrefix:
        prefixes: ["/dolt"]
  services:
    dolt-metrics:
      loadBalancer:
        serversTransport: qd-ca
        servers:
          - url: "https://dolt:11228"
  serversTransports:
    qd-ca:
      rootCAs: [/certs/ca.crt]
YML
  traefik_mtls
  if wait_for 30 tls traefik 8443 /dolt/metrics machine; then
    ok "Dolt's metrics through the Traefik metrics router (HTTPS to Dolt, verified against the CA): 200"
    describe "Dolt through Traefik"
  else ko "Dolt through Traefik: $(tls traefik 8443 /dolt/metrics machine) $(head -c 120 "$D/body")"; fi
  expect "Dolt through Traefik: no client certificate" "000 alert certificate required" "$(tls traefik 8443 /dolt/metrics -)"
  docker rm -f "$P-dolt" "$P-traefik" >/dev/null
  rm -f "$D/traefik/dolt.yml"
  # The listener's own words for a client certificate, tried under metrics.
  printf '  require_client_cert: true\n  ca_cert: /certs/ca.crt\n' >>"$D/dolt-tls/config.yaml"
  run dolt -v "$C:/certs:ro" -v "$D/dolt-tls/config.yaml:/etc/dolt/servercfg.d/config.yaml:ro" "$DOLT"
  sleep 20
  running=$(docker inspect -f '{{.State.Running}}' "$P-dolt")
  note "Dolt's lines on metrics: $(docker logs "$P-dolt" 2>&1 | grep -i 'metric' | tail -2 | tr '\n' ' ' | cut -c1-300)"
  note "Dolt with require_client_cert and ca_cert also under metrics: running=$running, with a certificate: $(tls dolt 11228 /metrics machine), without: $(tls dolt 11228 /metrics -), another CA's: $(tls dolt 11228 /metrics stranger)"
  [ "$running" = true ] || note "Dolt's last lines: $(docker logs "$P-dolt" 2>&1 | tail -2 | tr '\n' ' ' | cut -c1-240)"
  end_group
}
traefik_mtls() {
  mkdir -p "$D/traefik"
  cat >"$D/traefik/metrics.yml" <<'YML'
http:
  routers:
    metrics:
      entryPoints: [metrics]
      rule: "PathPrefix(`/metrics`)"
      service: prometheus@internal
      tls: {}
tls:
  options:
    default:
      minVersion: VersionTLS12
      clientAuth:
        caFiles: [/certs/ca.crt]
        clientAuthType: RequireAndVerifyClientCert
  stores:
    default:
      defaultCertificate:
        certFile: /certs/machine.crt
        keyFile: /certs/machine.key
YML
  run traefik -v "$C:/certs:ro" -v "$D/traefik:/dyn:ro" "$TRAEFIK" \
    --entrypoints.metrics.address=:8443 --providers.file.directory=/dyn \
    --metrics.prometheus=true --metrics.prometheus.manualRouting=true --log.level=ERROR
}

# --- postgres: PostgreSQL through Alloy's embedded exporter, over verify-full -------
group_postgres() {
  echo "== postgres: Alloy's prometheus.exporter.postgres, TLS and a client certificate"
  network
  mkdir -p "$D/pg"
  printf 'local all all trust\nhostssl all qd_metrics all cert\n' >"$D/pg/pg_hba.conf"
  printf 'CREATE ROLE qd_metrics LOGIN IN ROLE pg_monitor;\n' >"$D/pg/role.sql"
  chmod 755 "$D/pg"
  chmod 644 "$D/pg/"*
  run postgres -v "$C:/certs:ro" -v "$D/pg:/pg:ro" -v "$D/pg/role.sql:/docker-entrypoint-initdb.d/role.sql:ro" \
    -e POSTGRES_PASSWORD="$(openssl rand -hex 16)" --entrypoint sh "$POSTGRES" -c '
      mkdir -p /run/tls && cp /certs/machine.crt /certs/machine.key /certs/ca.crt /run/tls/ &&
      chown -R postgres:postgres /run/tls && chmod 600 /run/tls/machine.key &&
      exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/run/tls/machine.crt \
        -c ssl_key_file=/run/tls/machine.key -c ssl_ca_file=/run/tls/ca.crt -c hba_file=/pg/pg_hba.conf -c log_connections=on'
  mkdir -p "$D/pgc"
  cp "$D/ca.crt" "$D/machine.crt" "$D/machine.key" "$D/pgc/"
  chmod 600 "$D/pgc/machine.key"
  cat >"$D/alloy-pg.alloy" <<'ALLOY'
logging { level = "warn" }
prometheus.exporter.postgres "pg" {
  data_source_names = ["postgresql://qd_metrics@postgres:5432/postgres?sslmode=verify-full&sslrootcert=/pgc/ca.crt&sslcert=/pgc/machine.crt&sslkey=/pgc/machine.key"]
}
prometheus.scrape "pg" {
  targets         = prometheus.exporter.postgres.pg.targets
  scrape_interval = "5s"
  scrape_timeout  = "4s"
  forward_to      = []
}
ALLOY
  for _ in $(seq 30); do docker exec "$P-postgres" pg_isready -q -h 127.0.0.1 2>/dev/null && break; sleep 2; done
  run alloy -v "$D/pgc:/pgc:ro" -v "$D/alloy-pg.alloy:/etc/alloy/config.alloy:ro" "$ALLOY" \
    run --server.http.listen-addr=0.0.0.0:12345 --disable-reporting /etc/alloy/config.alloy
  wait_for 30 get "http://$(ip alloy):12345/-/ready" >/dev/null
  sleep 12
  get "http://$(ip alloy):12345/api/v0/component/prometheus.exporter.postgres.pg/metrics" >/dev/null
  if grep -q '^pg_up 1' "$D/body"; then
    ok "Alloy's exporter reached PostgreSQL 16 with sslmode=verify-full and a client certificate (pg_up 1)"
    describe "PostgreSQL through Alloy"
    note "statement text in any label: $(grep -c 'query=\|queryid=' "$D/body")"
  else ko "pg_up not 1: $(grep '^pg_up' "$D/body" | head -1) $(docker logs "$P-alloy" 2>&1 | grep -i 'error' | tail -1 | cut -c1-200)"; fi
  note "PostgreSQL's log of the exporter's sessions: $(docker logs "$P-postgres" 2>&1 | grep 'connection authorized: user=qd_metrics' | tail -1 | sed 's/.*connection authorized/connection authorized/' | cut -c1-200)"
  note "Alloy's errors: $(docker logs "$P-alloy" 2>&1 | grep -i 'level=error' | tail -1 | sed 's/.*sslkey=[^)]*)//' | cut -c1-300)"
  note "PostgreSQL's complaints: $(docker logs "$P-postgres" 2>&1 | grep -E 'FATAL|could not' | tail -2 | tr '\n' ' ' | cut -c1-300)"
  end_group
}

# --- trino: its /metrics -------------------------------------------------------------
group_trino() {
  echo "== trino: Trino 484's /metrics (alone: one JVM)"
  network
  run trino --memory=2g "$TRINO"
  if wait_for 180 get "http://$(ip trino):8080/v1/info"; then
    for _ in $(seq 60); do grep -q '"starting":false' "$D/body" && break; sleep 2; get "http://$(ip trino):8080/v1/info" >/dev/null; done
    note "Trino /metrics without a user: $(get "http://$(ip trino):8080/metrics" | cut -c1-3)"
    if [ "$(get "http://$(ip trino):8080/metrics" -H 'X-Trino-User: qd-metrics' | cut -c1-3)" = 200 ]; then
      ok "Trino 484: /metrics (OpenMetrics) with X-Trino-User"
      describe "Trino"
    else ko "Trino /metrics with a user: $(get "http://$(ip trino):8080/metrics" -H 'X-Trino-User: qd-metrics')"; fi
  else ko "Trino did not start: $(docker logs "$P-trino" 2>&1 | tail -2 | cut -c1-200)"; fi
  end_group
}

# --- the stores ----------------------------------------------------------------------
# alloy_push URL: Alloy scraping Traefik over mutual TLS, a text file of
# synthetic counters (as a worker could write) and itself, and sending all to
# URL over mutual TLS.
alloy_push() {
  mkdir -p "$D/textfile"
  cat >"$D/textfile/qd_worker.prom" <<'PROM'
# HELP qd_verify_jobs_total Jobs finished, by kind and result (synthetic).
# TYPE qd_verify_jobs_total counter
qd_verify_jobs_total{kind="evaluate",result="ok"} 3
qd_verify_jobs_total{kind="extract",result="failed"} 1
PROM
  chmod -R a+rX "$D/textfile"
  cat >"$D/alloy-push.alloy" <<ALLOY
logging { level = "warn" }
prometheus.scrape "traefik" {
  targets         = [{"__address__" = "traefik:8443", "service" = "traefik"}]
  scheme          = "https"
  scrape_interval = "5s"
  scrape_timeout  = "4s"
  sample_limit    = 5000
  label_limit     = 20
  tls_config {
    ca_file   = "/certs/ca.crt"
    cert_file = "/certs/machine.crt"
    key_file  = "/certs/machine.key"
  }
  forward_to = [prometheus.remote_write.store.receiver]
}
prometheus.exporter.unix "files" {
  set_collectors = ["textfile"]
  textfile { directory = "/textfile" }
}
prometheus.scrape "files" {
  targets         = prometheus.exporter.unix.files.targets
  scrape_interval = "5s"
  scrape_timeout  = "4s"
  forward_to      = [prometheus.remote_write.store.receiver]
}
prometheus.exporter.self "alloy" { }
prometheus.scrape "alloy" {
  targets         = prometheus.exporter.self.alloy.targets
  scrape_interval = "5s"
  scrape_timeout  = "4s"
  forward_to      = [prometheus.remote_write.store.receiver]
}
// RustFS pushes OTLP (it has no Prometheus endpoint): received over TLS.
otelcol.receiver.otlp "rustfs" {
  http {
    endpoint = "0.0.0.0:4318"
    tls {
      cert_file = "/certs/machine.crt"
      key_file  = "/certs/machine.key"
    }
  }
  output { metrics = [otelcol.exporter.prometheus.rustfs.input] }
}
otelcol.exporter.prometheus "rustfs" {
  forward_to = [prometheus.remote_write.store.receiver]
}
prometheus.remote_write "store" {
  external_labels = { vm = "qd-verify-machine" }
  endpoint {
    url = "$1"
    tls_config {
      ca_file   = "/certs/ca.crt"
      cert_file = "/certs/machine.crt"
      key_file  = "/certs/machine.key"
    }
  }
}
ALLOY
  run alloy -v "$C:/certs:ro" -v "$D/textfile:/textfile:ro" -v "$D/alloy-push.alloy:/etc/alloy/config.alloy:ro" \
    "$ALLOY" run --server.http.listen-addr=0.0.0.0:12345 --disable-reporting /etc/alloy/config.alloy
}
# query STORE PORT PREFIX PROMQL: the number of series the instant query returns.
query() {
  tls "$1" "$2" "$3/api/v1/query" machine -G --data-urlencode "query=$4" >/dev/null
  grep -o '"metric":' "$D/body" | wc -l
}
# A store must show all three sources within a minute.
check_store() { # NAME PORT PREFIX
  local i t=0 f=0 a=0
  for i in $(seq 30); do
    t=$(query "$1" "$2" "$3" 'traefik_config_reloads_total')
    f=$(query "$1" "$2" "$3" 'qd_verify_jobs_total')
    a=$(query "$1" "$2" "$3" 'alloy_build_info')
    [ "$t" -gt 0 ] && [ "$f" -gt 0 ] && [ "$a" -gt 0 ] && break
    sleep 3
  done
  expect "$1: Traefik's series (scraped over mutual TLS) arrived" 1 "$([ "$t" -gt 0 ] && echo 1 || echo 0)"
  expect "$1: the text file's series arrived (2 expected)" 2 "$f"
  expect "$1: Alloy's own series arrived" 1 "$([ "$a" -gt 0 ] && echo 1 || echo 0)"
  expect "$1: the vm label from Alloy's external_labels" 2 "$(query "$1" "$2" "$3" 'qd_verify_jobs_total{vm="qd-verify-machine"}')"
  query "$1" "$2" "$3" 'count({__name__=~".+"})' >/dev/null
  note "$1: series stored in all: $(grep -o '"value":\[[^]]*\]' "$D/body" | head -1)"
}

group_mimir() {
  echo "== mimir: Grafana Mimir 3.2.4 monolithic, blocks in a RustFS bucket over mutual TLS"
  network
  local key secret bucket=qd-metrics
  key=qd$(openssl rand -hex 8)
  secret=$(openssl rand -hex 16)
  mkdir -p "$D/rtls"
  cp "$D/machine.crt" "$D/rtls/rustfs_cert.pem"
  cp "$D/machine.key" "$D/rtls/rustfs_key.pem"
  cp "$D/ca.crt" "$D/rtls/client_ca.crt"
  chmod -R a+rX "$D/rtls"
  run rustfs -e RUSTFS_ACCESS_KEY="$key" -e RUSTFS_SECRET_KEY="$secret" -e RUSTFS_VOLUMES=/data \
    -e RUSTFS_OBS_LOGGER_LEVEL=warn -e RUSTFS_TLS_PATH=/tls -e RUSTFS_SERVER_MTLS_ENABLE=1 \
    -e RUSTFS_OBS_ENDPOINT=https://alloy:4318 -e RUSTFS_OBS_METRICS_EXPORT_ENABLED=true \
    -e RUSTFS_OBS_TRACES_EXPORT_ENABLED=false -e RUSTFS_OBS_LOGS_EXPORT_ENABLED=false \
    -e RUSTFS_OBS_METER_INTERVAL=5 -e RUSTFS_OBS_TLS_CA_FILE=/tls/client_ca.crt \
    -v "$D/rtls:/tls:ro" --tmpfs /data:uid=10001,gid=10001 "$RUSTFS" server --address :9000 /data
  s3() { # METHOD PATH
    get "https://rustfs:9000$2" -X "$1" --cacert "$D/ca.crt" --resolve "rustfs:9000:$(ip rustfs)" \
      --cert "$D/machine.crt" --key "$D/machine.key" --aws-sigv4 "aws:amz:us-east-1:s3" --user "$key:$secret"
  }
  for _ in $(seq 30); do [ "$(s3 PUT "/$bucket" | cut -c1-3)" = 200 ] && break; sleep 1; done
  expect "RustFS: the bucket made over mutual TLS" 200 "$(s3 GET "/$bucket?list-type=2" | cut -c1-3)"
  cat >"$D/mimir.yml" <<YML
multitenancy_enabled: false
target: all
server:
  http_listen_port: 9009
  grpc_listen_address: 127.0.0.1
  log_level: warn
  http_tls_config:
    cert_file: /certs/machine.crt
    key_file: /certs/machine.key
    client_ca_file: /certs/ca.crt
    client_auth_type: RequireAndVerifyClientCert
common:
  storage:
    backend: s3
    s3:
      endpoint: rustfs:9000
      bucket_name: $bucket
      access_key_id: $key
      secret_access_key: $secret
      region: us-east-1
      bucket_lookup_type: path
      http:
        tls_ca_path: /certs/ca.crt
        tls_cert_path: /certs/machine.crt
        tls_key_path: /certs/machine.key
blocks_storage:
  storage_prefix: blocks
  tsdb:
    dir: /data/tsdb
  bucket_store:
    sync_dir: /data/tsdb-sync
ruler_storage:
  storage_prefix: ruler
alertmanager_storage:
  storage_prefix: alertmanager
compactor:
  data_dir: /data/compactor
  sharding_ring: { kvstore: { store: memberlist } }
distributor:
  ring: { instance_addr: 127.0.0.1, kvstore: { store: memberlist } }
ingester:
  ring: { instance_addr: 127.0.0.1, kvstore: { store: memberlist }, replication_factor: 1 }
store_gateway:
  sharding_ring: { replication_factor: 1 }
# gRPC on the loopback only: the querier finds the frontend there.
frontend:
  address: 127.0.0.1
limits:
  compactor_blocks_retention_period: 400d
  max_global_series_per_user: 150000
  max_label_names_per_series: 20
usage_stats:
  enabled: false
YML
  chmod 644 "$D/mimir.yml"
  docker run --rm --name "$P-chmod" -v "$P-mimir-data:/d" alpine:3.20 chmod 1777 /d
  run mimir -v "$C:/certs:ro" -v "$D/mimir.yml:/etc/mimir.yml:ro" -v "$P-mimir-data:/data" \
    "$MIMIR" -config.file=/etc/mimir.yml
  if wait_for 120 tls mimir 9009 /ready machine; then ok "Mimir: ready over mutual TLS, no Kafka configured"
  else
    ko "Mimir not ready: $(tls mimir 9009 /ready machine) $(cut -c1-160 "$D/body" 2>/dev/null | head -1)"
    note "Mimir's last lines: $(docker logs "$P-mimir" 2>&1 | tail -3 | tr '\n' ' ' | cut -c1-400)"
    end_group
    return
  fi
  expect "Mimir: no client certificate" "000 alert certificate required" "$(tls mimir 9009 /ready -)"
  expect "Mimir: another CA's certificate" "000 alert unknown ca" "$(tls mimir 9009 /ready stranger)"
  note "Mimir: bucket objects right after start: $(s3 GET "/$bucket?list-type=2" >/dev/null; grep -o '<Key>[^<]*' "$D/body" | sed 's/<Key>//' | tr '\n' ' ')"
  traefik_mtls
  alloy_push "https://mimir:9009/api/v1/push"
  check_store mimir 9009 /prometheus
  local rn=""
  for _ in $(seq 20); do
    tls mimir 9009 "/prometheus/api/v1/label/__name__/values" machine >/dev/null
    rn=$(grep -oE '"rustfs_[a-z_]*"' "$D/body" | tr -d '"' | sort -u)
    [ -n "$rn" ] && break
    sleep 3
  done
  if [ -n "$rn" ]; then
    ok "RustFS's OTLP metrics reached Mimir through Alloy over TLS: $(echo "$rn" | wc -l) names"
    note "RustFS metric names by prefix: $(echo "$rn" | cut -d_ -f2 | sort | uniq -c | awk '{ printf "%s %s, ", $2, $1 }')"
    tls mimir 9009 "/prometheus/api/v1/labels" machine -G --data-urlencode 'match[]={__name__=~"rustfs_.+"}' >/dev/null
    note "RustFS label names: $(grep -oE '"[a-zA-Z_]+"' "$D/body" | tr -d '"' | grep -v '^status$\|^success$\|^data$' | tr '\n' ' ')"
    query mimir 9009 /prometheus 'count({__name__=~"rustfs_.+"})' >/dev/null
    note "RustFS series: $(grep -o '"value":\[[^]]*\]' "$D/body" | head -1)"
    for l in bucket path object key api; do
      tls mimir 9009 "/prometheus/api/v1/label/$l/values" machine -G --data-urlencode 'match[]={__name__=~"rustfs_.+"}' >/dev/null
      note "RustFS label $l values: $(tr -d '\n' <"$D/body" | sed 's/.*"data":\[//; s/\].*//' | cut -c1-200)"
    done
  else
    ko "no rustfs_ metric in Mimir; names with otel/s3: $(grep -oE '"[a-z_]*(s3|http_server|otel)[a-z_]*"' "$D/body" | sort -u | head -8 | tr '\n' ' ')"
    note "RustFS's lines on OTLP: $(docker logs "$P-rustfs" 2>&1 | grep -i 'otel\|otlp\|metric' | tail -2 | cut -c1-300)"
  fi
  expect "Mimir: a push without a client certificate" "000 alert certificate required" \
    "$(tls mimir 9009 /api/v1/push - -X POST)"
  # Ship the head to the bucket now rather than after two hours.
  note "Mimir: flush the ingester: $(tls mimir 9009 /ingester/flush machine -X POST | cut -c1-3)"
  local n=0
  for _ in $(seq 40); do
    s3 GET "/$bucket?list-type=2&prefix=blocks/" >/dev/null
    n=$(grep -o '<Key>[^<]*meta.json' "$D/body" | wc -l)
    [ "$n" -gt 0 ] && break
    sleep 3
  done
  expect "Mimir: a block's meta.json in the bucket after the flush (over mutual TLS)" 1 "$([ "$n" -gt 0 ] && echo 1 || echo 0)"
  s3 GET "/$bucket?list-type=2" >/dev/null
  note "Mimir: bucket keys: $(grep -o '<Key>' "$D/body" | wc -l), bytes: $(grep -o '<Size>[0-9]*' "$D/body" | sed 's/<Size>//' | awk '{ s += $1 } END { print s + 0 }')"
  note "Mimir: local disk after the flush: $(docker run --rm --name "$P-du" -v "$P-mimir-data:/d:ro" alpine:3.20 du -sh /d | cut -f1); its folders: $(docker run --rm --name "$P-du" -v "$P-mimir-data:/d:ro" alpine:3.20 sh -c 'cd /d && du -sh * | tr "\n\t" "  "')"
  note "Mimir: memory in use: $(docker stats --no-stream --format '{{.MemUsage}}' "$P-mimir")"
  end_group
}

group_prometheus() {
  echo "== prometheus: Prometheus v3.15.1 as a remote-write receiver behind mutual TLS"
  network
  cat >"$D/web.yml" <<'YML'
tls_server_config:
  cert_file: /certs/machine.crt
  key_file: /certs/machine.key
  client_ca_file: /certs/ca.crt
  client_auth_type: RequireAndVerifyClientCert
YML
  printf 'global: {}\nscrape_configs: []\n' >"$D/prometheus.yml"
  chmod 644 "$D/web.yml" "$D/prometheus.yml"
  run prometheus -v "$C:/certs:ro" -v "$D/web.yml:/etc/prometheus/web.yml:ro" \
    -v "$D/prometheus.yml:/etc/prometheus/prometheus.yml:ro" --tmpfs /prometheus:mode=1777 "$PROMETHEUS" \
    --config.file=/etc/prometheus/prometheus.yml --web.config.file=/etc/prometheus/web.yml \
    --web.enable-remote-write-receiver --storage.tsdb.path=/prometheus --storage.tsdb.retention.time=400d
  if wait_for 60 tls prometheus 9090 /-/ready machine; then ok "Prometheus: ready over mutual TLS"
  else ko "Prometheus not ready: $(tls prometheus 9090 /-/ready machine)"; end_group; return; fi
  expect "Prometheus: no client certificate" "000 alert certificate required" "$(tls prometheus 9090 /-/ready -)"
  expect "Prometheus: another CA's certificate" "000 alert unknown ca" "$(tls prometheus 9090 /-/ready stranger)"
  traefik_mtls
  alloy_push "https://prometheus:9090/api/v1/write"
  check_store prometheus 9090 ""
  note "Prometheus: local disk: $(docker exec "$P-prometheus" du -sh /prometheus 2>/dev/null | cut -f1) (busybox image); memory in use: $(docker stats --no-stream --format '{{.MemUsage}}' "$P-prometheus")"
  note "Prometheus: snapshot API (for a backup), needs --web.enable-admin-api: $(tls prometheus 9090 /api/v1/admin/tsdb/snapshot machine -X POST | cut -c1-3)"
  end_group
}

GROUPS_TO_RUN=("$@")
[ ${#GROUPS_TO_RUN[@]} -gt 0 ] || GROUPS_TO_RUN=(probe endpoints tls postgres trino mimir prometheus)
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
