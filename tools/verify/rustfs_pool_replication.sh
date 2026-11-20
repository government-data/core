#!/bin/bash
# How a lake pool replicates into the replica (decision 0066 step 11, the
# user's ruling 5 of 2026-11-14), past what rustfs_pool_nodes.sh and
# rustfs_source_proxy.sh found. A bucket's remote target is set once, for
# the whole deployment, and every node's RustFS replicates what it writes,
# so the target's endpoint must reach a proxy from every node. Two ways out
# are tried on one pool of four nodes, each with a proxy of its own in its
# network namespace (the repository's infra/worker/lake/entrypoint.sh,
# presenting the node's machine certificate to the replica's gate):
#
#   (a) a name every node resolves to its own loopback (ALIAS, by
#       --add-host as extra_hosts does), the proxy serving TLS there with a
#       node certificate that carries that name too (the CA signing an
#       extra name), the target secure; and, as a control, the same name
#       served with a certificate that does not carry it;
#   (b) plain HTTP to the proxy on the node's own loopback, the target not
#       secure: the entrypoint with its listener plain
#       (QD_LAKE_LISTEN_TLS=0), the hop to the gate still TLS with the
#       certificate; the
#       target named 127.0.0.1, then localhost, then ALIAS.
#
# For each, with each lake's platform admin keys alone (never root): the
# remote target, the rule, an object written through every node reaching
# the replica locked; one node's proxies stopped, what another node writes
# still arrives, and what that node wrote meanwhile arrives once they are
# back. And the plain listener answers on the node's loopback but not at
# its address. Then node 1's RustFS restarted: its proxies left in the old
# network namespace as they were, and under the pool compose file's loop
# back by themselves, an object written through node 1 arriving after.
# Last, every node's proxies under the loop and the four RustFS restarted
# at once: every proxy back by itself, an object written after arriving.
#
# RACE=N, last, writes N objects into each way's bucket, each looked for on
# the replica at once once its status says COMPLETED (noted).
#
# Throwaway containers only (qd-verify-poolrepl-…, on a network of their
# own, no port published), a throwaway CA, random keys that are never
# printed. Run from the repository root:
#
#   bash tools/verify/rustfs_pool_replication.sh
#
# Needs docker, curl 7.75 or later (--aws-sigv4), openssl 3, jq and uv.
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
TRAEFIK=${TRAEFIK_IMAGE:-traefik:v3.6}
WHOAMI=${WHOAMI_IMAGE:-traefik/whoami:v1.11}
P=qd-verify-poolrepl
NET=${QD_VERIFY_SUBNET:-10.231.79}
WAIT=${QD_VERIFY_WAIT:-90}
ALIAS=qd-lake-replication
ROOT=$(pwd)
D=$(mktemp -d)
chmod 700 "$D"
cleanup() {
  [ "${QD_VERIFY_KEEP:-0}" = 1 ] && return
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
expect() { # NAME EXPECTED ACTUAL
  if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi
}

# --- Certificates and keys -----------------------------------------------------
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/CN=qd verify poolrepl" -addext "basicConstraints=critical,CA:TRUE" \
  -keyout "$D/ca.key" -out "$D/ca.crt" 2>/dev/null
# cert FILE CN OU NAMES: a machine's certificate as the CA makes them, client
# and server use in one, NAMES (comma-separated DNS:…) its alternative names.
cert() {
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/O=qd/OU=$3/CN=$2" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=%s\n' "$4" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/ca.crt" -CAkey "$D/ca.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
}
rootkeys() { printf 'RUSTFS_ACCESS_KEY=qd%s\nRUSTFS_SECRET_KEY=%s\n' "$(openssl rand -hex 8)" "$(openssl rand -hex 20)" >"$D/$1"; }
(umask 077 && rootkeys root-pool.env && rootkeys root-replica.env)
tlsdir() { # NAME CERT: NAME's RustFS TLS directory, CERT as server and client
  local t=$D/tls-$1
  mkdir -p "$t"
  cp "$D/$2.crt" "$t/rustfs_cert.pem" && cp "$D/$2.key" "$t/rustfs_key.pem"
  cp "$D/$2.crt" "$t/client_cert.pem" && cp "$D/$2.key" "$t/client_key.pem"
  cp "$D/ca.crt" "$t/client_ca.crt" && cp "$D/ca.crt" "$t/ca.crt"
  chmod 755 "$t" && chmod 644 "$t"/*
}
machinedir() { # DIR CERT: what the node agent leaves, as a proxy reads it
  mkdir -p "$D/$1"
  cp "$D/ca.crt" "$D/$1/ca.crt"
  cat "$D/$2.crt" "$D/$2.key" >"$D/$1/machine.pem"
  chmod 755 "$D/$1" && chmod 644 "$D/$1"/*
}
docker network create --subnet "$NET.0/24" "$P" >/dev/null || exit 1
# The entrypoints as join.sh writes them (0755), readable by uid 10001.
cp "$ROOT/infra/rustfs/gate/entrypoint.sh" "$D/gate.sh"
cp "$ROOT/infra/worker/lake/entrypoint.sh" "$D/proxy.sh"
chmod 755 "$D/gate.sh" "$D/proxy.sh"
cert app-1 app-1 app DNS:app-1
cert replica-1 replica-1 lake-replica DNS:replica-1
tlsdir replica-1 replica-1
machinedir m-replica-1 replica-1
declare -A ADDR=([replica-1]=$NET.20)
for i in 1 2 3 4; do
  n=pool-$i
  ADDR[$n]=$NET.$((10 + i))
  cert "$n" "$n" lake "DNS:$n,DNS:$ALIAS"
  cert "$n-plain" "$n" lake "DNS:$n"
  tlsdir "$n" "$n"
  machinedir "m-$n" "$n"
  machinedir "m-$n-plain" "$n-plain"
done
echo synthetic >"$D/obj"

# --- The replica behind the repository's gate ------------------------------------
docker run -d --name "$P-check" --network "$P" "$WHOAMI" >/dev/null
docker run -d --name "$P-replica-1" --network "$P" --ip "${ADDR[replica-1]}" --env-file "$D/root-replica.env" \
  -e RUSTFS_VOLUMES=/data -e RUSTFS_TLS_PATH=/tls -v "$D/tls-replica-1:/tls:ro" "$IMAGE" server --address :9010 >/dev/null
docker run -d --name "$P-gate-replica-1" --network "container:$P-replica-1" --user 10001:10001 \
  --tmpfs /run/qd-gate:uid=10001,gid=10001,mode=0700 \
  -e QD_MACHINE_NAME=replica-1 -e "QD_LAKE_CHECK_URL=http://$P-check/" -e QD_GATE_UPSTREAM=https://127.0.0.1:9010 \
  -v "$D/gate.sh:/etc/qd/gate/entrypoint.sh:ro" -v "$D/m-replica-1:/run/qd-node:ro" \
  --entrypoint /etc/qd/gate/entrypoint.sh "$TRAEFIK" >/dev/null

# --- The pool: four nodes, each its own name and ALIAS on its loopback -------------
for i in 1 2 3 4; do
  n=pool-$i hosts=(--add-host "$n:127.0.0.1" --add-host "$ALIAS:127.0.0.1" --add-host "replica-1:${ADDR[replica-1]}")
  for j in 1 2 3 4; do [ "$j" = "$i" ] || hosts+=(--add-host "pool-$j:${ADDR[pool-$j]}"); done
  docker run -d --name "$P-$n" --network "$P" --ip "${ADDR[$n]}" "${hosts[@]}" --env-file "$D/root-pool.env" \
    -e "RUSTFS_VOLUMES=https://pool-{1...4}:9010/data" -e RUSTFS_TLS_PATH=/tls -e RUSTFS_SERVER_MTLS_ENABLE=1 \
    -v "$D/tls-$n:/tls:ro" "$IMAGE" server --address :9010 >/dev/null
done

# The proxies, each the repository's entrypoint: (a) as it is, on 9100,
# its certificate carrying ALIAS; the control on 9102 with one that does
# not; (b) its listener plain (QD_LAKE_LISTEN_TLS=0, as the pool's compose
# file sets it), on 9101.
expect "the entrypoint's listener is plain with QD_LAKE_LISTEN_TLS=0 alone" "1 1" \
  "$(grep -c '^0) LISTEN_TLS=.. ;;$' "$D/proxy.sh") $(grep -c '^      tls: {}. ;;$' "$D/proxy.sh")"
# ENTRY is a proxy's command (the entrypoint itself, or the pool compose
# file's loop around it), POLICY its restart policy.
ENTRY=(/etc/qd/lake/entrypoint.sh) POLICY=no
proxy() { # NODE KIND PORT ENTRYPOINT MACHINEDIR [LISTEN_TLS]
  docker rm -f "$P-$2-$1" >/dev/null 2>&1
  docker run -d --name "$P-$2-$1" --network "container:$P-$1" --user 10001:10001 --init \
    --restart "$POLICY" --tmpfs /run/qd-lake:uid=10001,gid=10001,mode=0700 -e "QD_LAKE_LISTEN_TLS=${6:-1}" \
    -e QD_LAKE_GATE=replica-1:9000 -e "QD_LAKE_LISTEN=127.0.0.1:$3" -e QD_LAKE_HTTP1_ONLY=1 \
    -v "$4:/etc/qd/lake/entrypoint.sh:ro" -v "$D/$5:/run/qd-node:ro" \
    --entrypoint "${ENTRY[0]}" "$TRAEFIK" "${ENTRY[@]:1}" >/dev/null
}
proxies() { # NODE: every proxy of NODE
  proxy "$1" name 9100 "$D/proxy.sh" "m-$1"
  proxy "$1" plain 9101 "$D/proxy.sh" "m-$1" 0
  proxy "$1" noname 9102 "$D/proxy.sh" "m-$1-plain"
}
for i in 1 2 3 4; do proxies "pool-$i"; done

# s3 NAME KEYS ARGS...: a request to NAME's RustFS on 9010, signed with the
# root keys of the file KEYS, presenting app-1's certificate.
s3() {
  local n=$1 k=$2
  shift 2
  (
    # shellcheck source=/dev/null
    . "$D/$k"
    printf 'user = "%s:%s"\naws-sigv4 = "aws:amz:us-east-1:s3"\ncert = "%s"\nkey = "%s"\n' \
      "$RUSTFS_ACCESS_KEY" "$RUSTFS_SECRET_KEY" "$D/app-1.crt" "$D/app-1.key"
  ) | curl -s -o /dev/null -w '%{http_code}' --cacert "$D/ca.crt" --max-time 15 \
    --resolve "$n:9010:${ADDR[$n]}" -K - "$@" 2>/dev/null
}
up() { # NAME KEYS: within WAIT seconds, NAME answers a signed request
  for _ in $(seq "$WAIT"); do
    [ "$(s3 "$1" "$2" "https://$1:9010/")" = 200 ] && echo up && return
    sleep 1
  done
  echo down
}
expect "the replica answers" up "$(up replica-1 root-replica.env)"
expect "the pool formed" up "$(up pool-1 root-pool.env)"

# The platform's admin user on each lake, as admin-user.sh and join.sh make
# it: on one node of the pool, on the replica.
ADMIN=qd-platform-admin
POLICY='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:*"],"Resource":["arn:aws:s3:::qd-tenant-*","arn:aws:s3:::qd-tenant-*/*"]},{"Effect":"Allow","Action":["admin:*"],"Resource":["arn:aws:s3:::*"]}]}'
adminuser() { # NAME ROOTKEYS OUT
  local secret base="https://$1:9010/rustfs/admin/v3" r
  secret=$(openssl rand -hex 32)
  (umask 077 && printf '{"secretKey":"%s","status":"enabled"}' "$secret" >"$D/user.json")
  r=$(s3 "$1" "$2" -X PUT --data-binary "$POLICY" "$base/add-canned-policy?name=$ADMIN")
  r=$r/$(s3 "$1" "$2" -X PUT --data-binary "@$D/user.json" "$base/add-user?accessKey=$ADMIN")
  r=$r/$(s3 "$1" "$2" -X PUT "$base/set-user-or-group-policy?policyName=$ADMIN&userOrGroup=$ADMIN&isGroup=false")
  rm -f "$D/user.json"
  (umask 077 && printf 'ADMIN_ACCESS_KEY=%s\nADMIN_SECRET_KEY=%s\n' "$ADMIN" "$secret" >"$D/$3")
  echo "$r"
}
expect "the admin user made on pool-1" 200/200/200 "$(adminuser pool-1 root-pool.env admin-pool.env)"
expect "the admin user made on the replica" 200/200/200 "$(adminuser replica-1 root-replica.env admin-replica.env)"

# --- The plain listener stays in the node's namespace -------------------------------
echo "== where the plain listener answers"
inside=$(docker exec "$P-pool-1" curl -s -o /dev/null -w '%{http_code}' --max-time 5 http://127.0.0.1:9101/ 2>/dev/null)
case $inside in 4??) ok "on pool-1's loopback the plain proxy reaches the replica's RustFS (unsigned: $inside)" ;;
*) ko "on pool-1's loopback the plain proxy answered '$inside'" ;; esac
expect "at pool-1's address, from the replica, nothing listens" 000 \
  "$(docker exec "$P-replica-1" curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://${ADDR[pool-1]}:9101/" 2>/dev/null)"
expect "nor for the TLS proxy (a)" 000 \
  "$(docker exec "$P-replica-1" curl -sk -o /dev/null -w '%{http_code}' --max-time 5 "https://${ADDR[pool-1]}:9100/" 2>/dev/null)"

# --- The replication, both ways ------------------------------------------------------
echo "== the replication from the pool, by the admin keys"
ADDRS=$(for n in "${!ADDR[@]}"; do printf '%s=%s,' "$n" "${ADDR[$n]}"; done)
# stopped NODE on|off: NODE's proxies stopped, or started again
cat >"$D/proxies.sh" <<SH
#!/bin/sh
if [ "\$2" = off ]; then docker stop $P-name-\$1 $P-plain-\$1 >/dev/null; else docker start $P-name-\$1 $P-plain-\$1 >/dev/null; fi
SH
chmod 700 "$D/proxies.sh"
if D=$D ADDRS=$ADDRS ALIAS=$ALIAS WAIT=$WAIT PROXIES="$D/proxies.sh" \
  uv run -q --no-project --with boto3==1.44.2 --with requests==2.34.2 \
  python3 -I tools/verify/rustfs_pool_replication.py; then :; else FAILED=1; fi

# --- pool-1's RustFS restarted ----------------------------------------------------
# Docker gives a restarted container a new network namespace and leaves
# those in its old one with their loopback alone (0071). pool-1's proxies
# first as they were, then under the loop of the pool's compose file, which
# must bring them back by themselves; each time an object written through
# pool-1 after the restart. With the proxies as they were, whether it
# arrives is noted, not checked: another node may replicate it.
restarted() { # LABEL check|note
  D=$D ADDRS=$ADDRS ALIAS=$ALIAS WAIT=$WAIT uv run -q --no-project --with boto3==1.44.2 \
    --with requests==2.34.2 python3 -I tools/verify/rustfs_pool_replication.py restarted "$1" "$2" || FAILED=1
}
ifaces() { docker exec "$P-$1-pool-1" grep -c : /proc/net/dev 2>/dev/null || echo none; }
listens() { docker exec "$P-pool-1" curl -s -o /dev/null -w '%{http_code}' --max-time 5 http://127.0.0.1:9101/ 2>/dev/null; }
restart1() {
  T0=$(date +%s)
  docker restart "$P-pool-1" >/dev/null
  expect "pool-1's RustFS restarted: it answers again" up "$(up pool-1 root-pool.env)"
}
stopping() { # NAME: docker stop of the proxy NAME: in ms, then its state and exit code
  local start took
  start=$(date +%s%N)
  docker stop "$P-$1" >/dev/null
  took=$((($(date +%s%N) - start) / 1000000))
  echo "$took $(docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$P-$1")"
}
echo "== pool-1's RustFS restarted, its proxies as they were (the control)"
restart1
expect "pool-1's proxies (a) and (b) left with their loopback alone" "1 1" "$(ifaces name) $(ifaces plain)"
expect "... the plain one not answering on RustFS's new loopback" 000 "$(listens)"
restarted control note
# The entrypoint alone, for the time docker stop takes: pool-2's, at work.
read -r base _ basecode <<<"$(stopping plain-pool-2)"
docker start "$P-plain-pool-2" >/dev/null

echo "== pool-1's proxies under the pool compose file's loop"
# The service's entrypoint as Compose runs it ($$ is its escape of $).
mapfile -d '' LOOP < <(uv run -q --no-project --with pyyaml==6.0.2 python3 -I -c '
import sys, yaml
e = yaml.safe_load(open(sys.argv[1]))["services"]["replication-proxy"]["entrypoint"]
sys.stdout.write("".join(x.replace("$$", "$") + "\0" for x in e))' infra/rustfs/pool/docker-compose.yml)
expect "the compose file's proxy: /bin/sh -c, a loop on /proc/net/dev around the entrypoint" "/bin/sh -c 1 1" \
  "${LOOP[0]} ${LOOP[1]} $(grep -c '^/etc/qd/lake/entrypoint.sh "$@" &$' <<<"${LOOP[2]}") $(grep -c 'grep -c : /proc/net/dev' <<<"${LOOP[2]}")"
# As the compose file's restart policy (unless-stopped) for its exit 1, bounded.
ENTRY=("${LOOP[@]}") POLICY=on-failure:5
proxy pool-1 name 9100 "$D/proxy.sh" m-pool-1
proxy pool-1 plain 9101 "$D/proxy.sh" m-pool-1 0
for _ in $(seq 30); do case $(listens) in 4??) break ;; esac; sleep 1; done
case $(listens) in 4??) ok "the looped plain proxy answers on pool-1's loopback" ;; *) ko "the looped plain proxy answered '$(listens)'" ;; esac
restart1
back=''
for _ in $(seq 180); do
  if [ "$(ifaces plain)" -gt 1 ] 2>/dev/null && [ "$(ifaces name)" -gt 1 ] 2>/dev/null; then
    case $(listens) in 4??) back=$(($(date +%s) - T0)) && break ;; esac
  fi
  sleep 1
done
expect "the looped proxies back in RustFS's new namespace by themselves" yes "${back:+yes}"
looped() { # KIND: restarted by its policy, after its loop said why
  local n said
  n=$(docker inspect -f '{{.RestartCount}}' "$P-$1-pool-1")
  said=$(docker logs "$P-$1-pool-1" 2>&1 | grep -c '^qd: RustFS restarted')
  [ "$n" -ge 1 ] && [ "$said" -ge 1 ] && echo yes || echo "no ($n restarts, the loop's line $said times)"
}
expect "... (a) left behind, ended by its loop and restarted" yes "$(looped name)"
expect "... (b) the same" yes "$(looped plain)"
echo "   (back ${back:-never} s after RustFS's restart began; restarted $(docker inspect -f '{{.RestartCount}}' "$P-name-pool-1") and $(docker inspect -f '{{.RestartCount}}' "$P-plain-pool-1") times)"
restarted looped check
read -r took state code <<<"$(stopping plain-pool-1)"
expect "docker stop ends the looped proxy, and it stays stopped" exited "$state"
echo "   (stopped in $took ms, exit code $code; pool-2's, the entrypoint alone, in $base ms, $basecode)"

echo "== every node's RustFS restarted at once, every proxy under the loop"
# The nodes here run RustFS without an init, so a kill from inside cannot
# end it: the four are restarted by docker restart, all at the same moment.
for i in 1 2 3 4; do
  proxy "pool-$i" name 9100 "$D/proxy.sh" "m-pool-$i"
  proxy "pool-$i" plain 9101 "$D/proxy.sh" "m-pool-$i" 0
done
# answering: how many nodes' plain and TLS proxies both answer on their
# RustFS's loopback.
answering() {
  local i n=0 a b
  for i in 1 2 3 4; do
    a=$(docker exec "$P-pool-$i" curl -s -o /dev/null -w '%{http_code}' --max-time 5 http://127.0.0.1:9101/ 2>/dev/null)
    b=$(docker exec "$P-pool-$i" curl -sk -o /dev/null -w '%{http_code}' --max-time 5 https://127.0.0.1:9100/ 2>/dev/null)
    case "$a $b" in 4??\ 4??) n=$((n + 1)) ;; esac
  done
  echo "$n"
}
for _ in $(seq 60); do [ "$(answering)" = 4 ] && break; sleep 1; done
expect "every node's looped proxies answer on its loopback" 4 "$(answering)"
T0=$(date +%s)
for i in 1 2 3 4; do docker restart "$P-pool-$i" >/dev/null & done
wait
expect "the four restarted: the pool answers again" up "$(up pool-1 root-pool.env)"
back=''
for _ in $(seq 180); do
  [ "$(answering)" = 4 ] && back=$(($(date +%s) - T0)) && break
  sleep 1
done
expect "every node's proxies back in their RustFS's new namespace by themselves" yes "${back:+yes}"
restarts=$(for i in 1 2 3 4; do for k in name plain; do docker inspect -f '{{.RestartCount}}' "$P-$k-pool-$i"; done; done | paste -sd' ')
echo "   (back ${back:-never} s after the restarts began; the eight proxies restarted $restarts times)"
restarted all-restarted check

# RACE=N: N objects through the nodes, each looked for on the replica once
# its status says COMPLETED (noted, not checked).
if [ -n "${RACE:-}" ]; then
  echo "== $RACE objects: COMPLETED on the pool, then one look at the replica"
  D=$D ADDRS=$ADDRS ALIAS=$ALIAS WAIT=$WAIT uv run -q --no-project --with boto3==1.44.2 \
    --with requests==2.34.2 python3 -I tools/verify/rustfs_pool_replication.py race "$RACE"
fi

echo
if [ "$FAILED" = 0 ]; then echo "ALL OK"; else
  echo "SOME CHECKS FAILED"
  exit 1
fi
