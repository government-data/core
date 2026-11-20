#!/bin/bash
# Verification of Dolt sql-server's TLS for the connections between VMs
# (decisions 0021, 0023 and 0030): whether it serves TLS with a certificate
# of the installation's CA, whether it can require and verify a client
# certificate of that CA by itself (no TLS proxy in front), per user
# (`REQUIRE X509 / SUBJECT / ISSUER`) and for the whole listener
# (`listener.ca_cert`, `listener.require_client_cert`), and whether it takes a
# renewed server certificate without a restart.
#
# What 2.6.1 offers (go/libraries/doltcore/servercfg at tag v2.6.1):
#   listener.tls_cert, tls_key       the server's pair, read once at start
#   listener.ca_cert                 CAs a client certificate is verified against
#   listener.require_client_cert     with ca_cert: RequireAndVerifyClientCert;
#                                    without it: RequireAnyClientCert (unverified!)
#   listener.require_secure_transport  no plaintext connection
#
# It runs its own throwaway Dolt (dolthub/dolt-sql-server:2.6.1, the image of
# infra/dolt) with a throwaway CA; nothing of the dev stack is touched. Its
# passwords are random and never printed. No rows at all. Run from the
# repository root:
#
#   bash tools/verify/dolt_mtls.sh
#
# Needs docker, openssl 3.4 or later (`-not_after`) and uv (pymysql).
set -u
export LC_ALL=C
IMAGE=${DOLT_IMAGE:-dolthub/dolt-sql-server:2.6.1}
NAME=qd-verify-dolt-mtls
D=$(mktemp -d)
chmod 700 "$D"
trap 'docker rm -f "$NAME" >/dev/null 2>&1; rm -rf "$D"' EXIT
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
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=%s\nsubjectAltName=DNS:dolt.qd.test\n' "$usage" >"$D/ext"
  openssl x509 -req -in "$D/$f.csr" -CA "$D/$ca.crt" -CAkey "$D/$ca.key" -CAcreateserial \
    -extfile "$D/ext" -out "$D/$f.crt" "$@" 2>/dev/null
}
ca ca
ca other
cert server ca "/CN=dolt.qd.test" serverAuth -days 1
cert worker ca "/O=qd/OU=worker/CN=worker-7" clientAuth -days 1
cert trino ca "/O=qd/OU=trino/CN=trino-1" clientAuth -days 1
cert stranger other "/O=qd/OU=worker/CN=worker-7" clientAuth -days 1
cert late ca "/O=qd/OU=worker/CN=worker-7" clientAuth \
  -not_before "$(date -u -d '-2 hours' +%Y%m%d%H%M%SZ)" -not_after "$(date -u -d '-1 hour' +%Y%m%d%H%M%SZ)"
T=$D/tls
mkdir "$T" && chmod 755 "$T"
put() { cp "$1" "$T/.$2.new" && chmod 644 "$T/.$2.new" && mv -f "$T/.$2.new" "$T/$2"; }
put "$D/server.crt" server.crt
put "$D/server.key" server.key
put "$D/ca.crt" ca.crt

# config REQUIRE_CLIENT_CERT: the server's settings, the shape of infra/dolt/config.yaml.
config() {
  mkdir -p "$D/cfg" && chmod 755 "$D/cfg"
  cat >"$D/cfg/config.yaml" <<EOF
log_level: warning
listener:
  host: 0.0.0.0
  port: 3306
  tls_cert: /tls/server.crt
  tls_key: /tls/server.key
  ca_cert: /tls/ca.crt
  require_secure_transport: true
  require_client_cert: $1
data_dir: /var/lib/dolt
EOF
  chmod 644 "$D/cfg/config.yaml"
}
config false
PW=$(openssl rand -hex 12)
docker rm -f "$NAME" >/dev/null 2>&1
docker run -d --name "$NAME" -e DOLT_ROOT_PASSWORD="$(openssl rand -hex 12)" -e DOLT_ROOT_HOST=localhost \
  -v "$T:/tls:ro" -v "$D/cfg:/etc/dolt/servercfg.d:ro" "$IMAGE" >/dev/null ||
  { echo "could not start $IMAGE"; exit 1; }
ip() { IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$NAME"); }
ip

# The client: pymysql, the server verified against our CA. Prints the user
# the server says we are, or the error; with mode `pem`, the server's
# certificate.
cat >"$D/q.py" <<'EOF'
import os, ssl as tls, sys, pymysql
user, crt, mode = sys.argv[1], sys.argv[2], sys.argv[3]
d, host = os.environ["D"], os.environ["IP"]
ssl = None
if mode != "plain":
    ssl = {"ca": f"{d}/ca.crt", "check_hostname": False}
    if crt != "-":
        ssl.update(cert=f"{d}/{crt}.crt", key=f"{d}/{crt}.key")
try:
    c = pymysql.connect(host=host, port=3306, user=user, password=os.environ["PW"], ssl=ssl, connect_timeout=5)
    if mode == "pem":
        print(tls.DER_cert_to_PEM_cert(c._sock.getpeercert(True)))
        sys.exit()
    with c.cursor() as cur:
        cur.execute("select current_user()")
        print(cur.fetchone()[0])
except Exception as e:
    msg = str(e.args[1] if len(e.args) > 1 else e).splitlines()[0]
    print(f"{type(e).__name__}: {msg}"[:120])
EOF
py() { D=$D IP=$IP PW=$PW uv run -q --with pymysql==1.1.2 python3 -I "$D/q.py" "$@"; }
q() { py "$1" "$2" "${3:-tls}" 2>&1 | tail -1; }
root() { docker exec -w /var/lib/dolt "$NAME" dolt sql -q "$1" 2>&1; }
# served: the serial of the server's certificate (as the worker, over MySQL's TLS).
served() { py qd_plain worker pem 2>/dev/null | openssl x509 -noout -serial 2>/dev/null; }
up() { ip; for _ in $(seq 60); do [ "$(q qd_plain worker)" = "qd_plain@%" ] && return 0; sleep 1; done; return 1; }
# The entrypoint starts the server, sets up root, and only then is ready:
# users made before that can be lost.
for _ in $(seq 90); do docker logs "$NAME" 2>&1 | grep -q 'Ready for connections' && break; sleep 1; done
for u in "qd_x509 REQUIRE X509" "qd_subject REQUIRE SUBJECT '/O=qd/OU=worker/CN=worker-7'" \
  "qd_issuer REQUIRE ISSUER '/CN=qd verify ca'" "qd_ssl REQUIRE SSL" "qd_plain"; do
  set -- $u
  root "CREATE USER $1@'%' IDENTIFIED BY '$PW' ${u#"$1"}" >/dev/null
done
expect "the five users exist" 5 "$(root "select count(*) from mysql.user where user like 'qd\_%'" | grep -o '[0-9]\+' | head -1)"
up || { echo "Dolt did not start with TLS: $(q qd_plain worker)"; docker logs "$NAME" 2>&1 | tail -5; exit 1; }
UNKNOWN_CA="TLSV1_ALERT_UNKNOWN_CA"
EXPIRED="SSLV3_ALERT_CERTIFICATE_EXPIRED"
REQUIRED="TLSV13_ALERT_CERTIFICATE_REQUIRED"

echo "== (a) TLS with a certificate of our CA (tls_cert, tls_key, require_secure_transport)"
expect "it serves our server certificate" "$(openssl x509 -noout -serial -in "$D/server.crt")" "$(served)"
expect "a user without REQUIRE, over TLS verified against our CA" "qd_plain@%" "$(q qd_plain -)"
like "the same without TLS: refused (require_secure_transport)" "UNAVAILABLE" "$(q qd_plain - plain)"

echo "== (b1) Per user, with ca_cert and require_client_cert: false"
expect "REQUIRE X509, a certificate of the CA" "qd_x509@%" "$(q qd_x509 worker)"
like "REQUIRE X509, no certificate" "Access denied" "$(q qd_x509 -)"
like "REQUIRE X509, a certificate of another CA (refused at the handshake)" "$UNKNOWN_CA" "$(q qd_x509 stranger)"
like "REQUIRE X509, an expired certificate (refused at the handshake)" "$EXPIRED" "$(q qd_x509 late)"
expect "REQUIRE SUBJECT, the worker's certificate" "qd_subject@%" "$(q qd_subject worker)"
like "REQUIRE SUBJECT, another certificate of the CA (OU=trino)" "Access denied" "$(q qd_subject trino)"
expect "REQUIRE ISSUER, a certificate of the CA" "qd_issuer@%" "$(q qd_issuer trino)"
expect "REQUIRE SSL, TLS without a certificate is enough" "qd_ssl@%" "$(q qd_ssl -)"
like "a certificate of another CA, even for a user without REQUIRE" "$UNKNOWN_CA" "$(q qd_plain stranger)"

echo "== (c) The server certificate replaced while running"
before=$(served)
cert server ca "/CN=dolt.qd.test" serverAuth -days 1
new=$(openssl x509 -noout -serial -in "$D/server.crt")
put "$D/server.key" server.key
put "$D/server.crt" server.crt
sleep 15
expect "after 15s the old one is still served (read once at start)" "$before" "$(served)"
docker kill -s HUP "$NAME" >/dev/null 2>&1
sleep 3
expect "SIGHUP is no reload: it stops the server" false "$(docker inspect -f '{{.State.Running}}' "$NAME")"
docker restart "$NAME" >/dev/null
up
expect "after a restart, the new one" "$new" "$(served)"

echo "== (b2) The whole listener: require_client_cert: true, with ca_cert"
config true
docker restart "$NAME" >/dev/null
up
expect "a user without REQUIRE, a certificate of the CA" "qd_plain@%" "$(q qd_plain worker)"
like "a user without REQUIRE, no certificate (refused at the handshake)" "$REQUIRED" "$(q qd_plain -)"
like "a certificate of another CA" "$UNKNOWN_CA" "$(q qd_plain stranger)"
like "an expired certificate" "$EXPIRED" "$(q qd_plain late)"
like "root's own CLI inside the container, with no certificate, is locked out too" "certificate required" "$(root "select 1")"

echo "== require_client_cert: true without ca_cert"
sed -i "/ca_cert/d" "$D/cfg/config.yaml"
docker restart "$NAME" >/dev/null
ip
for _ in $(seq 60); do [ "$(q qd_plain stranger)" = "qd_plain@%" ] && break; sleep 1; done
expect "a certificate of ANOTHER CA is accepted (RequireAnyClientCert: never without ca_cert)" \
  "qd_plain@%" "$(q qd_plain stranger)"
like "no certificate is still refused" "$REQUIRED" "$(q qd_plain -)"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
