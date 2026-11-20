#!/bin/bash
# Verification of the application VM's machine certificate in its Traefik
# (decision 0066, points 10, 13 and 15), with the repository's own files:
# infra/traefik/traefik.yml, every file of infra/traefik/dynamic and
# internal.yml as the installer copies them, and certificate.yml's
# entrypoint (infra/traefik/machine/entrypoint.sh) with its settings. The
# agent's directory is a volume written as the agent writes it (uid 65532,
# machine.pem 0600, written aside and renamed). It shows that Traefik starts
# before the agent's file, the people's entry point answering; that once the
# file is there the internal entry point serves the machine's certificate
# under its name and the serversTransport `lake` presents it to an upstream
# that requires a client certificate of the CA (a Traefik standing for the
# lake's gate); and that after a renewal both use the renewed one with no
# restart, while requests already open go on.
#
# Its own containers (qd-verify-traefik-app, -gate and -api, traefik/whoami
# as the API and behind the gate) and volume (qd-verify-traefik-app-agent);
# no published port and no Docker socket; nothing of the dev stack is
# touched. Throwaway CAs and keys, made here and removed at the end; only
# states and whether serials are equal are printed. Run from the repository
# root:
#
#   bash tools/verify/traefik_app_certificate.sh
#
# Needs docker with Compose v2, curl, openssl and python3.
set -u
export LC_ALL=C
IMAGE=${TRAEFIK_IMAGE:-traefik:v3.6}
WHOAMI=${WHOAMI_IMAGE:-traefik/whoami:v1.11}
NAME=qd-verify-traefik-app
GATE=$NAME-gate
BACK=$NAME-api
VOL=$NAME-agent
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
D=$(mktemp -d)
chmod 755 "$D"
trap 'docker rm -f "$NAME" "$GATE" "$BACK" >/dev/null 2>&1; docker volume rm -f "$VOL" >/dev/null 2>&1; rm -rf "$D"' EXIT
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

# --- What certificate.yml sets, read from Compose itself ----------------------
S=$D/src
mkdir -p "$S" && chmod 755 "$S"
set_by=$(ACME_EMAIL=verify@example.com QD_MACHINE_DIR="$S" docker compose \
  -f "$ROOT/infra/traefik/docker-compose.yml" -f "$ROOT/infra/traefik/certificate.yml" \
  config --format json 2>/dev/null | python3 -I -c '
import json, sys
t = json.load(sys.stdin)["services"]["traefik"]
e = t["environment"]
v = [m["target"] for m in t["volumes"] if not m.get("read_only")]
print(t.get("init"), " ".join(t["entrypoint"]), e["INTERNAL_CERT_FILE"], e["INTERNAL_KEY_FILE"], " ".join(sorted(v)))')
CERT_FILE=/etc/traefik/dynamic/app-machine.pem
expect "certificate.yml: init, the entrypoint, the copy as the internal certificate, the watched directory writable" \
  "True /etc/qd/traefik/entrypoint.sh $CERT_FILE $CERT_FILE /etc/traefik/dynamic /letsencrypt" "$set_by"

# --- Certificates: the installation's CA, the public dev CA -------------------
ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/O=qd/CN=qd verify $1" -addext "basicConstraints=critical,CA:TRUE" \
    -keyout "$D/$1.key" -out "$D/$1.crt" 2>/dev/null
}
cert() { # FILE CA SUBJECT USAGE SAN: also FILE.pem, the certificate then the key
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$3" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=%s\nsubjectAltName=%s\n' "$4" "$5" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/$2.crt" -CAkey "$D/$2.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  cat "$D/$1.crt" "$D/$1.key" >"$D/$1.pem"
}
ca machines
ca dev
cert local dev "/CN=localhost" serverAuth DNS:localhost
# The application VM's certificate, as the CA signs it: its name its only DNS name.
cert app1 machines "/O=qd/OU=app/CN=qd-app" serverAuth,clientAuth DNS:qd-app
cert app2 machines "/O=qd/OU=app/CN=qd-app" serverAuth,clientAuth DNS:qd-app
cert worker machines "/O=qd/OU=worker/CN=worker-1" serverAuth,clientAuth DNS:worker-1
cert lake machines "/O=qd/OU=lake/CN=lake-1" serverAuth,clientAuth DNS:lake-1
cat "$D/dev.crt" "$D/machines.crt" >"$D/trust.crt"
serial() { openssl x509 -noout -serial -in "$D/$1.crt" | cut -d= -f2; }
dec() { python3 -I -c 'import sys; print(int(sys.argv[1], 16))' "$1"; }
cp "$D/app1.pem" "$D/app2.pem" "$D/machines.crt" "$S/" && chmod 644 "$S"/*

# --- The agent's directory: written as the agent, aside and renamed -----------
docker rm -f "$NAME" "$GATE" "$BACK" >/dev/null 2>&1
docker volume rm -f "$VOL" >/dev/null 2>&1
docker volume create "$VOL" >/dev/null
docker run --rm -v "$VOL:/agent" --entrypoint sh "$IMAGE" -c 'chown 65532:65532 /agent && chmod 700 /agent'
agent() { # PEM: the agent's machine.pem from now on
  docker run --rm --user 65532:65532 -v "$VOL:/agent" -v "$S:/src:ro" --entrypoint sh "$IMAGE" -c "
    cp /src/machines.crt /agent/.ca.crt.new && mv -f /agent/.ca.crt.new /agent/ca.crt &&
    cp /src/$1.pem /agent/.machine.pem.new && chmod 600 /agent/.machine.pem.new &&
    mv -f /agent/.machine.pem.new /agent/machine.pem"
}

# --- The API and the upstream that requires a client certificate -------------
docker run -d --name "$BACK" "$WHOAMI" --port 8080 >/dev/null || { echo "could not start $WHOAMI"; exit 1; }
BIP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$BACK")
G=$D/gate
mkdir -p "$G" && cp "$D/lake.crt" "$D/lake.key" "$G/" && cp "$D/machines.crt" "$G/ca.crt"
cat >"$G/gate.yml" <<YML
http:
  routers:
    lake:
      entryPoints: [lake]
      rule: "PathPrefix(\`/\`)"
      tls: {}
      middlewares: [who]
      service: back
  middlewares:
    who:
      passTLSClientCert:
        info:
          serialNumber: true
  services:
    back:
      loadBalancer:
        servers:
          - url: "http://$BIP:8080"
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
chmod 755 "$G" && chmod 644 "$G"/*
docker run -d --name "$GATE" -v "$G:/g:ro" "$IMAGE" --entrypoints.lake.address=:9000 \
  --providers.file.directory=/g --log.level=ERROR >/dev/null || { echo "could not start the gate"; exit 1; }
GIP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$GATE")

# --- The application's Traefik, as certificate.yml runs it --------------------
Y=$D/dynamic C=$D/certs
mkdir -p "$Y" "$C" "$D/le" && chmod 755 "$Y" "$C" "$D/le"
cp "$ROOT"/infra/traefik/dynamic/*.yml "$ROOT/infra/traefik/internal.yml" "$Y/"
cp "$D/machines.crt" "$Y/machines-ca.crt"
printf 'http:\n  services:\n    verify-api:\n      loadBalancer:\n        servers:\n          - url: "http://%s:8080"\n' "$BIP" >"$Y/verify-api.yml"
cp "$D/local.crt" "$D/local.key" "$C/"
chmod 644 "$Y"/* "$C"/*
docker run -d --name "$NAME" --init --entrypoint /etc/qd/traefik/entrypoint.sh --add-host "lake-1:$GIP" \
  -e TRAEFIK_CERTIFICATESRESOLVERS_LETSENCRYPT_ACME_EMAIL=verify@example.com \
  -e APP_DOMAIN=localhost -e INTERNAL_DOMAIN=qd-app \
  -e INTERNAL_CERT_FILE="$CERT_FILE" -e INTERNAL_KEY_FILE="$CERT_FILE" -e QD_TRAEFIK_CHECK_EVERY=1 \
  -e INTERNAL_API_SERVICE=verify-api@file -e TRINO_RULES_SERVICE=verify-api@file \
  -e LAKE_TRANSPORT=lake@file -e LAKE_SERVER_NAME=lake-1 -e RUSTFS_URL=https://lake-1:9000 \
  -v "$ROOT/infra/traefik/traefik.yml:/etc/traefik/traefik.yml:ro" \
  -v "$ROOT/infra/traefik/machine/entrypoint.sh:/etc/qd/traefik/entrypoint.sh:ro" \
  -v "$Y:/etc/traefik/dynamic" -v "$C:/etc/traefik/certs:ro" -v "$D/le:/letsencrypt" \
  -v "$VOL:/run/qd-node:ro" \
  "$IMAGE" --configFile=/etc/traefik/traefik.yml >/dev/null || { echo "could not start $IMAGE"; exit 1; }
IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$NAME")
STARTED=$(docker inspect -f '{{.State.StartedAt}}' "$NAME")

# get PORT SNI PATH OUT [CURL OPTION...]: the status, the body in OUT.
get() {
  local port=$1 sni=$2 path=$3 out=$4
  shift 4
  curl -s -o "$out" -w '%{http_code}' --max-time 20 --cacert "$D/trust.crt" \
    --connect-to "$sni:$port:$IP:$port" "$@" "https://$sni:$port$path"
}
int() { get 9443 qd-app "$1" "$2" --cert "$D/worker.crt" --key "$D/worker.key"; }
lake() { get 443 localhost "/iceberg$1" "$2"; }
# The serial the internal entry point serves, compared with FILE's.
served() {
  local s
  s=$(openssl s_client -connect "$IP:9443" -servername qd-app -cert "$D/worker.crt" -key "$D/worker.key" \
    -CAfile "$D/machines.crt" </dev/null 2>/dev/null | openssl x509 -noout -serial 2>/dev/null | cut -d= -f2)
  [ "$s" = "$(serial "$1")" ] && echo "$1's" || echo "${s:+another}${s:-none}"
}
# The serial the upstream saw, in the last body, compared with FILE's.
presented() {
  local s
  s=$(tr -d '\r' <"$2" | sed -n 's/^X-Forwarded-Tls-Client-Cert-Info: .*SerialNumber%3D%22\([0-9]*\)%22.*/\1/p;s/^X-Forwarded-Tls-Client-Cert-Info: .*SerialNumber="\([0-9]*\)".*/\1/p' | head -1)
  [ "$s" = "$(dec "$(serial "$1")")" ] && echo "$1's" || echo "${s:+another}${s:-none}"
}
for _ in $(seq 30); do [ "$(get 443 localhost / "$D/body")" != 000 ] && break; sleep 1; done

echo "== (a) Started before the agent's file"
expect "the public entry point answers (no router for /: Traefik's 404)" 404 "$(get 443 localhost / "$D/body")"
expect "the internal entry point: no certificate to serve, no handshake" 000 "$(int /api/internal/machines/renew "$D/body")"
expect "no copy yet" no "$([ -e "$Y/app-machine.pem" ] && echo yes || echo no)"

echo "== (b) The agent's first certificate"
agent app1
for _ in $(seq 15); do [ "$(served app1)" = "app1's" ] && break; sleep 1; done
expect "copied into the watched directory, 0600" 600 "$(stat -c %a "$Y/app-machine.pem" 2>/dev/null)"
expect "the internal entry point serves the machine's certificate" "app1's" "$(served app1)"
expect "a worker reaches the API there, the name verified against the CA" "200 /internal/machines/renew" \
  "$(int /api/internal/machines/renew "$D/body") $(grep -m1 '^GET ' "$D/body" | tr -d '\r' | cut -d' ' -f2)"
expect "the public entry point's lake route reaches the upstream that requires a certificate" 200 \
  "$(lake /v1/config "$D/body")"
expect "which saw the machine's certificate presented" "app1's" "$(presented app1 "$D/body")"

echo "== (c) A renewal, with requests open"
int "/api/internal/machines/renew?wait=6s" "$D/open-int" >"$D/open-int.code" &
p1=$!
lake "/v1/slow?wait=6s" "$D/open-lake" >"$D/open-lake.code" &
p2=$!
sleep 1
agent app2
t=0
for t in $(seq 15); do [ "$(served app2)" = "app2's" ] && break; sleep 1; done
echo "   (served after about ${t}s, QD_TRAEFIK_CHECK_EVERY=1)"
expect "the internal entry point serves the renewed certificate" "app2's" "$(served app2)"
expect "the copy is the renewed one" yes \
  "$(docker run --rm -v "$Y:/y:ro" -v "$S:/s:ro" --entrypoint sh "$IMAGE" -c 'cmp -s /y/app-machine.pem /s/app2.pem && echo yes || echo no')"
expect "the lake route still answers" 200 "$(lake /v1/config "$D/body")"
expect "and presents the renewed certificate" "app2's" "$(presented app2 "$D/body")"
expect "a worker still reaches the API on the internal entry point" 200 "$(int /api/internal/machines/renew "$D/body")"
wait "$p1" "$p2"
expect "the request open on the internal entry point went on" 200 "$(cat "$D/open-int.code")"
expect "the request open through the lake route went on (it had presented the first one)" "200 app1's" \
  "$(cat "$D/open-lake.code") $(presented app1 "$D/open-lake")"
expect "Traefik never restarted" "0 $STARTED" \
  "$(docker inspect -f '{{.RestartCount}} {{.State.StartedAt}}' "$NAME")"
expect "the entrypoint said so, for the first file and the renewed one" 2 "$(docker logs "$NAME" 2>&1 | grep -c 'machine.pem copied')"
echo "   Traefik $(docker run --rm "$IMAGE" version 2>/dev/null | sed -n 's/^Version: *//p')"
echo "   Traefik's errors (before the agent's file, the copy it names was not there):"
docker logs "$NAME" 2>&1 | grep -i '"level":"error"' | grep -iv docker | cut -c1-200 | head -5

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
