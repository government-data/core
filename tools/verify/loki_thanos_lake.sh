#!/bin/bash
# Verification for Loki's move to the thanos object store client (decision
# 0066 point 12, the user's ruling of 2026-11-13; decision 0026 "To verify":
# Loki to RustFS with mutual TLS), with the repository's loki.yml:
#
# (a) Lines written by the older `s3` client (loki.yml as it was: its
#     storage block put back) are read by the thanos client from the same
#     bucket, chunks and index, by a Loki with nothing of its own (a fresh
#     /loki); and lines the thanos client wrote are read by the older one,
#     so going back is possible.
# (b) The thanos client through the lake's gate as infra/rustfs/gate.yml
#     runs it (gate/entrypoint.sh, as uid 10001, with forwardAuth): Loki
#     presents the application VM's machine.pem, the gate names it to the
#     check, and Loki reads the lines above and writes new ones. The API's
#     check is stood in by a Traefik requiring the gate's certificate (the
#     application's `lakegate` entry point) that answers 200 only for a
#     machine named with OU=app; RustFS sits behind the gate over plain
#     HTTP here (QD_GATE_UPSTREAM), not TLS as with certificate.yml.
#
# Its own network (qd-verify-thanos-net), containers (qd-verify-thanos-*)
# and volume; no published port; nothing of the dev stack is touched.
# Throwaway CA, keys and S3 keys, removed at the end; only counts and states
# are printed. From anywhere:
#
#   bash tools/verify/loki_thanos_lake.sh
#
# Needs docker, curl, openssl and python3.
set -u
export LC_ALL=C
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
img() { sed -n "s/^ *image: \${$1:-\(.*\)}$/\1/p" "$2"; }
LOKI=$(img LOKI_IMAGE "$ROOT/infra/observability/docker-compose.yml")
RUSTFS=$(img RUSTFS_IMAGE "$ROOT/infra/rustfs/docker-compose.yml")
TRAEFIK=traefik:v3.6
WHOAMI=traefik/whoami:v1.11
CURL=curlimages/curl:8.11.1
HELPER=alpine:3.20
NAME=qd-verify-thanos
NET=$NAME-net
VOL=$NAME-agent
ALL="$NAME-rustfs $NAME-loki $NAME-gate $NAME-api $NAME-whoami"
D=$(mktemp -d)
cleanup() {
  # shellcheck disable=SC2086
  docker rm -f $ALL >/dev/null 2>&1
  docker volume rm -f "$VOL" >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
cleanup
mkdir -p "$D" && chmod 755 "$D"
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }
ip() { docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$1"; }

# --- Certificates ----------------------------------------------------------------
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/O=qd/CN=qd verify machines" -addext "basicConstraints=critical,CA:TRUE" \
  -keyout "$D/ca.key" -out "$D/ca.crt" 2>/dev/null
cert() { # FILE SUBJECT DNS: FILE.pem, the certificate then its PKCS#8 key
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$2" \
    -keyout "$D/$1.k" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:%s\n' "$3" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/ca.crt" -CAkey "$D/ca.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  openssl pkey -in "$D/$1.k" -out "$D/$1.key"
  cat "$D/$1.crt" "$D/$1.key" >"$D/$1.pem"
  rm -f "$D/$1.k" "$D/$1.csr"
}
cert app "/O=qd/OU=app/CN=qd-app" qd-app
cert lake "/O=qd/OU=lake/CN=lake-1" lake-1
cert worker "/O=qd/OU=worker/CN=worker-1" worker-1
S=$D/src
mkdir -p "$S" && cp "$D"/*.pem "$D/ca.crt" "$S/" && chmod 755 "$S" && chmod 644 "$S"/*
docker network create "$NET" >/dev/null
docker volume create "$VOL" >/dev/null
# The agents' directories as the agents write them: the application VM's
# (uid 65532) and the lake node's (uid 10001), machine.pem 0600.
docker run --rm -v "$VOL:/agent" -v "$S:/src:ro" "$HELPER" sh -c '
  for d in app:65532 lake:10001; do n=${d%:*} u=${d#*:}; mkdir -p /agent/$n
    cp /src/ca.crt /agent/$n/ca.crt && cp /src/$n.pem /agent/$n/machine.pem
    chmod 700 /agent/$n && chmod 644 /agent/$n/ca.crt && chmod 600 /agent/$n/machine.pem
    chown -R $u:$u /agent/$n; done'

# --- The lake --------------------------------------------------------------------
KEY=qd$(openssl rand -hex 8)
SECRET=$(openssl rand -hex 16)
BUCKET=qd-observability
docker run -d --name "$NAME-rustfs" --network "$NET" --network-alias rustfs \
  -e RUSTFS_ACCESS_KEY="$KEY" -e RUSTFS_SECRET_KEY="$SECRET" -e RUSTFS_VOLUMES=/data \
  --tmpfs /data:uid=10001,gid=10001 "$RUSTFS" server --address :9000 /data >/dev/null ||
  { echo "could not start $RUSTFS"; exit 1; }
s3() { # METHOD PATH: the status; the body in $D/s3.out
  docker run --rm --network "$NET" "$CURL" -s -o - -w '\n%{http_code}' -X "$1" \
    --aws-sigv4 "aws:amz:us-east-1:s3" --user "$KEY:$SECRET" "http://rustfs:9000$2" >"$D/s3.out" 2>/dev/null
  tail -n1 "$D/s3.out"
}
for _ in $(seq 30); do [ "$(s3 PUT "/$BUCKET")" = 200 ] && break; sleep 1; done
keys() { s3 GET "/$BUCKET?list-type=2&prefix=$1" >/dev/null; grep -o '<Key>' "$D/s3.out" | wc -l; }

# --- Loki: the repository's loki.yml, and the same with the older client -----
cp "$ROOT/infra/observability/loki/loki.yml" "$D/thanos.yml"
python3 -I - "$D/thanos.yml" "$D/older.yml" <<'PY'
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
assert n == 1, "loki.yml's thanos storage block not found"
assert "use_thanos_objstore" not in new, "use_thanos_objstore left"
open(sys.argv[2], "w").write(new)
PY
chmod 644 "$D"/*.yml
expect "loki.yml uses the thanos client" yes "$(grep -q 'use_thanos_objstore: true' "$D/thanos.yml" && echo yes || echo no)"
# loki CONFIG ENDPOINT INSECURE FLAG...: a Loki with nothing of its own, plain HTTP.
loki() {
  local config=$1 endpoint=$2 insecure=$3
  shift 3
  docker rm -f "$NAME-loki" >/dev/null 2>&1
  docker run -d --name "$NAME-loki" --network "$NET" --user 65532:65532 \
    --tmpfs /loki:uid=65532,gid=65532 -v "$VOL:/agent:ro" -v "$D/$config:/etc/loki/loki.yml:ro" \
    -e LOKI_S3_ENDPOINT="$endpoint" -e LOKI_S3_INSECURE="$insecure" -e LOKI_S3_BUCKET=$BUCKET \
    -e LOKI_S3_ACCESS_KEY="$KEY" -e LOKI_S3_SECRET_KEY="$SECRET" \
    "$LOKI" -config.file=/etc/loki/loki.yml -config.expand-env=true -ingester.flush-on-shutdown=true "$@" \
    >/dev/null || { echo "could not start $LOKI"; exit 1; }
  LIP=$(ip "$NAME-loki")
  for _ in $(seq 90); do
    [ "$(curl -s -o /dev/null -w '%{http_code}' -m 3 "http://$LIP:3100/ready")" = 200 ] && return 0
    [ "$(docker inspect -f '{{.State.Running}}' "$NAME-loki")" = true ] || break
    sleep 1
  done
  docker logs "$NAME-loki" 2>&1 | grep -v ratestore | tail -3 | cut -c1-200
  return 1
}
START=$(($(date +%s) - 60))
push() { # MARK N: N lines "MARK-i" in the stream {verify="thanos",writer="MARK"}
  local i body
  for i in $(seq "$2"); do
    body=$(printf '{"streams":[{"stream":{"verify":"thanos","writer":"%s"},"values":[["%s","%s-%s"]]}]}' \
      "$1" "$(date +%s%N)" "$1" "$i")
    curl -s -o /dev/null -m 5 -H 'content-type: application/json' --data "$body" "http://$LIP:3100/loki/api/v1/push"
  done
}
# found MARK: how many of MARK's lines Loki returns.
found() {
  curl -s -m 30 -G "http://$LIP:3100/loki/api/v1/query_range" --data-urlencode "query={verify=\"thanos\",writer=\"$1\"}" \
    --data-urlencode "start=${START}000000000" --data-urlencode "end=$(($(date +%s) + 60))000000000" \
    --data-urlencode limit=1000 | python3 -I -c '
import json, sys
r = json.load(sys.stdin)["data"]["result"]
print(sum(len(s["values"]) for s in r))' 2>/dev/null || echo none
}
# done_writing: flush, then a graceful stop, which ships the index.
done_writing() {
  curl -s -o /dev/null -m 10 -X POST "http://$LIP:3100/flush"
  sleep 3
  docker stop -t 60 "$NAME-loki" >/dev/null
}

echo "== (a) The same bucket, written by one client and read by the other"
loki older.yml rustfs:9000 true || ko "the older client's Loki did not start"
push older 5
expect "the older client's Loki holds its 5 lines" 5 "$(found older)"
done_writing
expect "it wrote chunks and an index to the lake" "yes yes" \
  "$([ "$(keys fake/)" -gt 0 ] && echo yes || echo no) $([ "$(keys index/)" -gt 0 ] && echo yes || echo no)"
loki thanos.yml rustfs:9000 true || ko "the thanos client's Loki did not start"
expect "a fresh Loki with the thanos client reads the older client's lines from the lake" 5 "$(found older)"
push thanos 5
expect "and holds its own" 5 "$(found thanos)"
done_writing
loki older.yml rustfs:9000 true || ko "the older client's Loki did not start again"
expect "back to the older client: the thanos client's lines" 5 "$(found thanos)"
expect "and its own" 5 "$(found older)"
docker stop -t 60 "$NAME-loki" >/dev/null

echo "== (b) The thanos client through the gate's shape, forwardAuth included"
docker run -d --name "$NAME-whoami" --network "$NET" "$WHOAMI" --port 8080 >/dev/null
V=$D/api
mkdir -p "$V" && cp "$D/ca.crt" "$V/" && cp "$D/app.crt" "$V/api.crt" && cp "$D/app.key" "$V/api.key"
cat >"$V/api.yml" <<'YML'
http:
  routers:
    check:
      entryPoints: [lakegate]
      rule: "Path(`/api/internal/machines/lake/check`) && HeaderRegexp(`X-Forwarded-Tls-Client-Cert-Info`, `OU(%3D|=)app(%2C|,|%22|\")`)"
      tls: {}
      service: allowed
  services:
    allowed:
      loadBalancer:
        servers:
          - url: "http://qd-verify-thanos-whoami:8080"
tls:
  stores:
    default:
      defaultCertificate: {certFile: /v/api.crt, keyFile: /v/api.key}
  options:
    default:
      clientAuth:
        caFiles: [/v/ca.crt]
        clientAuthType: RequireAndVerifyClientCert
YML
chmod 755 "$V" && chmod 644 "$V"/*
docker run -d --name "$NAME-api" --network "$NET" --network-alias qd-app -v "$V:/v:ro" "$TRAEFIK" \
  --entrypoints.lakegate.address=:9444 --entrypoints.lakegate.forwardedHeaders.insecure=true \
  --providers.file.directory=/v --accesslog=true --accesslog.format=json --log.level=ERROR >/dev/null
install -m 755 "$ROOT/infra/rustfs/gate/entrypoint.sh" "$D/gate-entrypoint.sh"
docker run -d --name "$NAME-gate" --network "$NET" --network-alias lake-1 --init --user 10001:10001 \
  --entrypoint /etc/qd/gate/entrypoint.sh --tmpfs /run/qd-gate:uid=10001,gid=10001,mode=0700 \
  -v "$D/gate-entrypoint.sh:/etc/qd/gate/entrypoint.sh:ro" -v "$VOL:/agent:ro" \
  -e QD_GATE_SOURCE=/agent/lake -e QD_MACHINE_NAME=lake-1 -e QD_GATE_UPSTREAM=http://rustfs:9000 \
  -e QD_LAKE_CHECK_URL=https://qd-app:9444/api/internal/machines/lake/check "$TRAEFIK" >/dev/null
GIP=$(ip "$NAME-gate")
at_gate() { # PEM: the gate's status for a bucket listing presenting PEM (none: no certificate)
  local c=()
  [ "$1" != none ] && c=(--cert "$S/$1.pem")
  curl -s -o /dev/null -w '%{http_code}' -m 10 --cacert "$D/ca.crt" --resolve "lake-1:9000:$GIP" \
    "${c[@]}" "https://lake-1:9000/$BUCKET?list-type=2"
}
for _ in $(seq 30); do [ "$(at_gate worker)" != 000 ] && break; sleep 1; done
expect "the gate: no certificate, no handshake" 000 "$(at_gate none)"
expect "the gate: a worker refused by the check (the stand-in passes OU=app alone)" 404 "$(at_gate worker)"
expect "the gate: the application VM's certificate passes the check (unsigned: RustFS's 403)" 403 "$(at_gate app)"
T=-common.storage.object-store.s3.http
before=$(keys fake/)
loki thanos.yml lake-1:9000 false $T.tls-ca-path=/agent/app/ca.crt \
  $T.tls-cert-path=/agent/app/machine.pem $T.tls-key-path=/agent/app/machine.pem ||
  ko "the thanos client's Loki did not start behind the gate"
expect "through the gate it reads the lines both clients wrote" "5 5" "$(found older) $(found thanos)"
push gate 5
done_writing
expect "and writes its own chunks" yes "$([ "$(keys fake/)" -gt "$before" ] && echo yes || echo no)"
expect "the check was asked for Loki's machine and answered 200" yes \
  "$(docker logs "$NAME-api" 2>/dev/null | grep -q '"DownstreamStatus":200' && echo yes || echo no)"
loki thanos.yml lake-1:9000 false $T.tls-ca-path=/agent/app/ca.crt \
  $T.tls-cert-path=/agent/app/machine.pem $T.tls-key-path=/agent/app/machine.pem ||
  ko "the thanos client's Loki did not start again"
expect "a fresh Loki behind the gate reads them back" 5 "$(found gate)"

echo "   Loki $(docker run --rm "$LOKI" -version 2>/dev/null | sed -n 's/^loki, version \([^ ]*\).*/\1/p'),"\
  "RustFS ${RUSTFS##*:}, Traefik ${TRAEFIK##*:}"
[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
