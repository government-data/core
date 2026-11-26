#!/bin/bash
# Verification for one published port on the application VM (ruling 71 of
# 2026-11-26, decision 0091): the web, the machines' internal entry point
# (client certificate of the installation's CA required), the lake gates'
# check and the services' metrics, all behind 443, chosen by SNI.
#
# Two ways are checked on one entry point, with traefik:v3.6 (the image of
# infra/traefik) and traefik/whoami as the backend:
#
#   (A) HTTP routers of both names on the public entry point, the internal
#       ones with TLS options requiring the client certificate;
#   (B) TCP routers by HostSNI with TLS passthrough from the public entry
#       point to the existing `internal` and `lakegate` entry points, bound
#       to Traefik's own loopback (published nowhere), with the PROXY
#       protocol (v2) so the client's address still reaches them.
#
# For each: a client with no SNI, an unknown SNI, the public SNI with the
# internal Host (domain fronting), with no certificate, a certificate of
# another CA, or the CA's. And what happens to a forwarded certificate
# header (X-Forwarded-Tls-Client-Cert-Info, which the lake check needs from
# the gate) on an entry point that does not trust forwarded headers.
#
#   bash tools/verify/traefik_one_port.sh
#
# Needs docker, curl and openssl.
set -u
export LC_ALL=C
IMAGE=${TRAEFIK_IMAGE:-traefik:v3.6}
WHOAMI=${WHOAMI_IMAGE:-traefik/whoami:v1.11}
NAME=qd-verify-one-port
BACK=qd-verify-one-port-whoami
D=$(mktemp -d)
chmod 700 "$D"
trap 'docker rm -f "$NAME" "$BACK" >/dev/null 2>&1; rm -rf "$D"' EXIT
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

# --- Certificates ------------------------------------------------------------
ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/CN=qd verify $1" -addext "basicConstraints=critical,CA:TRUE" \
    -keyout "$D/$1.key" -out "$D/$1.crt" 2>/dev/null
}
cert() { # FILE CA SUBJECT USAGE SANS
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$3" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=%s\nsubjectAltName=%s\n' "$4" "$5" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/$2.crt" -CAkey "$D/$2.key" -CAcreateserial \
    -extfile "$D/ext" -out "$D/$1.crt" -days 1 2>/dev/null
}
ca ca
ca other
cert public other "/CN=app.qd.test" serverAuth "DNS:app.qd.test"
cert machine ca "/O=qd/OU=app/CN=app-1" serverAuth "DNS:app-1,DNS:lakegate.app-1"
cert worker ca "/O=qd/OU=worker/CN=worker-7" clientAuth "DNS:worker-7"
cert stranger other "/O=qd/OU=worker/CN=worker-7" clientAuth "DNS:worker-7"
cat "$D/ca.crt" "$D/other.crt" >"$D/trust.crt"
C=$D/certs Y=$D/dynamic
mkdir "$C" "$Y" && chmod 755 "$C" "$Y"
for f in public.crt public.key machine.crt machine.key ca.crt; do cp "$D/$f" "$C/$f"; done
chmod 644 "$C"/*

# --- Traefik ----------------------------------------------------------------------
docker rm -f "$NAME" "$BACK" >/dev/null 2>&1
docker run -d --name "$BACK" "$WHOAMI" >/dev/null || { echo "could not start $WHOAMI"; exit 1; }
BIP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$BACK")
cat >"$Y/dynamic.yml" <<EOF
tls:
  certificates:
    - {certFile: /certs/public.crt, keyFile: /certs/public.key}
    - {certFile: /certs/machine.crt, keyFile: /certs/machine.key}
  stores:
    default:
      defaultCertificate: {certFile: /certs/public.crt, keyFile: /certs/public.key}
  options:
    machines:
      minVersion: VersionTLS12
      clientAuth:
        caFiles: [/certs/ca.crt]
        clientAuthType: RequireAndVerifyClientCert
tcp:
  routers:
    internal:
      entryPoints: [passthrough]
      rule: "HostSNI(\`app-1\`)"
      tls: {passthrough: true}
      service: internal
    lakegate:
      entryPoints: [passthrough]
      rule: "HostSNI(\`lakegate.app-1\`)"
      tls: {passthrough: true}
      service: lakegate
  services:
    internal:
      loadBalancer:
        serversTransport: proxy
        servers: [{address: "127.0.0.1:9443"}]
    lakegate:
      loadBalancer:
        serversTransport: proxy
        servers: [{address: "127.0.0.1:9444"}]
  serversTransports:
    proxy:
      proxyProtocol: {version: 2}
http:
  routers:
    # (A) and (B): the public name, no client certificate.
    public:
      entryPoints: [mixed, passthrough]
      rule: "Host(\`app.qd.test\`)"
      tls: {}
      service: whoami
    # (A): the internal name on the same entry point as the public one.
    mixed-internal:
      entryPoints: [mixed]
      rule: "Host(\`app-1\`)"
      tls: {options: machines}
      middlewares: [machine-certificate]
      service: whoami
    # (B): the internal entry point and the gates' one, as infra/traefik's.
    internal:
      entryPoints: [internal]
      rule: "Host(\`app-1\`)"
      tls: {options: machines}
      middlewares: [machine-certificate]
      service: whoami
    lake-check:
      entryPoints: [lakegate]
      rule: "Host(\`lakegate.app-1\`) && Path(\`/api/internal/machines/lake/check\`)"
      tls: {options: machines}
      middlewares: [machine-certificate]
      service: whoami
  middlewares:
    machine-certificate:
      passTLSClientCert: {pem: true}
  services:
    whoami:
      loadBalancer:
        servers: [{url: "http://$BIP:80"}]
EOF
chmod 644 "$Y/dynamic.yml"
docker run -d --name "$NAME" -v "$Y:/dynamic:ro" -v "$C:/certs:ro" "$IMAGE" \
  --entryPoints.mixed.address=:8443 --entryPoints.passthrough.address=:443 \
  --entryPoints.internal.address=127.0.0.1:9443 \
  --entryPoints.internal.proxyProtocol.trustedIPs=127.0.0.1/32 \
  --entryPoints.lakegate.address=127.0.0.1:9444 \
  --entryPoints.lakegate.proxyProtocol.trustedIPs=127.0.0.1/32 \
  --entryPoints.lakegate.forwardedHeaders.insecure=true \
  --providers.file.directory=/dynamic --log.level=INFO >/dev/null ||
  { echo "could not start $IMAGE"; exit 1; }
IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$NAME")
GW=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.Gateway}}{{end}}' "$NAME")

# get PORT SNI HOST CERT PATH [CURL OPTION...]; CERT `-` is none. The status,
# or 000 and the TLS alert; the body in $D/body.
get() {
  local port=$1 sni=$2 host=$3 c=$4 path=$5 args=() code
  shift 5
  [ "$c" = - ] || args=(--cert "$D/$c.crt" --key "$D/$c.key")
  code=$(curl -sS -o "$D/body" -w '%{http_code}' --max-time 5 --cacert "$D/trust.crt" -H "Host: $host" \
    --connect-to "$sni:$port:$IP:$port" "${args[@]}" "$@" "https://$sni:$port$path" 2>"$D/err")
  if [ "$code" = 000 ]; then echo "000 $(grep -o -E 'alert [a-z ]*|SSL_ERROR[A-Z_]*' "$D/err" | head -1)"; else echo "$code"; fi
}
header() { grep -i "^$1:" "$D/body" | tr -d '\r' | sed 's/^[^:]*: //'; }
for _ in $(seq 30); do [ "$(get 443 app-1 app-1 worker /)" = 200 ] && break; sleep 1; done
[ "$(get 443 app-1 app-1 worker /)" = 200 ] || { echo "Traefik did not start"; docker logs "$NAME" 2>&1 | tail -5; exit 1; }
CHECK=/api/internal/machines/lake/check
INFO=X-Forwarded-Tls-Client-Cert-Info
NAMED="Subject%3D%22OU%3Dworker%2CCN%3Dworker-9%22"

for ep in "8443 (A) per-router TLS options" "443 (B) SNI passthrough"; do
  p=${ep%% *}
  echo "== $ep"
  expect "internal name, the CA's certificate" 200 "$(get "$p" app-1 app-1 worker /)"
  expect "internal name, no certificate" "000 alert certificate required" "$(get "$p" app-1 app-1 - /)"
  expect "internal name, another CA's certificate" "000 alert unknown ca" "$(get "$p" app-1 app-1 stranger /)"
  expect "public name, no certificate" 200 "$(get "$p" app.qd.test app.qd.test - /)"
  expect "public name, no certificate, Host internal (domain fronting)" \
    "$([ "$p" = 8443 ] && echo 421 || echo 404)" "$(get "$p" app.qd.test app-1 - /)"
  expect "public name, the CA's certificate offered, Host internal" \
    "$([ "$p" = 8443 ] && echo 421 || echo 404)" "$(get "$p" app.qd.test app-1 worker /)"
  expect "no SNI (by address), Host internal" \
    "$([ "$p" = 8443 ] && echo 421 || echo 404)" "$(get "$p" "$IP" app-1 - / -k)"
  expect "unknown SNI, Host internal" \
    "$([ "$p" = 8443 ] && echo 421 || echo 404)" "$(get "$p" other.qd.test app-1 - / -k)"
  expect "internal name, the CA's certificate, Host public: no public route there" \
    "$([ "$p" = 8443 ] && echo 421 || echo 404)" "$(get "$p" app-1 app.qd.test worker /)"
  get "$p" app-1 app-1 worker / -H "$INFO: $NAMED" -H "X-Forwarded-For: 192.0.2.7" >/dev/null
  expect "internal name: a client's $INFO is removed" "" "$(header "$INFO")"
  expect "internal name: a client's X-Forwarded-For is not believed" "$GW" "$(header X-Forwarded-For)"
done

echo "== (B) the lake gates' check on its own name, the gates' entry point"
expect "lakegate name, the CA's certificate" 200 "$(get 443 lakegate.app-1 lakegate.app-1 worker "$CHECK" -H "$INFO: $NAMED")"
expect "the gate's $INFO is kept" "$NAMED" "$(header "$INFO")"
expect "the client's address reaches it (PROXY protocol)" "$GW" "$(header X-Forwarded-For | sed 's/.*, //')"
expect "lakegate name, no certificate" "000 alert certificate required" "$(get 443 lakegate.app-1 lakegate.app-1 - "$CHECK")"
expect "lakegate name, another CA's certificate" "000 alert unknown ca" "$(get 443 lakegate.app-1 lakegate.app-1 stranger "$CHECK")"
get 443 app-1 app-1 worker "$CHECK" -H "$INFO: $NAMED" >/dev/null
expect "internal name, the check's path: the gate's $INFO is removed there" "" "$(header "$INFO")"
expect "public name, Host lakegate" 404 "$(get 443 app.qd.test lakegate.app-1 - "$CHECK")"

echo "== The loopback entry points are not reachable from outside the container"
expect "9443 from outside" "000" "$(get 9443 app-1 app-1 worker / | cut -c1-3)"
expect "9444 from outside" "000" "$(get 9444 lakegate.app-1 lakegate.app-1 worker / | cut -c1-3)"
docker logs "$NAME" 2>&1 | grep -iE '"level":"(error|warn)"|level=(error|warn)' | head -5

[ "$FAILED" = 0 ] && echo "All checks passed." || { echo "Some checks failed."; exit 1; }
