#!/bin/bash
# Verification of RustFS's TLS for the connections between VMs (decisions
# 0023, 0029 and 0030): whether it serves HTTPS with a certificate of the
# installation's CA, whether it can require a client certificate of that CA
# by itself (no TLS proxy in front), and whether it takes a renewed server
# certificate, and a client CA with a second CA added, without a restart.
#
# What 1.0.4 offers (crates/config/src/constants/tls.rs and
# docs/operations/tls-certificate-rotation.md at tag 1.0.4):
#   RUSTFS_TLS_PATH             rustfs_cert.pem, rustfs_key.pem (server)
#                               client_ca.crt (CAs a client certificate must chain to)
#   RUSTFS_SERVER_MTLS_ENABLE=1 require and verify a client certificate
#   RUSTFS_TLS_RELOAD_ENABLE=1  re-read the TLS directory (default off)
#   RUSTFS_TLS_RELOAD_INTERVAL  seconds between reads (default 30, minimum 5)
#
# It runs its own throwaway RustFS (rustfs/rustfs:1.0.4, the image of
# infra/rustfs) with a throwaway CA; nothing of the dev stack is touched.
# Its root credentials are random, only start the container and are never
# used for a request. Run from the repository root:
#
#   bash tools/verify/rustfs_mtls.sh
#
# Needs docker, curl and openssl 3.4 or later (`-not_after`).
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.4}
NAME=qd-verify-rustfs-mtls
RELOAD=5
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
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=%s\nsubjectAltName=DNS:localhost,DNS:rustfs.qd.test\n' "$usage" >"$D/ext"
  openssl x509 -req -in "$D/$f.csr" -CA "$D/$ca.crt" -CAkey "$D/$ca.key" -CAcreateserial \
    -extfile "$D/ext" -out "$D/$f.crt" "$@" 2>/dev/null
}
ca ca
ca other
cert server ca "/CN=rustfs.qd.test" serverAuth -days 1
cert worker ca "/O=qd/OU=worker/CN=worker-7" clientAuth -days 1
cert stranger other "/O=qd/OU=worker/CN=worker-7" clientAuth -days 1
cert late ca "/O=qd/OU=worker/CN=worker-7" clientAuth \
  -not_before "$(date -u -d '-2 hours' +%Y%m%d%H%M%SZ)" -not_after "$(date -u -d '-1 hour' +%Y%m%d%H%M%SZ)"

# The TLS directory, bind-mounted so files can be replaced from outside as a
# renewal agent on the VM would (write aside, then rename).
T=$D/tls
mkdir "$T"
put() { # SRC DEST: atomically into the TLS directory, readable by uid 10001
  cp "$1" "$T/.$2.new" && chmod 644 "$T/.$2.new" && mv -f "$T/.$2.new" "$T/$2"
}
put "$D/server.crt" rustfs_cert.pem
put "$D/server.key" rustfs_key.pem
put "$D/ca.crt" client_ca.crt
chmod 755 "$T"

# --- A throwaway RustFS that asks for a certificate ----------------------------
docker rm -f "$NAME" >/dev/null 2>&1
docker run -d --name "$NAME" \
  -e RUSTFS_ACCESS_KEY="qd$(openssl rand -hex 8)" -e RUSTFS_SECRET_KEY="$(openssl rand -hex 16)" \
  -e RUSTFS_VOLUMES=/data -e RUSTFS_OBS_LOGGER_LEVEL=warn \
  -e RUSTFS_TLS_PATH=/tls -e RUSTFS_SERVER_MTLS_ENABLE=1 \
  -e RUSTFS_TLS_RELOAD_ENABLE=1 -e RUSTFS_TLS_RELOAD_INTERVAL=$RELOAD \
  -v "$T:/tls:ro" "$IMAGE" server --address :9000 --console-enable /data >/dev/null ||
  { echo "could not start $IMAGE"; exit 1; }
IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$NAME")
# s3 CERT [PORT]: GET / with TLS, the server verified against our CA; CERT `-`
# is none. Prints the HTTP status; 000 and the TLS alert when the handshake
# fails.
s3() {
  local c=$1 port=${2:-9000} args=() code
  [ "$c" = - ] || args=(--cert "$D/$c.crt" --key "$D/$c.key")
  code=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 5 --cacert "$D/ca.crt" \
    --resolve "rustfs.qd.test:$port:$IP" "${args[@]}" "https://rustfs.qd.test:$port/" 2>"$D/err")
  if [ "$code" = 000 ]; then echo "000 $(grep -o 'alert [a-z ]*' "$D/err" | head -1)"; else echo "$code"; fi
}
for _ in $(seq 60); do
  case $(s3 worker) in 403 | 200 | 400) break ;; esac
  sleep 1
done
[ "$(s3 worker)" = 403 ] ||
  { echo "RustFS did not start with TLS: $(s3 worker)"; docker logs "$NAME" 2>&1 | tail -5; exit 1; }

echo "== (a) HTTPS with a certificate of our CA"
served() { # the serial of the certificate served on PORT
  echo | openssl s_client -connect "$IP:${1:-9000}" -servername rustfs.qd.test \
    -cert "$D/worker.crt" -key "$D/worker.key" 2>/dev/null | openssl x509 -noout -serial 2>/dev/null
}
expect "the S3 port serves our server certificate" "$(openssl x509 -noout -serial -in "$D/server.crt")" "$(served)"
like "it verifies against our CA (curl --cacert, no -k)" "403" "$(s3 worker)"
expect "plain HTTP on the TLS port gets no answer" 000 \
  "$(curl -s -o /dev/null -w "%{http_code}" --max-time 5 "http://$IP:9000/" 2>/dev/null)"

echo "== (b) Client certificates, natively (RUSTFS_SERVER_MTLS_ENABLE=1, client_ca.crt)"
# An S3 403 (AccessDenied, unsigned request) means the handshake passed; a
# refused handshake gives curl an error and status 000.
expect "a certificate of the CA: past the handshake, S3 answers" 403 "$(s3 worker)"
expect "no certificate: refused at the handshake" "000 alert certificate required" "$(s3 -)"
expect "a certificate of another CA: refused at the handshake" "000 alert unknown ca" "$(s3 stranger)"
expect "an expired certificate of the CA: refused at the handshake" "000 alert certificate expired" "$(s3 late)"
expect "the console port (9001), a certificate of the CA" 403 "$(s3 worker 9001)"
expect "the console port (9001), no certificate" "000 alert certificate required" "$(s3 - 9001)"
# A request signed with a key the server does not know, with the CA's
# certificate: S3 still checks the signature (the certificate is no identity).
like "a certificate does not sign S3 requests (still AccessDenied/InvalidAccessKeyId)" "403" \
  "$(curl -sS -o /dev/null -w '%{http_code}' --max-time 5 --cacert "$D/ca.crt" --resolve "rustfs.qd.test:9000:$IP" \
    --cert "$D/worker.crt" --key "$D/worker.key" --aws-sigv4 "aws:amz:us-east-1:s3" \
    --user "qdnotakey:qdnotasecret" "https://rustfs.qd.test:9000/qd-none" 2>&1)"

echo "== (c) The server certificate replaced (RUSTFS_TLS_RELOAD_INTERVAL=$RELOAD)"
before=$(served)
cert server ca "/CN=rustfs.qd.test" serverAuth -days 1
new=$(openssl x509 -noout -serial -in "$D/server.crt")
# The key first would break the pair for an instant; RustFS rejects a
# mismatched pair and keeps the last good one. Both, then.
put "$D/server.key" rustfs_key.pem
put "$D/server.crt" rustfs_cert.pem
start=$(date +%s)
took=
for _ in $(seq 40); do
  [ "$(served)" = "$new" ] && { took=$(($(date +%s) - start)); break; }
  sleep 1
done
if [ -n "$took" ]; then ok "the new one is served without a restart, after ${took}s (interval ${RELOAD}s)"; else ko "still the old one after 40s: $(served) (before $before)"; fi
expect "the container was not restarted" 0 "$(docker inspect -f '{{.RestartCount}}' "$NAME")"
expect "clients go on connecting" 403 "$(s3 worker)"
expect "and no certificate is still refused" "000 alert certificate required" "$(s3 -)"

echo "== The client CA extended with a second CA (a CA rotation)"
cat "$D/ca.crt" "$D/other.crt" >"$D/both.crt"
put "$D/both.crt" client_ca.crt
took=
start=$(date +%s)
for _ in $(seq 40); do
  [ "$(s3 stranger)" = 403 ] && { took=$(($(date +%s) - start)); break; }
  sleep 1
done
if [ -n "$took" ]; then ok "the second CA's certificate is accepted without a restart, after ${took}s"; else ko "the second CA's certificate is still refused after 40s: $(s3 stranger)"; fi
expect "the first CA's still is" 403 "$(s3 worker)"
put "$D/other.crt" client_ca.crt
took=
start=$(date +%s)
for _ in $(seq 40); do
  case $(s3 worker) in 000*) took=$(($(date +%s) - start)); break ;; esac
  sleep 1
done
if [ -n "$took" ]; then ok "the first CA removed from client_ca.crt: its certificates refused after ${took}s"; else ko "the first CA's certificate still accepted after 40s"; fi
# Every refused handshake is logged as a warning, with the reason and the
# peer, never the certificate.
like "RustFS logs refused handshakes" "UnknownIssuer" \
  "$(docker exec "$NAME" sh -c 'cat /logs/* 2>/dev/null' | grep tls_handshake_failed | grep -o 'UnknownIssuer' | head -1)"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
