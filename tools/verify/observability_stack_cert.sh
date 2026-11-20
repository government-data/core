#!/bin/bash
# Verification of infra/observability/certificate.yml (decision 0066 point 12,
# the user's rulings of 2026-11-13), the overlay itself: rendered by docker
# compose with the base file (and the development files without it), then
# run by docker compose as its own project, with the repository's files
# (loki.yml, config.alloy, Grafana's provisioning and grafana/tls/
# entrypoint.sh) and a throwaway CA. Beside it, in a test overlay of this
# script's own: RustFS behind a Traefik requiring a client certificate of
# the CA (the gate's TLS; its forwardAuth is in loki_thanos_lake.sh), a
# PostgreSQL serving TLS for Grafana's datasource, and Loki at a fixed
# address, which LOKI_ADDRESS gives Alloy and Grafana for its name, as
# another VM's Alloy is given the application VM's.
#
# It shows: the data volume made by Loki's former user handed to the agent's;
# Loki serving the machine's certificate and requiring one, ready over
# HTTPS, its gRPC port on its loopback alone, writing to the lake through
# the gate; Alloy pushing a container's lines over mutual TLS; Grafana
# reading Loki with the machine's certificate and PostgreSQL with verify-full
# against the CA, running as its own user; and after a renewal, Loki and
# Alloy on the renewed certificate with no restart, Grafana once restarted
# (what the node agent's QD_AFTER_RENEW does). And development's files
# without the overlay: Loki and Alloy over plain HTTP, as before.
#
# Mimir (decision 0071) is in the same files and not started here:
# mimir_stack_cert.sh runs it. Its own project (qd-verify-obscert, or
# QD_VERIFY_PROJECT: containers, network 10.213.77.0/24,
# volumes), Loki's port published on 127.0.0.1:13100 only, a probe
# container; Alloy reads the Docker socket, as the stack's does, collecting
# only the probe's project. Nothing of the dev stack is touched. Throwaway
# CA, keys and secrets, removed at the end; nothing secret is printed.
#
#   bash tools/verify/observability_stack_cert.sh
#
# Needs docker with Compose v2, curl, openssl and python3.
set -u
export LC_ALL=C
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
P=${QD_VERIFY_PROJECT:-qd-verify-obscert}
PROBE=$P-probe
NET=10.213.77
PORT=13100
HELPER=alpine:3.20
D=$(mktemp -d)
chmod 755 "$D"
OBS="-f $ROOT/infra/observability/docker-compose.yml"
# shellcheck disable=SC2086
compose() { docker compose --project-directory "$ROOT" --env-file "$D/env" -p $P $OBS \
  -f "$ROOT/infra/observability/certificate.yml" -f "$D/test.yml" "$@"; }
cleanup() {
  docker rm -f "$PROBE" "$P-dev-loki" "$P-dev-alloy" >/dev/null 2>&1
  [ -f "$D/test.yml" ] && compose down -v --remove-orphans >/dev/null 2>&1
  docker volume rm -f "${P}_loki-data" >/dev/null 2>&1
  docker run --rm -v "$D:/d" "$HELPER" rm -rf /d/agent /d/pg >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

# --- Certificates and secrets ------------------------------------------------------
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/O=qd/CN=qd verify machines" -addext "basicConstraints=critical,CA:TRUE" \
  -keyout "$D/ca.key" -out "$D/ca.crt" 2>/dev/null
cert() { # FILE SUBJECT DNS: FILE.crt, FILE.key (PKCS#8) and FILE.pem, both
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$2" \
    -keyout "$D/$1.k" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:%s\n' "$3" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/ca.crt" -CAkey "$D/ca.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  openssl pkey -in "$D/$1.k" -out "$D/$1.key"
  cat "$D/$1.crt" "$D/$1.key" >"$D/$1.pem"
  rm -f "$D/$1.k" "$D/$1.csr"
}
serial() { openssl x509 -noout -serial -in "$D/$1.crt" | cut -d= -f2; }
cert app1 "/O=qd/OU=app/CN=qd-app" qd-app
cert app2 "/O=qd/OU=app/CN=qd-app" qd-app
cert lake "/O=qd/OU=lake/CN=lake-1" lake-1
cert pg "/CN=qd-pg" qd-pg
mkdir -p "$D/src" "$D/secrets" "$D/gate" "$D/pg"
cp "$D/ca.crt" "$D/app1.pem" "$D/app2.pem" "$D/src/"
cp "$D/ca.crt" "$D/lake.crt" "$D/lake.key" "$D/gate/"
cp "$D/pg.crt" "$D/pg.key" "$D/pg/"
for s in grafana_admin_password grafana_oidc_client_secret grafana_pg_password grafana_metrics_password; do
  openssl rand -hex 16 >"$D/secrets/$s"
done
chmod 755 "$D"/src "$D"/secrets "$D"/gate "$D"/pg && chmod 644 "$D"/src/* "$D"/secrets/* "$D"/gate/* "$D"/pg/*
KEY=qd$(openssl rand -hex 8)
SECRET=$(openssl rand -hex 16)
cat >"$D/env" <<ENV
QD_SECRETS_DIR=$D/secrets
QD_MACHINE_DIR=$D/agent
LOKI_SERVER_NAME=qd-app
LOKI_S3_ENDPOINT=lake-1:9000
LOKI_S3_ACCESS_KEY=$KEY
LOKI_S3_SECRET_KEY=$SECRET
MIMIR_S3_ACCESS_KEY=$KEY
MIMIR_S3_SECRET_KEY=$SECRET
LOKI_HTTP_PORT=$PORT
LOKI_BIND=127.0.0.1
LOKI_ADDRESS=$NET.10
GRAFANA_PG_HOST=qd-pg
GRAFANA_PG_DATABASE=knime
OBS_VM=verify
OBS_LOG_PROJECTS=$PROBE
ENV
chmod 600 "$D/env"
# The agent's directory, as the application VM's agent writes it (uid 65532).
agent() { # PEM: machine.pem from now on, aside then renamed
  docker run --rm -v "$D:/d" "$HELPER" sh -c "mkdir -p /d/agent && chmod 700 /d/agent &&
    cp /d/src/ca.crt /d/agent/ca.crt && chmod 644 /d/agent/ca.crt &&
    cp /d/src/$1.pem /d/agent/.machine.pem.new && chmod 600 /d/agent/.machine.pem.new &&
    chown -R 65532:65532 /d/agent && mv -f /d/agent/.machine.pem.new /d/agent/machine.pem"
}

# --- (a) The overlay as Compose renders it -------------------------------------
# shellcheck disable=SC2086
render() { docker compose --project-directory "$ROOT" --env-file "$1" -p $P $OBS "${@:2}" config --format json 2>"$D/render.err"; }
render "$D/env" -f "$ROOT/infra/observability/certificate.yml" >"$D/cert.json"
expect "certificate.yml renders" 0 "$(test -s "$D/cert.json"; echo $?)"
python3 -I - "$D/cert.json" >"$D/cert.txt" <<'PY'
import json, sys
s = json.load(open(sys.argv[1]))["services"]
l, a, g, v = s["loki"], s["alloy"], s["grafana"], s["loki-volume"]
mounts = lambda svc: sorted(m["target"] + (":ro" if m.get("read_only") else "") for m in svc.get("volumes", []))
c = " ".join(l["command"])
print("loki", l["user"], "tls" if "-server.http-tls-client-auth=RequireAndVerifyClientCert" in c else "-",
      "grpc-lo" if "-server.grpc-listen-address=127.0.0.1" in c else "-",
      "s3-cert" if "-common.storage.object-store.s3.http.tls-cert-path=/run/qd-node/machine.pem" in c else "-",
      "/run/qd-node:ro" in mounts(l), l["ports"][0]["host_ip"] + ":" + l["ports"][0]["published"],
      l["depends_on"]["loki-volume"]["condition"], l["environment"]["LOKI_S3_INSECURE"])
print("alloy", a["environment"]["LOKI_PUSH_URL"], a["environment"]["LOKI_CA_FILE"],
      a["environment"]["LOKI_CLIENT_CERT_FILE"], " ".join(a["extra_hosts"]) if isinstance(a["extra_hosts"], list) else a["extra_hosts"])
print("grafana", g["user"], g["entrypoint"][0], g["environment"]["GF_PATHS_PROVISIONING"],
      g["environment"]["LOKI_URL"], g["environment"]["GRAFANA_PG_SSLMODE"], "/run/qd-node:ro" in mounts(g))
print("loki-volume", "/loki" in mounts(v))
PY
expect "Loki: the agent's user, client certificates required, gRPC on its loopback, the S3 client's certificate, the agent's files, its port, after the volume" \
  "loki 65532:65532 tls grpc-lo s3-cert True 127.0.0.1:$PORT service_completed_successfully false" "$(sed -n 1p "$D/cert.txt")"
expect "Alloy: HTTPS to Loki's name, the CA and the machine's certificate, the name's address" \
  "alloy https://qd-app:$PORT/loki/api/v1/push /run/qd-node/ca.crt /run/qd-node/machine.pem" \
  "$(sed -n 2p "$D/cert.txt" | cut -d' ' -f1-4)"
expect "Grafana: as root to copy, the entrypoint, its provisioning, Loki over HTTPS, verify-full, the agent's files" \
  "grafana 0:0 /etc/qd/grafana/entrypoint.sh /run/qd-grafana/provisioning https://qd-app:$PORT verify-full True" \
  "$(sed -n 3p "$D/cert.txt")"
grep -v '^LOKI_SERVER_NAME=' "$D/env" >"$D/env.no-name"
render "$D/env.no-name" -f "$ROOT/infra/observability/certificate.yml" >/dev/null
expect "without LOKI_SERVER_NAME it refuses to render" yes "$(grep -q LOKI_SERVER_NAME "$D/render.err" && echo yes || echo no)"
printf 'LOKI_S3_ACCESS_KEY=x\nLOKI_S3_SECRET_KEY=x\nMIMIR_S3_ACCESS_KEY=x\nMIMIR_S3_SECRET_KEY=x\n' >"$D/env.dev"
render "$D/env.dev" -f "$ROOT/infra/observability/compose.dev.yml" >"$D/dev.json"
expect "development (compose.dev.yml) is as before: Loki's own user and command, Alloy over plain HTTP, Grafana's own entrypoint" \
  "None -config.file=/etc/loki/loki.yml -config.expand-env=true http://127.0.0.1:3100/loki/api/v1/push None None None" \
  "$(python3 -I -c '
import json, sys
s = json.load(open(sys.argv[1]))["services"]
print(s["loki"].get("user"), " ".join(s["loki"]["command"]), s["alloy"]["environment"]["LOKI_PUSH_URL"],
      s["alloy"]["environment"].get("LOKI_CA_FILE"), s["grafana"].get("entrypoint"), s["grafana"].get("user"))' "$D/dev.json")"

# --- (b) Run: the lake, PostgreSQL, then the stack ----------------------------------
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
cat >"$D/test.yml" <<YML
networks:
  default:
    ipam:
      config:
        - subnet: $NET.0/24
services:
  loki:
    restart: "no"
    networks:
      default:
        ipv4_address: $NET.10
  alloy:
    restart: "no"
  grafana:
    restart: "no"
  rustfs:
    image: $(sed -n 's/^ *image: \${RUSTFS_IMAGE:-\(.*\)}$/\1/p' "$ROOT/infra/rustfs/docker-compose.yml")
    command: ["server", "--address", ":9000", "/data"]
    environment:
      RUSTFS_ACCESS_KEY: $KEY
      RUSTFS_SECRET_KEY: $SECRET
      RUSTFS_VOLUMES: /data
    tmpfs:
      - /data:uid=10001,gid=10001
  gate:
    image: traefik:v3.6
    command: ["--entrypoints.lake.address=:9000", "--providers.file.directory=/g", "--log.level=ERROR"]
    volumes:
      - $D/gate:/g:ro
    networks:
      default:
        aliases: [lake-1]
  pg:
    image: postgres:16-alpine
    command: ["postgres", "-c", "ssl=on", "-c", "ssl_cert_file=/pg/pg.crt", "-c", "ssl_key_file=/pg/pg.key"]
    environment:
      POSTGRES_USER: qd_grafana
      POSTGRES_DB: knime
      POSTGRES_PASSWORD_FILE: /run/secrets/grafana_pg_password
    secrets:
      - grafana_pg_password
    volumes:
      - $D/pg:/pg:ro
    networks:
      default:
        aliases: [qd-pg]
YML
docker run --rm -v "$D:/d" "$HELPER" sh -c 'chown 70:70 /d/pg/pg.key && chmod 600 /d/pg/pg.key'
agent app1
# Loki's volume as an installation before this overlay left it: made by uid 10001.
docker volume create --label com.docker.compose.project=$P --label com.docker.compose.volume=loki-data \
  "${P}_loki-data" >/dev/null
docker run --rm -v "${P}_loki-data:/loki" "$HELPER" sh -c 'mkdir -p /loki/wal && chown -R 10001:10001 /loki'
compose up -d rustfs gate pg >"$D/up.log" 2>&1 || { echo "could not start the lake and PostgreSQL"; tail -5 "$D/up.log"; exit 1; }
s3() { # METHOD PATH: the status; the body in $D/s3.out
  docker run --rm --network "${P}_default" curlimages/curl:8.11.1 -s -o - -w '\n%{http_code}' -X "$1" \
    --aws-sigv4 "aws:amz:us-east-1:s3" --user "$KEY:$SECRET" "http://rustfs:9000$2" >"$D/s3.out" 2>/dev/null
  tail -n1 "$D/s3.out"
}
for _ in $(seq 30); do [ "$(s3 PUT /qd-observability)" = 200 ] && break; sleep 1; done
objects() { s3 GET "/qd-observability?list-type=2&prefix=fake/" >/dev/null; grep -o '<Key>' "$D/s3.out" | wc -l; }
compose up -d loki alloy grafana >"$D/up.log" 2>&1 || { echo "could not start the stack"; tail -5 "$D/up.log"; exit 1; }
LOKI_C=$P-loki-1 GRAFANA_C=$P-grafana-1 ALLOY_C=$P-alloy-1
LIP=$NET.10
at() { # PEM PATH [CURL OPTION...]: Loki's status over HTTPS presenting PEM (none: no certificate)
  local pem=$1 path=$2
  shift 2
  local c=()
  [ "$pem" != none ] && c=(--cert "$D/$pem.pem")
  curl -s -o "$D/at.out" -w '%{http_code}' --max-time 10 --cacert "$D/ca.crt" \
    --resolve "qd-app:$PORT:$LIP" "${c[@]}" "$@" "https://qd-app:$PORT$path"
}
for _ in $(seq 90); do [ "$(at app1 /ready)" = 200 ] && break; sleep 1; done

echo "== (b) Loki"
expect "the volume made by uid 10001 handed to the agent's user" 65532 \
  "$(docker run --rm -v "${P}_loki-data:/loki" "$HELPER" stat -c %u /loki/wal)"
expect "Loki runs as the agent's user" 65532 "$(docker top "$LOKI_C" | awk 'NR == 2 { print $1 }')"
expect "ready over HTTPS, the machine's certificate presented" 200 "$(at app1 /ready)"
expect "no client certificate: no handshake" 000 "$(at none /ready)"
expect "published on 127.0.0.1 alone, over TLS" 200 \
  "$(curl -s -o /dev/null -w '%{http_code}' -m 5 --cacert "$D/ca.crt" --cert "$D/app1.pem" \
    --resolve "qd-app:$PORT:127.0.0.1" "https://qd-app:$PORT/ready")"
expect "its gRPC port not reached from another container" 000 \
  "$(docker run --rm --network "${P}_default" curlimages/curl:8.11.1 -s -o /dev/null -w '%{http_code}' -m 5 \
    --http2-prior-knowledge "http://$LIP:9095/")"
before=$(objects)
at app1 /loki/api/v1/push -H 'content-type: application/json' \
  --data "{\"streams\":[{\"stream\":{\"verify\":\"obscert\"},\"values\":[[\"$(date +%s%N)\",\"pushed\"]]}]}" >/dev/null
at app1 /flush -X POST >/dev/null
wrote=no
for _ in $(seq 20); do [ "$(objects)" -gt "$before" ] && { wrote=yes; break; }; sleep 1; done
expect "a flush writes to the lake through the gate, the machine's certificate presented" yes "$wrote"
LOKI_STARTED=$(docker inspect -f '{{.State.StartedAt}}' "$LOKI_C")

echo "== (c) Alloy"
docker run -d --name "$PROBE" --label com.docker.compose.project=$PROBE --label com.docker.compose.service=probe \
  "$HELPER" sh -c 'i=0; while :; do i=$((i+1)); echo "{\"level\":\"info\",\"msg\":\"probe $i\"}"; sleep 1; done' >/dev/null
probes() { # how many of the probe's lines Loki holds from the last S seconds
  local q
  q=$(python3 -I -c 'import urllib.parse as u; print(u.quote("{compose_project=\"'"$PROBE"'\",vm=\"verify\",level=\"info\"}"))')
  at app1 "/loki/api/v1/query_range?query=$q&since=$1s&limit=1000" >/dev/null
  python3 -I -c 'import json, sys; print(sum(len(s["values"]) for s in json.load(open(sys.argv[1]))["data"]["result"]))' \
    "$D/at.out" 2>/dev/null || echo 0
}
n=0
for _ in $(seq 60); do n=$(probes 60); [ "$n" -gt 0 ] && break; sleep 1; done
expect "a container's lines arrive over mutual TLS, with its labels" yes "$([ "$n" -gt 0 ] && echo yes || echo no)"
ALLOY_STARTED=$(docker inspect -f '{{.State.StartedAt}}' "$ALLOY_C")

echo "== (d) Grafana"
GPW=$(cat "$D/secrets/grafana_admin_password")
gf() { docker exec "$GRAFANA_C" wget -qO- --header "Authorization: Basic $(printf 'qd-emergency-admin:%s' "$GPW" | base64 -w0)" \
  "http://127.0.0.1:3000$1" 2>/dev/null; }
status() { gf "/api/datasources/uid/$1/health" | python3 -I -c 'import json, sys; print(json.load(sys.stdin).get("status"))' 2>/dev/null || echo none; }
for _ in $(seq 90); do [ "$(docker inspect -f '{{.State.Health.Status}}' "$GRAFANA_C")" = healthy ] && break; sleep 2; done
expect "healthy, by the compose file's own check" healthy "$(docker inspect -f '{{.State.Health.Status}}' "$GRAFANA_C")"
expect "Grafana runs as its own user, 472" 472 "$(docker top "$GRAFANA_C" | awk 'NR == 2 { print $1 }')"
expect "its copy of machine.pem: Grafana's, 0600" "472 600" "$(docker exec "$GRAFANA_C" stat -c '%u %a' /run/qd-grafana/machine.pem)"
for _ in $(seq 30); do [ "$(status loki)" = OK ] && break; sleep 2; done
expect "the Loki datasource reads Loki with the machine's certificate" OK "$(status loki)"
expect "the PostgreSQL datasource connects with verify-full against the installation's CA" OK "$(status postgres)"
expect "only presence shows in the API" "true true" "$(gf /api/datasources/uid/loki | python3 -I -c '
import json, sys; d = json.load(sys.stdin); print(str(d["jsonData"].get("tlsAuth")).lower(), str(d["secureJsonFields"].get("tlsClientKey")).lower())')"

echo "== (e) A renewal"
agent app2
served() {
  openssl s_client -connect "$LIP:$PORT" -servername qd-app -cert "$D/app1.crt" -key "$D/app1.key" \
    -CAfile "$D/ca.crt" </dev/null 2>/dev/null | openssl x509 -noout -serial 2>/dev/null | cut -d= -f2
}
for _ in $(seq 10); do [ "$(served)" = "$(serial app2)" ] && break; sleep 1; done
expect "Loki serves the renewed certificate" "$(serial app2)" "$(served)"
sleep 20
expect "Alloy's lines still arrive (the last 15 s)" yes "$([ "$(probes 15)" -gt 0 ] && echo yes || echo no)"
expect "Loki and Alloy never restarted" "$LOKI_STARTED $ALLOY_STARTED" \
  "$(docker inspect -f '{{.State.StartedAt}}' "$LOKI_C") $(docker inspect -f '{{.State.StartedAt}}' "$ALLOY_C")"
same() { docker exec -u 0 "$GRAFANA_C" cmp -s /run/qd-node/machine.pem /run/qd-grafana/machine.pem && echo yes || echo no; }
expect "Grafana keeps the copy it started with" no "$(same)"
docker restart "$GRAFANA_C" >/dev/null
for _ in $(seq 90); do [ "$(docker inspect -f '{{.State.Health.Status}}' "$GRAFANA_C")" = healthy ] && break; sleep 2; done
expect "restarted (QD_AFTER_RENEW), it copies the renewed one" yes "$(same)"
for _ in $(seq 30); do [ "$(status loki)" = OK ] && break; sleep 2; done
expect "and reads Loki with it" OK "$(status loki)"

echo "== (f) Development's way, without the overlay: plain HTTP, loki.yml and config.alloy as they are"
DEV=$(python3 -I -c '
import json, sys
s = json.load(open(sys.argv[1]))["services"]
print(s["loki"]["image"], s["alloy"]["image"], " ".join(s["loki"]["command"]))' "$D/dev.json")
read -r DEV_LOKI DEV_ALLOY DEV_CMD <<<"$DEV"
# shellcheck disable=SC2086
docker run -d --name "$P-dev-loki" --network "${P}_default" --tmpfs /loki:uid=10001,gid=10001 \
  -v "$ROOT/infra/observability/loki/loki.yml:/etc/loki/loki.yml:ro" \
  -e LOKI_S3_ENDPOINT=rustfs:9000 -e LOKI_S3_INSECURE=true -e LOKI_S3_ACCESS_KEY="$KEY" -e LOKI_S3_SECRET_KEY="$SECRET" \
  "$DEV_LOKI" $DEV_CMD >/dev/null
DIP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$P-dev-loki")
docker run -d --name "$P-dev-alloy" --network "${P}_default" \
  -v "$ROOT/infra/observability/alloy/config.alloy:/etc/alloy/config.alloy:ro" -v /var/run/docker.sock:/var/run/docker.sock:ro \
  -e LOKI_PUSH_URL="http://$DIP:3100/loki/api/v1/push" -e OBS_VM=dev-verify -e OBS_LOG_PROJECTS=$PROBE \
  "$DEV_ALLOY" run /etc/alloy/config.alloy --storage.path=/tmp/alloy --disable-reporting >/dev/null
dev_probes() {
  curl -s -m 10 -G "http://$DIP:3100/loki/api/v1/query_range" --data-urlencode "query={vm=\"dev-verify\"}" \
    --data-urlencode since=60s | python3 -I -c 'import json, sys; print(sum(len(s["values"]) for s in json.load(sys.stdin)["data"]["result"]))' \
    2>/dev/null || echo 0
}
n=0
for _ in $(seq 90); do n=$(dev_probes); [ "$n" -gt 0 ] && break; sleep 1; done
expect "Alloy, its TLS settings empty, pushes over plain HTTP" yes "$([ "$n" -gt 0 ] && echo yes || echo no)"
before=$(objects)
curl -s -o /dev/null -m 10 -X POST "http://$DIP:3100/flush"
wrote=no
for _ in $(seq 20); do [ "$(objects)" -gt "$before" ] && { wrote=yes; break; }; sleep 1; done
expect "Loki's thanos client writes to the lake over plain HTTP (LOKI_S3_INSECURE)" yes "$wrote"

echo "   $(docker run --rm grafana/loki:3.7.9 -version 2>/dev/null | sed -n 's/^loki, version \([^ ]*\).*/Loki \1/p'),"\
  "Alloy, Grafana and the rest as docker-compose.yml names them; Compose $(docker compose version --short)"
[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
