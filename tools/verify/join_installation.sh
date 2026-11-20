# shellcheck shell=bash disable=SC2034 # sourced: FAILED and FRONT_IP are the sourcing script's
# The installation the join verifications run their commands against,
# sourced by tools/verify/join_script.sh and metrics_join_worker.sh: this
# checkout's `server` on a throwaway PostgreSQL made by
# infra/postgres/initdb, the CA of `server ca-init`, and a Traefik 3.6 in
# front for TLS as `https://qd.example` (its certificate made here, trusted
# by the machine as a public one would be), the administrator signed in.
#
# The sourcing script sets, before:
#   P PGPORT APIPORT ROOT SERVER D   names, ports, the checkout, the binary
#                                    and the run's directory (with secret/,
#                                    0700, and dynamic/)
#   PG_ARGS (array, optional)        PostgreSQL's image and its flags, with
#                                    more `docker run` options before it
#                                    (postgres:16-alpine, no flags)
#   API_ENV (optional)               more lines of the API's environment
#   FRONT_EXT (optional)             one more extension of the front's
#                                    certificate
#   FAILED                           0
# and gets: PGC FRONT NET API API_HOST FRONT_IP, ok ko expect, sql, admin,
# issue ROLE NAME (a join token's answer in secret/NAME.json, never
# printed), field FILE KEY…, and install_cleanup.
PGC=$P-pg
FRONT=$P-front
NET=$P-net
SERVER_PID=
[ -n "${PG_ARGS+x}" ] || PG_ARGS=(postgres:16-alpine)

ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }
install_cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null && wait "$SERVER_PID" 2>/dev/null
  docker rm -f -v "$PGC" "$FRONT" >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
}

echo "== The installation: PostgreSQL, the CA, the API, Traefik in front"
mkdir -p "$D/infra/postgres" "$D/dynamic"
cp -r "$ROOT/infra/postgres/initdb" "$D/infra/postgres/initdb"
chmod -R a+rX "$D/infra"
docker rm -f -v "$PGC" "$FRONT" >/dev/null 2>&1
docker network rm "$NET" >/dev/null 2>&1
docker network create "$NET" >/dev/null || { echo "could not make the network"; exit 1; }
docker run -d --name "$PGC" --network "$NET" -p "127.0.0.1:$PGPORT:5432" \
  -v "$D/infra/postgres/initdb:/docker-entrypoint-initdb.d:ro" \
  -e POSTGRES_PASSWORD="$(openssl rand -hex 12)" -e POSTGRES_DB=qd -e APP_DB_USER=qd_app -e APP_DB_PASSWORD="${APP:=$(openssl rand -hex 12)}" \
  -e QD_QUEUE_PASSWORD="$(openssl rand -hex 12)" "${PG_ARGS[@]}" >/dev/null || { echo "could not start PostgreSQL"; exit 1; }
sql() { docker exec -u postgres "$PGC" psql -XAtq -d qd -c "$1" 2>/dev/null; }
for _ in $(seq 60); do [ "$(sql "select count(*) from pg_roles where rolname = 'qd_queue'")" = 1 ] && break; sleep 1; done
"$SERVER" ca-init --name verify --cert "$D/ca.crt" --key "$D/secret/ca.key" >/dev/null 2>&1 || { echo "ca-init failed"; exit 1; }
# The installation's public certificate, for qd.example (as Let's Encrypt's would be).
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 -subj "/CN=qd.example" \
  -addext "subjectAltName=DNS:qd.example" ${FRONT_EXT:+-addext "$FRONT_EXT"} -keyout "$D/dynamic/front.key" -out "$D/dynamic/front.crt" 2>/dev/null
API_HOST=$(docker network inspect "$NET" -f '{{(index .IPAM.Config 0).Gateway}}')
API=http://$API_HOST:$APIPORT
ADMIN_PASSWORD=Qd-$(openssl rand -hex 14)
cat >"$D/secret/api.env" <<EOF
DATABASE_URL=postgres://qd_app:$APP@127.0.0.1:$PGPORT/qd
APP_HOST=$API_HOST
APP_PORT=$APIPORT
APP_PUBLIC_URL=https://qd.example
SETTINGS_ENCRYPTION_KEY=$(openssl rand -base64 32)
OIDC_SIGNING_KEY=$("$SERVER" oidc-keygen 2>/dev/null | tail -1)
ADMIN_BOOTSTRAP_EMAIL=admin@example.com
ADMIN_BOOTSTRAP_PASSWORD=$ADMIN_PASSWORD
CA_CERT_FILE=$D/ca.crt
CA_KEY_FILE=$D/secret/ca.key
PAYMENT_PROVIDER=none
RUST_LOG=error
${API_ENV:-}
EOF
# shellcheck disable=SC2046
(exec env -i HOME="$HOME" PATH="$PATH" $(cat "$D/secret/api.env") "$SERVER" serve) >>"$D/api.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 90); do [ "$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "$API/health")" = 200 ] && break; sleep 1; done
# shellcheck disable=SC2046
env -i HOME="$HOME" PATH="$PATH" $(cat "$D/secret/api.env") "$SERVER" bootstrap-admin >/dev/null 2>&1
cat >"$D/dynamic/front.yml" <<EOF
tls:
  certificates:
    - certFile: /etc/traefik/dynamic/front.crt
      keyFile: /etc/traefik/dynamic/front.key
http:
  routers:
    api:
      rule: Host(\`qd.example\`) && PathPrefix(\`/api\`)
      entryPoints: [websecure]
      middlewares: [strip-api]
      service: api
      tls: {}
  middlewares:
    strip-api:
      stripPrefix:
        prefixes: [/api]
  services:
    api:
      loadBalancer:
        servers:
          - url: "$API"
EOF
chmod 755 "$D/dynamic" && chmod 644 "$D/dynamic"/*
docker run -d --name "$FRONT" --network "$NET" --network-alias qd.example -v "$D/dynamic:/etc/traefik/dynamic:ro" traefik:v3.6 \
  --entrypoints.websecure.address=:443 --providers.file.directory=/etc/traefik/dynamic --accesslog=true --accesslog.format=json >/dev/null ||
  { echo "could not start Traefik"; exit 1; }
FRONT_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$FRONT")

J=$D/secret/headers
login=$(curl -s -o /dev/null -w '%{http_code}' -D "$J" -H 'content-type: application/json' \
  -d "{\"email\":\"admin@example.com\",\"password\":\"$ADMIN_PASSWORD\"}" "$API/auth/login")
expect "the admin signs in to the API" 200 "$login"
[ "$login" = 200 ] || { tail -3 "$D/api.log" | cut -c1-300; exit 1; }
COOKIES=$(tr -d '\r' <"$J" | sed -n 's/^[Ss]et-[Cc]ookie: \([^;]*\).*/\1/p' | paste -sd';' | sed 's/;/; /g')
CSRF=$(printf '%s' "$COOKIES" | tr ';' '\n' | sed -n 's/^ *csrf=//p' | python3 -c 'import sys,urllib.parse;print(urllib.parse.unquote(sys.stdin.read().strip()))')
admin() { curl -s -H "Cookie: $COOKIES" -H "x-csrf-token: $CSRF" -H 'content-type: application/json' "$@"; }
# issue ROLE NAME: a join token's answer, kept in secret/NAME.json, never printed.
issue() { admin -d "{\"role\":\"$1\",\"pool\":\"default\"}" "$API/machines/join-tokens" >"$D/secret/$2.json"; }
field() { python3 -c 'import sys,json;v=json.load(open(sys.argv[1]));[v:=v[k] for k in sys.argv[2:]];print(v)' "$@"; }
