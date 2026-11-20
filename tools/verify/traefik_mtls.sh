#!/bin/bash
# Verification of Traefik's client certificates for the API's internal
# endpoints (decisions 0023, 0029 and 0030): an internal entry point whose
# TLS options require a certificate of the installation's CA next to a public
# one that does not, whether the internal one can be reached with the public
# options (SNI and Host tricks), how the client's identity reaches the
# backend (passTLSClientCert) and whether a client can forge that header,
# and whether a renewed server certificate and a client CA with a second CA
# added are taken without a restart.
#
# It runs its own throwaway Traefik (traefik:v3.6, the image of
# infra/traefik) with the file provider only, in front of traefik/whoami,
# with a throwaway CA; nothing of the dev stack is touched. Run from the
# repository root:
#
#   bash tools/verify/traefik_mtls.sh
#
# Needs docker, curl and openssl 3.4 or later (`-not_after`).
set -u
export LC_ALL=C
IMAGE=${TRAEFIK_IMAGE:-traefik:v3.6}
WHOAMI=${WHOAMI_IMAGE:-traefik/whoami:v1.11}
NAME=qd-verify-traefik-mtls
BACK=qd-verify-traefik-whoami
D=$(mktemp -d)
chmod 700 "$D"
trap 'docker rm -f "$NAME" "$BACK" >/dev/null 2>&1; rm -rf "$D"' EXIT
FAILED=0

ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { # NAME EXPECTED ACTUAL
  if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi
}
like() { # NAME PATTERN ACTUAL
  case "$3" in
    *"$2"*) ok "$1: $(echo "$3" | head -1 | cut -c1-110)" ;;
    *) ko "$1: expected '$2', got '$3'" ;;
  esac
}

# --- Certificates ------------------------------------------------------------
ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/CN=qd verify $1" -addext "basicConstraints=critical,CA:TRUE" \
    -keyout "$D/$1.key" -out "$D/$1.crt" 2>/dev/null
}
# cert FILE CA SUBJECT USAGE [OPENSSL X509 OPTION...]
cert() {
  local f=$1 ca=$2 subj=$3 usage=$4
  shift 4
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$subj" \
    -keyout "$D/$f.key" -out "$D/$f.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=%s\nsubjectAltName=DNS:app.qd.test,DNS:api.internal.qd.test\n' "$usage" >"$D/ext"
  openssl x509 -req -in "$D/$f.csr" -CA "$D/$ca.crt" -CAkey "$D/$ca.key" -CAcreateserial \
    -extfile "$D/ext" -out "$D/$f.crt" "$@" 2>/dev/null
}
ca ca
ca other
cert server ca "/CN=api.internal.qd.test" serverAuth -days 1
cert worker ca "/O=qd/OU=worker/CN=worker-7" clientAuth -days 1
cert stranger other "/O=qd/OU=worker/CN=worker-7" clientAuth -days 1
cert late ca "/O=qd/OU=worker/CN=worker-7" clientAuth \
  -not_before "$(date -u -d '-2 hours' +%Y%m%d%H%M%SZ)" -not_after "$(date -u -d '-1 hour' +%Y%m%d%H%M%SZ)"
C=$D/certs Y=$D/dynamic
mkdir "$C" "$Y" && chmod 755 "$C" "$Y"
put() { cp "$1" "$C/.$2.new" && chmod 644 "$C/.$2.new" && mv -f "$C/.$2.new" "$C/$2"; }
put "$D/server.crt" server.crt
put "$D/server.key" server.key
put "$D/ca.crt" ca.crt

# --- The backend and a throwaway Traefik ----------------------------------------
docker rm -f "$NAME" "$BACK" >/dev/null 2>&1
docker run -d --name "$BACK" "$WHOAMI" >/dev/null || { echo "could not start $WHOAMI"; exit 1; }
BIP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$BACK")
# tls_yml SERVER_CERT CA_FILE: the TLS part of the dynamic configuration.
tls_yml() {
  cat >"$Y/.tls.yml" <<EOF
tls:
  certificates:
    - certFile: $1
      keyFile: ${1%.crt}.key
  stores:
    default:
      defaultCertificate:
        certFile: $1
        keyFile: ${1%.crt}.key
  options:
    internal-mtls:
      minVersion: VersionTLS12
      clientAuth:
        caFiles: [$2]
        clientAuthType: RequireAndVerifyClientCert
EOF
  chmod 644 "$Y/.tls.yml" && mv -f "$Y/.tls.yml" "$Y/tls.yml"
}
tls_yml /certs/server.crt /certs/ca.crt
cat >"$Y/routers.yml" <<EOF
http:
  routers:
    public:
      rule: "Host(\`app.qd.test\`)"
      entryPoints: [public]
      tls: {}
      service: whoami
    internal:
      rule: "Host(\`api.internal.qd.test\`)"
      entryPoints: [internal]
      tls:
        options: internal-mtls
      service: whoami
      middlewares: [client-identity]
  middlewares:
    client-identity:
      passTLSClientCert:
        pem: true
        info:
          subject:
            commonName: true
            organization: true
            organizationalUnit: true
  services:
    whoami:
      loadBalancer:
        servers:
          - url: "http://$BIP:80"
EOF
chmod 644 "$Y/routers.yml"
docker run -d --name "$NAME" -v "$Y:/dynamic:ro" -v "$C:/certs:ro" "$IMAGE" \
  --entryPoints.public.address=:8443 --entryPoints.internal.address=:9443 \
  --providers.file.directory=/dynamic --providers.file.watch=true --log.level=INFO >/dev/null ||
  { echo "could not start $IMAGE"; exit 1; }
IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$NAME")

# get PORT SNI HOST CERT [CURL OPTION...]: GET / with SNI and a Host header
# of our choosing; CERT `-` is none. Prints the status, 000 and the TLS
# alert when the handshake fails; the body is in $D/body.
get() {
  local port=$1 sni=$2 host=$3 c=$4 args=() code
  shift 4
  [ "$c" = - ] || args=(--cert "$D/$c.crt" --key "$D/$c.key")
  code=$(curl -sS -o "$D/body" -w '%{http_code}' --max-time 5 --cacert "$D/ca.crt" -H "Host: $host" \
    --connect-to "$sni:$port:$IP:$port" "${args[@]}" "$@" "https://$sni:$port/" 2>"$D/err")
  if [ "$code" = 000 ]; then echo "000 $(grep -o 'alert [a-z ]*' "$D/err" | head -1)"; else echo "$code"; fi
}
int() { get 9443 api.internal.qd.test api.internal.qd.test "$@"; }
pub() { get 8443 app.qd.test app.qd.test "$@"; }
header() { grep -i "^$1:" "$D/body" | tr -d '\r'; }
for _ in $(seq 30); do [ "$(int worker)" = 200 ] && break; sleep 1; done
[ "$(int worker)" = 200 ] || { echo "Traefik did not start: $(int worker)"; docker logs "$NAME" 2>&1 | tail -5; exit 1; }
served() { # PORT SNI: the serial of the server certificate
  echo | openssl s_client -connect "$IP:$1" -servername "$2" -cert "$D/worker.crt" -key "$D/worker.key" 2>/dev/null |
    openssl x509 -noout -serial 2>/dev/null
}

echo "== (a) The internal entry point asks for a certificate of the CA; the public one does not"
expect "internal, a certificate of the CA" 200 "$(int worker)"
expect "internal, no certificate" "000 alert certificate required" "$(int -)"
expect "internal, a certificate of another CA" "000 alert unknown ca" "$(int stranger)"
expect "internal, an expired certificate of the CA" "000 alert certificate expired" "$(int late)"
expect "public, no certificate" 200 "$(pub -)"
# TLS options are chosen by SNI: another name gets the default options (no
# client certificate), so the handshake passes; the backend still is not
# reached: no router for that host, or 421 when SNI and Host differ.
expect "internal port, the public SNI and Host, no certificate: handshake passes, no router" 404 \
  "$(get 9443 app.qd.test app.qd.test -)"
expect "internal port, the public SNI with the internal Host, no certificate" 421 \
  "$(get 9443 app.qd.test api.internal.qd.test -)"
expect "internal port, an unknown SNI with the internal Host, no certificate" 421 \
  "$(get 9443 nothing.qd.test api.internal.qd.test - -k)"
expect "internal port, no SNI (by IP) with the internal Host, no certificate" 421 \
  "$(get 9443 "$IP" api.internal.qd.test - -k)"
expect "public port with the internal Host: the internal router is not there" 404 \
  "$(get 8443 app.qd.test api.internal.qd.test -)"

echo "== (b) The client's identity at the backend (passTLSClientCert)"
int worker >/dev/null
INFO=X-Forwarded-Tls-Client-Cert-Info
SUBJECT='Subject%3D%22O%3Dqd%2COU%3Dworker%2CCN%3Dworker-7%22'
like "$INFO, URL-encoded Subject=\"O=qd,OU=worker,CN=worker-7\" first" "$INFO: $SUBJECT" "$(header "$INFO")"
first=$(header X-Forwarded-Tls-Client-Cert | sed 's/^[^:]*: //' | python3 -I -c '
import sys, urllib.parse, base64
print(base64.b64encode(base64.b64decode(urllib.parse.unquote(sys.stdin.read().strip()).split(",")[0])).decode())')
expect "X-Forwarded-Tls-Client-Cert (pem: true), its first entry is the client's certificate" \
  "$(openssl x509 -noout -serial -in "$D/worker.crt")" \
  "$(printf -- '-----BEGIN CERTIFICATE-----\n%s\n-----END CERTIFICATE-----\n' "$first" | fold -w 64 | openssl x509 -noout -serial 2>&1)"
# Every certificate the client sends is listed, verified or not: one made by
# the client itself, sent after its real one, appears too. Only the first
# entry is the verified certificate.
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 -subj "/O=qd/OU=admin/CN=root" \
  -keyout "$D/fake.key" -out "$D/fake.crt" 2>/dev/null
cat "$D/worker.crt" "$D/fake.crt" >"$D/bundle.crt" && cp "$D/worker.key" "$D/bundle.key"
expect "a valid certificate followed by a self-made one: accepted" 200 "$(int bundle)"
like "and the self-made subject is listed after the real one" \
  "$SUBJECT%2CSubject%3D%22O%3Dqd%2COU%3Dadmin%2CCN%3Droot%22" "$(header "$INFO")"
FORGE=(-H "$INFO: Subject%3D%22O%3Dqd%2COU%3Dadmin%2CCN%3Droot%22" -H "X-Forwarded-Tls-Client-Cert: forged"
  -H "X_Forwarded_Tls_Client_Cert_Info: forged" -H "x-forwarded-tls-client-cert-info: forged")
int worker "${FORGE[@]}" >/dev/null
expect "internal, forged headers sent: one $INFO" 1 "$(grep -ci "^$INFO:" "$D/body")"
like "and it is Traefik's" "$INFO: $SUBJECT" "$(header "$INFO")"
expect "internal, nothing forged reaches the backend" "" "$(grep -i 'forged\|admin' "$D/body")"
pub - "${FORGE[@]}" >/dev/null
expect "public, forged headers sent: none reaches the backend" "" "$(grep -i 'client.cert' "$D/body")"

# within SECONDS COMMAND EXPECTED: how many seconds until COMMAND prints
# EXPECTED, or nothing.
within() {
  local start=$(date +%s)
  for _ in $(seq "$1"); do [ "$($2)" = "$3" ] && { echo $(($(date +%s) - start)); return; }; sleep 1; done
}
renew() { cert server ca "/CN=api.internal.qd.test" serverAuth -days 1; openssl x509 -noout -serial -in "$D/server.crt"; }
srv() { served 9443 api.internal.qd.test; }
stranger() { int stranger; }

echo "== (c) The server certificate replaced, outside the watched directory"
before=$(srv)
new=$(renew)
put "$D/server.key" server.key
put "$D/server.crt" server.crt
expect "the files alone: after 8s the old one is still served" "" "$(within 8 srv "$new")"
expect "it was the old one" "$before" "$(srv)"
touch "$Y/tls.yml"
took=$(within 10 srv "$new")
if [ -n "$took" ]; then ok "after touching the dynamic file, the new one, after ${took}s"; else ko "not even after touching the dynamic file: $(srv)"; fi

echo "== (c) The server certificate inside the watched directory"
cp "$D/server.crt" "$Y/server.crt" && cp "$D/server.key" "$Y/server.key" && chmod 644 "$Y"/server.*
tls_yml /dynamic/server.crt /certs/ca.crt
sleep 3
new=$(renew)
for f in key crt; do cp "$D/server.$f" "$Y/.server.$f.new" && chmod 644 "$Y/.server.$f.new" && mv -f "$Y/.server.$f.new" "$Y/server.$f"; done
took=$(within 10 srv "$new")
if [ -n "$took" ]; then ok "the files alone (an event in the directory), the new one after ${took}s"; else ko "not taken: $(srv)"; fi
expect "the container was never restarted" 0 "$(docker inspect -f '{{.RestartCount}}' "$NAME")"

echo "== (d) The client CA (caFiles) with a second CA added"
cat "$D/ca.crt" "$D/other.crt" >"$D/both.crt"
put "$D/both.crt" ca.crt
expect "outside the watched directory, the file alone: after 8s another CA's certificate still refused" "" \
  "$(within 8 stranger 200)"
touch "$Y/tls.yml"
took=$(within 10 stranger 200)
if [ -n "$took" ]; then ok "after touching the dynamic file, accepted after ${took}s"; else ko "still refused after touching: $(stranger)"; fi
expect "the first CA's too" 200 "$(int worker)"
cp "$D/ca.crt" "$Y/ca.crt" && chmod 644 "$Y/ca.crt"
tls_yml /dynamic/server.crt /dynamic/ca.crt
took=$(within 10 stranger "000 alert unknown ca")
if [ -n "$took" ]; then ok "caFiles moved into the watched directory (only the first CA): another CA refused after ${took}s"; else ko "not taken: $(stranger)"; fi
cp "$D/both.crt" "$Y/.ca.crt.new" && chmod 644 "$Y/.ca.crt.new" && mv -f "$Y/.ca.crt.new" "$Y/ca.crt"
took=$(within 10 stranger 200)
if [ -n "$took" ]; then ok "inside the watched directory, the file alone: the second CA accepted after ${took}s"; else ko "not taken: $(stranger)"; fi
expect "and the connections of the first CA go on" 200 "$(int worker)"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
