#!/bin/bash
# Verification of the hop from the lake's gate to the API (decision 0066,
# point 10): the gate (a Traefik in front of RustFS) asks the API through
# forwardAuth whether a machine may pass, and the API must know two things
# it can believe: who asks (the gate, a machine of the lake) and for whom
# (the machine at the gate). It shows, with Traefik 3.6 on both sides:
#   - forwardAuth presents the gate's own certificate to the application's
#     Traefik (forwardAuth.tls), which requires one of the CA;
#   - the application's Traefik writes the certificate of who asks in
#     X-Forwarded-Tls-Client-Cert (passTLSClientCert), whatever came in it:
#     that header is always the gate's, never the machine's;
#   - the gate can send the machine's identity in another header Traefik
#     itself writes, X-Forwarded-Tls-Client-Cert-Info (subject and serial,
#     the serial in decimal, the leaf first); on the machines' entry point
#     as it is, the application's Traefik removes it, as every X-Forwarded
#     header from an address it does not trust;
#   - on an entry point of its own that trusts the forwarded headers
#     (forwardedHeaders.insecure) it arrives; there any machine with a
#     certificate can send that header by hand, but not change who asks: the
#     API must answer only a machine of the lake.
#
# Two throwaway Traefik on the host's loopback (ports 59081 the machines'
# entry point, 59084 the gate's own, 59082 the gate) and a Python echo in
# place of the API and RustFS (59083), with a throwaway CA; nothing of the
# dev stack is touched. Run from the repository root:
#
#   bash tools/verify/traefik_lake_hop.sh
#
# Needs docker, curl, openssl and python3.
set -u
export LC_ALL=C
IMAGE=${TRAEFIK_IMAGE:-traefik:v3.6}
A=qd-verify-lake-hop-app
G=qd-verify-lake-hop-gate
MACHINES=59081
GATE=59082
ECHO=59083
OWN=59084
D=$(mktemp -d)
chmod 755 "$D"
mkdir -p "$D/app" "$D/gate"
ECHO_PID=
trap '[ -n "$ECHO_PID" ] && kill "$ECHO_PID" 2>/dev/null; docker rm -f "$A" "$G" >/dev/null 2>&1; rm -rf "$D"' EXIT
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 -subj "/O=qd/CN=qd verify" \
  -addext "basicConstraints=critical,CA:TRUE" -keyout "$D/ca.key" -out "$D/ca.crt" 2>/dev/null
leaf() { # NAME SUBJECT
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$2" -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=clientAuth,serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n' >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/ca.crt" -CAkey "$D/ca.key" -CAcreateserial -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
  cat "$D/$1.crt" "$D/$1.key" >"$D/$1.pem"
  chmod 644 "$D/$1.pem"
}
leaf app "/O=qd/OU=app/CN=app-1"
leaf lake "/O=qd/OU=lake/CN=lake-1"
leaf worker "/O=qd/OU=worker/CN=worker-7"
leaf other "/O=qd/OU=worker/CN=worker-8"
hex() { openssl x509 -noout -serial -in "$D/$1.crt" | cut -d= -f2; }
decimal() { python3 -c 'import sys;print(int(sys.argv[1],16))' "$(hex "$1")"; }

# In place of the API (the check) and of RustFS: it keeps the headers it got.
cat >"$D/echo.py" <<'EOF'
import http.server, json, sys
class Echo(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        name = "/check.json" if "lake/check" in self.path else "/backend.json"
        json.dump({k.lower(): v for k, v in self.headers.items()}, open(sys.argv[1] + name, "w"))
        self.send_response(200)
        self.send_header("Content-Length", "0")
        self.end_headers()
    do_PUT = do_POST = do_GET
    def log_message(self, *args):
        pass
http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[2])), Echo).serve_forever()
EOF
python3 "$D/echo.py" "$D" $ECHO &
ECHO_PID=$!

# The application's Traefik: the machines' entry point as infra/traefik has
# it, and one of the gate's own that trusts the forwarded headers.
cat >"$D/app/app.yml" <<EOF
tls:
  certificates: [{certFile: /c/app.pem, keyFile: /c/app.pem}]
  options:
    machines: {minVersion: VersionTLS12, clientAuth: {caFiles: [/c/ca.crt], clientAuthType: RequireAndVerifyClientCert}}
http:
  routers:
    machines:
      rule: "Host(\`localhost\`) && PathPrefix(\`/api/internal/machines/\`)"
      entryPoints: [internal]
      tls: {options: machines}
      service: api
      middlewares: [api-strip, machine-certificate]
    lake-check:
      rule: "Host(\`localhost\`) && Path(\`/api/internal/machines/lake/check\`)"
      entryPoints: [lakegate]
      tls: {options: machines}
      service: api
      middlewares: [api-strip, machine-certificate]
  middlewares:
    api-strip: {stripPrefix: {prefixes: [/api]}}
    machine-certificate: {passTLSClientCert: {pem: true}}
  services:
    api: {loadBalancer: {servers: [{url: "http://127.0.0.1:$ECHO"}]}}
EOF
# gate ADDRESS: the gate, asking at ADDRESS with its own certificate.
gate() {
  cat >"$D/gate/gate.yml" <<EOF
tls:
  certificates: [{certFile: /c/lake.pem, keyFile: /c/lake.pem}]
  options:
    default: {minVersion: VersionTLS12, clientAuth: {caFiles: [/c/ca.crt], clientAuthType: RequireAndVerifyClientCert}}
http:
  routers:
    lake: {rule: "PathPrefix(\`/\`)", entryPoints: [lake], tls: {}, service: rustfs, middlewares: [machine, ask]}
  middlewares:
    machine:
      passTLSClientCert:
        info: {serialNumber: true, subject: {organization: true, organizationalUnit: true, commonName: true}}
    ask:
      forwardAuth:
        address: https://localhost:$1/api/internal/machines/lake/check
        tls: {ca: /c/ca.crt, cert: /c/lake.pem, key: /c/lake.pem}
  services:
    rustfs: {loadBalancer: {servers: [{url: "http://127.0.0.1:$ECHO"}]}}
EOF
  sleep 3
}
docker rm -f "$A" "$G" >/dev/null 2>&1
docker run -d --name "$A" --network host --user "$(id -u)" -v "$D/app:/dyn:ro" -v "$D:/c:ro" "$IMAGE" \
  --providers.file.directory=/dyn --entrypoints.internal.address=127.0.0.1:$MACHINES \
  --entrypoints.lakegate.address=127.0.0.1:$OWN --entrypoints.lakegate.forwardedHeaders.insecure=true --log.level=ERROR >/dev/null
gate $MACHINES
docker run -d --name "$G" --network host --user "$(id -u)" -v "$D/gate:/dyn:ro" -v "$D:/c:ro" "$IMAGE" \
  --providers.file.directory=/dyn --providers.file.watch=true --entrypoints.lake.address=127.0.0.1:$GATE --log.level=ERROR >/dev/null
sleep 4
# saw FILE WHAT: who (the certificate header's CN), whom (the Info header's first subject and serial).
saw() {
  python3 - "$D/$1.json" "$2" <<'EOF'
import base64, json, re, subprocess, sys, urllib.parse
try:
    d = json.load(open(sys.argv[1]))
except OSError:
    print("nothing"); sys.exit()
if sys.argv[2] == "who":
    c = d.get("x-forwarded-tls-client-cert")
    if not c:
        print("none"); sys.exit()
    try:
        der = base64.b64decode(urllib.parse.unquote(c).split(",")[0])
        out = subprocess.run(["openssl", "x509", "-inform", "DER", "-noout", "-subject", "-nameopt", "RFC2253"], input=der, capture_output=True)
        print(out.stdout.decode().strip().removeprefix("subject=") or "not a certificate")
    except Exception:
        print("not a certificate")
else:
    info = urllib.parse.unquote(d.get("x-forwarded-tls-client-cert-info", ""))
    m = re.match(r'Subject="([^"]*)";SerialNumber="(\d+)"', info)
    print(f"{m.group(1)} {m.group(2)}" if m else (info or "none"))
EOF
}
through() { rm -f "$D/check.json" "$D/backend.json"; curl -s -o /dev/null -w '%{http_code}' --max-time 10 --cacert "$D/ca.crt" --cert "$D/$1.pem" -X PUT "https://localhost:$GATE/bucket/key"; }
# straight PORT CERT: asked by hand, forging both headers as worker-7.
straight() {
  rm -f "$D/check.json"
  curl -s -o /dev/null -w '%{http_code}' --max-time 10 --cacert "$D/ca.crt" --cert "$D/$2.pem" \
    -H "X-Forwarded-Tls-Client-Cert: forged" -H "X-Forwarded-Tls-Client-Cert-Info: Subject=\"O=qd,OU=worker,CN=worker-7\";SerialNumber=\"1\"" \
    "https://localhost:$1/api/internal/machines/lake/check"
}

echo "== The gate asks at the machines' entry point, as it is"
expect "a worker's request goes through the gate (the echo says yes)" 200 "$(through worker)"
expect "who asks, as the application's Traefik writes it: the gate" "CN=lake-1,OU=lake,O=qd" "$(saw check who)"
expect "for whom: the header the gate wrote is removed" none "$(saw check whom)"
expect "without a certificate nobody asks" 000 \
  "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 --cacert "$D/ca.crt" "https://localhost:$MACHINES/api/internal/machines/lake/check")"

echo "== The gate asks at an entry point of its own, which trusts forwarded headers"
gate $OWN
expect "a worker's request goes through the gate" 200 "$(through worker)"
expect "who asks: the gate" "CN=lake-1,OU=lake,O=qd" "$(saw check who)"
expect "for whom: the worker, its serial in decimal" "O=qd,OU=worker,CN=worker-7 $(decimal worker)" "$(saw check whom)"
expect "RustFS, behind, is not given the gate's certificate header" none "$(saw backend who)"
expect "another worker through the gate is itself" "O=qd,OU=worker,CN=worker-8 $(decimal other)" "$(through other >/dev/null; saw check whom)"
expect "the machines' entry point serves the check too, with no one named" "200 none" "$(straight $MACHINES other) $(saw check whom)"

echo "== A machine asks by hand at the gate's entry point, forging both headers"
expect "it is answered (the echo), having a certificate of the CA" 200 "$(straight $OWN other)"
expect "who asks cannot be forged: itself, a worker" "CN=worker-8,OU=worker,O=qd" "$(saw check who)"
expect "for whom can: what it wrote arrives" "O=qd,OU=worker,CN=worker-7 1" "$(saw check whom)"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
