#!/usr/bin/env bash
# The lake admin adapter's certificates in development (decision 0078, step
# 13), signed by the installation's CA that infra/traefik/gen-dev-internal.sh
# made (.dev/ca), in .dev/lake-admin:
#
#   ca.crt       the CA, which the adapter and the API trust alone
#   machine.pem  the adapter's: role `lake` (OU), named
#                lake-admin.<INTERNAL_DOMAIN>, the certificate then its key
#   app.pem      the API's: role `app`, the one the adapter answers
#
# The adapter runs in the lake's dev stack (infra/rustfs/compose.dev.yml,
# `task lake:up`) and is reached through 443 by its name (SNI) by
# that name, its TLS passed through to it (dynamic/lake-admin-dev.yml). Dev
# only: the files are readable by the adapter's user, and renewed by
# running this again (30 days).
#
#   infra/rustfs/gen-dev-lake-admin.sh
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
domain="${INTERNAL_DOMAIN:-internal.localhost}"
ca=.dev/ca
out=.dev/lake-admin
if [[ ! -f "$ca/ca.crt" || ! -f "$ca/ca.key" ]]; then
  echo "no CA in $ca: run infra/traefik/gen-dev-internal.sh first; the lake admin adapter is left out"
  exit 0
fi
mkdir -p "$out" && chmod 755 "$out"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# A machine's certificate as the CA signs them (decision 0066): O=qd, the
# role as OU, the name as CN and the only DNS name.
issue() {
  local role="$1" name="$2" file="$3" usage="$4"
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
    -subj "/O=qd/OU=$role/CN=$name" -keyout "$work/$file.key" -out "$work/$file.csr" 2>/dev/null
  openssl x509 -req -in "$work/$file.csr" -CA "$ca/ca.crt" -CAkey "$ca/ca.key" -CAcreateserial \
    -CAserial "$work/ca.srl" -days 30 -out "$work/$file.crt" \
    -extfile <(printf 'subjectAltName=DNS:%s\nbasicConstraints=CA:FALSE\nextendedKeyUsage=%s\n' "$name" "$usage") \
    2>/dev/null
  cat "$work/$file.crt" "$work/$file.key" >"$work/$file"
}
issue lake "lake-admin.$domain" machine.pem serverAuth,clientAuth
issue app app-dev app.pem clientAuth
cp "$ca/ca.crt" "$work/ca.crt"
# Written aside and renamed, so the adapter never reads half a file.
for f in ca.crt machine.pem app.pem; do
  install -m 644 "$work/$f" "$out/.$f.new" && mv -f "$out/.$f.new" "$out/$f"
done
echo "done: the lake admin adapter's certificates for lake-admin.$domain in $out"
