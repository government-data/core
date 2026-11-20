#!/bin/bash
# Verification of the internal entry point with the repository's own Traefik
# files (decision 0066, step 4): infra/traefik/traefik.yml as the static
# configuration, every file of infra/traefik/dynamic, watch.yml (so the API's
# public router is a file one, as in watch mode) and infra/traefik/internal.yml
# copied in as gen-dev-internal.sh does. It shows that the stack starts without
# a CA, that a client without a certificate of the CA cannot reach
# /api/internal/machines/renew on either entry point, that one with it does and
# the API gets X-Forwarded-Tls-Client-Cert, that the OIDC token path is
# reached there with the certificate too (step 5) while on the public entry
# point it gets no certificate header, forged or not, that the worker's routes
# are reached on the internal entry point with a certificate (step 7) and stay
# on the public one too, and that Trino's rules and its hold are reached on
# the internal one with the coordinator's certificate (step 9), the rules
# staying on the public one for the dev stack's key, with no certificate
# header there.
#
# It runs its own throwaway Traefik (traefik:v3.6) in front of traefik/whoami
# as the API, with throwaway CAs; nothing of the dev stack is touched (no
# Docker socket: the Docker provider finds nothing). Run from the repository
# root:
#
#   bash tools/verify/traefik_internal.sh
#
# Needs docker, curl, openssl and python3.
set -u
export LC_ALL=C
IMAGE=${TRAEFIK_IMAGE:-traefik:v3.6}
WHOAMI=${WHOAMI_IMAGE:-traefik/whoami:v1.11}
NAME=qd-verify-traefik-internal
BACK=qd-verify-traefik-internal-api
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
D=$(mktemp -d)
chmod 755 "$D"
trap 'docker rm -f "$NAME" "$BACK" >/dev/null 2>&1; rm -rf "$D"' EXIT
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

# --- Certificates: the installation's CA, another one, the public dev CA ------
ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/O=qd/CN=qd verify $1" -addext "basicConstraints=critical,CA:TRUE" \
    -keyout "$D/$1.key" -out "$D/$1.crt" 2>/dev/null
}
cert() { # FILE CA SUBJECT USAGE SAN
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$3" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=%s\nsubjectAltName=%s\n' "$4" "$5" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/$2.crt" -CAkey "$D/$2.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
}
ca machines
ca other
ca dev
cert local dev "/CN=localhost" serverAuth DNS:localhost
cert internal machines "/O=qd/CN=internal.localhost" serverAuth DNS:internal.localhost
cert worker machines "/O=qd/OU=worker/CN=worker-01" clientAuth DNS:worker-01
cert stranger other "/O=qd/OU=worker/CN=worker-01" clientAuth DNS:worker-01
cat "$D/dev.crt" "$D/machines.crt" >"$D/trust.crt"

Y=$D/dynamic C=$D/certs
mkdir -p "$Y" "$C" "$D/le" && chmod 755 "$Y" "$C" "$D/le"
cp "$ROOT"/infra/traefik/dynamic/*.yml "$ROOT/infra/traefik/watch.yml" "$Y/"
cp "$D/local.crt" "$D/local.key" "$C/"
chmod 644 "$Y"/* "$C"/*
put() { cp "$1" "$Y/.$2.new" && chmod 644 "$Y/.$2.new" && mv -f "$Y/.$2.new" "$Y/$2"; }

# --- The API (whoami on 8080, where watch.yml sends /api) and Traefik ---------
docker rm -f "$NAME" "$BACK" >/dev/null 2>&1
docker run -d --name "$BACK" "$WHOAMI" --port 8080 >/dev/null || { echo "could not start $WHOAMI"; exit 1; }
BIP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$BACK")
docker run -d --name "$NAME" --add-host "host.docker.internal:$BIP" --add-host "api:$BIP" \
  -e TRAEFIK_CERTIFICATESRESOLVERS_LETSENCRYPT_ACME_EMAIL=verify@example.com \
  -e APP_DOMAIN=localhost -e INTERNAL_DOMAIN=internal.localhost \
  -e INTERNAL_API_SERVICE=api-host@file -e TRINO_RULES_SERVICE=api-host@file \
  -v "$ROOT/infra/traefik/traefik.yml:/etc/traefik/traefik.yml:ro" \
  -v "$Y:/etc/traefik/dynamic:ro" -v "$C:/etc/traefik/certs:ro" -v "$D/le:/letsencrypt" \
  "$IMAGE" --configFile=/etc/traefik/traefik.yml >/dev/null || { echo "could not start $IMAGE"; exit 1; }
IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$NAME")

# get PORT SNI PATH CERT [CURL OPTION...]: the status (000 and the TLS alert
# when the handshake fails), the Host being SNI; the body is in $D/body.
get() {
  local port=$1 sni=$2 path=$3 c=$4 args=() code
  shift 4
  [ "$c" = - ] || args=(--cert "$D/$c.crt" --key "$D/$c.key")
  code=$(curl -sS -o "$D/body" -w '%{http_code}' --max-time 5 --cacert "$D/trust.crt" \
    --connect-to "$sni:$port:$IP:$port" "${args[@]}" "$@" "https://$sni:$port$path" 2>"$D/err")
  if [ "$code" = 000 ]; then echo "000 $(grep -o 'alert [a-z ]*' "$D/err" | head -1)"; else echo "$code"; fi
}
pub() { get 443 localhost "$@"; }
int() { get 9443 internal.localhost "$@"; }
# Whether the last answer was the API's, and for which path.
api() { grep -m1 '^GET ' "$D/body" | tr -d '\r' | cut -d' ' -f2; }
RENEW=/api/internal/machines/renew
for _ in $(seq 30); do [ "$(pub /api/probe -)" = 200 ] && break; sleep 1; done

echo "== (a) No CA yet: only the public entry point answers"
expect "public, /api/probe reaches the API without a certificate" "200 /probe" "$(pub /api/probe -) $(api)"
r=$(int "$RENEW" worker -k)
expect "internal, a certificate, before internal.yml: no router" "404 " "$r $(api)"
r=$(pub "$RENEW" -)
expect "public, the renew path: not the API" "" "$(api)"
echo "   (public answered $r: the web's catch-all)"

echo "== (b) internal.yml and its server certificate, but no CA file"
put "$D/internal.key" internal.key
put "$D/internal.crt" internal.crt
put "$ROOT/infra/traefik/internal.yml" internal.yml
sleep 4
# Traefik does not refuse to start: it builds no TLS configuration for the
# router ("invalid certificate(s) content" in its log), and the handshake fails.
expect "internal, the CA file missing: the handshake fails, with or without a certificate" "000 000 " \
  "$(int "$RENEW" - -k | cut -c1-3) $(int "$RENEW" worker -k | cut -c1-3) $(api)"
expect "public still serves" "200 /probe" "$(pub /api/probe -) $(api)"

echo "== (c) The CA in the watched directory"
put "$D/machines.crt" machines-ca.crt
touch "$Y/internal.yml"
for _ in $(seq 15); do [ "$(int "$RENEW" worker)" = 200 ] && break; sleep 1; done
expect "internal, a certificate of the CA: the API, without /api" "200 /internal/machines/renew" \
  "$(int "$RENEW" worker) $(api)"
first=$(grep -i '^X-Forwarded-Tls-Client-Cert:' "$D/body" | tr -d '\r' | sed 's/^[^:]*: //' |
  python3 -I -c 'import sys,urllib.parse; print(urllib.parse.unquote(sys.stdin.read().strip()).split(",")[0])')
serial=$(printf -- '-----BEGIN CERTIFICATE-----\n%s\n-----END CERTIFICATE-----\n' "$first" | fold -w 64 |
  openssl x509 -noout -serial 2>&1)
expect "and X-Forwarded-Tls-Client-Cert's first entry is that certificate" \
  "$(openssl x509 -noout -serial -in "$D/worker.crt")" "$serial"
expect "no Info header (pem only)" "" "$(grep -i '^X-Forwarded-Tls-Client-Cert-Info:' "$D/body")"
expect "internal, no certificate" "000 alert certificate required" "$(int "$RENEW" -)"
expect "internal, a certificate of another CA" "000 alert unknown ca" "$(int "$RENEW" stranger)"
expect "internal, a certificate, another API path: no router" 404 "$(int /api/probe worker)"
expect "internal port, the public SNI, no certificate: no router" 404 "$(get 9443 localhost "$RENEW" -)"
expect "internal port, the public SNI with the internal Host, no certificate" 421 \
  "$(get 9443 localhost "$RENEW" - -H 'Host: internal.localhost')"
r=$(pub "$RENEW" worker)
expect "public, the renew path even with a certificate: not the API" "" "$(api)"
echo "   (public answered $r)"
r=$(pub "$RENEW" - -H 'X-Forwarded-Tls-Client-Cert: forged')
expect "public, the renew path with a forged header: not the API" "" "$(api)"
echo "== (d) The machines' token (step 5): on the internal entry point with the certificate"
TOKEN=/api/oauth/token
expect "internal, the token path with a certificate: the API, without /api" "200 /oauth/token" \
  "$(int "$TOKEN" worker) $(api)"
expect "and with the certificate header" "yes" \
  "$(grep -qi '^X-Forwarded-Tls-Client-Cert:' "$D/body" && echo yes || echo no)"
expect "internal, the token path without a certificate" "000 alert certificate required" "$(int "$TOKEN" -)"
expect "internal, the token path with another CA's" "000 alert unknown ca" "$(int "$TOKEN" stranger)"
expect "public, the token path stays for people and secret clients" "200 /oauth/token" \
  "$(pub "$TOKEN" - -H 'X-Forwarded-Tls-Client-Cert: forged') $(api)"
expect "but a forged certificate header does not arrive" "no" \
  "$(grep -qi '^X-Forwarded-Tls-Client-Cert:' "$D/body" && echo yes || echo no)"
pub "$TOKEN" worker >/dev/null
expect "nor a real certificate's, sent on the public port" "no" \
  "$(grep -qi '^X-Forwarded-Tls-Client-Cert' "$D/body" && echo yes || echo no)"

echo "== (e) The worker's routes (step 7): on the internal entry point with the certificate"
JOBS=/api/internal/jobs/t/j
expect "internal, a job's details with a certificate: the API, without /api" "200 /internal/jobs/t/j" \
  "$(int "$JOBS" worker) $(api)"
expect "and with the certificate header" "yes" \
  "$(grep -qi '^X-Forwarded-Tls-Client-Cert:' "$D/body" && echo yes || echo no)"
expect "internal, a job's report with a certificate" "200 POST /internal/jobs/t/j/report" \
  "$(int "$JOBS/report" worker -X POST -d '{}') $(grep -m1 '^POST ' "$D/body" | tr -d '\r' | cut -d' ' -f1,2)"
expect "internal, the jobs' path without a certificate" "000 alert certificate required" "$(int "$JOBS" -)"
expect "internal, the jobs' path with another CA's" "000 alert unknown ca" "$(int "$JOBS" stranger)"
expect "internal port, the public SNI, the jobs' path: no router" 404 "$(get 9443 localhost "$JOBS" -)"
expect "public, the worker's routes stay (until the dev stack moves, step 8)" "200 /internal/jobs/t/j" \
  "$(pub "$JOBS" -) $(api)"
echo "== (f) Trino's rules and its hold (step 9): with the coordinator's certificate"
cert trino machines "/O=qd/OU=trino-coordinator/CN=trino-1" clientAuth DNS:trino-1
RULES=/api/internal/trino/rules
expect "internal, the rules with a certificate: the API, without /api" "200 /internal/trino/rules" \
  "$(int "$RULES" trino) $(api)"
expect "and with the certificate header, no query" "yes" \
  "$(grep -qi '^X-Forwarded-Tls-Client-Cert:' "$D/body" && echo yes || echo no)"
expect "internal, the rules without a certificate" "000 alert certificate required" "$(int "$RULES" -)"
expect "internal, the rules with another CA's" "000 alert unknown ca" "$(int "$RULES" stranger)"
expect "internal, a path beside the rules: no router" 404 "$(int /api/internal/trino/other trino)"
expect "internal, the hold with a certificate: the machines' router" "200 POST /internal/machines/trino/hold" \
  "$(int /api/internal/machines/trino/hold trino -X POST) $(grep -m1 '^POST ' "$D/body" | tr -d '\r' | cut -d' ' -f1,2)"
r=$(pub "$RULES" - -H 'X-Forwarded-Tls-Client-Cert: forged')
expect "public, Trino's rules route stays (the dev stack's key)" "200 /internal/trino/rules" "$r $(api)"
expect "but a forged certificate header does not arrive there" "no" \
  "$(grep -qi '^X-Forwarded-Tls-Client-Cert' "$D/body" && echo yes || echo no)"
pub "$RULES" trino >/dev/null
expect "nor the coordinator's real certificate, sent on the public port" "no" \
  "$(grep -qi '^X-Forwarded-Tls-Client-Cert' "$D/body" && echo yes || echo no)"
r=$(pub /api/internal/machines/trino/hold trino -X POST)
expect "public, the hold: not the API" "" "$(grep -m1 '^POST ' "$D/body" | tr -d '\r' | cut -d' ' -f2)"
echo "   (public answered $r)"
expect "the container was never restarted" 0 "$(docker inspect -f '{{.RestartCount}}' "$NAME")"
docker logs "$NAME" 2>&1 | grep -i 'machines\|internal\|ca\.crt' | grep -iv 'docker' | cut -c1-220 | head -5

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
