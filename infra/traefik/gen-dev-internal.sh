#!/usr/bin/env bash
# The internal entry point in development (decision 0066, step 4): the
# installation's CA made by `server ca-init` (once, in .dev/ca), a server
# certificate for INTERNAL_DOMAIN signed by it, and Traefik's files in its
# watched directory:
#
#   infra/traefik/dynamic/machines-ca.crt   the CA, the only client issuer
#   infra/traefik/dynamic/internal.crt/.key the internal entry point's own
#   infra/traefik/dynamic/internal.yml      a copy of infra/traefik/internal.yml
#
# Until this runs, nothing answers on the internal entry point and the rest of
# the stack is as before. Traefik takes the files without a restart; in watch
# mode `task dev:watch` copies them into .dev/traefik.
#
#   infra/traefik/gen-dev-internal.sh
#
# SERVER is how `server` is run (default: cargo run -q -p server --);
# INTERNAL_DOMAIN the name machines use (default internal.localhost, which
# resolves to this machine). The API then needs CA_CERT_FILE=.dev/ca/ca.crt
# and CA_KEY_FILE=.dev/ca/ca.key (tools/dev/api.sh sets them when they
# exist). Renewal through Traefik also needs INTERNAL_CLIENT_CERT_HEADER=
# trusted, only where the API cannot be reached but through Traefik.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
domain="${INTERNAL_DOMAIN:-internal.localhost}"
dynamic=infra/traefik/dynamic
ca=.dev/ca

if [[ ! -f "$ca/ca.crt" ]]; then
  mkdir -p "$ca" && chmod 700 "$ca"
  # shellcheck disable=SC2086 # SERVER is a command with its arguments.
  ${SERVER:-cargo run -q -p server --} ca-init --name "qd dev" --cert "$ca/ca.crt" --key "$ca/ca.key" >/dev/null
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/O=qd/CN=$domain" \
  -keyout "$work/internal.key" -out "$work/internal.csr" 2>/dev/null
openssl x509 -req -in "$work/internal.csr" -CA "$ca/ca.crt" -CAkey "$ca/ca.key" -CAcreateserial \
  -CAserial "$work/ca.srl" -days 30 -out "$work/internal.crt" \
  -extfile <(printf 'subjectAltName=DNS:%s,DNS:lakegate.%s\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n' "$domain" "$domain") \
  2>/dev/null
cp "$ca/ca.crt" "$work/machines-ca.crt"
cp infra/traefik/internal.yml "$work/internal.yml"
# Written aside and renamed (0066, point 13); the certificates first, so the
# configuration never names a file that is not there yet.
for f in machines-ca.crt internal.key internal.crt internal.yml; do
  chmod 644 "$work/$f"
  cp "$work/$f" "$dynamic/.$f.new" && mv -f "$dynamic/.$f.new" "$dynamic/$f"
done
echo "done: the internal entry point for $domain; its CA is $ca/ca.crt"
