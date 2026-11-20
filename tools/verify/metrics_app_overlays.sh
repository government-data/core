# shellcheck shell=bash
# Sourced by metrics_app_cert.sh (decision 0071 step 2): its environment, its
# test overlays and how it runs Traefik. Uses P, NET, D, T, ROOT_KEY,
# ROOT_SECRET, PY, TRAEFIK and HELPER from there.

# The environment of every project: the agent's directory, the VM's name, the
# address of its published ports (the pass-through `vm`), throwaway secrets.
# Compose needs the lake's keys to start anything; setup-storage.sh's replace
# these after the lake is up.
printf 'MIMIR_S3_ACCESS_KEY=x\nMIMIR_S3_SECRET_KEY=x\nLOKI_S3_ACCESS_KEY=x\nLOKI_S3_SECRET_KEY=x\n' >"$D/env"
cat >>"$D/env" <<ENV
APP_STACK=$P
QD_SECRETS_DIR=$D/secrets
QD_MACHINE_DIR=$D/agent
QD_EDGE_NETWORK=$P-edge
LOKI_SERVER_NAME=qd-app
LOKI_ADDRESS=$NET.10
LOKI_S3_ENDPOINT=lake-1:9000
GRAFANA_PG_HOST=qd-app
POSTGRES_SERVER_NAME=qd-app
POSTGRES_SUPERUSER_PASSWORD=$(openssl rand -hex 16)
APP_DB_PASSWORD=$(openssl rand -hex 16)
DOLT_ROOT_PASSWORD=$(openssl rand -hex 16)
DOLT_APP_PASSWORD=$(openssl rand -hex 16)
OBS_VM=verify
QD_METRICS_VOLUME=$P-metrics
OBS_LOG_PROJECTS=$P|$P-.+
ENV
chmod 600 "$D/env"

# The lake (RustFS behind a gate requiring a client certificate of the CA),
# as mimir_stack_cert.sh; `vm`, a TCP pass-through at LOKI_ADDRESS standing
# for the VM's published ports (Traefik's metrics entry point, Loki, Mimir,
# PostgreSQL), so that every client reaches them as `qd-app` and the TLS is
# the services' own; `fake`, a target for the limits and the keep lists.
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
{
  echo "tcp:"
  echo "  routers:"
  for p in 9445 3100 9009 5432; do
    printf '    p%s: {entryPoints: [p%s], rule: "HostSNI(`*`)", service: p%s}\n' "$p" "$p" "$p"
  done
  echo "  services:"
  printf '    p9445: {loadBalancer: {servers: [{address: "%s-traefik:9445"}]}}\n' "$P"
  echo '    p3100: {loadBalancer: {servers: [{address: "loki:3100"}]}}'
  echo '    p9009: {loadBalancer: {servers: [{address: "mimir:9009"}]}}'
  echo '    p5432: {loadBalancer: {servers: [{address: "postgres:5432"}]}}'
} >"$D/vm/vm.yml"
cat >"$D/www/serve.py" <<'PY'
import http.server, os
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        name = self.path.split("?")[0].strip("/").replace("/", "_")
        try:
            body = open(os.path.join("/www", name or "none"), "rb").read()
        except OSError:
            self.send_response(404); self.end_headers(); return
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; version=0.0.4")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers(); self.wfile.write(body)
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(("", 8000), H).serve_forever()
PY
# As Grafana: one allowed metric carrying forbidden labels (synthetic values),
# and one metric outside the lists. As Traefik: 6,000 series, past the sample
# limit. As Dolt: an allowed metric with 25 labels, past the label limit.
{
  l='handler="/api/verify",method="GET",status_code="200",usename="synthetic-user",application_name="synthetic-app",tenant="synthetic-tenant"'
  echo "grafana_http_request_duration_seconds_count{$l} 3"
  echo "grafana_http_request_duration_seconds_sum{$l} 0.5"
  echo "grafana_verify_unlisted_total 1"
} >"$D/www/grafana_metrics"
seq 6000 | awk '{ printf "traefik_service_requests_total{code=\"200\",method=\"GET\",protocol=\"http\",service=\"s%d\"} 1\n", $1 }' >"$D/www/metrics"
echo "dss_connects{$(seq 25 | awk '{ printf "%sl%d=\"a\"", (NR > 1 ? "," : ""), $1 }')} 1" >"$D/www/dolt_metrics"
chmod 644 "$D"/gate/* "$D"/vm/* "$D"/www/*

cat >"$D/test.yml" <<YML
networks:
  default:
    ipam:
      config:
        - subnet: $NET.0/24
  edge:
    external: true
    name: $P-edge
  backend:
    external: true
    name: $P-backend
services:
  loki:
    restart: "no"
    ports: !override
      - "127.0.0.1:13100:3100"
  loki-volume:
    restart: "no"
  mimir:
    restart: "no"
    ports: !override
      - "127.0.0.1:19009:9009"
  mimir-volume:
    restart: "no"
  grafana:
    restart: "no"
  alloy:
    restart: "no"
  rustfs:
    restart: "no"
    image: $(sed -n 's/^ *image: \${RUSTFS_IMAGE:-\(.*\)}$/\1/p' "$T/infra/rustfs/docker-compose.yml")
    command: ["server", "--address", ":9000", "/data"]
    environment:
      RUSTFS_ACCESS_KEY: $ROOT_KEY
      RUSTFS_SECRET_KEY: $ROOT_SECRET
      RUSTFS_VOLUMES: /data
    tmpfs:
      - /data:uid=10001,gid=10001
  gate:
    restart: "no"
    image: $TRAEFIK
    command: ["--entrypoints.lake.address=:9000", "--providers.file.directory=/g", "--log.level=ERROR"]
    volumes:
      - $D/gate:/g:ro
    networks:
      default:
        aliases: [lake-1]
  vm:
    restart: "no"
    image: $TRAEFIK
    command: ["--providers.file.directory=/f", "--log.level=ERROR", "--entrypoints.p9445.address=:9445",
      "--entrypoints.p3100.address=:3100", "--entrypoints.p9009.address=:9009", "--entrypoints.p5432.address=:5432"]
    volumes:
      - $D/vm:/f:ro
    networks:
      default:
        ipv4_address: $NET.10
      edge: {}
      backend: {}
  fake:
    restart: "no"
    image: $PY
    command: ["python", "-I", "/www/serve.py"]
    volumes:
      - $D/www:/www:ro
YML
# PostgreSQL's own network renamed (the file names data-quality-backend), no
# port on this machine; Dolt's port the same.
printf 'networks:\n  backend:\n    name: %s-backend\nservices:\n  postgres:\n    restart: "no"\n    ports: !reset []\n' "$P" >"$D/pg.yml"
printf 'services:\n  dolt:\n    restart: "no"\n    ports: !reset []\n' >"$D/dolt.yml"

# traefik_run NAME MODE [DOCKER OPTION...]: the application's Traefik with
# traefik.yml and every dynamic file. MODE empty: an installation's, with
# internal.yml, the CA and certificate.yml's entrypoint copying the agent's
# machine.pem; `plain`: development's, QD_TRAEFIK_METRICS=plain.
traefik_run() {
  local name=$1 mode=$2 y=$D/dyn/$1 extra=()
  shift 2
  mkdir -p "$y"
  cp "$T"/infra/traefik/dynamic/*.yml "$y/"
  if [ -z "$mode" ]; then
    cp "$T/infra/traefik/internal.yml" "$y/"
    cp "$D/ca.crt" "$y/machines-ca.crt"
    extra=(--init --entrypoint /etc/qd/traefik/entrypoint.sh -e INTERNAL_DOMAIN=qd-app
      -e INTERNAL_CERT_FILE=/etc/traefik/dynamic/app-machine.pem -e INTERNAL_KEY_FILE=/etc/traefik/dynamic/app-machine.pem
      -e QD_TRAEFIK_CHECK_EVERY=1 -v "$T/infra/traefik/machine/entrypoint.sh:/etc/qd/traefik/entrypoint.sh:ro"
      -v "$D/agent:/run/qd-node:ro")
  else
    extra=(-e QD_TRAEFIK_METRICS=plain)
  fi
  chmod 755 "$D/dyn" "$y" && chmod 644 "$y"/*
  docker run -d --name "$name" --network "$P-edge" --restart no "${extra[@]}" "$@" \
    -e TRAEFIK_CERTIFICATESRESOLVERS_LETSENCRYPT_ACME_EMAIL=verify@example.com -e APP_DOMAIN=localhost \
    -v "$T/infra/traefik/traefik.yml:/etc/traefik/traefik.yml:ro" -v "$y:/etc/traefik/dynamic" \
    "$TRAEFIK" --configFile=/etc/traefik/traefik.yml >/dev/null || { echo "could not start $name"; exit 1; }
}
