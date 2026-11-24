#!/usr/bin/env bash
# Verify (decision 0078, step 13): the lake admin adapter behind Traefik's
# internal entry point, its TLS passed through by SNI so its own mTLS holds
# end to end, beside an HTTP router of the same entry point that requires a
# client certificate, as internal.yml puts them, with the repository's
# infra/traefik/dynamic/lake-admin-dev.yml itself (QD_LAKE_ADMIN_DEV=1,
# INTERNAL_DOMAIN=internal.test). Checks:
#
#   (a) the adapter's own certificate (OU=lake) answers by its name;
#   (b) the API's certificate (OU=app) reads GET /about through the edge;
#   (c) another role of the CA (OU=worker) is refused by the adapter, 403;
#   (d) no client certificate: the adapter refuses the handshake;
#   (e) the entry point's HTTP router still answers its own name.
#
# Run alone (no build at the same time), after building the adapter:
#   cargo build -j 2 -p lake-rustfs-admin
#   bash tools/verify/lake_admin_edge.sh
# Traefik (TRAEFIK_IMAGE, traefik:v3.6) runs on the host's network, on the
# loopback alone (QD_VERIFY_PORT, 59443); the adapter on 127.0.0.1:9020, where
# that file sends it.
# Everything is removed in a trap, also on failure. Throwaway CA, no data.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
bin="${QD_VERIFY_ADAPTER:-${CARGO_TARGET_DIR:-$root/target}/debug/lake-rustfs-admin}"
image="${TRAEFIK_IMAGE:-traefik:v3.6}"
port="${QD_VERIFY_PORT:-59443}"
name="qs-verify-lake-admin-edge-$$"
D="$(mktemp -d)"
adapter=""
cleanup() {
  docker rm -f "$name" >/dev/null 2>&1 || true
  [[ -n "$adapter" ]] && kill "$adapter" 2>/dev/null || true
  rm -rf "$D"
}
trap cleanup EXIT
[[ -x "$bin" ]] || { echo "no adapter at $bin: cargo build -j 2 -p lake-rustfs-admin"; exit 2; }

# A throwaway CA and the certificates of each part.
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/O=qd/CN=verify CA" -keyout "$D/ca.key" -out "$D/ca.crt" 2>/dev/null
issue() {
  local subj="$1" dns="$2" file="$3"
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$subj" \
    -keyout "$D/$file.key" -out "$D/$file.csr" 2>/dev/null
  openssl x509 -req -in "$D/$file.csr" -CA "$D/ca.crt" -CAkey "$D/ca.key" -CAcreateserial \
    -days 1 -out "$D/$file.crt" \
    -extfile <(printf 'subjectAltName=DNS:%s\nextendedKeyUsage=serverAuth,clientAuth\n' "$dns") 2>/dev/null
  cat "$D/$file.crt" "$D/$file.key" >"$D/$file.pem"
}
issue "/O=qd/CN=internal.test" internal.test traefik
issue "/O=qd/OU=lake/CN=lake-admin.internal.test" lake-admin.internal.test adapter
issue "/O=qd/OU=app/CN=app-1" app-1 app
issue "/O=qd/OU=worker/CN=worker-1" worker-1 worker
mkdir -p "$D/node" "$D/dynamic"
cp "$D/ca.crt" "$D/node/ca.crt" && cp "$D/adapter.pem" "$D/node/machine.pem"
cp "$D/ca.crt" "$D/traefik.crt" "$D/traefik.key" "$D/dynamic/"
cp "$root/infra/traefik/dynamic/lake-admin-dev.yml" "$D/dynamic/"
chmod -R a+rX "$D"

QD_LAKE_ADMIN_ADDR=127.0.0.1:9020 QD_LAKE_ADMIN_TLS_DIR="$D/node" \
  QD_MACHINE_NAME=lake-admin.internal.test QD_LAKE_ADMIN_LAKE_URL=http://127.0.0.1:9 \
  RUST_LOG=warn "$bin" >"$D/adapter.log" 2>&1 &
adapter=$!

cat >"$D/traefik.yml" <<YML
entryPoints:
  internal:
    address: "127.0.0.1:$port"
ping:
  entryPoint: internal
providers:
  file:
    directory: /etc/traefik/dynamic
log:
  level: WARN
YML
cat >"$D/dynamic/edge.yml" <<'YML'
tls:
  certificates:
    - certFile: /etc/traefik/dynamic/traefik.crt
      keyFile: /etc/traefik/dynamic/traefik.key
  options:
    machines:
      clientAuth:
        caFiles: [/etc/traefik/dynamic/ca.crt]
        clientAuthType: RequireAndVerifyClientCert
http:
  routers:
    machines:
      rule: "Host(`internal.test`) && Path(`/ping`)"
      entryPoints: [internal]
      tls:
        options: machines
      service: ping@internal
YML
docker run -d --name "$name" --network host --restart no \
  -e QD_LAKE_ADMIN_DEV=1 -e INTERNAL_DOMAIN=internal.test \
  -v "$D/traefik.yml:/etc/traefik/traefik.yml:ro" -v "$D/dynamic:/etc/traefik/dynamic:ro" \
  "$image" >/dev/null
for _ in $(seq 1 40); do
  if curl -s -o /dev/null --max-time 1 "https://127.0.0.1:$port/" -k 2>/dev/null; then break; fi
  sleep 0.5
done
sleep 1

ask() { # name, cert (or "none"), path
  local host="$1" cert="$2" path="$3" with=()
  [[ "$cert" != none ]] && with=(--cert "$D/$cert.pem")
  curl -sS --max-time 5 --resolve "$host:$port:127.0.0.1" --cacert "$D/ca.crt" "${with[@]}" \
    -o "$D/body" -w '%{http_code}' "https://$host:$port$path"
}
fail=0
check() { if [[ "$2" == "$3" ]]; then echo "ok   $1: $2"; else echo "FAIL $1: $2, expected $3"; fail=1; fi; }

seen="$(openssl s_client -connect "127.0.0.1:$port" -servername lake-admin.internal.test \
  -cert "$D/app.crt" -key "$D/app.key" -CAfile "$D/ca.crt" </dev/null 2>/dev/null |
  openssl x509 -noout -subject 2>/dev/null || true)"
check "(a) the certificate answering by the adapter's name" "$(echo "$seen" | grep -o 'OU *= *lake' | tr -d ' ')" "OU=lake"
check "(b) GET /about with the API's certificate" "$(ask lake-admin.internal.test app /lake-admin/v1/about)" 200
echo "     $(cat "$D/body")"
check "(c) GET /about with a worker's certificate" "$(ask lake-admin.internal.test worker /lake-admin/v1/about)" 403
if ask lake-admin.internal.test none /lake-admin/v1/about >/dev/null 2>&1; then
  check "(d) no client certificate" "answered" "refused"
else
  check "(d) no client certificate" "refused" "refused"
fi
check "(e) the HTTP router's own name" "$(ask internal.test app /ping)" 200
date -u +"run %Y-%m-%dT%H:%MZ with $image ($(docker image inspect -f '{{.Id}}' "$image" | cut -c1-19))"
exit $fail
