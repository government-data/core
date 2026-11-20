#!/bin/bash
# Verification of a lake pool's installation with the one command (decision
# 0029, point 5; 0066 step 11, the user's rulings of 2026-11-14). With this
# checkout's `server` on a throwaway PostgreSQL made by infra/postgres/initdb,
# the CA of `server ca-init` and a Traefik 3.6 in front as
# `https://qd.example` (as join_lake_replica.sh), the administrator defines
# the pool `main` (PUT /lake/pools/main) and asks each node's command
# (POST /machines/join-tokens, role lake, its node and two drives); four
# "servers", Docker 27.5 (dind, Alpine) containers as root named lake-a to
# lake-d, each with two tmpfs as its drives, run the commands as given. A
# throwaway DNS server (dnsmasq) answers the servers' names, as an
# installation's DNS would: inside Docker in Docker the outer Docker's own
# resolver does not reach the inner containers. The gates' check of the API
# is stood in by a server that lets every machine pass (QD_LAKE_CHECK_URL,
# a line a person adds to .env), and the node agent by the stand-in of
# join_lake_replica.sh, which joins as the agent does and does not renew
# (QD_NODE_IMAGE). Each server's Docker is given the DNS server (--dns), as
# its containers cannot reach the outer Docker's resolver.
#
# It shows: the refusals, each writing nothing (a token run under another
# node's name, a peer whose name does not resolve, a drive not empty, too
# few drives); each node's peers.yml (the others at the addresses DNS gave,
# its own name left to the loopback) and its firewall rules (DOCKER-USER,
# the peers' port open to the other nodes alone); the nodes started one at
# a time, lake-d, the last of the layout, first: it says to run its command
# again once every node is up, and the others that the admin user is
# lake-d's; then lake-c, which completes the pool, and lake-d's command
# again at the same moment: lake-d alone makes the lake's admin user, its
# keys shown once in all the runs; a later run finding the user and showing
# none; the peers' port refused from a machine outside the pool and
# accepted between nodes; a node's command run again changing nothing (its
# files, its rules, its containers); the pool answering over TLS through
# each node's gate (a client certificate of the CA required), an object
# written through one gate read through another, every drive online; then
# every node's certificate renewed in its files and one node's RustFS
# restarted: its peers take it again and the pool answers with every drive.
# With the installation naming where the machines' metrics go (decision 0071
# step 4) and a throwaway Mimir 3.2.4 as qd-app requiring a certificate of
# the CA, lake-a's command keeps its --metrics-url (the other nodes' have it
# taken out, so that one machine alone loads Alloy's image): its Alloy, in
# RustFS's network namespace, pushes lake-a's series, RustFS's own among
# them with the node's token, which a push without it lacks (refused); the
# token is in no log and no `docker inspect`; RustFS restarted three ways
# (`compose restart`, which restarts Alloy too; `docker restart` of its
# container; its process killed, as a crash, its restart policy starting it),
# Alloy joins its new namespace and pushes again (the compose file's loop for
# the last two), and the pool answers with every drive. The pool here has no
# replica, so no replication proxy runs (rustfs_pool_replication.sh). Its
# Alloy also sends the machine (decision 0072, step 2): Mimir holds each
# drive's filesystem by its name (drive1, drive2, the tmpfs's size), the
# host's interfaces (here lake-a's own, the Docker in Docker container's,
# not RustFS's namespace where Alloy runs) from the network exporter alone,
# point 1's names and labels only; Alloy has no capability, no /host/proc,
# and /host/proc1 holds net alone.
#
# Throwaway containers, network and volumes only (qd-verify-pooljoin-…). No
# key or token is printed. Run from the repository root, with the server
# built (cargo build -p server):
#
#   bash tools/verify/join_lake_pool.sh
#
# Needs docker, curl 7.75 or later (--aws-sigv4), openssl 3, python3 and
# jq; docker:27.5.1-dind and alpine:3.20 (pulled if missing), and the
# network for alpine's packages (dnsmasq, curl).
set -u
export LC_ALL=C
P=qd-verify-pooljoin
PGC=$P-pg FRONT=$P-front NET=$P-net DNS=$P-dns CHECK=$P-check
SUB=${QD_VERIFY_SUBNET:-10.231.81}
PGPORT=59070
APIPORT=59071
DIND=docker:27.5.1-dind
AGENT=qd-verify-agent:pool
NODES="lake-a lake-b lake-c lake-d"
IMAGES="rustfs/rustfs:1.0.5 traefik:v3.6"
ALLOY=grafana/alloy:v1.21.3
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SERVER=${SERVER:-$ROOT/target/debug/server}
[ -x "$SERVER" ] || { echo "no server at $SERVER: cargo build -p server"; exit 1; }
"$SERVER" openapi 2>/dev/null | grep -q '/install/{role}/pool.env' ||
  { echo "$SERVER has no pool.env route: build it again"; exit 1; }
D=$(mktemp -d)
chmod 755 "$D"
FAILED=0
SERVER_PID=
mkdir -p "$D/infra/postgres" "$D/dynamic" "$D/agent"
mkdir -m 700 "$D/secret"
cp -r "$ROOT/infra/postgres/initdb" "$D/infra/postgres/initdb"
chmod -R a+rX "$D/infra"
cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null && wait "$SERVER_PID" 2>/dev/null
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f -v >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
  docker rmi "$AGENT" >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT

ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

echo "== The installation: PostgreSQL, the CA, the API, Traefik in front"
docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f -v >/dev/null 2>&1
docker run -d --name "$PGC" -p "127.0.0.1:$PGPORT:5432" -v "$D/infra/postgres/initdb:/docker-entrypoint-initdb.d:ro" \
  -e POSTGRES_PASSWORD="$(openssl rand -hex 12)" -e POSTGRES_DB=qd -e APP_DB_USER=qd_app -e APP_DB_PASSWORD="${APP:=$(openssl rand -hex 12)}" \
  -e QD_QUEUE_PASSWORD="$(openssl rand -hex 12)" postgres:16-alpine >/dev/null || { echo "could not start PostgreSQL"; exit 1; }
sql() { docker exec -u postgres "$PGC" psql -XAtq -d qd -c "$1" 2>/dev/null; }
for _ in $(seq 60); do [ "$(sql "select count(*) from pg_roles where rolname = 'qd_queue'")" = 1 ] && break; sleep 1; done
"$SERVER" ca-init --name verify --cert "$D/ca.crt" --key "$D/secret/ca.key" >/dev/null 2>&1 || { echo "ca-init failed"; exit 1; }
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 -subj "/CN=qd.example" \
  -addext "subjectAltName=DNS:qd.example" -keyout "$D/dynamic/front.key" -out "$D/dynamic/front.crt" 2>/dev/null
docker network rm "$NET" >/dev/null 2>&1
docker network create --subnet "$SUB.0/24" "$NET" >/dev/null || { echo "could not make the network"; exit 1; }
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
RUST_LOG=error
LOGIN_RATE_LIMIT_MAX_REQUESTS=200
INSTALL_RATE_LIMIT_MAX_REQUESTS=200
MACHINES_METRICS_PUSH_URL=https://qd-app:9009/api/v1/push
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
docker run -d --name "$FRONT" --network "$NET" --ip "$SUB.5" --network-alias qd.example -v "$D/dynamic:/etc/traefik/dynamic:ro" traefik:v3.6 \
  --entrypoints.websecure.address=:443 --providers.file.directory=/etc/traefik/dynamic --accesslog=true --accesslog.format=json >/dev/null ||
  { echo "could not start Traefik"; exit 1; }
FRONT_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$FRONT")
# Mimir as qd-app (0071 step 4): HTTPS with a certificate of the CA, a client's required.
mkdir -p "$D/mimir"
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/O=qd/OU=app/CN=qd-app" \
  -keyout "$D/mimir/app.key" -out "$D/mimir/app.csr" 2>/dev/null
printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:qd-app\n' >"$D/mimir/ext"
openssl x509 -req -in "$D/mimir/app.csr" -CA "$D/ca.crt" -CAkey "$D/secret/ca.key" -CAcreateserial \
  -extfile "$D/mimir/ext" -days 1 -out "$D/mimir/app.crt" 2>/dev/null
cp "$D/ca.crt" "$D/mimir/ca.crt"
printf '%s\n' 'multitenancy_enabled: false' 'target: all' 'server: { http_listen_port: 9009, grpc_listen_address: 127.0.0.1, log_level: warn }' \
  'common: { storage: { backend: filesystem, filesystem: { dir: /data/store } } }' \
  'blocks_storage: { storage_prefix: blocks, tsdb: { dir: /data/tsdb }, bucket_store: { sync_dir: /data/tsdb-sync } }' \
  'ruler_storage: { storage_prefix: ruler }' 'alertmanager_storage: { storage_prefix: alertmanager }' 'ruler: { rule_path: /data/ruler }' \
  'activity_tracker: { filepath: /data/activity.log }' \
  'compactor: { data_dir: /data/compactor, sharding_ring: { kvstore: { store: memberlist } } }' \
  'distributor: { ring: { instance_addr: 127.0.0.1, kvstore: { store: memberlist } } }' \
  'ingester: { ring: { instance_addr: 127.0.0.1, kvstore: { store: memberlist }, replication_factor: 1 } }' \
  'store_gateway: { sharding_ring: { replication_factor: 1 } }' 'frontend: { address: 127.0.0.1 }' 'usage_stats: { enabled: false }' >"$D/mimir/mimir.yml"
# Read by Mimir's user: the test's own throwaway key, given its mode here.
chmod 755 "$D/mimir" && chmod 644 "$D/mimir"/*
docker run -d --name "$P-mimir" --network "$NET" --ip "$SUB.7" --restart no --tmpfs /data:mode=1777 \
  -v "$D/mimir:/etc/mimir:ro" grafana/mimir:3.2.4 -config.file=/etc/mimir/mimir.yml \
  -server.http-tls-cert-path=/etc/mimir/app.crt -server.http-tls-key-path=/etc/mimir/app.key \
  -server.http-tls-ca-path=/etc/mimir/ca.crt -server.http-tls-client-auth=RequireAndVerifyClientCert >/dev/null ||
  { echo "could not start Mimir"; exit 1; }

J=$D/secret/headers
login=$(curl -s -o /dev/null -w '%{http_code}' -D "$J" -H 'content-type: application/json' \
  -d "{\"email\":\"admin@example.com\",\"password\":\"$ADMIN_PASSWORD\"}" "$API/auth/login")
expect "the admin signs in to the API" 200 "$login"
[ "$login" = 200 ] || { tail -3 "$D/api.log" | cut -c1-300; exit 1; }
COOKIES=$(tr -d '\r' <"$J" | sed -n 's/^[Ss]et-[Cc]ookie: \([^;]*\).*/\1/p' | paste -sd';' | sed 's/;/; /g')
CSRF=$(printf '%s' "$COOKIES" | tr ';' '\n' | sed -n 's/^ *csrf=//p' | python3 -c 'import sys,urllib.parse;print(urllib.parse.unquote(sys.stdin.read().strip()))')
admin() { curl -s -H "Cookie: $COOKIES" -H "x-csrf-token: $CSRF" -H 'content-type: application/json' "$@"; }
issue() { admin -d "{\"role\":\"$1\"}" "$API/machines/join-tokens" >"$D/secret/$2.json"; }
field() { python3 -c 'import sys,json;v=json.load(open(sys.argv[1]));[v:=v[k] for k in sys.argv[2:]];print(v)' "$@"; }


echo "== The agent's stand-in"
declare -A ADDR
# The stand-in for qd-node: the agent's join and files, nothing more.
cp "$D/dynamic/front.crt" "$D/agent/front.crt"
cat >"$D/agent/qd-node" <<EOF
#!/bin/sh
set -eu
A=\$QD_AGENT_DIR
umask 077
if [ ! -s "\$A/machine.pem" ]; then
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/CN=\$QD_MACHINE_NAME" \
    -keyout "\$A/machine.key" -out "\$A/req.csr" 2>/dev/null
  jq -n --rawfile t "\$QD_JOIN_TOKEN_FILE" --arg n "\$QD_MACHINE_NAME" --rawfile c "\$A/req.csr" \
    '{token:\$t,name:\$n,csr:\$c}' >"\$A/join.json"
  code=\$(curl -sS --resolve "qd.example:443:$FRONT_IP" --cacert /usr/local/share/qd-verify-front.crt \
    -H 'content-type: application/json' -d @"\$A/join.json" -o "\$A/joined.json" -w '%{http_code}' \
    "\$QD_PUBLIC_URL/install/join")
  rm -f "\$A/join.json" "\$A/req.csr"
  [ "\$code" = 201 ] || { echo "stand-in: join answered \$code"; exit 1; }
  jq -r .ca "\$A/joined.json" >"\$A/.ca.crt.new" && chmod 644 "\$A/.ca.crt.new" && mv "\$A/.ca.crt.new" "\$A/ca.crt"
  { jq -r .certificate "\$A/joined.json"; cat "\$A/machine.key"; } >"\$A/.machine.pem.new"
  mv "\$A/.machine.pem.new" "\$A/machine.pem"
  rm -f "\$A/joined.json" "\$QD_JOIN_TOKEN_FILE"
  echo "stand-in: joined"
fi
while :; do sleep 3600; done
EOF
printf 'FROM alpine:3.20\nRUN apk add --no-cache curl openssl jq\nCOPY --chmod=755 qd-node /usr/local/bin/qd-node\nCOPY --chmod=644 front.crt /usr/local/share/qd-verify-front.crt\n' >"$D/agent/Dockerfile"
docker build -q -t "$AGENT" "$D/agent" >/dev/null || { echo "could not build the agent's stand-in"; exit 1; }

echo "== The servers: DNS, the gates' check, Docker in Docker, their drives"
for image in $IMAGES "$ALLOY" "$DIND" alpine:3.20 traefik/whoami:v1.11 curlimages/curl:8.11.1; do
  docker image inspect "$image" >/dev/null 2>&1 || docker pull -q "$image" >/dev/null || { echo "no $image"; exit 1; }
done
# shellcheck disable=SC2016
docker run -d --name "$DNS" --network "$NET" --ip "$SUB.2" alpine:3.20 sh -c '
  apk add -q --no-cache dnsmasq && exec dnsmasq -k --log-facility=- --no-resolv --server=127.0.0.11 '"\
  --address=/qd.example/$SUB.5 --address=/check.verify/$SUB.6 --address=/qd-app/$SUB.7 --address=/lake-a/$SUB.11 \
  --address=/lake-b/$SUB.12 --address=/lake-c/$SUB.13 --address=/lake-d/$SUB.14" >/dev/null
docker run -d --name "$CHECK" --network "$NET" --ip "$SUB.6" traefik/whoami:v1.11 >/dev/null
cp "$D/dynamic/front.crt" "$D/secret/front.crt"
# calm: the setup is paced on this machine (AGENTS.md's builds): each node's
# Docker started, and its images loaded, once the 1-minute load is under 4
# (up to 10 minutes). Only the setup waits; no check changes.
calm() { for _ in $(seq 120); do awk '{exit !($1 < 4)}' /proc/loadavg && return; sleep 5; done; }
i=10
for n in $NODES; do
  i=$((i + 1))
  calm
  docker run -d --name "$P-$n" --privileged --network "$NET" --ip "$SUB.$i" --dns "$SUB.2" --hostname "$n" \
    -e DOCKER_TLS_CERTDIR= -v "$D/secret:/s:ro" "$DIND" --dns "$SUB.2" >/dev/null || { echo "could not start $n"; exit 1; }
  ADDR[$n]=$SUB.$i
done
on() { docker exec "$P-$1" sh -c "$2"; }
# The DNS server is up once it answers (it installs dnsmasq first).
for _ in $(seq 120); do on lake-a 'getent hosts lake-d' >/dev/null 2>&1 && break; sleep 1; done
for n in $NODES; do
  calm
  for _ in $(seq 60); do on "$n" 'docker info' >/dev/null 2>&1 && break; sleep 1; done
  on "$n" 'apk add -q --no-cache curl musl-utils' >/dev/null 2>&1 || { echo "no curl on $n"; exit 1; }
  for image in $IMAGES "$AGENT"; do docker save "$image" | docker exec -i "$P-$n" docker load -q >/dev/null; done
  if [ "$n" = lake-a ]; then
    for image in "$ALLOY" curlimages/curl:8.11.1; do docker save "$image" | docker exec -i "$P-$n" docker load -q >/dev/null; done
  fi
  on "$n" 'for d in d1 d2 full; do mkdir -p /srv/$d && mount -t tmpfs -o size=256m tmpfs /srv/$d; done
    echo synthetic >/srv/full/left; mkdir -p /opt/qd/lake && printf "QD_LAKE_CHECK_URL=http://check.verify/\nQD_NODE_IMAGE='"$AGENT"'\n" >/opt/qd/lake/.env'
done
expect "lake-a resolves lake-d by the DNS server, as join.sh will" "$SUB.14" \
  "$(on lake-a 'getent ahostsv4 lake-d' | head -1 | cut -d' ' -f1)"

# run NODE NAME SCRIPT [ENV...]: SCRIPT (in /s) as root on NODE; its exit
# status, its output in NAME.out.
run() {
  local node=$1 name=$2 script=$3
  shift 3
  on "$node" "mkdir -p /m/$name"
  docker exec -w "/m/$name" -e CURL_CA_BUNDLE=/s/front.crt "$@" "$P-$node" sh "/s/$script" >"$D/$name.out" 2>&1
}
said() { grep -c -- "$2" "$D/$1.out"; }
define() { admin -X PUT -d "{\"nodes\":[$1],\"drives\":2}" -o /dev/null -w '%{http_code}' "$API/lake/pools/main"; }
# command NODE NAME: NODE's command, with its two drives, as NAME.sh.
command() {
  admin -d "{\"role\":\"lake\",\"pool\":\"main\",\"node\":\"$1\",\"drives\":[\"/srv/d1\",\"/srv/d2\"]}" \
    "$API/machines/join-tokens" >"$D/secret/$2.json"
  field "$D/secret/$2.json" install command >"$D/secret/$2.sh"
  # lake-a alone runs Alloy (0071): the others' commands lose --metrics-url.
  [ "$1" = lake-a ] || sed -i '/--metrics-url /d' "$D/secret/$2.sh"
}

echo "== What stops it, writing nothing"
expect "a pool with a node no DNS knows" 200 "$(define '"lake-a","lake-b","lake-c","lake-x"')"
command lake-a x
refused() { # WHAT NODE SCRIPT MESSAGE
  run "$2" "${3%.sh}" "$3" -e QD_JOIN_NO_START=1 -e "QD_JOIN_DIR=/opt/qd-refused/${3%.sh}"
  expect "$1" "1 1 no files" "$? $(said "${3%.sh}" "$4") $(on "$2" "[ -e /opt/qd-refused/${3%.sh} ] && echo files || echo no files")"
}
refused "a peer whose name does not resolve" lake-a x.sh "lake-x does not resolve"
expect "the pool defined again, as it should be" 200 "$(define '"lake-a","lake-b","lake-c","lake-d"')"
for n in $NODES; do command "$n" "$n"; done
expect "each command names its node" "1 1" "$(grep -c -- "--role lake --pool 'main' --name 'lake-b'" "$D/secret/lake-b.sh") $(grep -c -- "--drive '/srv/d2'" "$D/secret/lake-b.sh")"
sed "s/--name 'lake-a'/--name 'lake-b'/" "$D/secret/lake-a.sh" >"$D/secret/other.sh"
refused "lake-a's command run as lake-b" lake-b other.sh "the token is for node lake-a"
sed "s|--drive '/srv/d2'|--drive '/srv/full'|" "$D/secret/lake-a.sh" >"$D/secret/full.sh"
refused "a drive not empty" lake-a full.sh "is not empty"
sed "s| --drive '/srv/d2'||" "$D/secret/lake-a.sh" >"$D/secret/one.sh"
refused "one drive where the pool has two" lake-a one.sh "2 drives each and 1 were given"
expect "no machine joined" 0 "$(sql 'select count(*) from machines')"

echo "== The four servers, one at a time, the last of the layout first"
run lake-d lake-d lake-d.sh -e QD_JOIN_LAKE_WAIT=60
expect "lake-d started first: the pool not formed, its command to be run again" "0 1 0" \
  "$? $(said lake-d 'the last of the pool, makes the lake.s admin user') $(said lake-d 'SHOWN ONLY THIS ONCE')"
for n in lake-a lake-b; do
  run "$n" "$n" "$n.sh"
  expect "$n started: not formed, the admin user lake-d's" "0 1 0" \
    "$? $(said "$n" 'has not formed yet.*the command of lake-d') $(said "$n" 'SHOWN ONLY THIS ONCE')"
done
expect "lake-a's peers.yml: the other three at their addresses, not itself" \
  "3 0 1" "$(on lake-a 'grep -c "\"lake-[bcd]:'"$SUB"'\.1[234]\"" /opt/qd/lake/peers.yml; grep -c "lake-a" /opt/qd/lake/peers.yml; grep -c "lake-d:'"$SUB"'.14" /opt/qd/lake/peers.yml' | paste -sd' ')"
# rules NODE: the script's rules on NODE, as iptables shows them.
rules() { on "$1" 'iptables -S QD-LAKE-PEERS; iptables -S DOCKER-USER | grep -c -- "-j QD-LAKE-PEERS"'; }
expect "lake-b's rules: the three other nodes, then DROP, one jump" \
  "-N QD-LAKE-PEERS|-A QD-LAKE-PEERS -s $SUB.11/32 -j RETURN|-A QD-LAKE-PEERS -s $SUB.13/32 -j RETURN|-A QD-LAKE-PEERS -s $SUB.14/32 -j RETURN|-A QD-LAKE-PEERS -j DROP|1" \
  "$(rules lake-b | paste -sd'|')"
# Both at once: lake-c completes the pool as lake-d's command runs again.
command lake-d lake-d2
run lake-c lake-c lake-c.sh &
c_pid=$!
run lake-d lake-d2 lake-d2.sh
d_status=$?
wait "$c_pid"
c_status=$?
expect "lake-c and lake-d's command again, at once: both end well" "0 0" "$c_status $d_status"
expect "… lake-c makes no user" "0 1" "$(said lake-c 'SHOWN ONLY THIS ONCE') $(said lake-c 'the command of lake-d')"
expect "… lake-d makes it and shows its keys" 1 "$(said lake-d2 'SHOWN ONLY THIS ONCE')"
expect "the admin user's keys shown once in all the runs" 1 "$(cat "$D"/lake-?.out "$D"/lake-d2.out | grep -c 'SHOWN ONLY THIS ONCE')"
sed -n 's/^  access key: /ADMIN_ACCESS=/p; s/^  secret key: /ADMIN_SECRET=/p' "$D/lake-d2.out" >"$D/secret/admin.env"
expect "the four nodes joined, pending" "4 4" "$(sql "select count(*) from machines where role = 'lake' and pool = 'main' and state = 'pending'") $(sql "select count(distinct name) from machines")"
expect "the root keys: one row, pool:main" "1 pool:main" "$(sql 'select count(*) from lake_root_keys') $(sql 'select lake from lake_root_keys')"
command lake-d again
run lake-d again again.sh
expect "a later run of lake-d finds the user and shows nothing" "0 1 0" "$? $(said again 'is there already') $(said again 'SHOWN ONLY THIS ONCE')"

echo "== The peers' port, open to the pool's nodes alone"
docker run -d --name "$P-outsider" --network "$NET" --ip "$SUB.40" alpine:3.20 sleep 1d >/dev/null
expect "lake-a's peers' port refused from a machine outside the pool" refused \
  "$(docker exec "$P-outsider" sh -c "nc -z -w 3 ${ADDR[lake-a]} 9010 && echo open || echo refused")"
expect "… its gate's port open to it" open \
  "$(docker exec "$P-outsider" sh -c "nc -z -w 3 ${ADDR[lake-a]} 9000 && echo open || echo refused")"
expect "… and the peers' port accepted from lake-c" open "$(on lake-c "nc -z -w 3 ${ADDR[lake-a]} 9010 && echo open || echo refused")"
# state NODE: what a run may change on NODE: its files (not the token the
# command leaves), its rules and its containers.
state() {
  on "$1" 'cd /opt/qd/lake && sha256sum compose.yml drives.yml peers.yml pool.env root-keys.env .env rustfs-tls.sh lake-gate.sh lake-proxy.sh ca.crt otlp-token lake.alloy $(ls metrics.yml 2>/dev/null)
    iptables -S QD-LAKE-PEERS; iptables -S DOCKER-USER; docker ps -q --no-trunc | sort'
}
state lake-a >"$D/before"
command lake-a again2
run lake-a again2 again2.sh
state lake-a >"$D/after"
expect "lake-a's command run again: ends well, nothing changed" "0 same" \
  "$? $(cmp -s "$D/before" "$D/after" && echo same || diff "$D/before" "$D/after" | grep -c '^[<>]')"

echo "== Through each gate, over TLS, a client certificate of the CA required"
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/O=qd/OU=app/CN=app-1" \
  -keyout "$D/secret/app.key" -out "$D/secret/app.csr" 2>/dev/null
printf 'extendedKeyUsage=clientAuth\nsubjectAltName=DNS:app-1\n' >"$D/secret/ext"
openssl x509 -req -in "$D/secret/app.csr" -CA "$D/ca.crt" -CAkey "$D/secret/ca.key" -CAcreateserial \
  -extfile "$D/secret/ext" -days 1 -out "$D/secret/app.crt" 2>/dev/null
# gate NODE ARGS...: a request through NODE's gate, by the admin user.
gate() {
  local n=$1
  shift
  (
    # shellcheck source=/dev/null
    . "$D/secret/admin.env"
    printf 'user = "%s:%s"\naws-sigv4 = "aws:amz:us-east-1:s3"\ncert = "%s"\nkey = "%s"\n' \
      "$ADMIN_ACCESS" "$ADMIN_SECRET" "$D/secret/app.crt" "$D/secret/app.key"
  ) | curl -s --cacert "$D/ca.crt" --max-time 30 --resolve "$n:9000:${ADDR[$n]}" -K - "$@" 2>/dev/null
}
code() { gate "$@" -o /dev/null -w '%{http_code}'; }
disks() { gate "$1" "https://$1:9000/rustfs/admin/v3/info" | jq -r '"\(.backend.onlineDisks // .info.backend.onlineDisks)/\(.backend.offlineDisks // .info.backend.offlineDisks)"' 2>/dev/null; }
seen() { local r=''; for _ in $(seq 90); do r=$(disks "$1"); [ "$r" = "$2" ] && break; sleep 2; done; echo "$r"; }
echo synthetic >"$D/obj"
expect "a warehouse made through lake-a's gate" 200 "$(code lake-a -X PUT https://lake-a:9000/qd-tenant-verify)"
expect "an object written through lake-b's gate" 200 "$(code lake-b -T "$D/obj" https://lake-b:9000/qd-tenant-verify/o.txt)"
for n in $NODES; do expect "read through $n's gate" synthetic "$(gate "$n" "https://$n:9000/qd-tenant-verify/o.txt")"; done
expect "every drive online, as lake-c says" 8/0 "$(seen lake-c 8/0)"
expect "lake-a's gate refuses a client without a certificate" 000 \
  "$(curl -s -o /dev/null -w '%{http_code}' --cacert "$D/ca.crt" --max-time 5 --resolve "lake-a:9000:${ADDR[lake-a]}" https://lake-a:9000/)"

echo "== A certificate renewal keeps the peers"
for n in $NODES; do
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/O=qd/OU=lake/CN=$n" \
    -keyout "$D/secret/$n.key" -out "$D/secret/$n.csr" 2>/dev/null
  printf 'extendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:%s\n' "$n" >"$D/secret/ext"
  openssl x509 -req -in "$D/secret/$n.csr" -CA "$D/ca.crt" -CAkey "$D/secret/ca.key" -CAcreateserial \
    -extfile "$D/secret/ext" -days 1 -out "$D/secret/$n.crt" 2>/dev/null
  chmod 644 "$D/secret/$n.key"
  on "$n" "cat /s/$n.crt /s/$n.key >/opt/qd/lake/machine/.m && chown 10001:10001 /opt/qd/lake/machine/.m &&
    chmod 600 /opt/qd/lake/machine/.m && mv /opt/qd/lake/machine/.m /opt/qd/lake/machine/machine.pem"
done
sleep "${QD_VERIFY_RELOAD:-75}"
# served NODE: the serial NODE's RustFS serves on the peers' port, asked
# from another node, as the port is open to the pool's nodes alone.
served() {
  local from=lake-a
  [ "$1" != lake-a ] || from=lake-b
  on "$from" "timeout 20 openssl s_client -connect ${ADDR[$1]}:9010 -servername $1 -cert /s/app.crt -key /s/app.key </dev/null 2>/dev/null |
    openssl x509 -noout -serial 2>/dev/null | cut -d= -f2"
}
r=ok
for n in $NODES; do [ "$(served "$n")" = "$(openssl x509 -noout -serial -in "$D/secret/$n.crt" | cut -d= -f2)" ] || r="$n serves $(served "$n")"; done
expect "every node's RustFS serves its renewed certificate" ok "$r"
on lake-b 'cd /opt/qd/lake && docker compose -f compose.yml -f drives.yml -f peers.yml --env-file .env restart rustfs' >/dev/null 2>&1
expect "lake-b's RustFS restarted: lake-a sees every drive" 8/0 "$(seen lake-a 8/0)"
# lake-b's own RustFS may still finish its start once lake-a sees its
# drives ("Service not ready: waiting for startup_finalization"): up to 60 s.
written=$(code lake-d -T "$D/obj" https://lake-d:9000/qd-tenant-verify/after.txt) t0=$(date +%s)
for _ in $(seq 30); do read_b=$(gate lake-b https://lake-b:9000/qd-tenant-verify/after.txt); [ "$read_b" = synthetic ] && break; sleep 2; done
expect "… and an object written through lake-d's gate is read through lake-b's within 60 s" "200 synthetic" "$written $read_b"
echo "   (read through lake-b $(($(date +%s) - t0)) s after lake-a saw every drive)"
expect "the peers' port still refused from outside after the renewal" refused \
  "$(docker exec "$P-outsider" sh -c "nc -z -w 3 ${ADDR[lake-b]} 9010 && echo open || echo refused")"
expect "the admin user's secret is in no file of a server's installation" 0 \
  "$(. "$D/secret/admin.env"; for n in $NODES; do on "$n" "grep -rl '$ADMIN_SECRET' /opt/qd /m 2>/dev/null | grep -v '\.out$'"; done | grep -c .)"

echo "== lake-a's Alloy pushes to Mimir, RustFS's series with the node's token (0071 step 4)"
L=/opt/qd/lake
expect "lake-a: otlp-token 0600 RustFS's, 64 hex digits; QD_RUSTFS_METRICS; profiles" "600 10001:10001 1|QD_RUSTFS_METRICS=true|COMPOSE_PROFILES=metrics" \
  "$(on lake-a "stat -c '%a %u:%g' $L/otlp-token | tr '\n' ' '; grep -Ecx '[0-9a-f]{64}' $L/otlp-token; grep -E '^(QD_RUSTFS_METRICS|COMPOSE_PROFILES)=' $L/.env" | paste -sd'|')"
expect "lake-b runs no Alloy and its RustFS pushes nothing" "no metrics.yml|false" \
  "$(on lake-b "[ -e $L/metrics.yml ] && echo metrics.yml || echo no metrics.yml; docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' qd-lake-rustfs-1 | sed -n 's/^RUSTFS_OBS_METRICS_EXPORT_ENABLED=//p'" | paste -sd'|')"
expect "metrics.yml gives qd-app to RustFS, in whose namespace Alloy is, and each drive to Alloy" \
  "rustfs:|- \"qd-app:$SUB.7\"|alloy:|- \"/srv/d1:/host/watch/drive1:ro\"|- \"/srv/d2:/host/watch/drive2:ro\"" \
  "$(on lake-a "grep -E '^  [a-z]+:|- ' $L/metrics.yml" | sed 's/^ *//' | paste -sd'|')"
mq() { docker run --rm --network "$NET" -v "$D/mimir:/c:ro" curlimages/curl:8.11.1 -sS -G --max-time 15 --cacert /c/ca.crt \
  --cert /c/app.crt --key /c/app.key "https://qd-app:9009/prometheus/api/v1/$1" --resolve "qd-app:9009:$SUB.7" --data-urlencode "$2" 2>/dev/null; }
count() { mq query "query=count($1)" | python3 -c 'import json,sys;r=json.load(sys.stdin)["data"]["result"];print(r[0]["value"][1] if r else 0)' 2>/dev/null; }
for _ in $(seq 60); do
  [ "$(count 'up{vm="lake-a"}')" = 4 ] && [ "$(count '{job="rustfs",vm="lake-a"}')" != 0 ] && break
  sleep 5
done
KEEP=$(sed -n '/^prometheus.relabel "rustfs"/,/^}/ s/^ *regex *= "\(rustfs_.*\)"$/\1/p' "$ROOT/infra/observability/alloy/lake.alloy" | head -1)
# The host's names, decision 0072 point 1's table, written here, not read from lake.alloy.
HOST_KEEP='node_cpu_seconds_total|node_load1|node_load5|node_memory_(MemTotal|MemAvailable|SwapTotal|SwapFree)_bytes|node_filesystem_(size_bytes|avail_bytes|files|files_free|readonly)|node_disk_(read_bytes_total|written_bytes_total|io_time_seconds_total)|node_network_(receive|transmit)_(bytes|errs|drop)_total|node_timex_offset_seconds|node_timex_sync_status|node_boot_time_seconds'
mq series 'match[]={vm="lake-a"}' | python3 -c '
import json, re, sys
s = json.load(sys.stdin)["data"]
ok_names = re.compile(r"up|node_textfile_mtime_seconds|node_textfile_scrape_error|process_cpu_seconds_total|process_resident_memory_bytes|qd_node_renewals_total|qd_node_certificate_expiry_timestamp_seconds|prometheus_remote_storage_samples_failed_total|" + sys.argv[1] + "|" + sys.argv[2])
ok_labels = {"__name__", "job", "instance", "file", "service", "result", "vm", "role", "component_id", "host", "reason", "remote_name", "url", "api", "method", "status_class", "pool", "drive", "le", "cpu", "mode", "mountpoint", "device"}
print(" ".join(sorted({x["job"] for x in s})), " ".join(sorted({x["role"] for x in s})),
      len([x for x in s if not ok_names.fullmatch(x["__name__"])]), len({k for x in s for k in x} - ok_labels))' "$KEEP" "$HOST_KEEP" >"$D/series.txt"
expect "series of vm lake-a: jobs, role, names and labels outside the lists" "alloy files host rustfs lake 0 0" "$(cat "$D/series.txt")"
echo "   (lake-a's RustFS series after the keep list: $(count '{job="rustfs",vm="lake-a"}'); drives online as lake-a sees them: $(mq query 'query=rustfs_cluster_health_drives_online_count{vm="lake-a"}' | python3 -c 'import json,sys;r=json.load(sys.stdin)["data"]["result"];print(r[0]["value"][1] if r else "-")'))"
# vals PROMQL LABEL: each series' LABEL=value, sorted, space-separated.
vals() { mq query "query=$1" | python3 -c 'import json,sys;print(" ".join(sorted(x["metric"].get(sys.argv[1],"")+"="+x["value"][1] for x in json.load(sys.stdin)["data"]["result"])))' "$2" 2>/dev/null; }
# ifaces CONTAINER|-: the interfaces in /proc/net/dev of lake-a (-) or of
# CONTAINER on it, without point 1's device_exclude.
ifaces() {
  local where="cat /proc/net/dev"
  [ "$1" = - ] || where="docker exec $1 cat /proc/net/dev"
  on lake-a "$where" | awk -F: 'NR>2{gsub(/ /,"",$1);print $1}' | grep -Ev '^(loop.*|ram.*|veth.*|docker.*|br-.*|lo)$' | sort | paste -sd' '
}
expect "lake-a's host scrapes up (host, network)" "host=1 network=1" "$(vals 'up{job="host",vm="lake-a"}' instance)"
expect "each drive's filesystem by its name, the tmpfs's size (statfs)" "drive1=268435456 drive2=268435456" \
  "$(vals 'node_filesystem_size_bytes{vm="lake-a"}' mountpoint)"
expect "… its free space, for each" "drive1 drive2" \
  "$(mq query 'query=node_filesystem_avail_bytes{vm="lake-a"} > 0' | python3 -c 'import json,sys;print(" ".join(sorted(x["metric"]["mountpoint"] for x in json.load(sys.stdin)["data"]["result"])))')"
HOSTIF=$(ifaces -) OWNIF=$(ifaces qd-lake-alloy-1)
expect "the host's interfaces ($HOSTIF), not RustFS's namespace's ($OWNIF), where Alloy runs" "$HOSTIF|differ" \
  "$(mq query 'query=node_network_receive_bytes_total{vm="lake-a"}' | python3 -c 'import json,sys;print(" ".join(sorted(x["metric"]["device"] for x in json.load(sys.stdin)["data"]["result"])))')|$([ "$HOSTIF" != "$OWNIF" ] && echo differ || echo same)"
expect "… from the network exporter alone" "network" "$(vals 'count by (instance) (node_network_receive_bytes_total{vm="lake-a"})' instance | sed 's/=[0-9]*//g')"
expect "CPU modes kept" "idle iowait system user" "$(mq query 'query=count by (mode) (node_cpu_seconds_total{vm="lake-a"})' | python3 -c 'import json,sys;print(" ".join(sorted(x["metric"]["mode"] for x in json.load(sys.stdin)["data"]["result"])))')"
expect "lake-a's Alloy: no capability, no new privileges, no /host/proc, /host/proc1 holds net alone" "[ALL] [no-new-privileges:true]|absent|net" \
  "$(on lake-a "docker inspect -f '{{.HostConfig.CapDrop}} {{.HostConfig.SecurityOpt}}' qd-lake-alloy-1; docker exec qd-lake-alloy-1 sh -c '[ -e /host/proc ] && echo present || echo absent; ls /host/proc1'" | paste -sd'|')"
echo "   (lake-a's host series: $(count '{job="host",vm="lake-a"}'); its drives' free bytes: $(vals 'node_filesystem_avail_bytes{vm="lake-a"}' mountpoint))"
on lake-a "umask 077 && printf 'authorization: Bearer %s\n' \"\$(cat $L/otlp-token)\" >/root/otlp-auth"
# otlp ARGS: an empty OTLP push to lake-a's receiver from RustFS's namespace.
otlp() { on lake-a "docker run --rm --user 0 --network container:qd-lake-rustfs-1 -v /root/otlp-auth:/auth:ro -v $L/ca.crt:/ca.crt:ro curlimages/curl:8.11.1 -s -o /dev/null -w '%{http_code}' --max-time 10 --cacert /ca.crt --resolve lake-a:4318:127.0.0.1 -H 'content-type: application/json' -d '{}' $* https://lake-a:4318/v1/metrics"; }
expect "lake-a's receiver, from RustFS's namespace: with the token, without" "200 401" "$(otlp -H @/auth) $(otlp)"
expect "… from RustFS's other network, at its address: no answer" 000 \
  "$(on lake-a "docker run --rm --network qd-lake_default curlimages/curl:8.11.1 -s -o /dev/null -w '%{http_code}' --max-time 5 -k https://\$(docker inspect -f '{{(index .NetworkSettings.Networks \"qd-lake_default\").IPAddress}}' qd-lake-rustfs-1):4318/v1/metrics")"
expect "the token in no log or docker inspect of lake-a's containers" 0 \
  "$(on lake-a "for c in node rustfs gate alloy; do docker logs qd-lake-\$c-1 2>&1; docker inspect qd-lake-\$c-1; done | grep -cF -f $L/otlp-token")"
# age PROMQL: seconds since the series' last sample, or 999.
age() { mq query "query=time() - $1" | python3 -c 'import json,sys;r=json.load(sys.stdin)["data"]["result"];print(int(float(r[0]["value"][1])) if r else 999)' 2>/dev/null || echo 999; }
# again LABEL: after lake-a's RustFS restarted at $restarted, within 4
# minutes, Alloy back in its namespace: a RustFS sample of lake-a newer than
# the restart (the old namespace reaches neither RustFS nor Mimir), 60 s on.
again() {
  local label=$1 r0=$2 a0=$3 said0=$4 fresh
  for _ in $(seq 48); do
    sleep 5
    [ $(($(date +%s) - restarted)) -ge 60 ] && [ "$(age 'timestamp(rustfs_process_uptime_seconds{vm="lake-a"})')" -lt 45 ] && break
  done
  fresh=$(age 'timestamp(rustfs_process_uptime_seconds{vm="lake-a"})')
  echo "   ($label, after $(($(date +%s) - restarted)) s: RustFS $([ "$(on lake-a 'docker inspect -f {{.State.StartedAt}} qd-lake-rustfs-1')" != "$r0" ] && echo restarted || echo "not restarted");" \
    "Alloy restarted by its policy $(($(on lake-a 'docker inspect -f {{.RestartCount}} qd-lake-alloy-1') - a0)) times, its loop's line" \
    "$(($(on lake-a "docker logs qd-lake-alloy-1 2>&1 | grep -c '^qd: RustFS restarted'") - said0)) times; last samples of Alloy" \
    "$(age 'max(timestamp(up{job="alloy",vm="lake-a"}))') s, of RustFS $fresh s old)"
  expect "$label: Alloy back in its namespace, lake-a's RustFS series fresh again" 1 \
    "$([ "$fresh" -lt 45 ] && [ "$fresh" -le $(($(date +%s) - restarted)) ] && echo 1 || echo 0)"
  [ "$fresh" -lt 45 ] ||
    echo "   (Alloy's last lines: $(on lake-a 'docker logs qd-lake-alloy-1 2>&1 | tail -3' | cut -c1-250 | tr '\n' ' '); RustFS's on OTLP: $(on lake-a 'docker logs qd-lake-rustfs-1 2>&1 | grep -i "otlp\|export\|metric" | tail -2' | cut -c1-250 | tr '\n' ' '))"
  expect "… and lake-c sees every drive" 8/0 "$(seen lake-c 8/0)"
}
# restart_a LABEL COMMAND: lake-a's RustFS restarted by COMMAND, run on lake-a.
restart_a() {
  local r0 a0 said0
  r0=$(on lake-a 'docker inspect -f {{.State.StartedAt}} qd-lake-rustfs-1')
  a0=$(on lake-a 'docker inspect -f {{.RestartCount}} qd-lake-alloy-1')
  said0=$(on lake-a "docker logs qd-lake-alloy-1 2>&1 | grep -c '^qd: RustFS restarted'")
  restarted=$(date +%s)
  on lake-a "$2" >/dev/null 2>&1
  again "$1" "$r0" "$a0" "$said0"
}
restart_a "compose restart" "cd $L && docker compose -f compose.yml -f drives.yml -f peers.yml -f metrics.yml --env-file .env restart rustfs"
# Docker leaves Alloy in the old namespace here: the compose file's loop.
restart_a "docker restart" "docker restart qd-lake-rustfs-1"
restart_a "RustFS killed" "docker exec -u 0 qd-lake-rustfs-1 sh -c 'kill -KILL -1'"
echo "   (servers' Docker $(on lake-a 'docker version -f {{.Server.Version}}'), Compose $(on lake-a 'docker compose version --short'))"

echo
if [ "$FAILED" = 0 ]; then echo "ALL OK"; else
  echo "SOME CHECKS FAILED"
  exit 1
fi
