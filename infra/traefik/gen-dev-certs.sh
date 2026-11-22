#!/usr/bin/env bash
# Development certificates: a local CA and a certificate for localhost (and
# the extra names given) signed by it. Traefik serves certs/local.{crt,key};
# the browsers that should trust it import certs/dev-ca.crt, as in production
# they trust the installation's CA.
#
#   infra/traefik/gen-dev-certs.sh [extra-name ...]
#   e.g.: infra/traefik/gen-dev-certs.sh quality-data.example.local 192.168.1.50
set -euo pipefail
cd "$(dirname "$0")/certs"

if [[ ! -f dev-ca.key ]]; then
  openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj "/CN=quality-data dev CA" \
    -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -keyout dev-ca.key -out dev-ca.crt 2>/dev/null
fi

san="DNS:localhost,IP:127.0.0.1"
for name in "$@"; do
  if [[ "$name" =~ ^[0-9.]+$ ]]; then san+=",IP:$name"; else san+=",DNS:$name"; fi
done

openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" -keyout local.key -out local.csr 2>/dev/null
openssl x509 -req -in local.csr -CA dev-ca.crt -CAkey dev-ca.key -CAcreateserial -days 825 \
  -extfile <(printf "subjectAltName=%s\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n" "$san") \
  -out local.crt 2>/dev/null
rm -f local.csr
chmod 644 local.crt local.key dev-ca.crt
echo "done: certs/local.crt ($san), CA in certs/dev-ca.crt"
