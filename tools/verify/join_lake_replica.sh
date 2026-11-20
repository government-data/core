#!/bin/bash
# Verification of the lake replica's installation with the one command
# (decision 0029, point 7; 0066 step 11 and the user's rulings of
# 2026-11-13: the root keys made by the application and given against the
# join token, the entrypoints served with their hashes in join.sh, the
# replica's drives as mounted paths, each its own filesystem and empty, a
# clock out of sync stopping it). With this checkout's `server` on a
# throwaway PostgreSQL made by infra/postgres/initdb, the CA of `server
# ca-init`, and a Traefik 3.6 in front for TLS as `https://qd.example` (its
# certificate made here, trusted by the machine as a public one would be),
# the administrator asks for a lake-replica's join token and runs the
# command the API answers, with its drives added (--drive), on a "machine":
# a Docker 27.5 (dind, Alpine) container as root, its host name replica-1,
# two tmpfs mounted as its drives, and curl added. It shows: the files the
# script leaves and their modes, the drives given to RustFS's user; run
# again, the same; the refusals (a drive not empty, a drive not a
# filesystem of its own, no drive, an entrypoint whose hash is not the one
# the script names, another role's token, the replica's token for another
# role, a clock an hour off), none writing anything; then the command
# started: the node agent joins with the token the script left, RustFS
# serves TLS with the machine's certificate, takes the root keys it was
# given and trusts the CA (ca.crt), the gate refuses a client without a
# certificate; the script makes the platform's admin user with the root
# keys and shows its keys once (the user's ruling of 2026-11-13), the user
# then doing what the platform's admin does with its own keys alone, its
# secret in no file of the installation's directory and in no container's
# log; the command once more is refused (401), changing nothing; and a
# command with a new token, run again, makes no user and shows no secret,
# the first one still working. With the installation naming where the
# machines' metrics go (decision 0071 step 4) and a throwaway Mimir 3.2.4 as
# qd-app requiring a certificate of the CA, the replica's Alloy pushes its
# series of vm replica-1, role lake-replica, within its lists, RustFS's own
# among them: RustFS pushes them over OTLP to Alloy with the node's token,
# which join.sh made (0600, RustFS's user's) and a second run keeps; a push
# without it or with another is refused, one from outside the network
# `otlp` does not reach the receiver, and the token is in no log and in no
# `docker inspect` of the replica's containers. Its Alloy also sends the
# machine (decision 0072, step 2): Mimir holds each drive's filesystem by
# its name (drive1, drive2, the tmpfs's size), the host's interfaces (here
# the machine's own, the Docker in Docker container's, not Alloy's
# container's) from the network exporter alone, point 1's names and labels
# only; Alloy has no capability, no /host/proc, and /host/proc1 holds net
# alone.
#
# The node agent is a stand-in (an Alpine image built here, `qd-node` a
# script): it joins as the agent does, its key made on the machine, the
# token read from join/token and deleted once joined, and writes ca.crt and
# machine.pem as the agent does, but does not renew. The worker's image,
# which holds the real agent, is not built here. The gate's check of the
# API (forwardAuth) is not reached: the internal entry point is not part
# of this installation.
#
# The API's limit per address on the install routes is raised
# (INSTALL_RATE_LIMIT_MAX_REQUESTS), and joining's (the login's,
# LOGIN_RATE_LIMIT_MAX_REQUESTS): every run here comes from one address, and
# a run of this role makes four requests on the first (30 a minute by
# default).
#
# Containers qd-verify-replica-pg, -front and -machine (the replica's own
# containers live inside the machine), network qd-verify-replica-net, the
# image qd-verify-agent:replica, port 127.0.0.1:59060 (PostgreSQL); the API
# listens on that network's gateway address, port 59061. Nothing of the
# dev stack is touched. Tokens, keys and passwords are never printed: the
# command is written to a 0600 file and run from it, keys are compared,
# never shown; the admin user's secret the script shows is taken out of its
# output into a 0600 file at once. States only. Run from the repository
# root, with the server
# built (cargo build -p server):
#
#   bash tools/verify/join_lake_replica.sh   (SERVER=target/debug/server)
#
# Needs docker (privileged containers), curl, openssl 3 and python3, and
# the images rustfs/rustfs:1.0.5, traefik:v3.6, curlimages/curl:8.11.1,
# docker:27.5.1-dind and alpine:3.20 (pulled if missing).
set -u
export LC_ALL=C
P=qd-verify-replica
PGC=$P-pg FRONT=$P-front NET=$P-net M=$P-machine
PGPORT=59060
APIPORT=59061
DIND=docker:27.5.1-dind
AGENT=qd-verify-agent:replica
IMAGES="rustfs/rustfs:1.0.5 traefik:v3.6 curlimages/curl:8.11.1 grafana/alloy:v1.21.3"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SERVER=${SERVER:-$ROOT/target/debug/server}
[ -x "$SERVER" ] || { echo "no server at $SERVER: cargo build -p server"; exit 1; }
"$SERVER" openapi 2>/dev/null | grep -q '/install/{role}/root-keys.env' ||
  { echo "$SERVER has no root keys route: build it again"; exit 1; }
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
  docker rm -f -v "$PGC" "$FRONT" "$M" "$P-mimir" >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
  docker rmi "$AGENT" >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT

ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }

echo "== The installation: PostgreSQL, the CA, the API, Traefik in front"
docker rm -f -v "$PGC" "$FRONT" "$M" >/dev/null 2>&1
docker run -d --name "$PGC" -p "127.0.0.1:$PGPORT:5432" -v "$D/infra/postgres/initdb:/docker-entrypoint-initdb.d:ro" \
  -e POSTGRES_PASSWORD="$(openssl rand -hex 12)" -e POSTGRES_DB=qd -e APP_DB_USER=qd_app -e APP_DB_PASSWORD="${APP:=$(openssl rand -hex 12)}" \
  -e QD_QUEUE_PASSWORD="$(openssl rand -hex 12)" postgres:16-alpine >/dev/null || { echo "could not start PostgreSQL"; exit 1; }
sql() { docker exec -u postgres "$PGC" psql -XAtq -d qd -c "$1" 2>/dev/null; }
for _ in $(seq 60); do [ "$(sql "select count(*) from pg_roles where rolname = 'qd_queue'")" = 1 ] && break; sleep 1; done
"$SERVER" ca-init --name verify --cert "$D/ca.crt" --key "$D/secret/ca.key" >/dev/null 2>&1 || { echo "ca-init failed"; exit 1; }
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 -subj "/CN=qd.example" \
  -addext "subjectAltName=DNS:qd.example" -keyout "$D/dynamic/front.key" -out "$D/dynamic/front.crt" 2>/dev/null
docker network rm "$NET" >/dev/null 2>&1
docker network create "$NET" >/dev/null || { echo "could not make the network"; exit 1; }
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
docker run -d --name "$FRONT" --network "$NET" --network-alias qd.example -v "$D/dynamic:/etc/traefik/dynamic:ro" traefik:v3.6 \
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
docker run -d --name "$P-mimir" --network "$NET" --network-alias qd-app --restart no --tmpfs /data:mode=1777 \
  -v "$D/mimir:/etc/mimir:ro" grafana/mimir:3.2.4 -config.file=/etc/mimir/mimir.yml \
  -server.http-tls-cert-path=/etc/mimir/app.crt -server.http-tls-key-path=/etc/mimir/app.key \
  -server.http-tls-ca-path=/etc/mimir/ca.crt -server.http-tls-client-auth=RequireAndVerifyClientCert >/dev/null ||
  { echo "could not start Mimir"; exit 1; }
MIMIR_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$P-mimir")

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

echo "== The machine: Docker in Docker, its drives, the images, the agent's stand-in"
for image in $IMAGES "$DIND" alpine:3.20; do
  docker image inspect "$image" >/dev/null 2>&1 || docker pull -q "$image" >/dev/null || { echo "no $image"; exit 1; }
done
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
cp "$D/dynamic/front.crt" "$D/secret/front.crt"
docker run -d --name "$M" --privileged --network "$NET" --hostname replica-1 -e DOCKER_TLS_CERTDIR= \
  -v "$D/secret:/s:ro" "$DIND" >/dev/null || { echo "could not start the machine"; exit 1; }
for _ in $(seq 60); do docker exec "$M" docker info >/dev/null 2>&1 && break; sleep 1; done
on() { docker exec "$M" sh -c "$1"; }
on 'apk add -q --no-cache curl' >/dev/null 2>&1 || { echo "could not add curl to the machine"; exit 1; }
for image in $IMAGES "$AGENT"; do docker save "$image" | docker exec -i "$M" docker load -q >/dev/null; done
# Its drives: two filesystems of their own, one with something on it, and
# a directory on the root's.
on 'for d in d1 d2 full; do mkdir -p /srv/$d && mount -t tmpfs -o size=256m tmpfs /srv/$d; done
    echo synthetic >/srv/full/left; mkdir -p /srv/plain'
expect "the drives: three filesystems of their own and a plain directory" "3 1" \
  "$(on 'grep -c " /srv/" /proc/mounts') $(on 'mountpoint -q /srv/plain || echo 1')"

issue lake-replica r1
field "$D/secret/r1.json" install command >"$D/secret/given.sh"
expect "the answer's command is for the role lake-replica" 1 "$(grep -c -- '--role lake-replica' "$D/secret/given.sh")"
# The command as given, its drives added at the end, as the person does.
sed '$ s|$| --drive /srv/d1 --drive /srv/d2|' "$D/secret/given.sh" >"$D/secret/r1.sh"
issue worker w1
chmod 600 "$D/secret"/*

# run DIR SCRIPT [ENV...]: SCRIPT (a file in /s) run as root in the machine,
# from /m/DIR; its exit status, its output in m-DIR.out.
run() {
  local dir=$1 script=$2
  shift 2
  on "mkdir -p /m/$dir"
  docker exec -w "/m/$dir" -e CURL_CA_BUNDLE=/s/front.crt "$@" "$M" sh "/s/$script" >"$D/m-$dir.out" 2>&1
}
nostart() { run "$@" -e QD_JOIN_NO_START=1; }
said() { grep -c "$2" "$D/m-$1.out"; }
nothing() { on "[ -e $1 ] && echo files || echo no files"; }
R=/opt/qd/lake-replica
listing() { on "cd $R && stat -c '%a %u:%g %n' ca.crt compose.yml drives.yml lake-gate.sh rustfs-tls.sh lake-replica.alloy metrics.yml root-keys.env otlp-token .env join join/token machine /srv/d1 /srv/d2"; }
# state: every file's name, mode, owner, size and SHA-256, but the agent's own.
state() {
  on "cd $R && find . -path ./machine -prune -o -print | sort | xargs stat -c '%n %a %u %s' &&
    find . -path ./machine -prune -o -type f -print | sort | xargs sha256sum" | sha256sum
}

echo "== The command as given, with its drives (not started)"
nostart run1 r1.sh
expect "it runs to the end" 0 $?
expect "what it said" "qd-join.sh: OK|qd-join: the installation's CA checked|qd-join: the lake-replica compose file and its files checked|qd-join: written to $R as replica-1|qd-join: not started (QD_JOIN_NO_START=1)" \
  "$(paste -sd'|' "$D/m-run1.out")"
expect "the files, their modes and owners; the drives RustFS's" \
  "644 0:0 ca.crt|644 0:0 compose.yml|644 0:0 drives.yml|755 0:0 lake-gate.sh|755 0:0 rustfs-tls.sh|644 0:0 lake-replica.alloy|644 0:0 metrics.yml|600 0:0 root-keys.env|600 10001:10001 otlp-token|600 0:0 .env|700 10001:10001 join|600 10001:10001 join/token|700 10001:10001 machine|750 10001:10001 /srv/d1|750 10001:10001 /srv/d2" \
  "$(listing | paste -sd'|')"
for f in compose.yml:infra/rustfs/replica/docker-compose.yml rustfs-tls.sh:infra/rustfs/tls/entrypoint.sh lake-gate.sh:infra/rustfs/gate/entrypoint.sh lake-replica.alloy:infra/observability/alloy/lake-replica.alloy; do
  expect "${f%%:*} is this version's ${f#*:}" "$(sha256sum "$ROOT/${f#*:}" | cut -d' ' -f1)" "$(on "sha256sum $R/${f%%:*}" | cut -d' ' -f1)"
done
expect "drives.yml mounts each drive in RustFS" 'RUSTFS_VOLUMES: "/data/1 /data/2"|- "/srv/d1:/data/1"|- "/srv/d2:/data/2"' \
  "$(on "grep -v '^#' $R/drives.yml" | grep -E 'VOLUMES|- "' | sed 's/^ *//' | paste -sd'|')"
expect "the environment's keys" "QD_MACHINE_NAME=replica-1|QD_CA_SHA256|QD_PUBLIC_URL=https://qd.example/api|APP_DOMAIN=qd.example|QD_JOIN_TOKEN_DIR=$R/join|QD_LAKE_DRIVES=/srv/d1 /srv/d2|OBS_VM=replica-1|METRICS_PUSH_URL=https://qd-app:9009/api/v1/push|QD_RUSTFS_METRICS=true|COMPOSE_PROFILES=metrics" \
  "$(on "cat $R/.env" | sed 's/^QD_CA_SHA256=.*/QD_CA_SHA256/' | paste -sd'|')"
expect "root-keys.env: the two keys, nothing else" "RUSTFS_ACCESS_KEY|RUSTFS_SECRET_KEY" "$(on "cut -d= -f1 $R/root-keys.env" | paste -sd'|')"
same=different
[ "$(on "sed -n 's/^RUSTFS_ACCESS_KEY=//p' $R/root-keys.env")" = "$(sql 'select access_key from lake_root_keys')" ] && same=same
expect "its access key is the one the application keeps (one lake)" "same 1" "$same $(sql 'select count(*) from lake_root_keys')"
expect "the secret is kept sealed" 0 "$(on "sed -n 's/^RUSTFS_SECRET_KEY=//p' $R/root-keys.env" | grep -cFf - <(sql 'select secret_key from lake_root_keys'))"
expect "otlp-token: 64 hex digits, one line" "1 64" "$(on "grep -Ecx '[0-9a-f]{64}' $R/otlp-token; wc -c <$R/otlp-token" | paste -sd' ')"
expect "given audited, once" "1" "$(sql "select count(*) from admin_audit_events where action = 'install:lake-replica' and resource = 'lake/replica'")"

echo "== Run again: the same (a drive holding what RustFS left is this server's)"
stamp=$(state)
token_sum=$(on "sha256sum <$R/otlp-token")
on 'mkdir -p /srv/d1/.rustfs.sys && echo synthetic >/srv/d1/.rustfs.sys/left'
nostart run2 r1.sh
expect "the same files, the same keys" "0 $stamp" "$? $(state)"
expect "the same otlp-token (compared, not shown)" same "$([ "$token_sum" = "$(on "sha256sum <$R/otlp-token")" ] && echo same || echo different)"
on 'rm -rf /srv/d1/.rustfs.sys'
expect "the machine has not joined: the token is unused" 0 "$(sql 'select count(*) from machines')"

echo "== What stops it, writing nothing"
# refused WHAT SCRIPT MESSAGE [ENV...]: run into a directory of its own
# (named after SCRIPT); exit 1, the message, no files.
refused() {
  local what=$1 script=$2 message=$3 dir=${2%.sh}
  shift 3
  nostart "$dir" "$script" -e QD_JOIN_DIR="/opt/qd-refused/$dir" "$@"
  expect "$what" "1 1 no files" "$? $(said "$dir" "$message") $(nothing "/opt/qd-refused/$dir")"
}
with() { sed "s|--drive /srv/d1 --drive /srv/d2|$1|" "$D/secret/r1.sh" >"$D/secret/$2.sh"; }
with "--drive /srv/full" full && refused "a drive not empty" full.sh "is not empty"
with "--drive /srv/plain" plain && refused "a drive not a filesystem of its own" plain.sh "not a filesystem of its own"
with "--drive /srv/d1 --drive /srv/d1" twice && refused "the same drive twice" twice.sh "same filesystem as another"
with "" nodrive && refused "no drive" nodrive.sh "give each drive with --drive"
# The script as the command downloaded it, naming another hash for rustfs-tls.sh.
on "sed 's/^RUSTFS_TLS_SHA256=.*/RUSTFS_TLS_SHA256=$(printf '0%.0s' $(seq 64))/' /m/run1/qd-join.sh" >"$D/secret/other-hash.sh"
sed 's/^.*&& sh qd-join.sh /sh \/s\/other-hash.sh /' "$D/secret/r1.sh" | sed '1,2{/^curl\|^ *&& echo/d}' >"$D/secret/hash.sh"
refused "an entrypoint not of the hash the script names" hash.sh "rustfs-tls.sh is not this version's"
worker_token=$(field "$D/secret/w1.json" token)
sed "s/--token '[^']*'/--token '$worker_token'/" "$D/secret/r1.sh" >"$D/secret/wtoken.sh"
refused "a worker's token for lake-replica" wtoken.sh "unknown, used or expired"
sed "s/--role lake-replica/--role worker/" "$D/secret/r1.sh" >"$D/secret/asworker.sh"
refused "the replica's token for the role worker" asworker.sh "unknown, used or expired"
mkdir -p "$D/skew"
printf '#!/bin/sh\nif [ "$*" = "-u +%%s" ]; then echo $(($(/bin/date -u +%%s) + 3600)); else exec /bin/date "$@"; fi\n' >"$D/secret/date"
on 'mkdir -p /skew && cp /s/date /skew/date && chmod 755 /skew/date'
before=$(docker logs "$FRONT" 2>/dev/null | grep -c '/api/install/lake-replica/')
cp "$D/secret/r1.sh" "$D/secret/skew.sh"
refused "a clock an hour off" skew.sh "off the installation's" -e PATH=/skew:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
expect "and no request with the token reached the installation" "$before" "$(docker logs "$FRONT" 2>/dev/null | grep -c '/api/install/lake-replica/')"

echo "== Started: the agent joins, RustFS with TLS and its keys, the gate"
on "echo QD_NODE_IMAGE=$AGENT >>$R/.env"
run run3 r1.sh
expect "it starts" "0 1" "$? $(said run3 'started: replica-1 joins')"
for _ in $(seq 90); do
  [ "$(on 'docker inspect -f "{{.State.Health.Status}}" qd-lake-replica-rustfs-1' 2>/dev/null)" = healthy ] && break
  sleep 2
done
expect "the machine joined with the token the script left: pending" "replica-1|lake-replica|pending" \
  "$(sql "select name || '|' || role || '|' || state from machines")"
expect "the token file deleted by the agent; its files" "no token|600 10001 ca.crt machine.key machine.pem" \
  "$(on "[ -e $R/join/token ] && echo token || echo no token")|$(on "cd $R/machine && stat -c '%a %u' machine.pem && ls" | paste -sd' ')"
expect "RustFS healthy, the gate running" "healthy running" \
  "$(on 'docker inspect -f "{{.State.Health.Status}}" qd-lake-replica-rustfs-1') $(on 'docker inspect -f "{{.State.Status}}" qd-lake-replica-gate-1')"
expect "RustFS's TLS directory: the CA trusted too" "ca.crt client_ca.crt rustfs_cert.pem rustfs_key.pem" \
  "$(on 'docker exec qd-lake-replica-rustfs-1 ls /run/rustfs-tls' | paste -sd' ')"
expect "the root keys in RustFS's environment alone" "0 0 1" \
  "$(for c in node gate rustfs; do on "docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' qd-lake-replica-$c-1" | grep -c '^RUSTFS_SECRET_KEY='; done | paste -sd' ')"
# s3 ARGS: from a container on RustFS's own network, as replica-1, the CA
# verified, signed with the keys of root-keys.env (in a 0600 curl config).
on "cd $R && . ./root-keys.env && umask 077 && printf 'user = \"%s:%s\"\naws-sigv4 = \"aws:amz:us-east-1:s3\"\n' \"\$RUSTFS_ACCESS_KEY\" \"\$RUSTFS_SECRET_KEY\" >/root/s3.cfg"
RUSTFS_IP=$(on 'docker inspect -f "{{(index .NetworkSettings.Networks \"qd-lake-replica_lake\").IPAddress}}" qd-lake-replica-rustfs-1')
s3() { on "docker run --rm --user 0 --network qd-lake-replica_lake -v /root/s3.cfg:/s3.cfg:ro -v $R/ca.crt:/ca.crt:ro curlimages/curl:8.11.1 -s -o /dev/null -w '%{http_code}' --max-time 10 --resolve replica-1:9000:$RUSTFS_IP -K /s3.cfg $*"; }
expect "over TLS with the machine's certificate, the CA's, by its name: the root keys list the buckets" 200 \
  "$(s3 --cacert /ca.crt https://replica-1:9000/)"
expect "and they make a bucket" 200 "$(s3 --cacert /ca.crt -X PUT https://replica-1:9000/qd-verify-replica)"
expect "without TLS: no answer" 000 "$(s3 http://replica-1:9000/)"
expect "the data is on the drives, RustFS's" "10001 10001" "$(on 'stat -c %u /srv/d1/.rustfs.sys /srv/d2/.rustfs.sys' | paste -sd' ')"
on "curl -s -o /dev/null --max-time 5 --cacert $R/ca.crt --resolve replica-1:9000:127.0.0.1 https://replica-1:9000/"
gate=$?
expect "the gate, on the machine's port, refuses a client without a certificate" "refused" "$([ $gate != 0 ] && echo refused || echo "answered")"

echo "== The platform's admin user, made by the script and shown once"
expect "shown once, on the terminal, to be pasted in Settings → Lake" "1 1 1" \
  "$(said run3 'SHOWN ONLY THIS ONCE') $(said run3 '^  access key: qd-platform-admin$') $(said run3 'Settings → Lake')"
umask 077
sed -n 's/^  secret key: //p' "$D/m-run3.out" >"$D/secret/admin.secret"
sed -i 's/^  secret key: .*/  secret key: (taken out)/' "$D/m-run3.out"
umask 022
expect "its secret: 64 hex digits, one" 1 "$(grep -cE '^[0-9a-f]{64}$' "$D/secret/admin.secret")"
expect "the script then gave it its policy" 1 "$(said run3 'the platform.s admin user has its policy')"
# admin_s3 METHOD PATH [BODY] [SECRET_FILE]: as qd-platform-admin, with its
# keys alone (no root key), by a curl in RustFS's container, its config on
# stdin; the HTTP status.
admin_s3() {
  { printf 'user = "qd-platform-admin:%s"\naws-sigv4 = "aws:amz:us-east-1:s3"\n' "$(cat "${4:-$D/secret/admin.secret}")"
    printf 'resolve = "replica-1:9000:127.0.0.1"\ncacert = "/run/rustfs-tls/ca.crt"\nsilent\nmax-time = 10\n'
    printf 'output = "/dev/null"\nwrite-out = "%%{http_code}"\nrequest = "%s"\nurl = "https://replica-1:9000%s"\n' "$1" "$2"
    [ -z "${3:-}" ] || printf 'data-binary = "%s"\n' "$(printf '%s' "$3" | sed 's/"/\\"/g')"
  } | docker exec -i "$M" docker exec -i qd-lake-replica-rustfs-1 curl -K -
}
expect "with its own keys, no root key: the admin API's info" 200 "$(admin_s3 GET /rustfs/admin/v3/info)"
expect "a tenant's warehouse and its table bucket" "200 200" \
  "$(admin_s3 PUT /qd-tenant-verify) $(admin_s3 PUT /iceberg/v1/buckets/qd-tenant-verify)"
expect "versioning on it" 200 "$(admin_s3 PUT '/qd-tenant-verify?versioning' '<VersioningConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Status>Enabled</Status></VersioningConfiguration>')"
expect "a pool's policy and user" "200 200" \
  "$(admin_s3 PUT /rustfs/admin/v3/add-canned-policy?name=qd-pool-verify '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject"],"Resource":["arn:aws:s3:::qd-tenant-verify/*"]}]}') $(admin_s3 PUT /rustfs/admin/v3/add-user?accessKey=qd-pool-verify "{\"secretKey\":\"$(openssl rand -hex 32)\",\"status\":\"enabled\"}")"
expect "a bucket outside qd-tenant-* denied" 403 "$(admin_s3 PUT /other-verify)"
# grep_secret WHERE: how many files under WHERE, in the machine, hold the secret.
grep_secret() { docker exec -i "$M" sh -c "grep -rlF -f - $1 2>/dev/null | wc -l" <"$D/secret/admin.secret"; }
expect "the secret in no file of the installation's directory" 0 "$(grep_secret $R)"
expect "nor in any container's log or settings there (/var/lib/docker/containers)" 0 "$(grep_secret /var/lib/docker/containers)"
logged=0
for c in node rustfs gate; do
  docker exec "$M" docker logs "qd-lake-replica-$c-1" 2>&1 | grep -qFf "$D/secret/admin.secret" && logged=$((logged + 1))
done
docker logs "$M" 2>&1 | grep -qFf "$D/secret/admin.secret" && logged=$((logged + 1))
expect "nor in docker logs of node, rustfs, gate, or the machine's own Docker" 0 "$logged"
echo "   (RustFS's own store on the drives: $(grep_secret '/srv/d1 /srv/d2') files hold it, as RustFS keeps its users)"

echo "== Its Alloy pushes to Mimir (0071 step 4)"
expect "metrics.yml: qd-app at the address the machine's DNS gives; each drive to Alloy, read-only" \
  "- \"qd-app:$MIMIR_IP\"|- \"/srv/d1:/host/watch/drive1:ro\"|- \"/srv/d2:/host/watch/drive2:ro\"" \
  "$(on "grep -- '- ' $R/metrics.yml" | sed 's/^ *//' | paste -sd'|')"
expect "Alloy runs as the agent's user (RustFS's), read-only" "10001:10001 true running" \
  "$(on "docker inspect -f '{{.Config.User}} {{.HostConfig.ReadonlyRootfs}} {{.State.Status}}' qd-lake-replica-alloy-1")"
mq() { docker run --rm --network "$NET" -v "$D/mimir:/c:ro" curlimages/curl:8.11.1 -sS -G --max-time 15 --cacert /c/ca.crt \
  --cert /c/app.crt --key /c/app.key "https://qd-app:9009/prometheus/api/v1/$1" --data-urlencode "$2" 2>/dev/null; }
count() { mq query "query=count($1)" | python3 -c 'import json,sys;r=json.load(sys.stdin)["data"]["result"];print(r[0]["value"][1] if r else 0)' 2>/dev/null; }
for _ in $(seq 60); do
  [ "$(count 'up{vm="replica-1"}')" = 4 ] && [ "$(count '{job="rustfs",vm="replica-1"}')" != 0 ] && break
  sleep 5
done
# RustFS's names: the keep list of lake-replica.alloy.
RUSTFS_KEEP=$(sed -n '/^prometheus.relabel "rustfs"/,/^}/ s/^ *regex *= "\(rustfs_.*\)"$/\1/p' "$ROOT/infra/observability/alloy/lake-replica.alloy" | head -1)
# The host's names, decision 0072 point 1's table, written here, not read from the file.
HOST_KEEP='node_cpu_seconds_total|node_load1|node_load5|node_memory_(MemTotal|MemAvailable|SwapTotal|SwapFree)_bytes|node_filesystem_(size_bytes|avail_bytes|files|files_free|readonly)|node_disk_(read_bytes_total|written_bytes_total|io_time_seconds_total)|node_network_(receive|transmit)_(bytes|errs|drop)_total|node_timex_offset_seconds|node_timex_sync_status|node_boot_time_seconds'
mq series 'match[]={vm="replica-1"}' | python3 -c '
import json, re, sys
s = json.load(sys.stdin)["data"]
ok_names = re.compile(r"up|node_textfile_mtime_seconds|node_textfile_scrape_error|process_cpu_seconds_total|process_resident_memory_bytes|qd_node_renewals_total|qd_node_certificate_expiry_timestamp_seconds|prometheus_remote_storage_samples_failed_total|" + sys.argv[1] + "|" + sys.argv[2])
ok_labels = {"__name__", "job", "instance", "file", "service", "result", "vm", "role", "component_id", "host", "reason", "remote_name", "url", "api", "method", "status_class", "pool", "drive", "le", "cpu", "mode", "mountpoint", "device"}
print(" ".join(sorted({x["job"] for x in s})), " ".join(sorted({x["role"] for x in s})),
      len([x for x in s if not ok_names.fullmatch(x["__name__"])]), len({k for x in s for k in x} - ok_labels))' "$RUSTFS_KEEP" "$HOST_KEEP" >"$D/series.txt"
expect "series of vm replica-1: jobs, role, names and labels outside the lists" "alloy files host rustfs lake-replica 0 0" "$(cat "$D/series.txt")"
echo "   (RustFS's series after the keep list: $(count '{job="rustfs",vm="replica-1"}'))"
expect "up is 1 for all four (files, alloy, host, network)" "1 1 1 1" "$(mq query 'query=up{vm="replica-1"}' | python3 -c 'import json,sys;print(" ".join(x["value"][1] for x in json.load(sys.stdin)["data"]["result"]))')"
# vals PROMQL LABEL: each series' LABEL=value, sorted, space-separated.
vals() { mq query "query=$1" | python3 -c 'import json,sys;print(" ".join(sorted(x["metric"].get(sys.argv[1],"")+"="+x["value"][1] for x in json.load(sys.stdin)["data"]["result"])))' "$2" 2>/dev/null; }
# ifaces CONTAINER|-: the interfaces in /proc/net/dev of the machine (-) or
# of CONTAINER on it, without point 1's device_exclude.
ifaces() {
  local where="cat /proc/net/dev"
  [ "$1" = - ] || where="docker exec $1 cat /proc/net/dev"
  on "$where" | awk -F: 'NR>2{gsub(/ /,"",$1);print $1}' | grep -Ev '^(loop.*|ram.*|veth.*|docker.*|br-.*|lo)$' | sort | paste -sd' '
}
expect "the host scrapes up (host, network)" "host=1 network=1" "$(vals 'up{job="host",vm="replica-1"}' instance)"
expect "each drive's filesystem by its name, the tmpfs's size (statfs)" "drive1=268435456 drive2=268435456" \
  "$(vals 'node_filesystem_size_bytes{vm="replica-1"}' mountpoint)"
expect "… its free space, for each" "drive1 drive2" \
  "$(mq query 'query=node_filesystem_avail_bytes{vm="replica-1"} > 0' | python3 -c 'import json,sys;print(" ".join(sorted(x["metric"]["mountpoint"] for x in json.load(sys.stdin)["data"]["result"])))')"
HOSTIF=$(ifaces -) OWNIF=$(ifaces qd-lake-replica-alloy-1)
expect "the host's interfaces ($HOSTIF), not Alloy's container's ($OWNIF)" "$HOSTIF|differ" \
  "$(mq query 'query=node_network_receive_bytes_total{vm="replica-1"}' | python3 -c 'import json,sys;print(" ".join(sorted(x["metric"]["device"] for x in json.load(sys.stdin)["data"]["result"])))')|$([ "$HOSTIF" != "$OWNIF" ] && echo differ || echo same)"
expect "… from the network exporter alone" "network" "$(vals 'count by (instance) (node_network_receive_bytes_total{vm="replica-1"})' instance | sed 's/=[0-9]*//g')"
expect "CPU modes kept" "idle iowait system user" "$(mq query 'query=count by (mode) (node_cpu_seconds_total{vm="replica-1"})' | python3 -c 'import json,sys;print(" ".join(sorted(x["metric"]["mode"] for x in json.load(sys.stdin)["data"]["result"])))')"
expect "Alloy: no capability, no new privileges, no /host/proc, /host/proc1 holds net alone" "[ALL] [no-new-privileges:true]|absent|net" \
  "$(on "docker inspect -f '{{.HostConfig.CapDrop}} {{.HostConfig.SecurityOpt}}' qd-lake-replica-alloy-1; docker exec qd-lake-replica-alloy-1 sh -c '[ -e /host/proc ] && echo present || echo absent; ls /host/proc1'" | paste -sd'|')"
echo "   (the host series: $(count '{job="host",vm="replica-1"}'); the drives' free bytes: $(vals 'node_filesystem_avail_bytes{vm="replica-1"}' mountpoint))"
expect "no port of Alloy published" "" "$(on "docker inspect -f '{{range \$p, \$b := .NetworkSettings.Ports}}{{\$p}}{{end}}' qd-lake-replica-alloy-1")"
# otlp NETWORK URL [CURL OPTION...]: an empty OTLP push from a container on
# NETWORK of the replica; the status. The token's header in a 0600 file.
on "umask 077 && printf 'authorization: Bearer %s\n' \"\$(cat $R/otlp-token)\" >/root/otlp-auth"
otlp() {
  local net=$1 url=$2
  shift 2
  on "docker run --rm --user 0 --network qd-lake-replica_$net -v /root/otlp-auth:/auth:ro -v $R/ca.crt:/ca.crt:ro curlimages/curl:8.11.1 -s -o /dev/null -w '%{http_code}' --max-time 10 --cacert /ca.crt -H 'content-type: application/json' -d '{}' $* $url"
}
expect "RustFS's receiver, from its network: with the token, without, with another" "200 401 401" \
  "$(otlp otlp https://replica-1:4318/v1/metrics -H @/auth) $(otlp otlp https://replica-1:4318/v1/metrics) $(otlp otlp https://replica-1:4318/v1/metrics -H "'authorization: Bearer $(openssl rand -hex 32)'")"
ALLOY_IP=$(on "docker inspect -f '{{(index .NetworkSettings.Networks \"qd-lake-replica_default\").IPAddress}}' qd-lake-replica-alloy-1")
expect "… from the replica's other network, at Alloy's address there: no answer" 000 "$(otlp default "https://$ALLOY_IP:4318/v1/metrics" -k -H @/auth)"
expect "the token in no log or docker inspect of the replica's containers; in no output of the script" "0 0" \
  "$(on "for c in node rustfs gate alloy; do docker logs qd-lake-replica-\$c-1 2>&1; docker inspect qd-lake-replica-\$c-1; done | grep -cF -f $R/otlp-token") $(cat "$D"/m-*.out | grep -cFf <(on "cat $R/otlp-token"))"

echo "== The command again: its token used"
stamp=$(state)
run run4 r1.sh
expect "401, the files unchanged" "1 1 $stamp" "$? $(said run4 'unknown, used or expired') $(state)"

echo "== A new command for the replica, run again: no user made, no secret shown"
issue lake-replica r2
field "$D/secret/r2.json" install command | sed '$ s|$| --drive /srv/d1 --drive /srv/d2|' >"$D/secret/r2.sh"
chmod 600 "$D/secret/r2.sh"
run run5 r2.sh
expect "it runs to the end, the user there already" "0 1 1" \
  "$? $(said run5 'is there already') $(said run5 'the platform.s admin user has its policy')"
expect "no secret shown" "0 0" "$(said run5 'SHOWN ONLY') $(grep -cE '[0-9a-f]{64}' "$D/m-run5.out")"
expect "the first secret still the user's" 200 "$(admin_s3 GET /rustfs/admin/v3/info)"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
