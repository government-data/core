#!/bin/bash
# What a lake pool's installation would build on (decision 0029, "Lake
# nodes", point 5; decision 0066 step 11, the lake roles' rulings 1, 2 and 5
# of 2026-11-13), past what rustfs_lake_nodes.sh found. Each node stands
# for a server of its own: RustFS on a port of its own (9010) for the
# pool, every node's name resolved by the others to that node's address
# and by itself as the rulings would have it, client certificates of the
# CA required between nodes, each node with its own certificate as the
# installation's CA makes them (O=qd, OU=lake, CN and the one DNS name the
# machine's name), the same root keys on every node.
#
#   (a) how RUSTFS_VOLUMES may name the nodes: the ellipsis over numbered
#       names, and a plain list of four unrelated names;
#   (b) a node whose own name resolves to an address that is not its own;
#   (c) the nodes started one at a time, as the one command is run on one
#       server after another: what the first does alone for a minute, three
#       for up to five minutes, and when the fourth comes; and a node one of
#       whose peers' names does not resolve at all;
#   (d) a gate in front of each node (the repository's
#       infra/rustfs/gate/entrypoint.sh, in the node's network namespace,
#       the API's check stood in by a server that lets every machine pass),
#       with RustFS requiring client certificates: as it was before it
#       presented one, and as it is, presenting the node's certificate to
#       RustFS;
#   (e) the platform's admin user made once, on one node, and found by the
#       others; and what RustFS offers to make something only once: a
#       bucket made twice, an object put with If-None-Match: * twice;
#   (f) a node stopped and started again, and every node restarted at once;
#   (g) a certificate renewal: every node's certificate made to expire, the
#       server certificate renewed on every node and the client one on three
#       of them, then one node restarted so that its peers shake hands anew.
#
# Throwaway containers only (qd-verify-pool-…, on a network of their own,
# no port published), a throwaway CA, random root keys that are never
# printed. Run from the repository root:
#
#   bash tools/verify/rustfs_pool_nodes.sh
#
# Needs docker, curl 7.75 or later (--aws-sigv4), openssl 3.4 or later
# (-not_after) and jq.
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
TRAEFIK=${TRAEFIK_IMAGE:-traefik:v3.6}
WHOAMI=${WHOAMI_IMAGE:-traefik/whoami:v1.11}
P=qd-verify-pool
NET=${QD_VERIFY_SUBNET:-10.231.77}
WAIT=${QD_VERIFY_WAIT:-60}
ROOT=$(pwd)
D=$(mktemp -d)
chmod 700 "$D"
cleanup() {
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f >/dev/null 2>&1
  docker network rm "$P" >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() {
  printf '[FAIL] %s\n' "$*"
  FAILED=1
}
note() { printf '[NOTE] %s\n' "$*"; }
expect() { # NAME EXPECTED ACTUAL
  if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi
}

# --- Certificates and keys -----------------------------------------------------
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/CN=qd verify pool" -addext "basicConstraints=critical,CA:TRUE" \
  -keyout "$D/ca.key" -out "$D/ca.crt" 2>/dev/null
# cert NAME OU [SECONDS]: a machine's certificate as the CA makes them,
# client and server use in one, the name its CN and only DNS name; for
# SECONDS from now when given, a day otherwise. A new key each time.
cert() {
  local until=()
  [ -z "${3:-}" ] || until=(-not_after "$(date -u -d "+$3 seconds" +%Y%m%d%H%M%SZ)")
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/O=qd/OU=$2/CN=$1" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:%s\n' "$1" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/ca.crt" -CAkey "$D/ca.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 "${until[@]}" -out "$D/$1.crt" 2>/dev/null
}
serial() { openssl x509 -noout -serial -in "$1" | cut -d= -f2; }
printf 'RUSTFS_ACCESS_KEY=qd%s\nRUSTFS_SECRET_KEY=%s\n' "$(openssl rand -hex 8)" "$(openssl rand -hex 20)" >"$D/root.env"
chmod 600 "$D/root.env"
# tlsdir NAME [server|client|both]: what NAME's RustFS reads in its TLS
# directory: its certificate as server and as client, the CA as the
# clients' CA and as the one it trusts calling out.
tlsdir() {
  local t=$D/tls-$1 w=${2:-both}
  mkdir -p "$t"
  if [ "$w" != client ]; then
    cp "$D/$1.crt" "$t/.c" && mv -f "$t/.c" "$t/rustfs_cert.pem"
    cp "$D/$1.key" "$t/.k" && mv -f "$t/.k" "$t/rustfs_key.pem"
  fi
  if [ "$w" != server ]; then
    cp "$D/$1.crt" "$t/.c" && mv -f "$t/.c" "$t/client_cert.pem"
    cp "$D/$1.key" "$t/.k" && mv -f "$t/.k" "$t/client_key.pem"
  fi
  cp "$D/ca.crt" "$t/client_ca.crt"
  cp "$D/ca.crt" "$t/ca.crt"
  chmod 755 "$t"
  chmod 644 "$t"/*
}
# machinedir NAME: what the node agent leaves, as the gate reads it.
machinedir() {
  mkdir -p "$D/m-$1"
  cp "$D/ca.crt" "$D/m-$1/ca.crt"
  cat "$D/$1.crt" "$D/$1.key" >"$D/m-$1/machine.pem"
  chmod 755 "$D/m-$1"
  chmod 644 "$D/m-$1"/*
}
docker network create --subnet "$NET.0/24" "$P" >/dev/null || exit 1
cert app-1 app
echo synthetic >"$D/obj"

# Every node of a case: NAME=ADDRESS, the address its peers reach it at.
declare -A ADDR=([none]=-)
NEXT=10
# node CASE NAME VOLUMES [SELF]: NAME's RustFS at ADDR[NAME], every other
# name of the case resolved to its address and its own to SELF (by default
# 127.0.0.1); a name whose address is `-` is left unresolvable.
node() {
  local c=$1 n=$2 vols=$3 self=${4:-127.0.0.1} hosts=() m
  for m in "${!ADDR[@]}"; do
    case $m in "$c"-*) ;; *) continue ;; esac
    if [ "$m" = "$n" ]; then
      hosts+=(--add-host "$m:$self")
    elif [ "${ADDR[$m]}" != - ]; then hosts+=(--add-host "$m:${ADDR[$m]}"); fi
  done
  # Restarted as the compose files' `restart: unless-stopped` would.
  docker run -d --name "$P-$n" --hostname "$P-$n" --restart unless-stopped --network "$P" --ip "${ADDR[$n]}" "${hosts[@]}" \
    --env-file "$D/root.env" -e RUSTFS_OBS_LOGGER_LEVEL=info -e "RUSTFS_VOLUMES=$vols" \
    -e RUSTFS_TLS_PATH=/tls -e RUSTFS_SERVER_MTLS_ENABLE=1 \
    -e RUSTFS_TLS_RELOAD_ENABLE=1 -e RUSTFS_TLS_RELOAD_INTERVAL=5 \
    -v "$D/tls-$n:/tls:ro" "$IMAGE" server --address :9010 >/dev/null
}
# nodes CASE N...: the case's nodes CASE-N, each with its certificate.
nodes() {
  local c=$1 i
  shift
  for i in "$@"; do
    ADDR[$c-$i]=$NET.$NEXT
    NEXT=$((NEXT + 1))
  done
}
# s3 NAME ARGS...: a request to NAME's RustFS, signed with the root keys,
# presenting app-1's certificate, verified against the CA.
s3() {
  local n=$1
  shift
  (
    # shellcheck source=/dev/null
    . "$D/root.env"
    printf 'user = "%s:%s"\naws-sigv4 = "aws:amz:us-east-1:s3"\ncert = "%s"\nkey = "%s"\n' \
      "$RUSTFS_ACCESS_KEY" "$RUSTFS_SECRET_KEY" "$D/app-1.crt" "$D/app-1.key"
  ) | curl -s --cacert "$D/ca.crt" --max-time 15 --resolve "$n:9010:${ADDR[$n]}" -K - "$@" 2>/dev/null
}
status() { s3 "$1" -o /dev/null -w '%{http_code}' "${@:2}"; }
# formed A B: within WAIT seconds, an object written on A and read on B.
formed() {
  local a=$1 b=$2 key=check-$RANDOM.txt
  for _ in $(seq "$WAIT"); do
    status "$a" -X PUT "https://$a:9010/qd-verify" >/dev/null
    if [ "$(status "$a" -T "$D/obj" "https://$a:9010/qd-verify/$key")" = 200 ] &&
      [ "$(s3 "$b" "https://$b:9010/qd-verify/$key")" = synthetic ]; then
      echo formed
      return
    fi
    sleep 1
  done
  echo "not formed ($(status "$a" -T "$D/obj" "https://$a:9010/qd-verify/$key"))"
}
running() { docker inspect -f '{{.State.Running}}' "$P-$1" 2>/dev/null; }
lines() { docker exec "$P-$1" grep -i -E -- "$2" /logs/rustfs.log 2>/dev/null | tail -n "${3:-2}" | jq -r '.message' 2>/dev/null | cut -c1-200; }
restarts() { docker inspect -f '{{.RestartCount}}' "$P-$1"; }
logged() { docker exec "$P-$1" grep -c -i -- "$2" /logs/rustfs.log 2>/dev/null | head -1; }
# disks NAME: what NAME's admin info says of the pool's drives, online/offline.
disks() {
  s3 "$1" "https://$1:9010/rustfs/admin/v3/info" |
    jq -r '"\(.backend.onlineDisks // .info.backend.onlineDisks)/\(.backend.offlineDisks // .info.backend.offlineDisks)"' 2>/dev/null
}
seen() { # NAME EXPECTED: what NAME sees of the drives, within WAIT seconds
  local r=''
  for _ in $(seq "$WAIT"); do
    r=$(disks "$1")
    [ "$r" = "$2" ] && break
    sleep 1
  done
  echo "$r"
}
# reads NAME KEY: the object KEY read on NAME, within WAIT seconds (the
# first requests after a peer goes away wait for it).
reads() {
  local r=''
  for _ in $(seq "$WAIT"); do
    r=$(s3 "$1" "https://$1:9010/qd-verify/$2")
    [ "$r" = synthetic ] && break
    sleep 1
  done
  echo "$r"
}
down() { docker ps -aq --filter "name=^$P-$1-" | xargs -r docker rm -f >/dev/null; }
setup() { # CASE N...: certificates and TLS directories of the case's nodes
  local c=$1 i
  shift
  nodes "$c" "$@"
  for i in "$@"; do
    cert "$c-$i" lake
    tlsdir "$c-$i"
  done
}

# --- (a) How the volumes name the nodes -------------------------------------------
echo "== (a) RUSTFS_VOLUMES"
setup ell 1 2 3 4
for i in 1 2 3 4; do node ell "ell-$i" "https://ell-{1...4}:9010/data"; done
expect "the ellipsis over numbered names (https://ell-{1...4}:9010/data): written on node 1, read on node 3" \
  formed "$(formed ell-1 ell-3)"
down ell
for n in alpha bravo charlie delta; do
  nodes list "$n"
  cert "list-$n" lake
  tlsdir "list-$n"
done
LIST=""
for n in alpha bravo charlie delta; do LIST="$LIST https://list-$n:9010/data"; done
LIST=${LIST# }
for n in alpha bravo charlie delta; do node list "list-$n" "$LIST"; done
r=$(formed list-alpha list-charlie)
if [ "$r" = formed ]; then ok "a plain list of four unrelated names: written on one, read on another"; else
  note "a plain list of four unrelated names: $r"
  docker logs "$P-list-alpha" 2>&1 | grep -i -m 3 -E 'error|invalid|ellipses' | cut -c1-200
fi
down list

# --- (b) Its own name elsewhere ---------------------------------------------------
echo "== (b) a node whose own name resolves to an address that is not its own"
setup self 1 2 3 4
node self self-1 "https://self-{1...4}:9010/data" 192.0.2.10
sleep 15
r="running=$(running self-1)"
note "own name at 192.0.2.10, alone: $r; its last lines:"
lines self-1 'local|endpoint|error' 3
for i in 2 3 4; do node self "self-$i" "https://self-{1...4}:9010/data"; done
r=$(formed self-2 self-3)
note "with the other three each resolving itself to its loopback: $r; node 1 running=$(running self-1), restarted $(restarts self-1) times"
lines self-1 'local|endpoint|error|fatal' 3
down self

# --- (c) One server at a time -----------------------------------------------------
echo "== (c) the nodes started one at a time"
setup pool 1 2 3 4
VOLS="https://pool-{1...4}:9010/data"
node pool pool-1 "$VOLS"
sleep 20
expect "node 1 alone, its peers' names resolving to servers not up yet: still running" true "$(running pool-1)"
note "… a bucket asked of it answers $(status pool-1 -X PUT "https://pool-1:9010/qd-verify"); waiting lines in its log: $(logged pool-1 wait)"
lines pool-1 'wait|quorum|offline'
sleep 40
note "… after a minute alone: running=$(running pool-1), restarted $(restarts pool-1) times"
node pool pool-2 "$VOLS"
node pool pool-3 "$VOLS"
# Three of four, for as long as the next server may take to be installed.
for _ in $(seq "${QD_VERIFY_PARTIAL:-300}"); do
  [ "$(($(restarts pool-1) + $(restarts pool-2) + $(restarts pool-3)))" -ge 2 ] && break
  sleep 1
done
note "three of four up for a while: a bucket asked of node 1 answers $(status pool-1 -X PUT "https://pool-1:9010/qd-verify"); restarts of nodes 1 to 3: $(restarts pool-1) $(restarts pool-2) $(restarts pool-3)"
for i in 1 2 3; do
  f=$(docker logs "$P-pool-$i" 2>&1 | grep -m 1 FATAL | cut -c1-140)
  [ -z "$f" ] || note "… node $i: $f"
done
node pool pool-4 "$VOLS"
expect "the fourth up: the pool forms, written on node 1, read on node 4" formed "$(formed pool-1 pool-4)"
expect "… every drive online, as node 2 sees them" 4/0 "$(seen pool-2 4/0)"
note "… restarts of nodes 1 to 4 by then: $(restarts pool-1) $(restarts pool-2) $(restarts pool-3) $(restarts pool-4)"
setup unres 1 2 3 4
ADDR[unres-4]=-
node unres unres-1 "https://unres-{1...4}:9010/data"
sleep 20
note "a node one of whose peers' names does not resolve: running=$(running unres-1); its lines:"
lines unres-1 'resolve|lookup|dns|error'
docker logs "$P-unres-1" 2>&1 | grep -m 1 FATAL | cut -c1-200
down unres

# --- (d) A gate per node ----------------------------------------------------------
echo "== (d) a gate in front of each node, RustFS requiring client certificates"
docker run -d --name "$P-check" --network "$P" "$WHOAMI" >/dev/null
cert worker-1 worker
for i in 1 2 3 4; do machinedir "pool-$i"; done
# The entrypoint as join.sh writes it (0755), whose transport to RustFS
# presents the machine's certificate, and as it was before it did: the same
# file without those three lines of its serversTransports.
cp "$ROOT/infra/rustfs/gate/entrypoint.sh" "$D/gate-cert.sh"
sed '/^      certificates:$/{N;N;/certFile: \$SRC\/machine.pem.*keyFile: \$SRC\/machine.pem/d}' \
  "$ROOT/infra/rustfs/gate/entrypoint.sh" >"$D/gate.sh"
# transport FILE: whether FILE's transport to RustFS names a certificate.
transport() { grep -A1 'rootCAs: \[\$SRC/ca.crt\]' "$1" | grep -c '^      certificates:$'; }
expect "the repository's gate presents the machine's certificate to RustFS" 1 "$(transport "$D/gate-cert.sh")"
expect "… the gate as it was, without it, three lines fewer" "0 3" \
  "$(transport "$D/gate.sh") $(($(wc -l <"$D/gate-cert.sh") - $(wc -l <"$D/gate.sh")))"
chmod 755 "$D/gate-cert.sh" "$D/gate.sh"
# gate NAME [cert]: the repository's gate in NAME's network namespace, on
# 9000, in front of RustFS on 9010; with `cert`, its dynamic file made to
# present the node's certificate to RustFS too.
gate() {
  local n=$1 g=$P-gate-$1
  docker rm -f "$g" >/dev/null 2>&1
  local e=$D/gate.sh
  [ "${2:-}" != cert ] || e=$D/gate-cert.sh
  docker run -d --name "$g" --network "container:$P-$n" --user 10001:10001 \
    --tmpfs /run/qd-gate:uid=10001,gid=10001,mode=0700 \
    -e "QD_MACHINE_NAME=$n" -e "QD_LAKE_CHECK_URL=http://$P-check/" \
    -e QD_GATE_UPSTREAM=https://127.0.0.1:9010 -e QD_GATE_CHECK_EVERY=2 \
    -v "$e:/etc/qd/gate/entrypoint.sh:ro" -v "$D/m-$n:/run/qd-node:ro" \
    --entrypoint /etc/qd/gate/entrypoint.sh "$TRAEFIK" >/dev/null
}
# viagate NAME ARGS...: a request through NAME's gate, signed with the root
# keys, presenting worker-1's certificate.
viagate() {
  local n=$1
  shift
  (
    # shellcheck source=/dev/null
    . "$D/root.env"
    printf 'user = "%s:%s"\naws-sigv4 = "aws:amz:us-east-1:s3"\ncert = "%s"\nkey = "%s"\n' \
      "$RUSTFS_ACCESS_KEY" "$RUSTFS_SECRET_KEY" "$D/worker-1.crt" "$D/worker-1.key"
  ) | curl -s --cacert "$D/ca.crt" --max-time 10 --resolve "$n:9000:${ADDR[$n]}" -K - "$@" 2>/dev/null
}
gate pool-2
sleep 6
r=$(viagate pool-2 -o /dev/null -w '%{http_code}' "https://pool-2:9000/qd-verify/")
case $r in 2??) ko "the gate as it was, RustFS requiring a client certificate of it: $r" ;;
*) ok "the gate as it was, RustFS requiring a client certificate of it: refused ($r)" ;; esac
for i in 1 2 3 4; do gate "pool-$i" cert; done
sleep 6
expect "the gate presenting the node's certificate: a write through node 2's gate" 200 \
  "$(viagate pool-2 -o /dev/null -w '%{http_code}' -T "$D/obj" "https://pool-2:9000/qd-verify/via-gate.txt")"
expect "… read through node 4's gate" synthetic "$(viagate pool-4 "https://pool-4:9000/qd-verify/via-gate.txt")"
expect "… without a client certificate node 3's gate refuses the handshake" 000 \
  "$(curl -s -o /dev/null -w '%{http_code}' --cacert "$D/ca.crt" --max-time 5 --resolve "pool-3:9000:${ADDR[pool-3]}" https://pool-3:9000/ 2>/dev/null)"

# --- (e) The admin user, made once ------------------------------------------------
echo "== (e) the platform's admin user on a pool"
ADMIN=qd-platform-admin
POLICY='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:*"],"Resource":["arn:aws:s3:::qd-tenant-*","arn:aws:s3:::qd-tenant-*/*"]},{"Effect":"Allow","Action":["admin:*"],"Resource":["arn:aws:s3:::*"]}]}'
SECRET=$(openssl rand -hex 32)
userinfo() { status "$1" "https://$1:9010/rustfs/admin/v3/user-info?accessKey=$ADMIN"; }
expect "before: user-info on node 1" 404 "$(userinfo pool-1)"
expect "the policy set on node 1" 200 "$(status pool-1 -X PUT --data-binary "$POLICY" \
  "https://pool-1:9010/rustfs/admin/v3/add-canned-policy?name=$ADMIN")"
(umask 077 && printf '{"secretKey":"%s","status":"enabled"}' "$SECRET" >"$D/user.json")
expect "the user made on node 1" 200 "$(status pool-1 -X PUT --data-binary "@$D/user.json" \
  "https://pool-1:9010/rustfs/admin/v3/add-user?accessKey=$ADMIN")"
rm -f "$D/user.json"
expect "… the policy attached on node 1" 200 "$(status pool-1 -X PUT \
  "https://pool-1:9010/rustfs/admin/v3/set-user-or-group-policy?policyName=$ADMIN&userOrGroup=$ADMIN&isGroup=false")"
for i in 2 3 4; do expect "user-info on node $i at once" 200 "$(userinfo "pool-$i")"; done
asadmin() { # NAME ARGS...: signed by the admin user, not root
  local n=$1
  shift
  printf 'user = "%s:%s"\naws-sigv4 = "aws:amz:us-east-1:s3"\ncert = "%s"\nkey = "%s"\n' \
    "$ADMIN" "$SECRET" "$D/app-1.crt" "$D/app-1.key" |
    curl -s -o /dev/null -w '%{http_code}' --cacert "$D/ca.crt" --max-time 10 \
      --resolve "$n:9010:${ADDR[$n]}" -K - "$@" 2>/dev/null
}
# What RustFS offers to make something once on a pool: a bucket made twice,
# and a PutObject with If-None-Match: *, each on two nodes.
expect "a bucket made on node 1, then on node 2" "200 200" \
  "$(status pool-1 -X PUT https://pool-1:9010/qd-verify-once) $(status pool-2 -X PUT https://pool-2:9010/qd-verify-once)"
expect "an object put with If-None-Match: * on node 1, then on node 3" "200 412" \
  "$(status pool-1 -H 'If-None-Match: *' -T "$D/obj" https://pool-1:9010/qd-verify-once/o) $(status pool-3 -H 'If-None-Match: *' -T "$D/obj" https://pool-3:9010/qd-verify-once/o)"
expect "with its keys alone, a warehouse made on node 4" 200 "$(asadmin pool-4 -X PUT https://pool-4:9010/qd-tenant-verify)"
expect "… and node 3's admin info" 200 "$(asadmin pool-3 https://pool-3:9010/rustfs/admin/v3/info)"

# --- (f) Restarts -----------------------------------------------------------------
echo "== (f) restarts"
docker stop "$P-pool-2" >/dev/null
r=$(s3 pool-1 --max-time 120 -o /dev/null -w '%{http_code} %{time_total}' -T "$D/obj" https://pool-1:9010/qd-verify/while-down.txt)
case $r in "200 "*) ok "node 2 stopped: written on node 1 (200, in ${r#* }s)" ;; *) ko "node 2 stopped: a write on node 1 answered $r" ;; esac
r=$(s3 pool-1 --max-time 120 -o /dev/null -w '%{http_code} %{time_total}' -T "$D/obj" https://pool-1:9010/qd-verify/while-down-2.txt)
note "… a second write on node 1: $r (code, seconds)"
expect "… read on node 3" synthetic "$(reads pool-3 while-down.txt)"
docker start "$P-pool-2" >/dev/null
expect "node 2 started again: it reads what was written while it was down" synthetic "$(reads pool-2 while-down.txt)"
docker restart "$P-pool-1" "$P-pool-2" "$P-pool-3" "$P-pool-4" >/dev/null
expect "every node restarted at once: the pool forms again" formed "$(formed pool-3 pool-1)"
expect "… the admin user still there" 200 "$(userinfo pool-2)"
for i in 1 2 3 4; do docker rm -f "$P-gate-pool-$i" >/dev/null; done
down pool

# --- (g) A certificate renewal ----------------------------------------------------
echo "== (g) a certificate renewal"
LIFE=${QD_VERIFY_CERT_LIFE:-90}
nodes renew 1 2 3 4
for i in 1 2 3 4; do
  cert "renew-$i" lake "$LIFE"
  tlsdir "renew-$i"
done
born=$(date +%s)
for i in 1 2 3 4; do node renew "renew-$i" "https://renew-{1...4}:9010/data"; done
expect "a pool of certificates that end in ${LIFE}s forms" formed "$(formed renew-1 renew-2)"
# Renewed: the server's certificate on every node; the client's on 1 to 3.
for i in 1 2 3 4; do
  cert "renew-$i" lake
  if [ "$i" = 4 ]; then tlsdir "renew-$i" server; else tlsdir "renew-$i"; fi
done
sleep 12
served() { # NAME: the serial NAME's RustFS serves
  openssl s_client -connect "${ADDR[$1]}:9010" -servername "$1" </dev/null 2>/dev/null |
    openssl x509 -noout -serial 2>/dev/null | cut -d= -f2
}
r=ok
for i in 1 2 3 4; do [ "$(served "renew-$i")" = "$(serial "$D/renew-$i.crt")" ] || r="node $i serves $(served "renew-$i")"; done
expect "every node serves its renewed certificate, nothing restarted" ok "$r"
left=$((born + LIFE + 5 - $(date +%s)))
[ "$left" -le 0 ] || sleep "$left"
expect "the first certificates expired: the pool still serves (connections already open)" formed "$(formed renew-1 renew-2)"
docker restart "$P-renew-3" >/dev/null
r=$(seen renew-1 4/0)
if [ "$r" = 4/0 ]; then ok "node 3 restarted: node 1 (client certificate renewed in its files, not restarted) sees every drive: 4/0"; else
  note "node 3 restarted: node 1 (client certificate renewed in its files, not restarted) sees $r"
fi
r=$(seen renew-4 4/0)
note "… node 4 (only its server certificate renewed) sees $r"
note "… lines saying expired: node 3's log $(logged renew-3 expired), node 4's $(logged renew-4 expired)"
docker restart "$P-renew-1" "$P-renew-2" >/dev/null
r=$(seen renew-1 4/0)
note "nodes 1 and 2 restarted: node 1 sees $r; node 4 sees $(disks renew-4)"
down renew

echo
if [ "$FAILED" = 0 ]; then echo "ALL OK"; else
  echo "SOME CHECKS FAILED"
  exit 1
fi
