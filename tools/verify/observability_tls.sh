#!/bin/bash
# Verification of TLS between Alloy, Loki, Grafana and the lake (decisions
# 0066 point 12, 0023's ports table, 0026 point 6 and its "To verify": Loki
# to RustFS with mutual TLS), with the stack's images and the repository's
# loki.yml. Throwaway containers only; it decides nothing, it records what
# each piece does:
#
# (a) Loki's older S3 client (loki.yml's before 2026-11-13), to a lake that
#     requires a client certificate of the CA (a Traefik standing for the
#     gate of 0066 point 10, without its forwardAuth): it has no setting
#     for one.
# (b) Loki with the thanos object store client, loki.yml's now, which has a
#     client certificate setting: writes through the gate; a replaced
#     client certificate, taken or not without a restart.
# (c) Loki serving HTTPS with a machine certificate (machine.pem as both
#     certificate and key, read as the node agent writes it: uid 65532, 0600)
#     and requiring a client certificate of the CA: who gets in, whether a
#     subject can be told apart, plain HTTP, its gRPC port.
# (d) Alloy's loki.write over TLS: the CA, a client certificate, none, a
#     wrong CA, plain HTTP; a replaced client certificate.
# (e) Grafana's Loki datasource provisioned with the CA and a client
#     certificate from files ($__file{}), and without one; a replaced file,
#     before and after a provisioning reload.
# (f) Loki's own server certificate and client CA replaced in place.
#
# Its own network (qd-verify-obs-net), containers (qd-verify-obs-*) and
# volume (qd-verify-obs-agent); no published port, no Docker socket; nothing
# of the dev stack is touched. Throwaway CAs, keys and S3 keys, made here and
# removed at the end; only states, counts and whether serials are equal are
# printed. Run from anywhere:
#
#   bash tools/verify/observability_tls.sh
#
# Needs docker, curl, openssl and python3.
set -u
export LC_ALL=C
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
img() { sed -n "s/^ *image: \${$1:-\(.*\)}$/\1/p" "$2"; }
OBS=$ROOT/infra/observability/docker-compose.yml
LOKI=$(img LOKI_IMAGE "$OBS")
ALLOY=$(img ALLOY_IMAGE "$OBS")
GRAFANA=$(img GRAFANA_IMAGE "$OBS")
RUSTFS=$(img RUSTFS_IMAGE "$ROOT/infra/rustfs/docker-compose.yml")
TRAEFIK=traefik:v3.6
CURL=curlimages/curl:8.11.1
HELPER=alpine:3.20
NAME=qd-verify-obs
NET=$NAME-net
VOL=$NAME-agent
ALL="$NAME-rustfs $NAME-gate $NAME-loki $NAME-alloy $NAME-grafana"
D=$(mktemp -d)
chmod 755 "$D"
cleanup() {
  # shellcheck disable=SC2086
  docker rm -f $ALL >/dev/null 2>&1
  docker volume rm -f "$VOL" >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
cleanup
mkdir -p "$D"
chmod 755 "$D"
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }
ip() { docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$1"; }

# --- Certificates: the installation's CA and another ---------------------------
ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/O=qd/CN=qd verify $1" -addext "basicConstraints=critical,CA:TRUE" \
    -keyout "$D/$1.key" -out "$D/$1.crt" 2>/dev/null
}
# cert FILE CA SUBJECT DNS: FILE.pem, the certificate then its PKCS#8 key, as
# the node agent's machine.pem; client and server use in one, as 0066 point 2.
cert() {
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$3" \
    -keyout "$D/$1.k" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:%s\n' "$4" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/$2.crt" -CAkey "$D/$2.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  openssl pkey -in "$D/$1.k" -out "$D/$1.key"
  cat "$D/$1.crt" "$D/$1.key" >"$D/$1.pem"
  rm -f "$D/$1.k" "$D/$1.csr"
}
serial() { openssl x509 -noout -serial -in "$D/$1.crt" | cut -d= -f2; }
ca machines
ca other
cert app1 machines "/O=qd/OU=app/CN=qd-app" qd-app
cert app2 machines "/O=qd/OU=app/CN=qd-app" qd-app
cert worker1 machines "/O=qd/OU=worker/CN=worker-1" worker-1
cert worker2 machines "/O=qd/OU=worker/CN=worker-1" worker-1
cert lake machines "/O=qd/OU=lake/CN=lake-1" lake-1
cert admin machines "/O=qd/OU=admin/CN=someone" someone
cert stranger other "/O=qd/OU=app/CN=qd-app" qd-app
cat "$D/machines.crt" "$D/other.crt" >"$D/both.crt"
S=$D/src
mkdir -p "$S" && cp "$D"/*.pem "$D/machines.crt" "$D/both.crt" "$S/" && chmod 755 "$S" && chmod 644 "$S"/*

# --- The agents' directories, written as the node agent writes them ------------
# app (Loki's server certificate), s3 (Loki's client to the lake), grafana
# and worker (Alloy's): one directory each, so each can be replaced alone; on
# an installation the first three are the application VM's one machine.pem.
docker network create "$NET" >/dev/null
docker volume create "$VOL" >/dev/null
docker run --rm -v "$VOL:/agent" "$HELPER" sh -c \
  'for d in app s3 grafana worker; do mkdir -p /agent/$d; chmod 700 /agent/$d; done; chown -R 65532:65532 /agent'
# agent DIR PEM [CA]: DIR's machine.pem from now on (and ca.crt), aside then renamed.
agent() {
  docker run --rm --user 65532:65532 -v "$VOL:/agent" -v "$S:/src:ro" "$HELPER" sh -c "
    cp /src/${3:-machines.crt} /agent/$1/.ca.crt.new && mv -f /agent/$1/.ca.crt.new /agent/$1/ca.crt &&
    cp /src/$2.pem /agent/$1/.machine.pem.new && chmod 600 /agent/$1/.machine.pem.new &&
    mv -f /agent/$1/.machine.pem.new /agent/$1/machine.pem"
}
agent app app1
agent s3 app1
agent grafana app1
agent worker worker1

# --- The lake: RustFS behind a gate that requires a client certificate ---------
KEY=qd$(openssl rand -hex 8)
SECRET=$(openssl rand -hex 16)
BUCKET=qd-observability
docker run -d --name "$NAME-rustfs" --network "$NET" --network-alias rustfs \
  -e RUSTFS_ACCESS_KEY="$KEY" -e RUSTFS_SECRET_KEY="$SECRET" -e RUSTFS_VOLUMES=/data \
  --tmpfs /data:uid=10001,gid=10001 "$RUSTFS" server --address :9000 /data >/dev/null ||
  { echo "could not start $RUSTFS"; exit 1; }
s3() { # METHOD PATH: the status, the body in $D/s3.out
  docker run --rm --network "$NET" "$CURL" -s -o - -w '\n%{http_code}' -X "$1" \
    --aws-sigv4 "aws:amz:us-east-1:s3" --user "$KEY:$SECRET" "http://rustfs:9000$2" >"$D/s3.out" 2>/dev/null
  tail -n1 "$D/s3.out"
}
for _ in $(seq 30); do [ "$(s3 PUT "/$BUCKET")" = 200 ] && break; sleep 1; done
objects() { s3 GET "/$BUCKET?list-type=2" >/dev/null; grep -o '<Key>' "$D/s3.out" | wc -l; }
G=$D/gate
mkdir -p "$G" && cp "$D/lake.crt" "$D/lake.key" "$D/machines.crt" "$G/"
cat >"$G/gate.yml" <<'YML'
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
        caFiles: [/g/machines.crt]
        clientAuthType: RequireAndVerifyClientCert
YML
chmod 755 "$G" && chmod 644 "$G"/*
docker run -d --name "$NAME-gate" --network "$NET" --network-alias lake-1 -v "$G:/g:ro" "$TRAEFIK" \
  --entrypoints.lake.address=:9000 --providers.file.directory=/g --log.level=ERROR >/dev/null ||
  { echo "could not start the gate"; exit 1; }

# --- Loki ----------------------------------------------------------------------
# The repository's loki.yml, with the thanos object store client; and the same
# with the older `s3` client loki.yml had before (its storage block put back).
cp "$ROOT/infra/observability/loki/loki.yml" "$D/loki-thanos.yml"
python3 -I - "$D/loki-thanos.yml" "$D/loki-older.yml" <<'PY'
import re, sys
text = open(sys.argv[1]).read()
older = """  storage:
    s3:
      endpoint: ${LOKI_S3_ENDPOINT}
      insecure: ${LOKI_S3_INSECURE:-false}
      bucketnames: ${LOKI_S3_BUCKET:-qd-observability}
      access_key_id: ${LOKI_S3_ACCESS_KEY}
      secret_access_key: ${LOKI_S3_SECRET_KEY}
      region: us-east-1
      s3forcepathstyle: true

schema_config:"""
new, n = re.subn(r"  storage:\n    object_store:\n.*?\n\nschema_config:", older, text, flags=re.S)
assert n == 1 and "use_thanos_objstore" not in new, "loki.yml's thanos storage block not found"
open(sys.argv[2], "w").write(new)
PY
chmod 644 "$D"/loki*.yml
A=/agent/app
# loki CONFIG FLAG...: Loki as the agent's user, serving HTTPS with app's
# machine.pem and requiring a client certificate of app's ca.crt.
loki() {
  local config=$1
  shift
  docker rm -f "$NAME-loki" >/dev/null 2>&1
  docker run -d --name "$NAME-loki" --network "$NET" --network-alias qd-app --user 65532:65532 \
    --tmpfs /loki:uid=65532,gid=65532 -v "$VOL:/agent:ro" -v "$D/$config:/etc/loki/loki.yml:ro" \
    -e LOKI_S3_ENDPOINT=lake-1:9000 -e LOKI_S3_INSECURE=false -e LOKI_S3_BUCKET=$BUCKET \
    -e LOKI_S3_ACCESS_KEY="$KEY" -e LOKI_S3_SECRET_KEY="$SECRET" \
    "$LOKI" -config.file=/etc/loki/loki.yml -config.expand-env=true \
    -server.http-tls-cert-path=$A/machine.pem -server.http-tls-key-path=$A/machine.pem \
    -server.http-tls-client-auth=RequireAndVerifyClientCert -server.http-tls-ca-path=$A/ca.crt \
    -server.http-idle-timeout=2s "$@" >/dev/null || { echo "could not start $LOKI"; exit 1; }
  LIP=$(ip "$NAME-loki")
  for _ in $(seq 60); do [ "$(at app1 /ready)" = 200 ] && break; sleep 1; done
}
# at PEM PATH [CURL OPTION...]: Loki's status over HTTPS presenting PEM (none: no certificate).
at() {
  local pem=$1 path=$2
  shift 2
  local c=()
  [ "$pem" != none ] && c=(--cert "$S/$pem.pem")
  curl -s -o "$D/at.out" -w '%{http_code}' --max-time 10 --cacert "$D/machines.crt" \
    --resolve "qd-app:3100:$LIP" "${c[@]}" "$@" "https://qd-app:3100$path"
}
ROUND=0
push() { # a line in a stream of its own, then a flush to the lake
  ROUND=$((ROUND + 1))
  local body
  body=$(printf '{"streams":[{"stream":{"verify":"obs-tls","round":"%s"},"values":[["%s","round %s"]]}]}' \
    "$ROUND" "$(date +%s%N)" "$ROUND")
  at app1 /loki/api/v1/push -H 'content-type: application/json' --data "$body" >/dev/null
  at app1 /flush -X POST >/dev/null
}
# wrote BEFORE: whether objects were added to the bucket within 20 s.
wrote() { for _ in $(seq 20); do [ "$(objects)" -gt "$1" ] && { echo yes; return; }; sleep 1; done; echo no; }

echo "== (a) Loki's older S3 client, to a lake requiring client certificates"
loki loki-older.yml -common.storage.s3.http.ca-file=$A/ca.crt
for _ in $(seq 60); do [ "$(docker inspect -f '{{.State.Running}}' "$NAME-loki")" = false ] && break; sleep 1; done
expect "Loki does not start: it has no client certificate setting, and its compactor reads the lake at start" \
  "false yes" "$(docker inspect -f '{{.State.Running}}' "$NAME-loki") $(docker logs "$NAME-loki" 2>&1 |
    grep -q 'init compactor' && echo yes || echo no)"
expect "Loki says why: the gate asked for a certificate it did not give" yes \
  "$(docker logs "$NAME-loki" 2>&1 | grep -qiE 'certificate required|bad certificate|handshake failure' && echo yes || echo no)"

echo "== (b) The thanos object store client, presenting the machine's certificate"
T=-common.storage.object-store.s3.http
THANOS="$T.tls-ca-path=/agent/s3/ca.crt $T.tls-cert-path=/agent/s3/machine.pem $T.tls-key-path=/agent/s3/machine.pem"
# shellcheck disable=SC2086
loki loki-thanos.yml $THANOS $T.idle-conn-timeout=1s
expect "Loki ready" 200 "$(at app1 /ready)"
expect "its gRPC port, as loki.yml leaves it (certificate.yml does not), answers plain HTTP/2 from another container" yes \
  "$(docker run --rm --network "$NET" "$CURL" -s -o /dev/null -w '%{http_code}' -m 5 --http2-prior-knowledge \
    http://qd-app:9095/ | grep -qv '^000$' && echo yes || echo no)"
before=$(objects)
push
expect "a flush writes its chunks to the lake through the gate" yes "$(wrote "$before")"
agent s3 stranger
sleep 3
before=$(objects)
push
s3_after_other=$(wrote "$before")
expect "its client certificate replaced by another CA's: the next flush is refused, the file read again" \
  no "$s3_after_other"
expect "the gate's refusal, in Loki's log" yes \
  "$(docker logs "$NAME-loki" 2>&1 | grep -q 'tls: unknown certificate authority' && echo yes || echo no)"
agent s3 app2
sleep 3
before=$(objects)
push
expect "the renewed certificate of the CA in its place: the next flush" yes "$(wrote "$before")"
# shellcheck disable=SC2086
loki loki-thanos.yml $THANOS -server.grpc-listen-address=127.0.0.1
LOKI_STARTED=$(docker inspect -f '{{.State.StartedAt}}' "$NAME-loki")
push
expect "gRPC on 127.0.0.1 alone: not reached from another container, Loki still answers queries" "000 200" \
  "$(docker run --rm --network "$NET" "$CURL" -s -o /dev/null -w '%{http_code}' -m 5 --http2-prior-knowledge \
    http://qd-app:9095/) $(at app1 '/loki/api/v1/query_range?query=%7Bverify%3D%22obs-tls%22%7D')"
expect "and they find the line pushed since" yes "$(grep -q "round $ROUND" "$D/at.out" && echo yes || echo no)"

echo "== (c) Who reaches Loki (RequireAndVerifyClientCert, the installation's CA)"
expect "the application VM's certificate" 200 "$(at app1 /ready)"
expect "no certificate: no handshake" 000 "$(at none /ready)"
expect "another CA's: no handshake" 000 "$(at stranger /ready)"
expect "a worker's certificate may push" 204 "$(at worker1 /loki/api/v1/push -H 'content-type: application/json' \
  --data "{\"streams\":[{\"stream\":{\"verify\":\"obs-tls\",\"from\":\"worker\"},\"values\":[[\"$(date +%s%N)\",\"w\"]]}]}")"
expect "and may read every line (push and query share the port)" 200 "$(at worker1 /loki/api/v1/labels)"
expect "any OU of the CA too (OU=admin): Loki tells no subject apart" 200 "$(at admin /loki/api/v1/labels)"
expect "plain HTTP on the TLS port" 400 \
  "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://$LIP:3100/ready")"
expect "the name verified: the certificate under another name refused by curl" 000 \
  "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 --cacert "$D/machines.crt" --cert "$S/app1.pem" \
    --resolve "other:3100:$LIP" https://other:3100/ready)"

echo "== (d) Alloy ($ALLOY) pushing over TLS"
L=$D/logs
mkdir -p "$L" && chmod 755 "$L"
for c in good nocert wrongca plain; do : >"$L/$c.log"; done
chmod 644 "$L"/*
W=/agent/worker
{
  for c in good nocert wrongca plain; do
    case $c in
      good) url=https://qd-app:3100 tls="ca_file = \"$W/ca.crt\"
      cert_file = \"$W/machine.pem\"
      key_file = \"$W/machine.pem\"" ;;
      nocert) url=https://qd-app:3100 tls="ca_file = \"$W/ca.crt\"" ;;
      wrongca) url=https://qd-app:3100 tls="ca_file = \"/src/other.crt\"
      cert_file = \"$W/machine.pem\"
      key_file = \"$W/machine.pem\"" ;;
      plain) url=http://qd-app:3100 tls="" ;;
    esac
    cat <<ALLOY
local.file_match "$c" {
  path_targets = [{"__path__" = "/logs/$c.log", "case" = "$c"}]
}
loki.source.file "$c" {
  targets    = local.file_match.$c.targets
  forward_to = [loki.write.$c.receiver]
}
loki.write "$c" {
  endpoint {
    url = "$url/loki/api/v1/push"
    batch_wait = "500ms"
    tls_config {
      $tls
    }
  }
}
ALLOY
  done
} >"$D/config.alloy"
cp "$D/other.crt" "$S/other.crt" && chmod 644 "$D/config.alloy" "$S/other.crt"
docker run -d --name "$NAME-alloy" --network "$NET" -v "$VOL:/agent:ro" -v "$S:/src:ro" -v "$L:/logs:ro" \
  -v "$D/config.alloy:/etc/alloy/config.alloy:ro" "$ALLOY" run /etc/alloy/config.alloy \
  --storage.path=/tmp/alloy --disable-reporting >/dev/null || { echo "could not start $ALLOY"; exit 1; }
ALLOY_STARTED=$(docker inspect -f '{{.State.StartedAt}}' "$NAME-alloy")
# arrived CASE MARK: whether Loki holds MARK in CASE's stream within 20 s.
arrived() {
  local q
  q=$(python3 -I -c 'import sys, urllib.parse as u; print(u.quote(sys.argv[1]))' "{case=\"$1\"} |= \"$2\"")
  for _ in $(seq "${3:-20}"); do
    at app1 "/loki/api/v1/query_range?query=$q&since=10m" >/dev/null
    grep -q "$2" "$D/at.out" && { echo yes; return; }
    sleep 1
  done
  echo no
}
line() { echo "line $2" >>"$L/$1.log"; }
for c in good nocert wrongca plain; do line $c "$c-1"; done
expect "with the CA and the worker's certificate: the line arrives" yes "$(arrived good good-1 40)"
expect "with no client certificate: it does not" no "$(arrived nocert nocert-1 5)"
expect "trusting another CA: it does not" no "$(arrived wrongca wrongca-1 5)"
expect "plain HTTP: it does not" no "$(arrived plain plain-1 5)"
agent worker stranger
sleep 4
line good good-2
alloy_after_other=$(arrived good good-2 15)
expect "its client certificate replaced by another CA's: the next line is refused, the file read again" \
  no "$alloy_after_other"
agent worker worker2
sleep 4
line good good-3
expect "the renewed certificate of the CA in its place: the next line" yes "$(arrived good good-3 60)"
expect "Alloy never restarted" "0 $ALLOY_STARTED" "$(docker inspect -f '{{.RestartCount}} {{.State.StartedAt}}' "$NAME-alloy")"

echo "== (e) Grafana ($GRAFANA)'s Loki datasource over TLS, files provisioned"
P=$D/prov
mkdir -p "$P/datasources" && chmod -R 755 "$P"
cat >"$P/datasources/loki.yml" <<'YML'
apiVersion: 1
datasources:
  - name: Loki
    uid: loki
    type: loki
    access: proxy
    url: https://qd-app:3100
    jsonData:
      tlsAuth: true
      tlsAuthWithCACert: true
    secureJsonData:
      tlsCACert: $__file{/agent/grafana/ca.crt}
      tlsClientCert: $__file{/agent/grafana/machine.pem}
      tlsClientKey: $__file{/agent/grafana/machine.pem}
  - name: Loki without a certificate
    uid: loki-nocert
    type: loki
    access: proxy
    url: https://qd-app:3100
    jsonData:
      tlsAuthWithCACert: true
    secureJsonData:
      tlsCACert: $__file{/agent/grafana/ca.crt}
YML
chmod 644 "$P/datasources/loki.yml"
GPW=$(openssl rand -hex 16)
docker run -d --name "$NAME-grafana" --network "$NET" --user 65532:65532 \
  --tmpfs /var/lib/grafana:uid=65532,gid=65532,exec -v "$VOL:/agent:ro" -v "$P:/etc/grafana/provisioning:ro" \
  -e GF_SECURITY_ADMIN_PASSWORD="$GPW" -e GF_ANALYTICS_REPORTING_ENABLED=false \
  -e GF_ANALYTICS_CHECK_FOR_UPDATES=false -e GF_LOG_LEVEL=warn "$GRAFANA" >/dev/null ||
  { echo "could not start $GRAFANA"; exit 1; }
GIP=$(ip "$NAME-grafana")
gf() { curl -s -o "$D/gf.out" -w '%{http_code}' --max-time 20 -u "admin:$GPW" "$@"; }
for _ in $(seq 60); do [ "$(gf "http://$GIP:3000/api/health")" = 200 ] && break; sleep 1; done
expect "Grafana's own health, as the compose file's check asks it" 200 "$(gf "http://$GIP:3000/api/health")"
# health UID: the datasource's status (OK or ERROR); asked again if Grafana answers neither.
health() {
  local s
  for _ in 1 2 3; do
    gf "http://$GIP:3000/api/datasources/uid/$1/health" >/dev/null
    s=$(python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1])).get("status", "none"))' \
      "$D/gf.out" 2>/dev/null || echo none)
    [ "$s" = none ] || break
    sleep 2
  done
  echo "$s"
}
# The Loki datasource is a plugin Grafana starts as its own process, from its data directory.
for _ in $(seq 60); do [ "$(health loki)" = OK ] && break; sleep 1; done
expect "Grafana runs as the agent's user and reads its 0600 files" OK "$(health loki)"
expect "the datasource without a client certificate" ERROR "$(health loki-nocert)"
expect "only presence shows in the API" true "$(gf "http://$GIP:3000/api/datasources/uid/loki" >/dev/null;
  python3 -I -c 'import json, sys; f = json.load(open(sys.argv[1]))["secureJsonFields"]; print(str(f.get("tlsClientKey")).lower())' "$D/gf.out")"
agent grafana stranger
sleep 4
expect "its client certificate replaced by another CA's: still connects, the file not read again" OK "$(health loki)"
sleep 6
expect "nor 10 s later" OK "$(health loki)"
expect "the provisioning reload" 200 "$(gf -X POST "http://$GIP:3000/api/admin/provisioning/datasources/reload")"
# until HEALTH: the datasource's health once it is HEALTH, within 15 s.
until_health() { for _ in $(seq 15); do [ "$(health loki)" = "$1" ] && break; sleep 1; done; health loki; }
expect "after the reload the file is read: another CA's refused" ERROR "$(until_health ERROR)"
agent grafana app2
gf -X POST "http://$GIP:3000/api/admin/provisioning/datasources/reload" >/dev/null
expect "the renewed certificate of the CA, after a reload" OK "$(until_health OK)"

echo "== (f) Loki's own certificate and client CA replaced in place"
served() {
  openssl s_client -connect "$LIP:3100" -servername qd-app -cert "$S/worker2.pem" -CAfile "$D/machines.crt" \
    </dev/null 2>/dev/null | openssl x509 -noout -serial 2>/dev/null | cut -d= -f2
}
expect "Loki serves app1" "$(serial app1)" "$(served)"
agent app app2
t=0
for t in $(seq 10); do [ "$(served)" = "$(serial app2)" ] && break; sleep 1; done
expect "after machine.pem is renewed, with no restart (about ${t}s)" "$(serial app2)" "$(served)"
expect "another CA's client before its CA is trusted" 000 "$(at stranger /ready)"
agent app app2 both.crt
sleep 2
expect "another CA added to ca.crt: its client still refused, the CA read once at start" 000 "$(at stranger /ready)"
expect "Loki never restarted" "0 $LOKI_STARTED" "$(docker inspect -f '{{.RestartCount}} {{.State.StartedAt}}' "$NAME-loki")"

echo "   Loki $(docker run --rm "$LOKI" -version 2>/dev/null | sed -n 's/^loki, version \([^ ]*\).*/\1/p'),"\
  "Alloy $(docker run --rm "$ALLOY" --version 2>/dev/null | sed -n 's/^alloy, version \([^ ]*\).*/\1/p'),"\
  "Grafana ${GRAFANA##*:}, RustFS ${RUSTFS##*:}, Traefik ${TRAEFIK##*:}"
[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
